use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
pub struct IdentityArgs {
    #[command(subcommand)]
    pub cmd: IdentityCmd,
}

#[derive(Debug, Subcommand)]
pub enum IdentityCmd {
    /// Create or load local Wayfarer identity (idempotent)
    Create,
    /// Display current Wayfarer identity (wayfarer_id, device_id, keys)
    Show,
    /// Rotate signing keys while keeping the same wayfarer_id
    Rotate,
    /// Destroy local identity and create a new one (destructive, irreversible)
    Reset {
        #[arg(long, help = "Required safety flag to confirm destructive reset")]
        confirm: bool,
    },
}

pub fn run(args: &IdentityArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(
    args: &IdentityArgs,
    _state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match args.cmd {
        IdentityCmd::Create => {
            let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
            Ok(("identity_created".to_string(), identity_payload(identity)))
        }
        IdentityCmd::Show => {
            let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
            Ok(("identity_loaded".to_string(), identity_payload(identity)))
        }
        IdentityCmd::Rotate => {
            let identity = crate::aethos_core::identity_store::regenerate_local_identity()?;
            Ok(("identity_rotated".to_string(), identity_payload(identity)))
        }
        IdentityCmd::Reset { confirm: true } => {
            crate::aethos_core::identity_store::delete_wayfarer_id()?;
            let identity = crate::aethos_core::identity_store::regenerate_local_identity()?;
            Ok(("identity_reset".to_string(), identity_payload(identity)))
        }
        IdentityCmd::Reset { confirm: false } => {
            let message = "--confirm flag required for reset";
            crate::output::emit_error(message);
            Err(message.to_string())
        }
    }
}

fn identity_payload(
    identity: crate::aethos_core::identity_store::LocalIdentitySummary,
) -> serde_json::Value {
    json!({
        "wayfarer_id": identity.wayfarer_id,
        "device_id": identity.device_id,
        "verifying_key_b64": identity.verifying_key_b64,
        "device_name": identity.device_name,
    })
}

#[cfg(test)]
mod tests {
    use super::{execute, IdentityArgs, IdentityCmd};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("aethos-cli-{label}-{}-{nanos}", std::process::id()))
    }

    fn identity_file_path(base_dir: &Path) -> PathBuf {
        base_dir.join("aethos-linux").join("identity.json")
    }

    fn test_state(base_dir: &Path) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(base_dir.to_str(), None, false)
    }

    #[test]
    fn create_generates_identity_payload() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("create");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = IdentityArgs {
            cmd: IdentityCmd::Create,
        };
        let (event_type, data) = execute(&args, &state).expect("create identity");

        assert_eq!(event_type, "identity_created");
        assert!(identity_file_path(&base_dir).exists());
        assert!(data["wayfarer_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(data["device_id"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(data["verifying_key_b64"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        assert!(data["device_name"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn show_is_idempotent_and_loads_existing_identity() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("show");
        let state = test_state(&base_dir);
        state.setup_env();

        let create_args = IdentityArgs {
            cmd: IdentityCmd::Create,
        };
        let (_, created) = execute(&create_args, &state).expect("create identity");

        let show_args = IdentityArgs {
            cmd: IdentityCmd::Show,
        };
        let (event_type, loaded) = execute(&show_args, &state).expect("show identity");

        assert_eq!(event_type, "identity_loaded");
        assert_eq!(loaded, created);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn rotate_regenerates_identity() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("rotate");
        let state = test_state(&base_dir);
        state.setup_env();

        let create_args = IdentityArgs {
            cmd: IdentityCmd::Create,
        };
        let (_, created) = execute(&create_args, &state).expect("create identity");

        let rotate_args = IdentityArgs {
            cmd: IdentityCmd::Rotate,
        };
        let (event_type, rotated) = execute(&rotate_args, &state).expect("rotate identity");

        assert_eq!(event_type, "identity_rotated");
        assert_ne!(rotated["wayfarer_id"], created["wayfarer_id"]);
        assert_ne!(rotated["verifying_key_b64"], created["verifying_key_b64"]);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn reset_requires_confirm_flag() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("reset-no-confirm");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = IdentityArgs {
            cmd: IdentityCmd::Reset { confirm: false },
        };
        let error = execute(&args, &state).expect_err("reset should fail without confirm");

        assert_eq!(error, "--confirm flag required for reset");
        assert!(!identity_file_path(&base_dir).exists());

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn reset_deletes_and_regenerates_identity() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("reset-confirm");
        let state = test_state(&base_dir);
        state.setup_env();

        let create_args = IdentityArgs {
            cmd: IdentityCmd::Create,
        };
        let (_, created) = execute(&create_args, &state).expect("create identity");

        let reset_args = IdentityArgs {
            cmd: IdentityCmd::Reset { confirm: true },
        };
        let (event_type, reset) = execute(&reset_args, &state).expect("reset identity");

        assert_eq!(event_type, "identity_reset");
        assert!(identity_file_path(&base_dir).exists());
        assert_ne!(reset["wayfarer_id"], created["wayfarer_id"]);
        assert_ne!(reset["verifying_key_b64"], created["verifying_key_b64"]);

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
