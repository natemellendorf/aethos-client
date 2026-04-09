use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use dbus::arg::{ArgType, PropMap, RefArg, Variant};
use dbus::blocking::stdintf::org_freedesktop_dbus::ObjectManager;
use dbus::blocking::Connection;
use dbus::Path;

const AETHOS_BLE_PRIMARY_SERVICE_UUID: &str = "181aa585-5a29-50f9-87f7-0e6cd20dee4e";
const AETHOS_BLE_PRIMARY_SERVICE_UUID_LE: [u8; 16] = [
    0x4e, 0xee, 0x0d, 0xd2, 0x6c, 0x0e, 0xf7, 0x87, 0xf9, 0x50, 0x29, 0x5a, 0x85, 0xa5, 0x1a, 0x18,
];
const BLE_IDENTITY_V1_VERSION: u8 = 0x01;
const BLE_IDENTITY_V1_PAYLOAD_LEN: usize = 12;
const BLE_IDENTITY_V1_IDENTITY_REF_LEN: usize = 8;
type BluezManagedObjects = HashMap<Path<'static>, HashMap<String, PropMap>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySignal {
    pub peer_hint: String,
    pub observed_at_unix_ms: u64,
    pub rssi: Option<i16>,
    pub bearer_type: &'static str,
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleObservationRejection {
    pub reason_code: &'static str,
    pub reason_label: &'static str,
    pub source: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleSourcePollReport {
    pub accepted: Vec<DiscoverySignal>,
    pub rejected: Vec<BleObservationRejection>,
}

impl BleSourcePollReport {
    fn empty() -> Self {
        Self {
            accepted: Vec::new(),
            rejected: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleIdentityCapabilities {
    pub lan: bool,
    pub mpc: bool,
    pub relay: bool,
    pub normalized_bits: u16,
    pub raw_bits: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleIdentityPayloadV1 {
    pub version: u8,
    pub identity_rotating: bool,
    pub identity_private: bool,
    pub capabilities: BleIdentityCapabilities,
    pub identity_ref: [u8; BLE_IDENTITY_V1_IDENTITY_REF_LEN],
}

impl BleIdentityPayloadV1 {
    pub fn peer_hint(&self) -> String {
        format!("ble-idref:{}", bytes_to_hex_lower(&self.identity_ref))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BleParseReject {
    MissingPrimaryServiceUuid,
    MalformedPrimaryServiceUuidList,
    MissingIdentityPayload,
    DuplicateIdentityPayloadInPdu,
    ConflictingIdentityPayloadCopies,
    MalformedPayloadLength,
    UnsupportedVersion,
    ReservedFlagBitsSet,
    InvalidOrZeroIdentityRef,
    MalformedAdStructure,
}

impl BleParseReject {
    fn as_reason_code(self) -> &'static str {
        match self {
            Self::MissingPrimaryServiceUuid => "missing_primary_service_uuid",
            Self::MalformedPrimaryServiceUuidList => "malformed_primary_service_uuid_list",
            Self::MissingIdentityPayload => "missing_identity_payload",
            Self::DuplicateIdentityPayloadInPdu => "duplicate_identity_payload",
            Self::ConflictingIdentityPayloadCopies => "conflicting_identity_payload",
            Self::MalformedPayloadLength => "malformed_payload_length",
            Self::UnsupportedVersion => "unsupported_version",
            Self::ReservedFlagBitsSet => "reserved_flag_bits_set",
            Self::InvalidOrZeroIdentityRef => "invalid_or_zero_identity_ref",
            Self::MalformedAdStructure => "malformed_ad_structure",
        }
    }

    fn as_reason_label(self) -> &'static str {
        match self {
            Self::MissingPrimaryServiceUuid => "missing primary service UUID",
            Self::MalformedPrimaryServiceUuidList => "malformed primary service UUID list",
            Self::MissingIdentityPayload => "missing identity payload",
            Self::DuplicateIdentityPayloadInPdu => "duplicate identity payload",
            Self::ConflictingIdentityPayloadCopies => "conflicting identity payload",
            Self::MalformedPayloadLength => "malformed payload length",
            Self::UnsupportedVersion => "unsupported version",
            Self::ReservedFlagBitsSet => "reserved flag bits set",
            Self::InvalidOrZeroIdentityRef => "invalid or zero identity_ref",
            Self::MalformedAdStructure => "malformed AD structure",
        }
    }
}

pub trait BleDiscoverySource {
    fn poll_signals(&mut self, now_unix_ms: u64) -> Vec<DiscoverySignal>;

    fn poll_signals_with_diagnostics(&mut self, now_unix_ms: u64) -> BleSourcePollReport {
        BleSourcePollReport {
            accepted: self.poll_signals(now_unix_ms),
            rejected: Vec::new(),
        }
    }
}

pub struct BleDiscoveryGate {
    dedupe_window: Duration,
    last_seen_by_peer: HashMap<String, u64>,
}

pub struct GatePollResult {
    pub ready: Vec<DiscoverySignal>,
    pub deduped_count: usize,
    pub rejected: Vec<BleObservationRejection>,
}

impl BleDiscoveryGate {
    pub fn new(dedupe_window: Duration) -> Self {
        Self {
            dedupe_window,
            last_seen_by_peer: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn poll_ready(
        &mut self,
        source: &mut dyn BleDiscoverySource,
        now_unix_ms: u64,
    ) -> Vec<DiscoverySignal> {
        self.poll_ready_with_stats(source, now_unix_ms).ready
    }

    pub fn poll_ready_with_stats(
        &mut self,
        source: &mut dyn BleDiscoverySource,
        now_unix_ms: u64,
    ) -> GatePollResult {
        let report = source.poll_signals_with_diagnostics(now_unix_ms);
        let mut out = Vec::new();
        let mut deduped_count = 0;
        for signal in report.accepted {
            let allow = self
                .last_seen_by_peer
                .get(&signal.peer_hint)
                .map(|previous| {
                    signal.observed_at_unix_ms.saturating_sub(*previous)
                        >= self.dedupe_window.as_millis() as u64
                })
                .unwrap_or(true);
            if allow {
                self.last_seen_by_peer
                    .insert(signal.peer_hint.clone(), signal.observed_at_unix_ms);
                out.push(signal);
            } else {
                deduped_count += 1;
            }
        }
        GatePollResult {
            ready: out,
            deduped_count,
            rejected: report.rejected,
        }
    }
}

pub enum DiscoveryAdapter {
    Simulated(SimulatedBleDiscoverySource),
    BluetoothCtl(BluetoothCtlDiscoverySource),
    Disabled,
}

impl BleDiscoverySource for DiscoveryAdapter {
    fn poll_signals(&mut self, now_unix_ms: u64) -> Vec<DiscoverySignal> {
        match self {
            Self::Simulated(source) => source.poll_signals(now_unix_ms),
            Self::BluetoothCtl(source) => source.poll_signals(now_unix_ms),
            Self::Disabled => Vec::new(),
        }
    }

    fn poll_signals_with_diagnostics(&mut self, now_unix_ms: u64) -> BleSourcePollReport {
        match self {
            Self::Simulated(source) => source.poll_signals_with_diagnostics(now_unix_ms),
            Self::BluetoothCtl(source) => source.poll_signals_with_diagnostics(now_unix_ms),
            Self::Disabled => BleSourcePollReport::empty(),
        }
    }
}

pub fn discovery_adapter_from_env() -> DiscoveryAdapter {
    if let Ok(raw) = std::env::var("AETHOS_BLE_SIMULATED_SIGNALS") {
        let simulated = SimulatedBleDiscoverySource::from_env_string(&raw);
        if !simulated.pending.is_empty() {
            return DiscoveryAdapter::Simulated(simulated);
        }
    }

    let enabled = std::env::var("AETHOS_DISABLE_BLE")
        .ok()
        .map(|value| value.trim() != "1")
        .unwrap_or(true);
    if !enabled {
        return DiscoveryAdapter::Disabled;
    }

    DiscoveryAdapter::BluetoothCtl(BluetoothCtlDiscoverySource::default())
}

#[derive(Debug, Clone)]
struct SimulatedSignalSeed {
    kind: SimulatedSignalSeedKind,
    rssi: Option<i16>,
}

#[derive(Debug, Clone)]
enum SimulatedSignalSeedKind {
    Canonical {
        primary_advertisement_hex: String,
        scan_response_hex: String,
    },
    LegacyPeerHint(String),
}

pub struct SimulatedBleDiscoverySource {
    pending: Vec<SimulatedSignalSeed>,
    emitted_once: bool,
}

impl SimulatedBleDiscoverySource {
    fn from_env_string(raw: &str) -> Self {
        let pending = raw
            .split(',')
            .filter_map(|entry| {
                let trimmed = entry.trim();
                if trimmed.is_empty() {
                    return None;
                }
                let mut parts = trimmed.split('@');
                let seed_raw = parts.next()?.trim();
                let rssi = parts
                    .next()
                    .and_then(|value| value.trim().parse::<i16>().ok());
                let kind = if let Some(ad) = seed_raw.strip_prefix("ad:") {
                    let (primary_advertisement_hex, scan_response_hex) = ad.split_once("|sr:")?;
                    SimulatedSignalSeedKind::Canonical {
                        primary_advertisement_hex: primary_advertisement_hex.trim().to_string(),
                        scan_response_hex: scan_response_hex.trim().to_string(),
                    }
                } else {
                    let peer_hint = seed_raw.to_string();
                    if peer_hint.is_empty() {
                        return None;
                    }
                    SimulatedSignalSeedKind::LegacyPeerHint(peer_hint)
                };
                Some(SimulatedSignalSeed { kind, rssi })
            })
            .collect::<Vec<_>>();
        Self {
            pending,
            emitted_once: false,
        }
    }
}

impl BleDiscoverySource for SimulatedBleDiscoverySource {
    fn poll_signals(&mut self, now_unix_ms: u64) -> Vec<DiscoverySignal> {
        self.poll_signals_with_diagnostics(now_unix_ms).accepted
    }

    fn poll_signals_with_diagnostics(&mut self, now_unix_ms: u64) -> BleSourcePollReport {
        if self.emitted_once {
            return BleSourcePollReport::empty();
        }
        self.emitted_once = true;
        let mut report = BleSourcePollReport::empty();
        for seed in &self.pending {
            match &seed.kind {
                SimulatedSignalSeedKind::Canonical {
                    primary_advertisement_hex,
                    scan_response_hex,
                } => {
                    let Some(primary_advertisement) = hex_decode(primary_advertisement_hex) else {
                        report.rejected.push(build_rejection(
                            BleParseReject::MalformedAdStructure,
                            "simulated",
                            format!(
                                "invalid primary advertisement hex: {primary_advertisement_hex}"
                            ),
                        ));
                        continue;
                    };
                    let Some(scan_response) = hex_decode(scan_response_hex) else {
                        report.rejected.push(build_rejection(
                            BleParseReject::MalformedAdStructure,
                            "simulated",
                            format!("invalid scan response hex: {scan_response_hex}"),
                        ));
                        continue;
                    };

                    match parse_ble_identity_v1(&primary_advertisement, &scan_response) {
                        Ok(identity) => report.accepted.push(DiscoverySignal {
                            peer_hint: identity.peer_hint(),
                            observed_at_unix_ms: now_unix_ms,
                            rssi: seed.rssi,
                            bearer_type: "ble",
                            source: "simulated",
                        }),
                        Err(reject) => report.rejected.push(build_rejection(
                            reject,
                            "simulated",
                            "canonical payload rejected".to_string(),
                        )),
                    }
                }
                SimulatedSignalSeedKind::LegacyPeerHint(peer_hint) => {
                    report.accepted.push(DiscoverySignal {
                        peer_hint: peer_hint.clone(),
                        observed_at_unix_ms: now_unix_ms,
                        rssi: seed.rssi,
                        bearer_type: "ble",
                        source: "simulated-legacy",
                    });
                }
            }
        }
        report
    }
}

pub struct BluetoothCtlDiscoverySource {
    poll_interval: Duration,
    last_poll: Option<Instant>,
    allow_legacy_name_hints: bool,
    bluez: Option<Connection>,
    bluez_adapter_path: Option<String>,
    bluez_discovery_requested: bool,
}

impl Default for BluetoothCtlDiscoverySource {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(8),
            last_poll: None,
            allow_legacy_name_hints: std::env::var("AETHOS_BLE_ALLOW_LEGACY_NAME_HINTS")
                .ok()
                .map(|value| value.trim() == "1")
                .unwrap_or(false),
            bluez: None,
            bluez_adapter_path: None,
            bluez_discovery_requested: false,
        }
    }
}

impl BleDiscoverySource for BluetoothCtlDiscoverySource {
    fn poll_signals(&mut self, now_unix_ms: u64) -> Vec<DiscoverySignal> {
        self.poll_signals_with_diagnostics(now_unix_ms).accepted
    }

    fn poll_signals_with_diagnostics(&mut self, now_unix_ms: u64) -> BleSourcePollReport {
        if let Some(last_poll) = self.last_poll {
            if last_poll.elapsed() < self.poll_interval {
                return BleSourcePollReport::empty();
            }
        }
        self.last_poll = Some(Instant::now());

        if let Some(report) = self.poll_signals_via_bluez_dbus(now_unix_ms) {
            return report;
        }

        let output = match Command::new("bluetoothctl").arg("devices").output() {
            Ok(output) => output,
            Err(_) => return BleSourcePollReport::empty(),
        };
        if !output.status.success() {
            return BleSourcePollReport::empty();
        }

        parse_bluetoothctl_devices(
            &String::from_utf8_lossy(&output.stdout),
            now_unix_ms,
            self.allow_legacy_name_hints,
        )
    }
}

impl BluetoothCtlDiscoverySource {
    fn poll_signals_via_bluez_dbus(&mut self, now_unix_ms: u64) -> Option<BleSourcePollReport> {
        if self.bluez.is_none() {
            self.bluez = Connection::new_system().ok();
        }
        let managed = {
            let connection = self.bluez.as_ref()?;
            read_bluez_managed_objects(connection)?
        };
        if !self.bluez_discovery_requested {
            if let Some(path) = find_bluez_adapter_path(&managed) {
                self.bluez_adapter_path = Some(path.clone());
                self.bluez_discovery_requested = true;
                if let Some(connection) = self.bluez.as_ref() {
                    request_bluez_start_discovery(connection, &path);
                }
            }
        }
        Some(parse_bluez_managed_objects(
            &managed,
            now_unix_ms,
            self.allow_legacy_name_hints,
        ))
    }
}

fn read_bluez_managed_objects(connection: &Connection) -> Option<BluezManagedObjects> {
    let proxy = connection.with_proxy("org.bluez", "/", Duration::from_millis(1200));
    proxy.get_managed_objects().ok()
}

fn find_bluez_adapter_path(managed: &BluezManagedObjects) -> Option<String> {
    managed
        .iter()
        .find(|(_, interfaces)| interfaces.contains_key("org.bluez.Adapter1"))
        .map(|(path, _)| path.to_string())
}

fn request_bluez_start_discovery(connection: &Connection, adapter_path: &str) {
    let proxy = connection.with_proxy("org.bluez", adapter_path, Duration::from_millis(1200));
    let mut filter = PropMap::new();
    filter.insert("Transport".to_string(), Variant(Box::new("le".to_string())));
    filter.insert("DuplicateData".to_string(), Variant(Box::new(true)));
    let _: Result<(), _> = proxy.method_call("org.bluez.Adapter1", "SetDiscoveryFilter", (filter,));
    let _: Result<(), _> = proxy.method_call("org.bluez.Adapter1", "StartDiscovery", ());
}

fn parse_bluez_managed_objects(
    managed: &BluezManagedObjects,
    now_unix_ms: u64,
    allow_legacy_name_hints: bool,
) -> BleSourcePollReport {
    let mut report = BleSourcePollReport::empty();
    for interfaces in managed.values() {
        let Some(device) = interfaces.get("org.bluez.Device1") else {
            continue;
        };

        let address = prop_string(device, "Address").unwrap_or_else(|| "unknown".to_string());
        let name = prop_string(device, "Name").unwrap_or_default();
        let rssi = prop_i16(device, "RSSI");
        let has_primary_uuid = prop_string_array(device, "UUIDs")
            .iter()
            .any(|uuid| uuid_matches_aethos_primary(uuid));
        let service_data = prop_service_data(device);
        let primary_service_data = service_data
            .iter()
            .find(|(uuid, _)| uuid_matches_aethos_primary(uuid))
            .map(|(_, payload)| payload.clone());

        if has_primary_uuid {
            if let Some(payload) = primary_service_data {
                match parse_identity_payload_v1_from_service_data(&payload) {
                    Ok(identity) => report.accepted.push(DiscoverySignal {
                        peer_hint: identity.peer_hint(),
                        observed_at_unix_ms: now_unix_ms,
                        rssi,
                        bearer_type: "ble",
                        source: "bluez-dbus",
                    }),
                    Err(reject) => report.rejected.push(build_rejection(
                        reject,
                        "bluez-dbus",
                        format!(
                            "device={} name={} payload_len={}",
                            address,
                            name,
                            payload.len()
                        ),
                    )),
                }
                continue;
            }

            report.rejected.push(build_rejection(
                BleParseReject::MissingIdentityPayload,
                "bluez-dbus",
                format!("device={} name={}", address, name),
            ));
            continue;
        }

        if allow_legacy_name_hints {
            let peer_hint = if let Some(stripped) = name.strip_prefix("aethos-") {
                stripped.trim().to_string()
            } else if let Some(stripped) = name.strip_prefix("AETHOS-") {
                stripped.trim().to_string()
            } else {
                String::new()
            };
            if !peer_hint.is_empty() {
                report.accepted.push(DiscoverySignal {
                    peer_hint,
                    observed_at_unix_ms: now_unix_ms,
                    rssi,
                    bearer_type: "ble",
                    source: "bluez-dbus-legacy",
                });
                continue;
            }
        }

        report.rejected.push(build_rejection(
            BleParseReject::MissingPrimaryServiceUuid,
            "bluez-dbus",
            format!("device={} name={}", address, name),
        ));
    }
    report
}

fn prop_string(props: &PropMap, key: &str) -> Option<String> {
    props.get(key)?.0.as_str().map(|value| value.to_string())
}

fn prop_i16(props: &PropMap, key: &str) -> Option<i16> {
    let value = props.get(key)?.0.as_i64()?;
    i16::try_from(value).ok()
}

fn prop_string_array(props: &PropMap, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|value| value.0.as_iter())
        .map(|iter| {
            iter.filter_map(|entry| entry.as_str().map(|value| value.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn prop_service_data(props: &PropMap) -> HashMap<String, Vec<u8>> {
    let mut out = HashMap::new();
    let Some(raw) = props.get("ServiceData") else {
        return out;
    };
    let Some(entries) = raw.0.as_iter() else {
        return out;
    };

    for entry in entries {
        let Some(mut parts) = entry.as_iter() else {
            continue;
        };
        let Some(key) = parts
            .next()
            .and_then(|item| item.as_str().map(|value| value.to_string()))
        else {
            continue;
        };
        let Some(value_arg) = parts.next() else {
            continue;
        };
        let payload_arg = unwrap_variant(value_arg);
        if let Some(payload) = refarg_bytes(payload_arg) {
            out.insert(key, payload);
        }
    }

    out
}

fn unwrap_variant(arg: &dyn RefArg) -> &dyn RefArg {
    if arg.arg_type() != ArgType::Variant {
        return arg;
    }
    arg.as_iter()
        .and_then(|mut iter| iter.next())
        .unwrap_or(arg)
}

fn refarg_bytes(arg: &dyn RefArg) -> Option<Vec<u8>> {
    let iter = arg.as_iter()?;
    let mut out = Vec::new();
    for item in iter {
        let value = item.as_u64()?;
        out.push(u8::try_from(value).ok()?);
    }
    Some(out)
}

fn uuid_matches_aethos_primary(raw: &str) -> bool {
    normalize_uuid_text(raw) == normalize_uuid_text(AETHOS_BLE_PRIMARY_SERVICE_UUID)
}

fn normalize_uuid_text(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<String>()
}

fn parse_identity_payload_v1_from_service_data(
    payload: &[u8],
) -> Result<BleIdentityPayloadV1, BleParseReject> {
    if payload.len() != BLE_IDENTITY_V1_PAYLOAD_LEN {
        return Err(BleParseReject::MalformedPayloadLength);
    }
    if payload[0] != BLE_IDENTITY_V1_VERSION {
        return Err(BleParseReject::UnsupportedVersion);
    }
    let flags = payload[1];
    if flags & 0b1111_1100 != 0 {
        return Err(BleParseReject::ReservedFlagBitsSet);
    }
    let capabilities_raw = u16::from_le_bytes([payload[2], payload[3]]);
    let mut identity_ref = [0u8; BLE_IDENTITY_V1_IDENTITY_REF_LEN];
    identity_ref.copy_from_slice(&payload[4..12]);
    if identity_ref.iter().all(|byte| *byte == 0) {
        return Err(BleParseReject::InvalidOrZeroIdentityRef);
    }
    let normalized_bits = capabilities_raw & 0x0007;
    Ok(BleIdentityPayloadV1 {
        version: payload[0],
        identity_rotating: flags & 0x01 != 0,
        identity_private: flags & 0x02 != 0,
        capabilities: BleIdentityCapabilities {
            lan: normalized_bits & 0x0001 != 0,
            mpc: normalized_bits & 0x0002 != 0,
            relay: normalized_bits & 0x0004 != 0,
            normalized_bits,
            raw_bits: capabilities_raw,
        },
        identity_ref,
    })
}

fn parse_bluetoothctl_devices(
    raw: &str,
    now_unix_ms: u64,
    allow_legacy_name_hints: bool,
) -> BleSourcePollReport {
    let mut report = BleSourcePollReport::empty();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("Device ") {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let _device = parts.next();
        let Some(mac_raw) = parts.next() else {
            continue;
        };
        let mac = mac_raw.to_ascii_lowercase();
        let name = parts.collect::<Vec<_>>().join(" ");
        if allow_legacy_name_hints {
            let peer_hint = if let Some(stripped) = name.strip_prefix("aethos-") {
                stripped.trim().to_string()
            } else if let Some(stripped) = name.strip_prefix("AETHOS-") {
                stripped.trim().to_string()
            } else {
                String::new()
            };
            if !peer_hint.is_empty() {
                report.accepted.push(DiscoverySignal {
                    peer_hint,
                    observed_at_unix_ms: now_unix_ms,
                    rssi: None,
                    bearer_type: "ble",
                    source: "bluetoothctl-legacy",
                });
                continue;
            }
        }

        report.rejected.push(build_rejection(
            BleParseReject::MissingPrimaryServiceUuid,
            "bluetoothctl",
            format!("device={mac} name={name}"),
        ));
    }
    report
}

fn parse_ble_identity_v1(
    primary_advertisement: &[u8],
    scan_response: &[u8],
) -> Result<BleIdentityPayloadV1, BleParseReject> {
    ensure_primary_uuid_list_contains_aethos(primary_advertisement)?;
    let scan_payload = extract_identity_payload_from_pdu(scan_response)?;
    let primary_payload = extract_identity_payload_from_pdu(primary_advertisement)?;
    let payload = match (scan_payload, primary_payload) {
        (Some(scan), Some(primary)) => {
            if scan != primary {
                return Err(BleParseReject::ConflictingIdentityPayloadCopies);
            }
            scan
        }
        (Some(scan), None) => scan,
        (None, Some(primary)) => primary,
        (None, None) => return Err(BleParseReject::MissingIdentityPayload),
    };

    if payload.len() != BLE_IDENTITY_V1_PAYLOAD_LEN {
        return Err(BleParseReject::MalformedPayloadLength);
    }
    if payload[0] != BLE_IDENTITY_V1_VERSION {
        return Err(BleParseReject::UnsupportedVersion);
    }
    let flags = payload[1];
    if flags & 0b1111_1100 != 0 {
        return Err(BleParseReject::ReservedFlagBitsSet);
    }
    let capabilities_raw = u16::from_le_bytes([payload[2], payload[3]]);
    let mut identity_ref = [0u8; BLE_IDENTITY_V1_IDENTITY_REF_LEN];
    identity_ref.copy_from_slice(&payload[4..12]);
    if identity_ref.iter().all(|byte| *byte == 0) {
        return Err(BleParseReject::InvalidOrZeroIdentityRef);
    }

    let normalized_bits = capabilities_raw & 0x0007;
    Ok(BleIdentityPayloadV1 {
        version: payload[0],
        identity_rotating: flags & 0x01 != 0,
        identity_private: flags & 0x02 != 0,
        capabilities: BleIdentityCapabilities {
            lan: normalized_bits & 0x0001 != 0,
            mpc: normalized_bits & 0x0002 != 0,
            relay: normalized_bits & 0x0004 != 0,
            normalized_bits,
            raw_bits: capabilities_raw,
        },
        identity_ref,
    })
}

fn ensure_primary_uuid_list_contains_aethos(
    primary_advertisement: &[u8],
) -> Result<(), BleParseReject> {
    let ad_structures = parse_ad_structures(primary_advertisement)?;
    let mut saw_aethos_uuid = false;
    for (ad_type, data) in ad_structures {
        if ad_type != 0x06 && ad_type != 0x07 {
            continue;
        }
        if data.len() % 16 != 0 {
            return Err(BleParseReject::MalformedPrimaryServiceUuidList);
        }
        if data
            .chunks_exact(16)
            .any(|uuid| uuid == AETHOS_BLE_PRIMARY_SERVICE_UUID_LE)
        {
            saw_aethos_uuid = true;
        }
    }
    if saw_aethos_uuid {
        Ok(())
    } else {
        Err(BleParseReject::MissingPrimaryServiceUuid)
    }
}

fn extract_identity_payload_from_pdu(pdu: &[u8]) -> Result<Option<Vec<u8>>, BleParseReject> {
    let ad_structures = parse_ad_structures(pdu)?;
    let mut payload: Option<Vec<u8>> = None;
    for (ad_type, data) in ad_structures {
        if ad_type != 0x21 {
            continue;
        }
        if data.len() < 16 {
            return Err(BleParseReject::MalformedPayloadLength);
        }
        if data[0..16] != AETHOS_BLE_PRIMARY_SERVICE_UUID_LE {
            continue;
        }
        if payload.is_some() {
            return Err(BleParseReject::DuplicateIdentityPayloadInPdu);
        }
        payload = Some(data[16..].to_vec());
    }
    Ok(payload)
}

fn parse_ad_structures(raw: &[u8]) -> Result<Vec<(u8, &[u8])>, BleParseReject> {
    let mut entries = Vec::new();
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let len = raw[cursor] as usize;
        cursor += 1;
        if len == 0 {
            break;
        }
        if cursor + len > raw.len() {
            return Err(BleParseReject::MalformedAdStructure);
        }
        let ad_type = raw[cursor];
        let data = &raw[cursor + 1..cursor + len];
        entries.push((ad_type, data));
        cursor += len;
    }
    Ok(entries)
}

fn build_rejection(
    reject: BleParseReject,
    source: &'static str,
    detail: String,
) -> BleObservationRejection {
    BleObservationRejection {
        reason_code: reject.as_reason_code(),
        reason_label: reject.as_reason_label(),
        source,
        detail,
    }
}

fn hex_decode(raw: &str) -> Option<Vec<u8>> {
    let cleaned = raw
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>();
    if cleaned.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for idx in (0..cleaned.len()).step_by(2) {
        let byte = u8::from_str_radix(&cleaned[idx..idx + 2], 16).ok()?;
        out.push(byte);
    }
    Some(out)
}

fn bytes_to_hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(nibble_to_hex((byte >> 4) & 0x0f));
        out.push(nibble_to_hex(byte & 0x0f));
    }
    out
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => '0',
    }
}

#[allow(dead_code)]
pub fn canonical_ble_primary_service_uuid() -> &'static str {
    AETHOS_BLE_PRIMARY_SERVICE_UUID
}

#[allow(dead_code)]
pub fn build_primary_uuid_list_ad() -> Vec<u8> {
    let mut out = vec![17u8, 0x07u8];
    out.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
    out
}

#[allow(dead_code)]
pub fn build_identity_service_data_ad(payload: &BleIdentityPayloadV1) -> Vec<u8> {
    let mut out = vec![29u8, 0x21u8];
    out.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
    out.push(payload.version);
    let mut flags = 0u8;
    if payload.identity_rotating {
        flags |= 0x01;
    }
    if payload.identity_private {
        flags |= 0x02;
    }
    out.push(flags);
    out.extend_from_slice(&payload.capabilities.raw_bits.to_le_bytes());
    out.extend_from_slice(&payload.identity_ref);
    out
}

#[allow(dead_code)]
pub fn parse_canonical_ble_observation(
    primary_advertisement: &[u8],
    scan_response: &[u8],
    now_unix_ms: u64,
    rssi: Option<i16>,
    source: &'static str,
) -> Result<DiscoverySignal, BleObservationRejection> {
    let identity =
        parse_ble_identity_v1(primary_advertisement, scan_response).map_err(|reject| {
            build_rejection(
                reject,
                source,
                format!(
                    "uuid={} primary_len={} scan_len={}",
                    AETHOS_BLE_PRIMARY_SERVICE_UUID,
                    primary_advertisement.len(),
                    scan_response.len()
                ),
            )
        })?;
    Ok(DiscoverySignal {
        peer_hint: identity.peer_hint(),
        observed_at_unix_ms: now_unix_ms,
        rssi,
        bearer_type: "ble",
        source,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn canonical_primary_advertisement() -> Vec<u8> {
        build_primary_uuid_list_ad()
    }

    fn canonical_scan_response(
        payload_overrides: Option<[u8; BLE_IDENTITY_V1_PAYLOAD_LEN]>,
    ) -> Vec<u8> {
        let payload = payload_overrides.unwrap_or([
            0x01, 0x03, 0x07, 0x00, 0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe,
        ]);
        let mut out = vec![29u8, 0x21u8];
        out.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn simulated_source_emits_once_for_deterministic_harness() {
        let mut source = SimulatedBleDiscoverySource::from_env_string(
            "ad:11074eee0dd26c0ef787f950295a85a51a18|sr:1d214eee0dd26c0ef787f950295a85a51a1801030700deadbeefcafebabe@-55,peer-beta@-49",
        );
        let first = source.poll_signals(1000);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].peer_hint, "ble-idref:deadbeefcafebabe");
        assert_eq!(first[0].rssi, Some(-55));
        let second = source.poll_signals(2000);
        assert!(second.is_empty());
    }

    #[test]
    fn gate_dedupes_duplicate_signals_within_window() {
        struct InlineSource {
            frames: Vec<Vec<DiscoverySignal>>,
        }
        impl BleDiscoverySource for InlineSource {
            fn poll_signals(&mut self, _now_unix_ms: u64) -> Vec<DiscoverySignal> {
                if self.frames.is_empty() {
                    return Vec::new();
                }
                self.frames.remove(0)
            }
        }

        let signal = DiscoverySignal {
            peer_hint: "peer-1".to_string(),
            observed_at_unix_ms: 1000,
            rssi: Some(-60),
            bearer_type: "ble",
            source: "test",
        };
        let signal_soon = DiscoverySignal {
            observed_at_unix_ms: 1500,
            ..signal.clone()
        };
        let signal_later = DiscoverySignal {
            observed_at_unix_ms: 9000,
            ..signal
        };
        let mut source = InlineSource {
            frames: vec![vec![signal_soon.clone()], vec![signal_later.clone()]],
        };
        let mut gate = BleDiscoveryGate::new(Duration::from_secs(5));
        gate.last_seen_by_peer.insert("peer-1".to_string(), 1000);

        let first = gate.poll_ready(&mut source, 1500);
        assert!(first.is_empty());
        let second = gate.poll_ready(&mut source, 9000);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].observed_at_unix_ms, 9000);
    }

    #[test]
    fn gate_reports_deduped_counts_for_visibility_layers() {
        struct InlineSource {
            signals: Vec<DiscoverySignal>,
        }
        impl BleDiscoverySource for InlineSource {
            fn poll_signals(&mut self, _now_unix_ms: u64) -> Vec<DiscoverySignal> {
                std::mem::take(&mut self.signals)
            }
        }

        let mut gate = BleDiscoveryGate::new(Duration::from_secs(5));
        gate.last_seen_by_peer.insert("peer-a".to_string(), 10_000);
        let mut source = InlineSource {
            signals: vec![
                DiscoverySignal {
                    peer_hint: "peer-a".to_string(),
                    observed_at_unix_ms: 12_000,
                    rssi: Some(-55),
                    bearer_type: "ble",
                    source: "test",
                },
                DiscoverySignal {
                    peer_hint: "peer-b".to_string(),
                    observed_at_unix_ms: 12_000,
                    rssi: Some(-57),
                    bearer_type: "ble",
                    source: "test",
                },
            ],
        };

        let result = gate.poll_ready_with_stats(&mut source, 12_000);
        assert_eq!(result.deduped_count, 1);
        assert_eq!(result.ready.len(), 1);
        assert_eq!(result.ready[0].peer_hint, "peer-b");
        assert!(result.rejected.is_empty());
    }

    #[test]
    fn parser_supports_aethos_name_or_ephemeral_fallback() {
        let raw = "Device AA:BB:CC:DD:EE:FF aethos-wayfarer-peer\nDevice 11:22:33:44:55:66 Other Device\n";
        let strict = parse_bluetoothctl_devices(raw, 123, false);
        assert!(strict.accepted.is_empty());
        assert_eq!(strict.rejected.len(), 2);
        assert_eq!(
            strict.rejected[0].reason_code,
            "missing_primary_service_uuid"
        );

        let legacy = parse_bluetoothctl_devices(raw, 123, true);
        assert_eq!(legacy.accepted.len(), 1);
        assert_eq!(legacy.accepted[0].peer_hint, "wayfarer-peer");
    }

    #[test]
    fn canonical_advertisement_is_accepted() {
        let parsed = parse_ble_identity_v1(
            &canonical_primary_advertisement(),
            &canonical_scan_response(None),
        )
        .expect("must parse canonical identity");
        assert_eq!(parsed.version, 1);
        assert!(parsed.identity_rotating);
        assert!(parsed.identity_private);
        assert_eq!(parsed.peer_hint(), "ble-idref:deadbeefcafebabe");
    }

    #[test]
    fn wrong_uuid_is_rejected() {
        let mut primary = canonical_primary_advertisement();
        primary[2] = 0xff;
        let error = parse_ble_identity_v1(&primary, &canonical_scan_response(None))
            .expect_err("must reject wrong uuid");
        assert_eq!(error.as_reason_label(), "missing primary service UUID");
    }

    #[test]
    fn malformed_payload_length_is_rejected() {
        let mut scan = canonical_scan_response(None);
        scan[0] = 0x1e;
        scan.push(0xff);
        let error = parse_ble_identity_v1(&canonical_primary_advertisement(), &scan)
            .expect_err("must reject malformed payload length");
        assert_eq!(error.as_reason_label(), "malformed payload length");
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut payload = [0u8; BLE_IDENTITY_V1_PAYLOAD_LEN];
        payload.copy_from_slice(&[0x02, 0x03, 0x07, 0x00, 1, 2, 3, 4, 5, 6, 7, 8]);
        let error = parse_ble_identity_v1(
            &canonical_primary_advertisement(),
            &canonical_scan_response(Some(payload)),
        )
        .expect_err("must reject unsupported version");
        assert_eq!(error.as_reason_label(), "unsupported version");
    }

    #[test]
    fn zero_identity_ref_is_rejected() {
        let payload = [0x01, 0x03, 0x07, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        let error = parse_ble_identity_v1(
            &canonical_primary_advertisement(),
            &canonical_scan_response(Some(payload)),
        )
        .expect_err("must reject zero identity_ref");
        assert_eq!(error.as_reason_label(), "invalid or zero identity_ref");
    }

    #[test]
    fn capabilities_decode_is_normalized_and_forward_compatible() {
        let payload = [0x01, 0x00, 0x0f, 0x80, 1, 2, 3, 4, 5, 6, 7, 8];
        let parsed = parse_ble_identity_v1(
            &canonical_primary_advertisement(),
            &canonical_scan_response(Some(payload)),
        )
        .expect("must parse capabilities");
        assert_eq!(parsed.capabilities.raw_bits, 0x800f);
        assert_eq!(parsed.capabilities.normalized_bits, 0x0007);
        assert!(parsed.capabilities.lan);
        assert!(parsed.capabilities.mpc);
        assert!(parsed.capabilities.relay);
    }

    #[test]
    fn parse_canonical_observation_returns_structured_rejection_reason() {
        let mut primary = canonical_primary_advertisement();
        primary[0] = 16;
        let rejected = parse_canonical_ble_observation(
            &primary,
            &canonical_scan_response(None),
            10,
            None,
            "test",
        )
        .expect_err("must reject malformed advertisement");
        assert_eq!(rejected.reason_code, "malformed_ad_structure");
        assert_eq!(rejected.reason_label, "malformed AD structure");
    }

    #[derive(Debug, Deserialize)]
    struct FixtureManifest {
        fixtures: Vec<FixtureManifestEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct FixtureManifestEntry {
        fixture: String,
    }

    #[derive(Debug, Deserialize)]
    struct BleIdentityFixture {
        #[serde(rename = "fixtureID")]
        fixture_id: String,
        #[serde(rename = "primaryAdvertisementHex")]
        primary_advertisement_hex: String,
        #[serde(rename = "scanResponseHex")]
        scan_response_hex: String,
        expected: BleIdentityFixtureExpected,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "outcome", rename_all = "lowercase")]
    enum BleIdentityFixtureExpected {
        Accepted {
            #[serde(rename = "peerHint")]
            peer_hint: String,
            version: u8,
            #[serde(rename = "identityRotating")]
            identity_rotating: bool,
            #[serde(rename = "identityPrivate")]
            identity_private: bool,
            #[serde(rename = "capabilitiesNormalizedBits")]
            capabilities_normalized_bits: u16,
            #[serde(rename = "capabilitiesRawBits")]
            capabilities_raw_bits: u16,
        },
        Rejected {
            #[serde(rename = "reasonCode")]
            reason_code: String,
            #[serde(rename = "reasonLabel")]
            reason_label: String,
        },
    }

    fn fixture_suite_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/ble/identity-v1")
    }

    #[test]
    fn fixture_suite_matches_canonical_acceptance_and_rejection_diagnostics() {
        let manifest_path = fixture_suite_root().join("manifest.json");
        let manifest_raw = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("failed reading {}: {err}", manifest_path.display()));
        let manifest: FixtureManifest = serde_json::from_str(&manifest_raw)
            .unwrap_or_else(|err| panic!("failed parsing {}: {err}", manifest_path.display()));

        assert!(
            !manifest.fixtures.is_empty(),
            "fixture manifest must include at least one vector"
        );

        for entry in manifest.fixtures {
            let fixture_path = fixture_suite_root().join(entry.fixture.trim_start_matches("./"));
            let raw = fs::read_to_string(&fixture_path)
                .unwrap_or_else(|err| panic!("failed reading {}: {err}", fixture_path.display()));
            let fixture: BleIdentityFixture = serde_json::from_str(&raw)
                .unwrap_or_else(|err| panic!("failed parsing {}: {err}", fixture_path.display()));

            let primary = hex_decode(&fixture.primary_advertisement_hex).unwrap_or_else(|| {
                panic!(
                    "fixture {} has invalid primaryAdvertisementHex",
                    fixture.fixture_id
                )
            });
            let scan = hex_decode(&fixture.scan_response_hex).unwrap_or_else(|| {
                panic!("fixture {} has invalid scanResponseHex", fixture.fixture_id)
            });

            match fixture.expected {
                BleIdentityFixtureExpected::Accepted {
                    peer_hint,
                    version,
                    identity_rotating,
                    identity_private,
                    capabilities_normalized_bits,
                    capabilities_raw_bits,
                } => {
                    let parsed = parse_ble_identity_v1(&primary, &scan).unwrap_or_else(|err| {
                        panic!(
                            "fixture {} expected accepted but got rejection: {}",
                            fixture.fixture_id,
                            err.as_reason_code()
                        )
                    });
                    assert_eq!(
                        parsed.peer_hint(),
                        peer_hint,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(parsed.version, version, "fixture {}", fixture.fixture_id);
                    assert_eq!(
                        parsed.identity_rotating, identity_rotating,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(
                        parsed.identity_private, identity_private,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(
                        parsed.capabilities.normalized_bits, capabilities_normalized_bits,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(
                        parsed.capabilities.raw_bits, capabilities_raw_bits,
                        "fixture {}",
                        fixture.fixture_id
                    );

                    let observation = parse_canonical_ble_observation(
                        &primary,
                        &scan,
                        1_700_000_000_000,
                        Some(-51),
                        "fixture-suite",
                    )
                    .unwrap_or_else(|err| {
                        panic!(
                            "fixture {} expected accepted observation but got rejection: {}",
                            fixture.fixture_id, err.reason_code
                        )
                    });
                    assert_eq!(
                        observation.peer_hint, peer_hint,
                        "fixture {}",
                        fixture.fixture_id
                    );
                }
                BleIdentityFixtureExpected::Rejected {
                    reason_code,
                    reason_label,
                } => {
                    let rejection = parse_ble_identity_v1(&primary, &scan)
                        .expect_err("fixture marked rejected must fail closed");
                    assert_eq!(
                        rejection.as_reason_code(),
                        reason_code,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(
                        rejection.as_reason_label(),
                        reason_label,
                        "fixture {}",
                        fixture.fixture_id
                    );

                    let observation = parse_canonical_ble_observation(
                        &primary,
                        &scan,
                        1_700_000_000_000,
                        None,
                        "fixture-suite",
                    )
                    .expect_err("fixture marked rejected must fail closed");
                    assert_eq!(
                        observation.reason_code, reason_code,
                        "fixture {}",
                        fixture.fixture_id
                    );
                    assert_eq!(
                        observation.reason_label, reason_label,
                        "fixture {}",
                        fixture.fixture_id
                    );
                }
            }
        }
    }
}
