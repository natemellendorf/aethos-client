use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
pub struct ContactsArgs {
    #[command(subcommand)]
    pub cmd: ContactsCmd,
}

#[derive(Debug, Subcommand)]
pub enum ContactsCmd {
    /// Add or update a contact alias for a wayfarer_id
    Add {
        #[arg(long, help = "Wayfarer ID (64 lowercase hex chars)")]
        id: String,
        #[arg(long, help = "Friendly display name for this contact")]
        alias: String,
    },
    /// Remove a contact by wayfarer_id
    Remove {
        #[arg(long, help = "Wayfarer ID to remove")]
        id: String,
    },
    /// List all saved contacts
    List,
}

pub fn run(args: &ContactsArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(
    args: &ContactsArgs,
    _state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match &args.cmd {
        ContactsCmd::Add { id, alias } => {
            if !crate::aethos_core::protocol::is_valid_wayfarer_id(id) {
                let message = "invalid wayfarer_id: must be 64 lowercase hex characters";
                crate::output::emit_error(message);
                return Err(message.to_string());
            }
            let mut contacts = crate::aethos_core::identity_store::load_contact_aliases()?;
            contacts.insert(id.clone(), alias.clone());
            crate::aethos_core::identity_store::save_contact_aliases(&contacts)?;
            Ok((
                "contact_added".to_string(),
                json!({"id": id, "alias": alias}),
            ))
        }
        ContactsCmd::Remove { id } => {
            if !crate::aethos_core::protocol::is_valid_wayfarer_id(id) {
                let message = "invalid wayfarer_id: must be 64 lowercase hex characters";
                crate::output::emit_error(message);
                return Err(message.to_string());
            }
            let mut contacts = crate::aethos_core::identity_store::load_contact_aliases()?;
            contacts.remove(id.as_str());
            crate::aethos_core::identity_store::save_contact_aliases(&contacts)?;
            Ok(("contact_removed".to_string(), json!({"id": id})))
        }
        ContactsCmd::List => {
            let contacts = crate::aethos_core::identity_store::load_contact_aliases()?;
            let list: Vec<serde_json::Value> = contacts
                .iter()
                .map(|(id, alias)| json!({"id": id, "alias": alias}))
                .collect();
            Ok(("contacts_list".to_string(), json!({"contacts": list})))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, ContactsArgs, ContactsCmd};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("aethos-cli-{label}-{}-{nanos}", std::process::id()))
    }

    fn test_state(base_dir: &Path) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(base_dir.to_str(), None, false)
    }

    const VALID_ID: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    #[test]
    fn contacts_add_list_remove_lifecycle() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("contacts-lifecycle");
        let state = test_state(&base_dir);
        state.setup_env();

        // add
        let args = ContactsArgs {
            cmd: ContactsCmd::Add {
                id: VALID_ID.to_string(),
                alias: "Alice".to_string(),
            },
        };
        let (event_type, data) = execute(&args, &state).expect("add contact");
        assert_eq!(event_type, "contact_added");
        assert_eq!(data["id"].as_str().unwrap(), VALID_ID);
        assert_eq!(data["alias"].as_str().unwrap(), "Alice");

        // list
        let args = ContactsArgs {
            cmd: ContactsCmd::List,
        };
        let (event_type, data) = execute(&args, &state).expect("list contacts");
        assert_eq!(event_type, "contacts_list");
        let contacts = data["contacts"].as_array().unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0]["id"].as_str().unwrap(), VALID_ID);
        assert_eq!(contacts[0]["alias"].as_str().unwrap(), "Alice");

        // remove
        let args = ContactsArgs {
            cmd: ContactsCmd::Remove {
                id: VALID_ID.to_string(),
            },
        };
        let (event_type, data) = execute(&args, &state).expect("remove contact");
        assert_eq!(event_type, "contact_removed");
        assert_eq!(data["id"].as_str().unwrap(), VALID_ID);

        // list should be empty
        let args = ContactsArgs {
            cmd: ContactsCmd::List,
        };
        let (_, data) = execute(&args, &state).expect("list after remove");
        assert_eq!(data["contacts"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn contacts_add_invalid_id_rejected() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("contacts-invalid-id");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ContactsArgs {
            cmd: ContactsCmd::Add {
                id: "not-a-valid-id".to_string(),
                alias: "Bob".to_string(),
            },
        };
        let err = execute(&args, &state).expect_err("should reject invalid id");
        assert!(err.contains("invalid wayfarer_id"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn contacts_remove_invalid_id_rejected() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("contacts-remove-invalid");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ContactsArgs {
            cmd: ContactsCmd::Remove {
                id: "UPPERCASE_NOT_VALID_HEX_ID_TOO_SHORT".to_string(),
            },
        };
        let err = execute(&args, &state).expect_err("should reject invalid id");
        assert!(err.contains("invalid wayfarer_id"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
