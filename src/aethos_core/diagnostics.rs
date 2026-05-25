use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, Utc};
use flume::{Receiver, Sender};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub const DIAGNOSTICS_SCHEMA_VERSION: &str = "diagnostics-event-schema-v1";
const DEFAULT_LOCAL_FILE_NAME: &str = "aethos-diagnostics.jsonl";
const DEFAULT_LOCAL_DIR_NAME: &str = "aethos-linux";
const DEFAULT_ROTATED_FILE_NAME: &str = "aethos-diagnostics.1.jsonl";
const DEFAULT_MAX_LOCAL_FILE_BYTES: u64 = 5 * 1024 * 1024;

static REPORTER: OnceLock<ReporterHandle> = OnceLock::new();
static PROCESS_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsRunCreateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_run_id: Option<String>,
    pub app: String,
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_case_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsRunRecord {
    pub run_id: String,
    pub app: String,
    pub platform: String,
    pub status: String,
    pub created_at_utc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_utc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_case_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsEventIngestRequest {
    pub events: Vec<DiagnosticEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsEventIngestResponse {
    pub accepted: usize,
    pub dropped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsTimelineResponse {
    pub run_id: String,
    pub events: Vec<DiagnosticEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsMissingTransition {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub expected_after: String,
    pub missing: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_event_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsErrorSummary {
    pub reason_code: String,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsItemSummary {
    pub item_id: String,
    pub highest_phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsRunSummaryResponse {
    pub run_id: String,
    pub event_count: usize,
    pub highest_protocol_phase: String,
    pub item_ids_sent: Vec<String>,
    pub item_ids_received: Vec<String>,
    pub missing_transitions: Vec<DiagnosticsMissingTransition>,
    pub top_errors: Vec<DiagnosticsErrorSummary>,
    pub items: Vec<DiagnosticsItemSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    pub schema_version: String,
    pub run_id: String,
    pub session_id: String,
    pub encounter_id: String,
    pub event_id: String,
    pub timestamp_utc: String,
    pub platform: String,
    pub app: String,
    pub build_sha: String,
    pub component: String,
    pub event_type: String,
    pub phase: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticEventInput {
    pub component: String,
    pub event_type: String,
    pub phase: String,
    pub result: String,
    pub peer_id: Option<String>,
    pub remote_peer_id: Option<String>,
    pub item_id: Option<String>,
    pub bearer: Option<String>,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub fields: Option<Value>,
    pub encounter_id: Option<String>,
}

impl DiagnosticEventInput {
    pub fn new(component: impl Into<String>, event_type: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            event_type: event_type.into(),
            phase: "runtime".to_string(),
            result: "ok".to_string(),
            peer_id: None,
            remote_peer_id: None,
            item_id: None,
            bearer: None,
            reason_code: None,
            message: None,
            fields: None,
            encounter_id: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ReporterConfig {
    run_id: String,
    session_id: String,
    collector_url: Option<String>,
    app: String,
    build_sha: String,
    local_file_path: PathBuf,
    max_local_file_bytes: u64,
}

#[derive(Debug)]
struct ReporterHandle {
    config: ReporterConfig,
    sender: Sender<DiagnosticEvent>,
}

pub fn current_run_id() -> Option<String> {
    resolve_run_id()
}

pub fn collector_url() -> Option<String> {
    std::env::var("AETHOS_DIAGNOSTICS_COLLECTOR_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

pub fn attach_current_run(component: &str, app: &str) {
    let Some(run_id) = current_run_id() else {
        return;
    };
    std::env::set_var("AETHOS_DIAGNOSTICS_RUN_ID", &run_id);
    std::env::set_var("AETHOS_APP_NAME", app);

    let mut fields = Map::new();
    if let Ok(value) = std::env::var("AETHOS_E2E_SCENARIO") {
        if !value.trim().is_empty() {
            fields.insert("scenario".to_string(), json!(value));
        }
    }
    if let Ok(value) = std::env::var("AETHOS_E2E_TEST_CASE_ID") {
        if !value.trim().is_empty() {
            fields.insert("test_case_id".to_string(), json!(value));
        }
    }

    emit_event(DiagnosticEventInput {
        component: component.to_string(),
        event_type: "diag.run.attached".to_string(),
        phase: "run".to_string(),
        result: "ok".to_string(),
        peer_id: None,
        remote_peer_id: None,
        item_id: None,
        bearer: None,
        reason_code: None,
        message: Some("diagnostics run attached".to_string()),
        fields: Some(Value::Object(fields)),
        encounter_id: Some("run".to_string()),
    });
}

pub fn emit_app_lifecycle(component: &str, state: &str, message: Option<&str>) {
    let event_type = if state == "start" {
        "app.start"
    } else {
        "app.stop"
    };
    emit_event(DiagnosticEventInput {
        component: component.to_string(),
        event_type: event_type.to_string(),
        phase: "app".to_string(),
        result: "ok".to_string(),
        peer_id: None,
        remote_peer_id: None,
        item_id: None,
        bearer: None,
        reason_code: None,
        message: message.map(|value| value.to_string()),
        fields: None,
        encounter_id: Some("app".to_string()),
    });
}

pub fn emit_ui_projection(context: &str, result: &str, message: Option<&str>) {
    let event_type = format!("ui.projection.{result}");
    emit_event(DiagnosticEventInput {
        component: "ui".to_string(),
        event_type,
        phase: "ui".to_string(),
        result: result.to_string(),
        peer_id: None,
        remote_peer_id: None,
        item_id: None,
        bearer: None,
        reason_code: None,
        message: message.map(|value| value.to_string()),
        fields: Some(json!({"context": context})),
        encounter_id: Some("ui".to_string()),
    });
}

pub fn emit_inbox_import(result: &str, item_id: Option<&str>, count: usize, message: Option<&str>) {
    let event_type = match result {
        "started" => "inbox.import.started",
        "failed" => "inbox.import.failed",
        _ => "inbox.import.succeeded",
    };
    emit_event(DiagnosticEventInput {
        component: "inbox".to_string(),
        event_type: event_type.to_string(),
        phase: "import".to_string(),
        result: result.to_string(),
        peer_id: None,
        remote_peer_id: None,
        item_id: item_id.map(|value| value.to_string()),
        bearer: None,
        reason_code: None,
        message: message.map(|value| value.to_string()),
        fields: Some(json!({"count": count})),
        encounter_id: Some("import".to_string()),
    });
}

pub fn emit_protocol_frame(
    direction: &str,
    transport: &str,
    frame_type: &str,
    peer_id: Option<&str>,
    item_id: Option<&str>,
    extra_fields: Option<Value>,
) {
    let mapping = match (direction, frame_type) {
        ("sent", "HELLO") => Some(("hello.sent", "hello", "ok")),
        ("received", "HELLO") => Some(("hello.received", "hello", "ok")),
        ("sent", "SUMMARY") => Some(("summary.sent", "summary", "ok")),
        ("received", "SUMMARY") => Some(("summary.received", "summary", "ok")),
        ("sent", "REQUEST") => Some(("request.sent", "request", "ok")),
        ("received", "REQUEST") => Some(("request.received", "request", "ok")),
        ("sent", "TRANSFER") => Some(("transfer.sent", "transfer", "ok")),
        ("received", "TRANSFER") => Some(("transfer.received", "transfer", "ok")),
        ("sent", "RECEIPT") => Some(("receipt.sent", "receipt", "ok")),
        ("received", "RECEIPT") => Some(("receipt.received", "receipt", "ok")),
        _ => None,
    };

    let Some((event_type, phase, result)) = mapping else {
        return;
    };

    let mut fields = as_object(extra_fields);
    fields.insert("transport".to_string(), json!(transport));
    fields.insert("direction".to_string(), json!(direction));
    fields.insert("frame_type".to_string(), json!(frame_type));

    emit_event(DiagnosticEventInput {
        component: format!("protocol.{transport}"),
        event_type: event_type.to_string(),
        phase: phase.to_string(),
        result: result.to_string(),
        peer_id: peer_id.map(|value| value.to_string()),
        remote_peer_id: None,
        item_id: item_id.map(|value| value.to_string()),
        bearer: Some(transport.to_string()),
        reason_code: None,
        message: None,
        fields: Some(Value::Object(fields)),
        encounter_id: None,
    });
}

pub fn emit_event(input: DiagnosticEventInput) {
    let Some(handle) = reporter_handle() else {
        return;
    };

    let sequence = PROCESS_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    let mut fields = as_object(input.fields);
    fields.insert("sequence".to_string(), json!(sequence));
    let event = DiagnosticEvent {
        schema_version: DIAGNOSTICS_SCHEMA_VERSION.to_string(),
        run_id: handle.config.run_id.clone(),
        session_id: handle.config.session_id.clone(),
        encounter_id: input.encounter_id.unwrap_or_else(|| {
            infer_encounter_id(&fields).unwrap_or_else(|| "unknown".to_string())
        }),
        event_id: format!("{}-{sequence:08}", handle.config.session_id),
        timestamp_utc: now_rfc3339(),
        platform: std::env::consts::OS.to_string(),
        app: handle.config.app.clone(),
        build_sha: handle.config.build_sha.clone(),
        component: input.component,
        event_type: input.event_type,
        phase: input.phase,
        result: input.result,
        peer_id: input.peer_id,
        remote_peer_id: input.remote_peer_id,
        item_id: input.item_id,
        bearer: input.bearer,
        reason_code: input.reason_code,
        message: input.message.map(|value| sanitize_message(&value)),
        fields: Some(Value::Object(fields)),
    };
    let _ = handle.sender.try_send(event);
}

pub fn report_from_log(level: &str, message: &str, event_name: &str, fields: Value) {
    let mapped = match event_name {
        "send_message_start" => Some(("request.planned", "request", "ok", "messaging")),
        "bonjour_peer_discovered" | "encounter_discovery_observed" => {
            Some(("discovery.signal.detected", "discovery", "ok", "discovery"))
        }
        "gossip_request_duplicate_ignored" => Some((
            "discovery.signal.ignored",
            "discovery",
            "ignored",
            "discovery",
        )),
        "encounter_control_exchange_started"
        | "relay_encounter_open"
        | "gossip_encounter_start" => Some(("encounter.opened", "encounter", "ok", "encounter")),
        "encounter_bearer_selected" | "encounter_bearer_upgrade_applied" => {
            Some(("bearer.selected", "encounter", "ok", "encounter"))
        }
        "gossip_encounter_quiet_check_started" => Some((
            "encounter.quiet_round_started",
            "encounter",
            "started",
            "encounter",
        )),
        "gossip_encounter_idle_grace_started" => Some((
            "encounter.idle_grace_started",
            "encounter",
            "started",
            "encounter",
        )),
        "gossip_encounter_resumed_before_prune" => Some((
            "encounter.idle_grace_cancelled",
            "encounter",
            "ok",
            "encounter",
        )),
        "gossip_known_peer_nudged" => Some((
            "encounter.outbox_work_detected",
            "encounter",
            "ok",
            "encounter",
        )),
        "bonjour_advertisement_started" => Some((
            "lan.bonjour.candidate_created",
            "discovery",
            "started",
            "discovery",
        )),
        "bonjour_endpoint_resolved" => Some((
            "lan.bonjour.candidate_created",
            "discovery",
            "ok",
            "discovery",
        )),
        "multicast_discovery_restarted" => {
            Some(("lan.route.refreshed", "discovery", "ok", "discovery"))
        }
        "multicast_discovery_error" => Some((
            "lan.multicast.join_failed",
            "discovery",
            "failed",
            "discovery",
        )),
        "lan_route_selected" => Some(("lan.route.selected", "discovery", "ok", "discovery")),
        "LAN discovery bearer active" if message.contains("Multicast") => {
            Some(("lan.multicast.socket_bound", "discovery", "ok", "discovery"))
        }
        "encounter_closed" | "gossip_encounter_end" | "gossip_tcp_encounter_done" => {
            Some(("encounter.closed", "encounter", "ok", "encounter"))
        }
        "relay_session_state" if message.contains("state=connected") => {
            Some(("relay.connected", "relay", "ok", "relay"))
        }
        "relay_session_state" if message.contains("state=closed") => {
            Some(("relay.disconnected", "relay", "ok", "relay"))
        }
        "transfer_import_start" => Some(("inbox.import.started", "import", "started", "inbox")),
        "transfer_import_done" => Some(("inbox.import.succeeded", "import", "ok", "inbox")),
        "chat_snapshot_emit_failed" => Some(("ui.projection.failed", "ui", "failed", "ui")),
        "gossip_transfer_imported_messages" => {
            Some(("inbox.import.succeeded", "import", "ok", "inbox"))
        }
        "outbound_app_body_built" | "gossip_record_local_payload_ok" => {
            Some(("request.planned", "request", "ok", "messaging"))
        }
        "bonjour_discovery_error"
        | "ble_advertiser_error"
        | "gossip_frame_handle_error"
        | "gossip_recv_error" => Some(("error", "error", "error", "runtime")),
        _ if level.eq_ignore_ascii_case("ERROR") => Some(("error", "error", "error", "runtime")),
        _ => None,
    };

    let Some((event_type, phase, result, component)) = mapped else {
        return;
    };
    let fields_map = as_object(Some(fields));
    emit_event(DiagnosticEventInput {
        component: component.to_string(),
        event_type: event_type.to_string(),
        phase: phase.to_string(),
        result: result.to_string(),
        peer_id: string_field(&fields_map, &["peer", "from", "to"]),
        remote_peer_id: string_field(&fields_map, &["remote_peer_id"]),
        item_id: string_field(&fields_map, &["item_id"]),
        bearer: string_field(&fields_map, &["bearer", "transport"]),
        reason_code: string_field(&fields_map, &["reason", "error"]),
        message: Some(sanitize_message(message)),
        fields: Some(Value::Object(fields_map)),
        encounter_id: None,
    });
}

fn reporter_handle() -> Option<&'static ReporterHandle> {
    if REPORTER.get().is_none() {
        let config = reporter_config()?;
        let (sender, receiver) = flume::bounded::<DiagnosticEvent>(4096);
        spawn_worker(config.clone(), receiver);
        let _ = REPORTER.set(ReporterHandle { config, sender });
    }
    REPORTER.get()
}

fn reporter_config() -> Option<ReporterConfig> {
    let run_id = resolve_run_id()?;
    let app = std::env::var("AETHOS_APP_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "aethos-client".to_string());
    Some(ReporterConfig {
        run_id,
        session_id: std::env::var("AETHOS_DIAGNOSTICS_SESSION_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(default_session_id),
        collector_url: collector_url(),
        app,
        build_sha: std::env::var("AETHOS_BUILD_SHA")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string()),
        local_file_path: diagnostics_local_file_path(),
        max_local_file_bytes: std::env::var("AETHOS_DIAGNOSTICS_MAX_LOCAL_FILE_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_LOCAL_FILE_BYTES),
    })
}

fn spawn_worker(config: ReporterConfig, receiver: Receiver<DiagnosticEvent>) {
    std::thread::spawn(move || {
        let agent = config.collector_url.as_ref().map(|_| {
            ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_millis(250))
                .timeout_read(Duration::from_millis(400))
                .timeout_write(Duration::from_millis(400))
                .build()
        });
        while let Ok(event) = receiver.recv() {
            let _ = append_local_event(&config, &event);
            if let (Some(base_url), Some(agent)) = (config.collector_url.as_ref(), agent.as_ref()) {
                let _ = agent
                    .post(&format!("{base_url}/api/v1/diagnostics/events"))
                    .send_json(json!(DiagnosticsEventIngestRequest {
                        events: vec![event.clone()]
                    }));
            }
        }
    });
}

fn append_local_event(config: &ReporterConfig, event: &DiagnosticEvent) -> Result<(), String> {
    rotate_local_file_if_needed(&config.local_file_path, config.max_local_file_bytes)?;
    if let Some(parent) = config.local_file_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create diagnostics dir: {err}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.local_file_path)
        .map_err(|err| format!("open diagnostics file: {err}"))?;
    let line = serde_json::to_string(event)
        .map_err(|err| format!("serialize diagnostics event: {err}"))?;
    writeln!(file, "{line}").map_err(|err| format!("write diagnostics file: {err}"))
}

fn rotate_local_file_if_needed(path: &Path, max_bytes: u64) -> Result<(), String> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() < max_bytes {
        return Ok(());
    }
    let rotated = path.with_file_name(DEFAULT_ROTATED_FILE_NAME);
    let _ = fs::remove_file(&rotated);
    fs::rename(path, &rotated).map_err(|err| format!("rotate diagnostics file: {err}"))
}

fn resolve_run_id() -> Option<String> {
    for key in ["AETHOS_DIAGNOSTICS_RUN_ID", "AETHOS_E2E_RUN_ID"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn diagnostics_local_file_path() -> PathBuf {
    if let Ok(state_dir) = std::env::var("AETHOS_STATE_DIR") {
        if !state_dir.trim().is_empty() {
            return Path::new(&state_dir)
                .join(DEFAULT_LOCAL_DIR_NAME)
                .join(DEFAULT_LOCAL_FILE_NAME);
        }
    }
    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join(DEFAULT_LOCAL_DIR_NAME)
                .join(DEFAULT_LOCAL_FILE_NAME);
        }
    }
    std::env::temp_dir().join(DEFAULT_LOCAL_FILE_NAME)
}

fn infer_encounter_id(fields: &Map<String, Value>) -> Option<String> {
    string_field(fields, &["encounter_id", "peer", "relay_ws"])
}

fn default_session_id() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    format!("session-{ts}-{}", std::process::id())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn as_object(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        Some(other) => {
            let mut map = Map::new();
            map.insert("value".to_string(), other);
            map
        }
        None => Map::new(),
    }
}

fn string_field(fields: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        fields
            .get(*key)
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
    })
}

fn sanitize_message(message: &str) -> String {
    if message.len() > 512 {
        format!("{}…", &message[..512])
    } else {
        message.to_string()
    }
}

pub fn summarize_events(run_id: &str, events: &[DiagnosticEvent]) -> DiagnosticsRunSummaryResponse {
    let mut event_count = 0usize;
    let mut highest_protocol_phase = "run".to_string();
    let mut item_ids_sent = Vec::<String>::new();
    let mut item_ids_received = Vec::<String>::new();
    let mut per_item_phases = BTreeMap::<String, Vec<&DiagnosticEvent>>::new();
    let mut errors = BTreeMap::<String, DiagnosticsErrorSummary>::new();

    for event in events {
        event_count += 1;
        if phase_rank(&event.phase) > phase_rank(&highest_protocol_phase) {
            highest_protocol_phase = event.phase.clone();
        }
        if let Some(item_id) = event.item_id.as_ref() {
            per_item_phases
                .entry(item_id.clone())
                .or_default()
                .push(event);
        }
        if event.event_type == "transfer.sent" {
            if let Some(item_id) = event.item_id.as_ref() {
                item_ids_sent.push(item_id.clone());
            }
        }
        if matches!(
            event.event_type.as_str(),
            "transfer.received" | "inbox.import.succeeded"
        ) {
            if let Some(item_id) = event.item_id.as_ref() {
                item_ids_received.push(item_id.clone());
            }
        }
        if event.event_type == "error" || event.result == "error" || event.result == "failed" {
            let key = event
                .reason_code
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let entry = errors
                .entry(key.clone())
                .or_insert(DiagnosticsErrorSummary {
                    reason_code: key,
                    count: 0,
                    last_message: None,
                });
            entry.count += 1;
            entry.last_message = event.message.clone();
        }
    }

    item_ids_sent.sort();
    item_ids_sent.dedup();
    item_ids_received.sort();
    item_ids_received.dedup();

    let mut items = Vec::new();
    let mut missing_transitions = Vec::new();
    for (item_id, item_events) in per_item_phases {
        let highest = item_events
            .iter()
            .max_by_key(|event| phase_rank(&event.phase))
            .map(|event| event.phase.clone())
            .unwrap_or_else(|| "run".to_string());
        let last_event_type = item_events.last().map(|event| event.event_type.clone());
        if !item_events
            .iter()
            .any(|event| event.event_type == "transfer.received")
            && item_events
                .iter()
                .any(|event| event.event_type == "request.sent")
        {
            missing_transitions.push(DiagnosticsMissingTransition {
                item_id: Some(item_id.clone()),
                expected_after: "request.sent".to_string(),
                missing: "transfer.received".to_string(),
                last_seen_event_type: last_event_type.clone(),
            });
        }
        if !item_events
            .iter()
            .any(|event| event.event_type == "inbox.import.succeeded")
            && item_events
                .iter()
                .any(|event| event.event_type == "transfer.received")
        {
            missing_transitions.push(DiagnosticsMissingTransition {
                item_id: Some(item_id.clone()),
                expected_after: "transfer.received".to_string(),
                missing: "inbox.import.succeeded".to_string(),
                last_seen_event_type: last_event_type.clone(),
            });
        }
        items.push(DiagnosticsItemSummary {
            item_id,
            highest_phase: highest,
            last_event_type,
        });
    }

    if events
        .iter()
        .any(|event| event.event_type == "discovery.started")
        && !events
            .iter()
            .any(|event| event.event_type == "encounter.opened")
    {
        missing_transitions.push(DiagnosticsMissingTransition {
            item_id: None,
            expected_after: "discovery.started".to_string(),
            missing: "encounter.opened".to_string(),
            last_seen_event_type: Some("discovery.started".to_string()),
        });
    }
    if events.iter().any(|event| event.event_type == "hello.sent")
        && !events
            .iter()
            .any(|event| event.event_type == "summary.received")
        && !events
            .iter()
            .any(|event| event.event_type == "summary.sent")
    {
        missing_transitions.push(DiagnosticsMissingTransition {
            item_id: None,
            expected_after: "hello.sent".to_string(),
            missing: "summary.received".to_string(),
            last_seen_event_type: Some("hello.sent".to_string()),
        });
    }
    if events
        .iter()
        .any(|event| event.event_type == "inbox.import.succeeded")
        && !events
            .iter()
            .any(|event| event.event_type == "ui.projection.succeeded")
    {
        missing_transitions.push(DiagnosticsMissingTransition {
            item_id: None,
            expected_after: "inbox.import.succeeded".to_string(),
            missing: "ui.projection.succeeded".to_string(),
            last_seen_event_type: Some("inbox.import.succeeded".to_string()),
        });
    }

    let mut top_errors = errors.into_values().collect::<Vec<_>>();
    top_errors.sort_by(|left, right| right.count.cmp(&left.count));

    items.sort_by(|left, right| left.item_id.cmp(&right.item_id));
    missing_transitions.sort_by(|left, right| left.missing.cmp(&right.missing));

    DiagnosticsRunSummaryResponse {
        run_id: run_id.to_string(),
        event_count,
        highest_protocol_phase,
        item_ids_sent,
        item_ids_received,
        missing_transitions,
        top_errors,
        items,
    }
}

fn phase_rank(phase: &str) -> usize {
    match phase {
        "run" => 0,
        "app" => 1,
        "discovery" => 2,
        "encounter" => 3,
        "hello" => 4,
        "summary" => 5,
        "request" => 6,
        "transfer" => 7,
        "import" => 8,
        "ui" => 9,
        "relay" => 10,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{phase_rank, summarize_events, DiagnosticEvent};

    fn sample_event(event_type: &str, phase: &str, item_id: Option<&str>) -> DiagnosticEvent {
        DiagnosticEvent {
            schema_version: super::DIAGNOSTICS_SCHEMA_VERSION.to_string(),
            run_id: "run-1".to_string(),
            session_id: "session-1".to_string(),
            encounter_id: "encounter-1".to_string(),
            event_id: format!("event-{event_type}"),
            timestamp_utc: "2026-04-21T00:00:00.000Z".to_string(),
            platform: "linux".to_string(),
            app: "aethos".to_string(),
            build_sha: "abc123".to_string(),
            component: "test".to_string(),
            event_type: event_type.to_string(),
            phase: phase.to_string(),
            result: "ok".to_string(),
            peer_id: None,
            remote_peer_id: None,
            item_id: item_id.map(|value| value.to_string()),
            bearer: None,
            reason_code: None,
            message: None,
            fields: None,
        }
    }

    #[test]
    fn summarize_events_reports_missing_ui_projection() {
        let events = vec![
            sample_event("request.sent", "request", Some("item-1")),
            sample_event("transfer.received", "transfer", Some("item-1")),
            sample_event("inbox.import.succeeded", "import", Some("item-1")),
        ];
        let summary = summarize_events("run-1", &events);
        assert!(summary
            .missing_transitions
            .iter()
            .any(|item| item.missing == "ui.projection.succeeded"));
    }

    #[test]
    fn phase_rank_orders_protocol_progression() {
        assert!(phase_rank("transfer") > phase_rank("hello"));
        assert!(phase_rank("ui") > phase_rank("transfer"));
    }
}
