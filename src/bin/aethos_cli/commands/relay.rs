use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Debug, Args)]
pub struct RelayArgs {
    #[command(subcommand)]
    pub cmd: RelayCmd,
}

#[derive(Debug, Subcommand)]
pub enum RelayCmd {
    /// Check connectivity to the configured relay endpoint
    Health,
    /// Show detailed relay connection diagnostics
    Diagnostics {
        #[arg(long, help = "Override relay endpoint for this check")]
        relay: Option<String>,
    },
    /// Pull pending messages from relay into local gossip store
    Sync,
}

pub fn run(args: &RelayArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(args: &RelayArgs, state: &crate::state::CliState) -> Result<(String, Value), String> {
    match &args.cmd {
        RelayCmd::Health => Ok(("relay_health".to_string(), relay_health_payload(state)?)),
        RelayCmd::Diagnostics { relay } => Ok((
            "relay_diagnostics".to_string(),
            relay_diagnostics_payload(state, relay.as_deref())?,
        )),
        RelayCmd::Sync => relay_sync_payload(state),
    }
}

fn relay_health_payload(state: &crate::state::CliState) -> Result<Value, String> {
    let endpoint = state.relay_endpoint.clone();
    let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
    let result =
        crate::relay::client::connect_to_relay_gossipv1_with_auth(&endpoint, &identity, None);

    if result.starts_with("connected") {
        Ok(json!({
            "chip_state": "ok",
            "endpoint": endpoint,
            "reachable": true,
        }))
    } else {
        Ok(json!({
            "chip_state": "error",
            "endpoint": endpoint,
            "reachable": false,
            "error": result,
        }))
    }
}

fn relay_diagnostics_payload(
    state: &crate::state::CliState,
    relay_override: Option<&str>,
) -> Result<Value, String> {
    let endpoint = relay_override
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&state.relay_endpoint)
        .to_string();
    let relay_http = crate::relay::client::normalize_http_endpoint(&endpoint);
    let relay_ws = crate::relay::client::to_ws_endpoint(&relay_http);
    let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
    let started_at = std::time::Instant::now();
    let result =
        crate::relay::client::connect_to_relay_gossipv1_with_auth(&relay_ws, &identity, None);
    let connection_ms = started_at.elapsed().as_millis() as u64;
    let reachable = result.starts_with("connected");

    Ok(json!({
        "chip_state": if reachable { "ok" } else { "error" },
        "endpoint": endpoint,
        "relay_http": relay_http,
        "relay_ws": relay_ws,
        "ws_derivation_ok": relay_ws.starts_with("ws://") || relay_ws.starts_with("wss://"),
        "reachable": reachable,
        "connection_ms": connection_ms,
        "error": if reachable { Value::Null } else { json!(result) },
    }))
}

fn relay_sync_payload(state: &crate::state::CliState) -> Result<(String, Value), String> {
    let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
    let relay_ws = state.relay_endpoint.clone();
    let report =
        crate::relay::client::run_relay_encounter_gossipv1(&relay_ws, &identity, None, None)
            .map_err(|err| {
                let message = format!("relay sync failed: {err}");
                crate::output::emit_error(&message);
                message
            })?;

    Ok((
        "relay_sync".to_string(),
        json!({
            "pulled_messages": report.pulled_messages.len(),
            "pushed_messages": report.transferred_items,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::{execute, RelayArgs, RelayCmd};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("aethos-cli-{label}-{}-{nanos}", std::process::id()))
    }

    fn test_state(base_dir: &Path, relay: Option<&str>) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(base_dir.to_str(), relay, false)
    }

    #[test]
    fn health_reports_unreachable_relay_as_structured_status() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("relay-health");
        let state = test_state(&base_dir, Some("wss://localhost:19999"));
        state.setup_env();

        let args = RelayArgs {
            cmd: RelayCmd::Health,
        };
        let (event_type, data) = execute(&args, &state).expect("health result");

        assert_eq!(event_type, "relay_health");
        assert_eq!(data["endpoint"], "wss://localhost:19999");
        assert_eq!(data["reachable"], false);
        assert_eq!(data["chip_state"], "error");
        assert!(data["error"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn diagnostics_reports_derived_endpoint_and_connection_failure() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("relay-diagnostics");
        let state = test_state(&base_dir, Some("localhost:19999"));
        state.setup_env();

        let args = RelayArgs {
            cmd: RelayCmd::Diagnostics { relay: None },
        };
        let (event_type, data) = execute(&args, &state).expect("diagnostics result");

        assert_eq!(event_type, "relay_diagnostics");
        assert_eq!(data["endpoint"], "localhost:19999");
        assert_eq!(data["relay_http"], "http://localhost:19999");
        assert_eq!(data["relay_ws"], "ws://localhost:19999/ws");
        assert_eq!(data["ws_derivation_ok"], true);
        assert_eq!(data["reachable"], false);
        assert!(data["connection_ms"].as_u64().unwrap_or(0) < 10_000);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn sync_reports_connection_error() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("relay-sync");
        let state = test_state(&base_dir, Some("wss://localhost:19999"));
        state.setup_env();

        let args = RelayArgs {
            cmd: RelayCmd::Sync,
        };
        let err = execute(&args, &state).expect_err("sync should fail without relay");

        assert!(err.contains("relay sync failed"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
