use std::time::{Duration, Instant};

use dbus::arg::PropMap;
use dbus::blocking::stdintf::org_freedesktop_dbus::ObjectManager;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use dbus::Path;
use dbus_crossroads::Crossroads;

use crate::aethos_core::ble_discovery::{
    build_primary_uuid_list_ad, canonical_ble_primary_service_uuid,
};

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
    pub primary_ad_hex: String,
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

        match BluezAdvertisementHandle::register() {
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
    includes: Vec<String>,
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
    fn register() -> Result<BluezAdvertisementHandle, String> {
        let primary_ad = build_primary_uuid_list_ad();
        let primary_ad_hex = bytes_to_hex_lower(&primary_ad);

        let attempts = advertisement_attempts();
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
                            primary_ad_hex,
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
            .property("Includes")
            .get(|_, cfg: &mut AdvertisementConfig| Ok(cfg.includes.clone()));
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

// V2 §4.2: UUID-only, no service data.
fn advertisement_attempts() -> Vec<AdvertisementAttempt> {
    let uuid = canonical_ble_primary_service_uuid().to_string();

    let mode = std::env::var("AETHOS_BLE_ADVERTISER_MODE")
        .ok()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let extended = AdvertisementAttempt {
        mode: "extended-secondary-1m",
        include_secondary_channel_property: true,
        config: AdvertisementConfig {
            advertisement_type: "peripheral".to_string(),
            service_uuids: vec![uuid.clone()],
            includes: Vec::new(),
            discoverable: true,
            secondary_channel: Some("1M".to_string()),
        },
    };
    let legacy = AdvertisementAttempt {
        mode: "legacy-peripheral-fallback",
        include_secondary_channel_property: false,
        config: AdvertisementConfig {
            advertisement_type: "peripheral".to_string(),
            service_uuids: vec![uuid.clone()],
            includes: Vec::new(),
            discoverable: true,
            secondary_channel: None,
        },
    };
    let uuid_only_broadcast = AdvertisementAttempt {
        mode: "uuid-only-broadcast-baseline",
        include_secondary_channel_property: false,
        config: AdvertisementConfig {
            advertisement_type: "broadcast".to_string(),
            service_uuids: vec![uuid],
            includes: Vec::new(),
            discoverable: true,
            secondary_channel: None,
        },
    };

    if mode == "legacy" {
        vec![legacy, extended, uuid_only_broadcast]
    } else if mode == "uuid-only" || mode == "broadcast" {
        vec![uuid_only_broadcast, legacy, extended]
    } else {
        vec![extended, legacy, uuid_only_broadcast]
    }
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
    use super::bytes_to_hex_lower;
    use crate::aethos_core::ble_discovery::build_primary_uuid_list_ad;

    #[test]
    fn v2_primary_uuid_ad_matches_expected_bytes() {
        let primary_ad = build_primary_uuid_list_ad();
        assert_eq!(
            bytes_to_hex_lower(&primary_ad),
            "11074eee0dd26c0ef787f950295a85a51a18"
        );
    }

    #[test]
    fn v2_primary_uuid_ad_is_exactly_18_bytes() {
        let primary_ad = build_primary_uuid_list_ad();
        assert_eq!(primary_ad.len(), 18);
    }

    #[test]
    fn v2_advertisement_carries_no_service_data() {
        let attempts = super::advertisement_attempts();
        assert!(!attempts.is_empty());
        for attempt in &attempts {
            assert!(
                attempt.config.service_uuids.len() == 1,
                "mode={}: expected exactly 1 service UUID",
                attempt.mode
            );
            assert_eq!(
                attempt.config.service_uuids[0], "181aa585-5a29-50f9-87f7-0e6cd20dee4e",
                "mode={}",
                attempt.mode
            );
        }
    }
}
