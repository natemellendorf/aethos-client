use std::collections::HashMap;
use std::process::Command;
use std::time::{Duration, Instant};

use dbus::arg::{ArgType, PropMap, RefArg, Variant};
use dbus::blocking::stdintf::org_freedesktop_dbus::ObjectManager;
use dbus::blocking::Connection;
use dbus::Path;

// ── Frozen constants (unchanged across protocol versions) ───────────────────

/// Primary Aethos Discovery Service UUID (128-bit, frozen).
/// Derivation: UUIDv5(DNS, "aethos.ble.discovery.identity.v1") — name retains
/// `v1` because the UUID is frozen and MUST NOT change across versions.
pub const AETHOS_BLE_PRIMARY_SERVICE_UUID: &str = "181aa585-5a29-50f9-87f7-0e6cd20dee4e";

/// Little-endian BLE byte order of the primary service UUID (on-air encoding).
pub const AETHOS_BLE_PRIMARY_SERVICE_UUID_LE: [u8; 16] = [
    0x4e, 0xee, 0x0d, 0xd2, 0x6c, 0x0e, 0xf7, 0x87, 0xf9, 0x50, 0x29, 0x5a, 0x85, 0xa5, 0x1a, 0x18,
];

type BluezManagedObjects = HashMap<Path<'static>, HashMap<String, PropMap>>;

// ── V2 wakeup hint types ────────────────────────────────────────────────────

/// A validated BLE wakeup hint per v2 §7.1.
///
/// Carries NO identity — only signals that an Aethos peer is nearby.
/// The `ble_address` is unstable and non-identifying (v2 §9.3); it is used
/// solely for debounce (§7.3) and activation window tracking (§8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeupHint {
    pub ble_address: String,
    pub received_at: Instant,
}

// ── Discovery signal (v2 semantics) ─────────────────────────────────────────

/// A discovery signal emitted by a BLE source.
///
/// In v2 the `peer_hint` field contains a BLE address (or opaque handle for
/// simulated sources). This value is **unstable and non-identifying** — it is
/// used for debounce and activation-window keying only.  Identity is
/// established exclusively through the post-connection Encounter handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySignal {
    pub peer_hint: String,
    pub observed_at_unix_ms: u64,
    pub rssi: Option<i16>,
    pub bearer_type: &'static str,
    pub source: &'static str,
}

// ── Observation rejection ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleObservationRejection {
    pub reason_code: &'static str,
    pub reason_label: &'static str,
    pub source: &'static str,
    pub detail: String,
}

// ── Poll report ─────────────────────────────────────────────────────────────

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

// ── V2 parse rejection (slimmed — identity variants removed) ────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BleParseReject {
    /// Advertisement does not contain the Aethos UUID in a valid UUID-list.
    MissingPrimaryServiceUuid,
    /// AD type 0x06/0x07 UUID-list has invalid length ((len-1) % 16 != 0).
    MalformedPrimaryServiceUuidList,
    /// Raw AD bytes are structurally invalid.
    MalformedAdStructure,
}

impl BleParseReject {
    fn as_reason_code(self) -> &'static str {
        match self {
            Self::MissingPrimaryServiceUuid => "missing_primary_service_uuid",
            Self::MalformedPrimaryServiceUuidList => "malformed_primary_service_uuid_list",
            Self::MalformedAdStructure => "malformed_ad_structure",
        }
    }

    fn as_reason_label(self) -> &'static str {
        match self {
            Self::MissingPrimaryServiceUuid => "missing primary service UUID",
            Self::MalformedPrimaryServiceUuidList => "malformed primary service UUID list",
            Self::MalformedAdStructure => "malformed AD structure",
        }
    }
}

// ── V2 wakeup hint acceptance (§6) ──────────────────────────────────────────

/// Accept a BLE advertisement as an Aethos v2 wakeup hint.
///
/// Per v2 §6.1 the advertisement is accepted if and only if AD type 0x06 or
/// 0x07 contains the Aethos primary service UUID in a structurally valid
/// UUID-list.  Any AD type 0x21 (service data) is ignored per §6.3.
///
/// Returns `Ok(())` on acceptance or the structural rejection reason.
pub fn accept_v2_wakeup_hint(ad_bytes: &[u8]) -> Result<(), BleParseReject> {
    ensure_primary_uuid_list_contains_aethos(ad_bytes)
}

/// Accept a raw AD observation and produce a `DiscoverySignal` if valid.
///
/// This is the v2 replacement for the former `parse_canonical_ble_observation`.
/// No identity payload is inspected — only UUID presence in a valid list.
#[allow(dead_code)]
pub fn accept_v2_wakeup_observation(
    ad_bytes: &[u8],
    ble_address: &str,
    now_unix_ms: u64,
    rssi: Option<i16>,
    source: &'static str,
) -> Result<DiscoverySignal, BleObservationRejection> {
    accept_v2_wakeup_hint(ad_bytes).map_err(|reject| {
        build_rejection(
            reject,
            source,
            format!(
                "uuid={} address={} ad_len={}",
                AETHOS_BLE_PRIMARY_SERVICE_UUID,
                ble_address,
                ad_bytes.len()
            ),
        )
    })?;
    Ok(DiscoverySignal {
        peer_hint: ble_address.to_string(),
        observed_at_unix_ms: now_unix_ms,
        rssi,
        bearer_type: "ble",
        source,
    })
}

// ── Discovery source trait ──────────────────────────────────────────────────

pub trait BleDiscoverySource {
    fn poll_signals(&mut self, now_unix_ms: u64) -> Vec<DiscoverySignal>;

    fn poll_signals_with_diagnostics(&mut self, now_unix_ms: u64) -> BleSourcePollReport {
        BleSourcePollReport {
            accepted: self.poll_signals(now_unix_ms),
            rejected: Vec::new(),
        }
    }
}

// ── Debounce gate (v2 §7.3 — BLE-address keyed, 30s default) ───────────────

pub struct BleDiscoveryGate {
    debounce_window: Duration,
    last_seen_by_address: HashMap<String, u64>,
}

pub struct GatePollResult {
    pub ready: Vec<DiscoverySignal>,
    pub deduped_count: usize,
    pub rejected: Vec<BleObservationRejection>,
}

impl BleDiscoveryGate {
    pub fn new(debounce_window: Duration) -> Self {
        Self {
            debounce_window,
            last_seen_by_address: HashMap::new(),
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
                .last_seen_by_address
                .get(&signal.peer_hint)
                .map(|previous| {
                    signal.observed_at_unix_ms.saturating_sub(*previous)
                        >= self.debounce_window.as_millis() as u64
                })
                .unwrap_or(true);
            if allow {
                self.last_seen_by_address
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

// ── Discovery activation window (v2 §8) ────────────────────────────────────

/// Tracks per-BLE-address discovery activation windows.
///
/// After a wakeup hint passes debounce, an activation window is opened for
/// that address.  During the window the scanner MAY attempt connections.
/// After expiry, no connection attempts are made until the next post-debounce
/// wakeup hint.
pub struct ActivationWindowTracker {
    window_duration: Duration,
    max_concurrent: usize,
    active: HashMap<String, Instant>,
}

impl ActivationWindowTracker {
    /// Create a new tracker.
    ///
    /// * `window_duration` — length of each activation window (v2 §8.2: 10-60s).
    /// * `max_concurrent` — maximum simultaneous windows (v2 §8.3: recommended 4).
    pub fn new(window_duration: Duration, max_concurrent: usize) -> Self {
        Self {
            window_duration,
            max_concurrent,
            active: HashMap::new(),
        }
    }

    /// Expire windows whose lifetime has elapsed.
    pub fn expire_stale(&mut self) {
        let now = Instant::now();
        self.active
            .retain(|_, opened_at| now.duration_since(*opened_at) < self.window_duration);
    }

    /// Try to open a window for `ble_address`.
    ///
    /// Returns `true` if a window was opened (or was already active).
    /// Returns `false` if the concurrent window limit is reached.
    pub fn open_window(&mut self, ble_address: &str) -> bool {
        self.expire_stale();
        if self.active.contains_key(ble_address) {
            return true; // already active
        }
        if self.active.len() >= self.max_concurrent {
            return false; // at capacity
        }
        self.active.insert(ble_address.to_string(), Instant::now());
        true
    }

    /// Check whether `ble_address` has an active window.
    #[allow(dead_code)]
    pub fn is_active(&self, ble_address: &str) -> bool {
        self.active.get(ble_address).is_some_and(|opened_at| {
            Instant::now().duration_since(*opened_at) < self.window_duration
        })
    }

    /// Number of currently active windows (before expiry sweep).
    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

// ── Discovery adapter ───────────────────────────────────────────────────────

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

// ── Simulated source (v2 — UUID-only matching) ─────────────────────────────

#[derive(Debug, Clone)]
struct SimulatedSignalSeed {
    kind: SimulatedSignalSeedKind,
    rssi: Option<i16>,
}

#[derive(Debug, Clone)]
enum SimulatedSignalSeedKind {
    /// Raw AD bytes — validated for UUID presence only (v2).
    Canonical { ad_hex: String, ble_address: String },
    /// Bare peer-hint string (test convenience, no AD validation).
    LegacyPeerHint(String),
}

pub struct SimulatedBleDiscoverySource {
    pending: Vec<SimulatedSignalSeed>,
    emitted_once: bool,
}

impl SimulatedBleDiscoverySource {
    /// Parse the `AETHOS_BLE_SIMULATED_SIGNALS` env-var format.
    ///
    /// V2 format: `ad:<ad_hex>|addr:<ble_address>@<rssi>` — UUID-only check.
    /// Legacy format: `<peer_hint>@<rssi>` — pass-through, no AD validation.
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
                    let (ad_hex, addr) = ad.split_once("|addr:")?;
                    SimulatedSignalSeedKind::Canonical {
                        ad_hex: ad_hex.trim().to_string(),
                        ble_address: addr.trim().to_string(),
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
                    ad_hex,
                    ble_address,
                } => {
                    let Some(ad_bytes) = hex_decode(ad_hex) else {
                        report.rejected.push(build_rejection(
                            BleParseReject::MalformedAdStructure,
                            "simulated",
                            format!("invalid ad hex: {ad_hex}"),
                        ));
                        continue;
                    };

                    match accept_v2_wakeup_hint(&ad_bytes) {
                        Ok(()) => report.accepted.push(DiscoverySignal {
                            peer_hint: ble_address.clone(),
                            observed_at_unix_ms: now_unix_ms,
                            rssi: seed.rssi,
                            bearer_type: "ble",
                            source: "simulated",
                        }),
                        Err(reject) => report.rejected.push(build_rejection(
                            reject,
                            "simulated",
                            format!("wakeup hint rejected for address={ble_address}"),
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

// ── BlueZ D-Bus source ─────────────────────────────────────────────────────

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

// ── BlueZ D-Bus helpers ────────────────────────────────────────────────────

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

/// Parse BlueZ D-Bus managed objects for v2 wakeup hints.
///
/// Acceptance: device exposes the Aethos UUID in its `UUIDs` property.
/// The BLE address is used as the `peer_hint` (unstable, for debounce only).
/// Any service data (0x21) is ignored per v2 §6.3.
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

        if has_primary_uuid {
            // V2: UUID presence is sufficient. No payload inspection.
            // Service data (0x21) is ignored per §6.3.
            report.accepted.push(DiscoverySignal {
                peer_hint: address.clone(),
                observed_at_unix_ms: now_unix_ms,
                rssi,
                bearer_type: "ble",
                source: "bluez-dbus",
            });
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

// ── D-Bus property helpers ─────────────────────────────────────────────────

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

#[allow(dead_code)]
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

// ── UUID matching ───────────────────────────────────────────────────────────

fn uuid_matches_aethos_primary(raw: &str) -> bool {
    normalize_uuid_text(raw) == normalize_uuid_text(AETHOS_BLE_PRIMARY_SERVICE_UUID)
}

fn normalize_uuid_text(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<String>()
}

// ── bluetoothctl text parser ────────────────────────────────────────────────

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

// ── AD structure parsing (reused from v1, unchanged) ────────────────────────

/// Check that a raw AD byte stream contains the Aethos primary UUID in an
/// AD type 0x06 or 0x07 UUID-list with valid structure.
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

// ── Rejection builder ───────────────────────────────────────────────────────

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

// ── Hex helpers ─────────────────────────────────────────────────────────────

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

#[allow(dead_code)]
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

// ── Public helpers ──────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn canonical_ble_primary_service_uuid() -> &'static str {
    AETHOS_BLE_PRIMARY_SERVICE_UUID
}

/// Build a conforming v2 advertisement PDU: AD type 0x07 with the Aethos UUID.
#[allow(dead_code)]
pub fn build_primary_uuid_list_ad() -> Vec<u8> {
    let mut out = vec![17u8, 0x07u8];
    out.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
    out
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── AD byte builders for test convenience ───────────────────────────

    /// Canonical v2 advertisement: AD type 0x07 with the Aethos UUID only.
    fn v2_uuid_only_ad() -> Vec<u8> {
        build_primary_uuid_list_ad()
    }

    /// AD type 0x06 (Incomplete List) with the Aethos UUID.
    fn v2_uuid_incomplete_list_ad() -> Vec<u8> {
        let mut out = vec![17u8, 0x06u8];
        out.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
        out
    }

    /// AD with Aethos UUID in 0x07 AND a legacy 0x21 service data entry.
    fn v2_uuid_with_service_data_ad() -> Vec<u8> {
        let mut out = v2_uuid_only_ad();
        // Append AD type 0x21 with Aethos UUID + 12 bytes of junk payload
        let mut sd = vec![29u8, 0x21u8];
        sd.extend_from_slice(&AETHOS_BLE_PRIMARY_SERVICE_UUID_LE);
        sd.extend_from_slice(&[
            0x01, 0x03, 0x07, 0x00, 0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0xbe,
        ]);
        out.extend_from_slice(&sd);
        out
    }

    /// AD with no Aethos UUID at all.
    fn non_aethos_ad() -> Vec<u8> {
        let mut out = vec![17u8, 0x07u8];
        // Random non-Aethos UUID
        out.extend_from_slice(&[0x00; 16]);
        out
    }

    // ── accept_v2_wakeup_hint tests ─────────────────────────────────────

    #[test]
    fn v2_uuid_in_0x07_is_accepted() {
        assert!(accept_v2_wakeup_hint(&v2_uuid_only_ad()).is_ok());
    }

    #[test]
    fn v2_uuid_in_0x06_is_accepted() {
        assert!(accept_v2_wakeup_hint(&v2_uuid_incomplete_list_ad()).is_ok());
    }

    #[test]
    fn v2_uuid_absent_is_rejected() {
        let err = accept_v2_wakeup_hint(&non_aethos_ad()).unwrap_err();
        assert_eq!(err, BleParseReject::MissingPrimaryServiceUuid);
    }

    #[test]
    fn v2_service_data_alongside_uuid_is_ignored_and_accepted() {
        // §6.3: presence of 0x21 does not invalidate the wakeup hint.
        assert!(accept_v2_wakeup_hint(&v2_uuid_with_service_data_ad()).is_ok());
    }

    #[test]
    fn v2_invalid_uuid_list_length_is_rejected() {
        // 0x07 entry with 15 bytes (not multiple of 16)
        let ad = vec![
            16u8, 0x07u8, 0x4e, 0xee, 0x0d, 0xd2, 0x6c, 0x0e, 0xf7, 0x87, 0xf9, 0x50, 0x29, 0x5a,
            0x85, 0xa5, 0x1a,
        ];
        let err = accept_v2_wakeup_hint(&ad).unwrap_err();
        assert_eq!(err, BleParseReject::MalformedPrimaryServiceUuidList);
    }

    #[test]
    fn v2_empty_advertisement_is_rejected() {
        let err = accept_v2_wakeup_hint(&[]).unwrap_err();
        assert_eq!(err, BleParseReject::MissingPrimaryServiceUuid);
    }

    #[test]
    fn v2_malformed_ad_structure_is_rejected() {
        // Length byte claims more data than available
        let ad = vec![0x20, 0x07, 0x01];
        let err = accept_v2_wakeup_hint(&ad).unwrap_err();
        assert_eq!(err, BleParseReject::MalformedAdStructure);
    }

    // ── accept_v2_wakeup_observation tests ──────────────────────────────

    #[test]
    fn v2_observation_returns_signal_with_ble_address_as_peer_hint() {
        let signal = accept_v2_wakeup_observation(
            &v2_uuid_only_ad(),
            "AA:BB:CC:DD:EE:FF",
            1000,
            Some(-55),
            "test",
        )
        .expect("must accept");
        assert_eq!(signal.peer_hint, "AA:BB:CC:DD:EE:FF");
        assert_eq!(signal.observed_at_unix_ms, 1000);
        assert_eq!(signal.rssi, Some(-55));
        assert_eq!(signal.bearer_type, "ble");
    }

    #[test]
    fn v2_observation_rejects_missing_uuid() {
        let err =
            accept_v2_wakeup_observation(&non_aethos_ad(), "AA:BB:CC:DD:EE:FF", 1000, None, "test")
                .expect_err("must reject");
        assert_eq!(err.reason_code, "missing_primary_service_uuid");
    }

    // ── Debounce gate tests ─────────────────────────────────────────────

    #[test]
    fn gate_debounces_duplicate_signals_within_window() {
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
            peer_hint: "AA:BB:CC:DD:EE:FF".to_string(),
            observed_at_unix_ms: 1000,
            rssi: Some(-60),
            bearer_type: "ble",
            source: "test",
        };
        let signal_soon = DiscoverySignal {
            observed_at_unix_ms: 5000,
            ..signal.clone()
        };
        let signal_later = DiscoverySignal {
            observed_at_unix_ms: 35_000,
            ..signal
        };
        let mut source = InlineSource {
            frames: vec![vec![signal_soon.clone()], vec![signal_later.clone()]],
        };
        // 30s debounce window (v2 default)
        let mut gate = BleDiscoveryGate::new(Duration::from_secs(30));
        gate.last_seen_by_address
            .insert("AA:BB:CC:DD:EE:FF".to_string(), 1000);

        // 5s after last seen — within 30s window, should be debounced
        let first = gate.poll_ready(&mut source, 5000);
        assert!(first.is_empty());

        // 35s after last seen — outside 30s window, should pass
        let second = gate.poll_ready(&mut source, 35_000);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].observed_at_unix_ms, 35_000);
    }

    #[test]
    fn gate_reports_deduped_counts() {
        struct InlineSource {
            signals: Vec<DiscoverySignal>,
        }
        impl BleDiscoverySource for InlineSource {
            fn poll_signals(&mut self, _now_unix_ms: u64) -> Vec<DiscoverySignal> {
                std::mem::take(&mut self.signals)
            }
        }

        let mut gate = BleDiscoveryGate::new(Duration::from_secs(30));
        gate.last_seen_by_address
            .insert("AA:BB:CC:DD:EE:FF".to_string(), 10_000);
        let mut source = InlineSource {
            signals: vec![
                DiscoverySignal {
                    peer_hint: "AA:BB:CC:DD:EE:FF".to_string(),
                    observed_at_unix_ms: 12_000,
                    rssi: Some(-55),
                    bearer_type: "ble",
                    source: "test",
                },
                DiscoverySignal {
                    peer_hint: "11:22:33:44:55:66".to_string(),
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
        assert_eq!(result.ready[0].peer_hint, "11:22:33:44:55:66");
        assert!(result.rejected.is_empty());
    }

    // ── Activation window tests ─────────────────────────────────────────

    #[test]
    fn activation_window_opens_and_limits_concurrent() {
        let mut tracker = ActivationWindowTracker::new(Duration::from_secs(30), 2);
        assert!(tracker.open_window("AA:BB:CC:DD:EE:FF"));
        assert!(tracker.open_window("11:22:33:44:55:66"));
        // Third should be rejected (max 2)
        assert!(!tracker.open_window("77:88:99:AA:BB:CC"));
        assert_eq!(tracker.active_count(), 2);
    }

    #[test]
    fn activation_window_allows_reopening_existing_address() {
        let mut tracker = ActivationWindowTracker::new(Duration::from_secs(30), 1);
        assert!(tracker.open_window("AA:BB:CC:DD:EE:FF"));
        // Same address should succeed even at capacity
        assert!(tracker.open_window("AA:BB:CC:DD:EE:FF"));
        assert_eq!(tracker.active_count(), 1);
    }

    // ── Simulated source tests (v2 format) ──────────────────────────────

    #[test]
    fn simulated_source_v2_accepts_uuid_only_ad() {
        let ad_hex = "11074eee0dd26c0ef787f950295a85a51a18";
        let env_str = format!("ad:{ad_hex}|addr:AA:BB:CC:DD:EE:FF@-55,peer-beta@-49");
        let mut source = SimulatedBleDiscoverySource::from_env_string(&env_str);
        let first = source.poll_signals(1000);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].peer_hint, "AA:BB:CC:DD:EE:FF");
        assert_eq!(first[0].rssi, Some(-55));
        assert_eq!(first[1].peer_hint, "peer-beta");
        let second = source.poll_signals(2000);
        assert!(second.is_empty());
    }

    #[test]
    fn simulated_source_v2_rejects_missing_uuid() {
        // AD with non-Aethos UUID
        let ad_hex = "110700000000000000000000000000000000";
        let env_str = format!("ad:{ad_hex}|addr:AA:BB:CC:DD:EE:FF@-55");
        let mut source = SimulatedBleDiscoverySource::from_env_string(&env_str);
        let report = source.poll_signals_with_diagnostics(1000);
        assert!(report.accepted.is_empty());
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(
            report.rejected[0].reason_code,
            "missing_primary_service_uuid"
        );
    }

    // ── bluetoothctl parser tests ───────────────────────────────────────

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

    // ── AD structure parsing tests ──────────────────────────────────────

    #[test]
    fn parse_ad_structures_returns_correct_types_and_data() {
        let ad = v2_uuid_only_ad();
        let structures = parse_ad_structures(&ad).expect("valid AD");
        assert_eq!(structures.len(), 1);
        assert_eq!(structures[0].0, 0x07);
        assert_eq!(structures[0].1, &AETHOS_BLE_PRIMARY_SERVICE_UUID_LE[..]);
    }

    #[test]
    fn build_primary_uuid_list_ad_produces_valid_v2_advertisement() {
        let ad = build_primary_uuid_list_ad();
        // Must be accepted as a valid v2 wakeup hint
        assert!(accept_v2_wakeup_hint(&ad).is_ok());
        // Must be exactly 18 bytes: 1 (length) + 1 (type 0x07) + 16 (UUID)
        assert_eq!(ad.len(), 18);
    }
}
