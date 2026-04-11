use std::collections::HashMap;
use std::time::{Duration, Instant};

use dbus::arg::PropMap;
use dbus::blocking::stdintf::org_freedesktop_dbus::ObjectManager;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::Path;
use dbus_crossroads::Crossroads;
use sha2::{Digest, Sha256};

use crate::aethos_core::ble_discovery::{
    build_identity_service_data_ad, build_primary_uuid_list_ad, canonical_ble_primary_service_uuid,
    BleIdentityCapabilities, BleIdentityPayloadV1,
};
use crate::aethos_core::identity_store::ensure_local_identity;
use crate::aethos_core::protocol::is_valid_wayfarer_id;

const AETHOS_BLE_IDREF_CONTEXT_V1: &[u8] = b"aethos:ble:idref:v1\0";
const ADVERTISEMENT_OBJECT_PATH: &str = "/org/aethos/ble/advertisement0";

pub enum AdvertiserPollEvent {
    Started(EmittedCanonicalAdvertisement),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct EmittedCanonicalAdvertisement {
    pub source: &'static str,
    pub mode: &'static str,
    pub uuid: String,
    pub wayfarer_id: String,
    pub primary_ad_hex: String,
    pub service_data_ad_hex: String,
    pub payload_hex: String,
    pub identity_ref_hex: String,
}

pub struct CanonicalBleAdvertiser {
    active: Option<BluezAdvertisementHandle>,
    retry_interval: Duration,
    last_attempt: Option<Instant>,
}

impl CanonicalBleAdvertiser {
    pub fn new() -> Self {
        Self {
            active: None,
            retry_interval: Duration::from_secs(15),
            last_attempt: None,
        }
    }

    pub fn poll(&mut self) -> Option<AdvertiserPollEvent> {
        if ble_advertising_is_disabled() {
            self.active = None;
            return None;
        }

        if let Some(active) = self.active.as_mut() {
            if let Err(err) = active.pump() {
                self.active = None;
                return Some(AdvertiserPollEvent::Error(format!(
                    "bluez advertisement loop failed: {err}"
                )));
            }
            return None;
        }

        let should_wait = self
            .last_attempt
            .map(|last| last.elapsed() < self.retry_interval)
            .unwrap_or(false);
        if should_wait {
            return None;
        }
        self.last_attempt = Some(Instant::now());

        let identity = match ensure_local_identity() {
            Ok(identity) => identity,
            Err(err) => {
                return Some(AdvertiserPollEvent::Error(format!(
                    "local identity unavailable: {err}"
                )));
            }
        };

        let payload = match build_canonical_identity_payload(&identity.wayfarer_id) {
            Ok(payload) => payload,
            Err(err) => {
                return Some(AdvertiserPollEvent::Error(format!(
                    "failed building canonical BLE identity payload: {err}"
                )));
            }
        };

        match BluezAdvertisementHandle::register(&identity.wayfarer_id, &payload) {
            Ok(handle) => {
                let report = handle.report.clone();
                self.active = Some(handle);
                Some(AdvertiserPollEvent::Started(report))
            }
            Err(err) => Some(AdvertiserPollEvent::Error(format!(
                "failed registering BlueZ canonical BLE advertisement: {err}"
            ))),
        }
    }
}

#[derive(Clone)]
struct AdvertisementConfig {
    advertisement_type: String,
    service_uuids: Vec<String>,
    service_data: HashMap<String, Vec<u8>>,
    discoverable: bool,
    secondary_channel: Option<String>,
}

struct AdvertisementAttempt {
    mode: &'static str,
    include_secondary_channel_property: bool,
    config: AdvertisementConfig,
}

struct BluezAdvertisementHandle {
    connection: Connection,
    adapter_path: String,
    advertisement_path: Path<'static>,
    report: EmittedCanonicalAdvertisement,
}

impl BluezAdvertisementHandle {
    fn register(
        wayfarer_id: &str,
        payload: &BleIdentityPayloadV1,
    ) -> Result<BluezAdvertisementHandle, String> {
        let primary_ad = build_primary_uuid_list_ad();
        let service_data_ad = build_identity_service_data_ad(payload);
        let payload_bytes = payload_to_bytes(payload);
        let primary_ad_hex = bytes_to_hex_lower(&primary_ad);
        let service_data_ad_hex = bytes_to_hex_lower(&service_data_ad);
        let payload_hex = bytes_to_hex_lower(&payload_bytes);
        let identity_ref_hex = bytes_to_hex_lower(&payload.identity_ref);

        let mut service_data = HashMap::new();
        service_data.insert(
            canonical_ble_primary_service_uuid().to_string(),
            payload_bytes.to_vec(),
        );

        let attempts = advertisement_attempts(service_data);
        let mut failures = Vec::new();
        for attempt in attempts {
            let connection =
                Connection::new_system().map_err(|err| format!("dbus connect failed: {err}"))?;
            let adapter_path = find_advertising_adapter_path(&connection)?;
            let advertisement_path = Path::new(ADVERTISEMENT_OBJECT_PATH)
                .map_err(|_| "invalid advertisement object path".to_string())?
                .into_static();

            register_advertisement_object(
                &connection,
                &advertisement_path,
                attempt.config,
                attempt.include_secondary_channel_property,
            )?;
            match register_with_bluez_manager(&connection, &adapter_path, &advertisement_path) {
                Ok(()) => {
                    return Ok(Self {
                        connection,
                        adapter_path,
                        advertisement_path,
                        report: EmittedCanonicalAdvertisement {
                            source: "bluez-le-advertiser",
                            mode: attempt.mode,
                            uuid: canonical_ble_primary_service_uuid().to_string(),
                            wayfarer_id: wayfarer_id.to_string(),
                            primary_ad_hex,
                            service_data_ad_hex,
                            payload_hex,
                            identity_ref_hex,
                        },
                    });
                }
                Err(err) => {
                    failures.push(format!("{}: {}", attempt.mode, err));
                }
            }
        }

        Err(format!(
            "register advertisement failed across {} mode(s): {}",
            failures.len(),
            failures.join(" | ")
        ))
    }

    fn pump(&mut self) -> Result<(), String> {
        self.connection
            .process(Duration::from_millis(0))
            .map_err(|err| format!("dbus process failed: {err}"))?;
        Ok(())
    }
}

impl Drop for BluezAdvertisementHandle {
    fn drop(&mut self) {
        let proxy = self.connection.with_proxy(
            "org.bluez",
            &self.adapter_path,
            Duration::from_millis(1200),
        );
        let _: Result<(), _> = proxy.method_call(
            "org.bluez.LEAdvertisingManager1",
            "UnregisterAdvertisement",
            (self.advertisement_path.clone(),),
        );
    }
}

fn ble_advertising_is_disabled() -> bool {
    std::env::var("AETHOS_DISABLE_BLE")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || std::env::var("AETHOS_DISABLE_BLE_ADVERTISING")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

fn build_canonical_identity_payload(wayfarer_id: &str) -> Result<BleIdentityPayloadV1, String> {
    let identity_ref = derive_stable_identity_ref(wayfarer_id)?;
    let capabilities = BleIdentityCapabilities {
        lan: true,
        mpc: false,
        relay: false,
        normalized_bits: 0x0001,
        raw_bits: 0x0001,
    };
    Ok(BleIdentityPayloadV1 {
        version: 0x01,
        identity_rotating: false,
        identity_private: false,
        capabilities,
        identity_ref,
    })
}

fn derive_stable_identity_ref(wayfarer_id: &str) -> Result<[u8; 8], String> {
    let wayfarer_bytes = parse_wayfarer_id_hex(wayfarer_id)?;
    let mut hasher = Sha256::new();
    hasher.update(AETHOS_BLE_IDREF_CONTEXT_V1);
    hasher.update(wayfarer_bytes);
    let digest = hasher.finalize();
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest[..8]);
    if out.iter().all(|byte| *byte == 0) {
        return Err("identity_ref derivation produced zero bytes".to_string());
    }
    Ok(out)
}

fn parse_wayfarer_id_hex(wayfarer_id: &str) -> Result<[u8; 32], String> {
    if !is_valid_wayfarer_id(wayfarer_id) {
        return Err("invalid wayfarer_id; expected 64 lowercase hex chars".to_string());
    }
    let mut out = [0u8; 32];
    for idx in 0..32 {
        let from = idx * 2;
        let to = from + 2;
        out[idx] = u8::from_str_radix(&wayfarer_id[from..to], 16)
            .map_err(|err| format!("failed to parse wayfarer_id hex at byte {idx}: {err}"))?;
    }
    Ok(out)
}

fn payload_to_bytes(payload: &BleIdentityPayloadV1) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[0] = payload.version;
    let mut flags = 0u8;
    if payload.identity_rotating {
        flags |= 0x01;
    }
    if payload.identity_private {
        flags |= 0x02;
    }
    out[1] = flags;
    let capabilities = payload.capabilities.raw_bits.to_le_bytes();
    out[2] = capabilities[0];
    out[3] = capabilities[1];
    out[4..12].copy_from_slice(&payload.identity_ref);
    out
}

fn register_advertisement_object(
    connection: &Connection,
    advertisement_path: &Path<'static>,
    config: AdvertisementConfig,
    include_secondary_channel_property: bool,
) -> Result<(), String> {
    let mut crossroads = Crossroads::new();
    let iface_token = crossroads.register("org.bluez.LEAdvertisement1", |builder| {
        builder.method("Release", (), (), |_ctx, _cfg, ()| Ok(()));
        builder
            .property("Type")
            .get(|_, cfg: &mut AdvertisementConfig| Ok(cfg.advertisement_type.clone()));
        builder
            .property("ServiceUUIDs")
            .get(|_, cfg: &mut AdvertisementConfig| Ok(cfg.service_uuids.clone()));
        builder
            .property("ServiceData")
            .get(|_, cfg: &mut AdvertisementConfig| Ok(cfg.service_data.clone()));
        builder
            .property("Discoverable")
            .get(|_, cfg: &mut AdvertisementConfig| Ok(cfg.discoverable));
        if include_secondary_channel_property {
            builder
                .property("SecondaryChannel")
                .get(|_, cfg: &mut AdvertisementConfig| {
                    Ok(cfg
                        .secondary_channel
                        .clone()
                        .unwrap_or_else(|| "1M".to_string()))
                });
        }
    });
    crossroads.insert(advertisement_path.clone(), &[iface_token], config);
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |msg, conn| {
            let _ = crossroads.handle_message(msg, conn);
            true
        }),
    );
    Ok(())
}

fn advertisement_attempts(service_data: HashMap<String, Vec<u8>>) -> Vec<AdvertisementAttempt> {
    let uuid = canonical_ble_primary_service_uuid().to_string();
    let extended = AdvertisementAttempt {
        mode: "extended-secondary-1m",
        include_secondary_channel_property: true,
        config: AdvertisementConfig {
            advertisement_type: "peripheral".to_string(),
            service_uuids: vec![uuid.clone()],
            service_data: service_data.clone(),
            discoverable: true,
            secondary_channel: Some("1M".to_string()),
        },
    };
    let legacy = AdvertisementAttempt {
        mode: "legacy-peripheral-fallback",
        include_secondary_channel_property: false,
        config: AdvertisementConfig {
            advertisement_type: "peripheral".to_string(),
            service_uuids: vec![uuid],
            service_data,
            discoverable: true,
            secondary_channel: None,
        },
    };

    if prefer_legacy_advertiser_mode() {
        vec![legacy, extended]
    } else {
        vec![extended, legacy]
    }
}

fn prefer_legacy_advertiser_mode() -> bool {
    std::env::var("AETHOS_BLE_ADVERTISER_MODE")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("legacy"))
        .unwrap_or(false)
}

fn register_with_bluez_manager(
    connection: &Connection,
    adapter_path: &str,
    advertisement_path: &Path<'static>,
) -> Result<(), String> {
    let proxy = connection.with_proxy("org.bluez", adapter_path, Duration::from_millis(8_000));
    let options = PropMap::new();
    let before = advertising_capacity_snapshot(connection, adapter_path);
    let result: Result<(), _> = proxy.method_call(
        "org.bluez.LEAdvertisingManager1",
        "RegisterAdvertisement",
        (advertisement_path.clone(), options),
    );
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            let err_text = err.to_string();
            let after = advertising_capacity_snapshot(connection, adapter_path);
            if is_register_timeout(&err_text)
                && before
                    .zip(after)
                    .map(|(before, after)| after.0 > before.0)
                    .unwrap_or(false)
            {
                return Ok(());
            }

            if let Some((active_instances, available_instances, capacity_estimate)) = after {
                Err(format!(
                    "register advertisement failed: {err_text} (active_instances={} available_instances={} capacity_estimate={})",
                    active_instances, available_instances, capacity_estimate
                ))
            } else {
                Err(format!("register advertisement failed: {err_text}"))
            }
        }
    }
}

fn is_register_timeout(err_text: &str) -> bool {
    err_text.contains("Did not receive a reply")
}

fn advertising_capacity_snapshot(
    connection: &Connection,
    adapter_path: &str,
) -> Option<(u8, u8, u8)> {
    let proxy = connection.with_proxy("org.bluez", adapter_path, Duration::from_millis(1200));
    let active_instances: u8 = proxy
        .get("org.bluez.LEAdvertisingManager1", "ActiveInstances")
        .ok()?;
    let available_instances: u8 = proxy
        .get("org.bluez.LEAdvertisingManager1", "SupportedInstances")
        .ok()?;
    Some((
        active_instances,
        available_instances,
        active_instances.saturating_add(available_instances),
    ))
}

fn find_advertising_adapter_path(connection: &Connection) -> Result<String, String> {
    let proxy = connection.with_proxy("org.bluez", "/", Duration::from_millis(1200));
    let managed = proxy
        .get_managed_objects()
        .map_err(|err| format!("failed reading BlueZ objects: {err}"))?;
    managed
        .iter()
        .find(|(_, interfaces)| interfaces.contains_key("org.bluez.LEAdvertisingManager1"))
        .map(|(path, _)| path.to_string())
        .ok_or_else(|| "no LEAdvertisingManager1 adapter available".to_string())
}

fn bytes_to_hex_lower(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        build_canonical_identity_payload, bytes_to_hex_lower, derive_stable_identity_ref,
        payload_to_bytes,
    };
    use crate::aethos_core::ble_discovery::{
        build_identity_service_data_ad, build_primary_uuid_list_ad,
    };

    const VECTOR_WAYFARER_ID: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn stable_identity_ref_derivation_matches_vector() {
        let derived = derive_stable_identity_ref(VECTOR_WAYFARER_ID).expect("derive identity_ref");
        assert_eq!(bytes_to_hex_lower(&derived), "d6b6fc2bf0f08cdf");
    }

    #[test]
    fn canonical_payload_builder_packs_exact_12_bytes() {
        let payload = build_canonical_identity_payload(VECTOR_WAYFARER_ID).expect("build payload");
        let payload_bytes = payload_to_bytes(&payload);
        assert_eq!(payload_bytes.len(), 12);
        assert_eq!(
            bytes_to_hex_lower(&payload_bytes),
            "01000100d6b6fc2bf0f08cdf"
        );
    }

    #[test]
    fn canonical_ad_structures_match_contract_bytes() {
        let payload = build_canonical_identity_payload(VECTOR_WAYFARER_ID).expect("build payload");
        let primary_ad = build_primary_uuid_list_ad();
        let service_data_ad = build_identity_service_data_ad(&payload);
        assert_eq!(
            bytes_to_hex_lower(&primary_ad),
            "11074eee0dd26c0ef787f950295a85a51a18"
        );
        assert_eq!(
            bytes_to_hex_lower(&service_data_ad),
            "1d214eee0dd26c0ef787f950295a85a51a1801000100d6b6fc2bf0f08cdf"
        );
    }
}
