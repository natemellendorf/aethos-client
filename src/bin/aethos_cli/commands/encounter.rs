use serde_json::json;

use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct EncounterArgs {
    #[command(subcommand)]
    pub command: EncounterCmd,
}

#[derive(Debug, Subcommand)]
pub enum EncounterCmd {
    /// Show BLE encounter/discovery status (placeholder on non-mobile)
    Status,
}

pub fn run_status(state: &crate::state::CliState) -> Result<(), String> {
    let args = EncounterArgs {
        command: EncounterCmd::Status,
    };
    let (event_type, data) = execute(&args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(
    args: &EncounterArgs,
    _state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match &args.command {
        EncounterCmd::Status => Ok((
            "encounter_activity".to_string(),
            json!({
                "ble_discovery_status": "unavailable",
                "ble_discovery_enabled": false,
                "recent_ble_sightings_count": 0,
                "last_transfer_bearer": null,
                "last_discovery_bearer": null,
                "events": [],
            }),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, EncounterArgs, EncounterCmd};

    #[test]
    fn status_emits_static_activity_snapshot_shape() {
        let state =
            crate::state::CliState::from_cli_args(Some("/tmp/aethos-cli-encounter"), None, false);

        let args = EncounterArgs {
            command: EncounterCmd::Status,
        };

        let (event_type, data) = execute(&args, &state).expect("encounter status");

        assert_eq!(event_type, "encounter_activity");
        assert_eq!(data["ble_discovery_status"], "unavailable");
        assert_eq!(data["ble_discovery_enabled"], false);
        assert_eq!(data["recent_ble_sightings_count"], 0);
        assert!(data["last_transfer_bearer"].is_null());
        assert!(data["last_discovery_bearer"].is_null());
        assert!(data["events"]
            .as_array()
            .is_some_and(|events| events.is_empty()));
    }
}
