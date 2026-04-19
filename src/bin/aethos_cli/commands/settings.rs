use std::fs;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Args)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub cmd: SettingsCmd,
}

#[derive(Debug, Subcommand)]
pub enum SettingsCmd {
    /// Display current settings
    Show,
    /// Update one or more settings
    Update {
        #[arg(long, help = "Relay endpoint URL (e.g. wss://relay.example.com)")]
        relay_endpoint: Option<String>,
        #[arg(long, help = "Enable or disable verbose logging (true/false)")]
        verbose: Option<bool>,
        #[arg(
            long,
            help = "Enable or disable relay sync background service (true/false)"
        )]
        relay_sync: Option<bool>,
        #[arg(
            long,
            help = "Enable or disable LAN gossip sync background service (true/false)"
        )]
        gossip_sync: Option<bool>,
        #[arg(long, help = "Default message time-to-live in seconds")]
        ttl: Option<u64>,
    },
    /// Reset all settings to defaults
    Reset,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppSettings {
    #[serde(alias = "relay_sync_enabled")]
    pub relay_sync_enabled: bool,
    #[serde(alias = "gossip_sync_enabled")]
    pub gossip_sync_enabled: bool,
    #[serde(alias = "verbose_logging_enabled")]
    pub verbose_logging: bool,
    #[serde(default, deserialize_with = "deserialize_relay_endpoint")]
    pub relay_endpoint: Option<String>,
    #[serde(alias = "message_ttl_seconds")]
    pub message_ttl_seconds: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            relay_sync_enabled: true,
            gossip_sync_enabled: true,
            verbose_logging: false,
            relay_endpoint: None,
            message_ttl_seconds: 3600,
        }
    }
}

pub fn run(args: &SettingsArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(args: &SettingsArgs, state: &crate::state::CliState) -> Result<(String, Value), String> {
    match &args.cmd {
        SettingsCmd::Show => {
            let settings = load_app_settings(state)?;
            Ok(("settings_loaded".to_string(), settings_payload(&settings)))
        }
        SettingsCmd::Update {
            relay_endpoint,
            verbose,
            relay_sync,
            gossip_sync,
            ttl,
        } => {
            let mut settings = load_app_settings(state)?;
            let mut updated = Map::new();

            if let Some(value) = relay_endpoint {
                settings.relay_endpoint = normalize_relay_endpoint(Some(value.clone()));
                updated.insert("relay_endpoint".to_string(), json!(settings.relay_endpoint));
            }
            if let Some(value) = verbose {
                settings.verbose_logging = *value;
                updated.insert("verbose".to_string(), json!(value));
            }
            if let Some(value) = relay_sync {
                settings.relay_sync_enabled = *value;
                updated.insert("relay_sync".to_string(), json!(value));
            }
            if let Some(value) = gossip_sync {
                settings.gossip_sync_enabled = *value;
                updated.insert("gossip_sync".to_string(), json!(value));
            }
            if let Some(value) = ttl {
                settings.message_ttl_seconds = *value;
                updated.insert("ttl".to_string(), json!(value));
            }

            save_app_settings(state, &settings)?;
            Ok(("settings_updated".to_string(), Value::Object(updated)))
        }
        SettingsCmd::Reset => {
            let settings = AppSettings::default();
            save_app_settings(state, &settings)?;
            Ok(("settings_reset".to_string(), settings_payload(&settings)))
        }
    }
}

fn settings_path(state: &crate::state::CliState) -> PathBuf {
    state.data_dir.join("settings.json")
}

fn load_app_settings(state: &crate::state::CliState) -> Result<AppSettings, String> {
    let path = settings_path(state);
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read settings file {}: {error}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|error| format!("failed to parse settings file {}: {error}", path.display()))
}

fn save_app_settings(state: &crate::state::CliState, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(state);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create settings directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let encoded = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("failed to serialize settings: {error}"))?;
    fs::write(&path, encoded)
        .map_err(|error| format!("failed to write settings file {}: {error}", path.display()))
}

fn settings_payload(settings: &AppSettings) -> Value {
    json!({
        "relay_sync_enabled": settings.relay_sync_enabled,
        "gossip_sync_enabled": settings.gossip_sync_enabled,
        "verbose_logging": settings.verbose_logging,
        "relay_endpoint": settings.relay_endpoint,
        "message_ttl_seconds": settings.message_ttl_seconds,
    })
}

fn normalize_relay_endpoint(value: Option<String>) -> Option<String> {
    value.and_then(|candidate| {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn deserialize_relay_endpoint<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error, Visitor};
    use std::fmt;

    struct RelayEndpointVisitor;

    impl<'de> Visitor<'de> for RelayEndpointVisitor {
        type Value = Option<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a relay endpoint string, array, or null")
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            Deserialize::deserialize(deserializer)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: Error,
        {
            self.visit_str(&value)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            while let Some(value) = seq.next_element::<String>()? {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Ok(Some(trimmed.to_string()));
                }
            }
            Ok(None)
        }
    }

    deserializer.deserialize_any(RelayEndpointVisitor)
}

#[cfg(test)]
mod tests {
    use super::{execute, AppSettings, SettingsArgs, SettingsCmd};
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

    #[test]
    fn settings_show_defaults_when_file_missing() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("settings-show");
        let state = test_state(&base_dir);

        let args = SettingsArgs {
            cmd: SettingsCmd::Show,
        };
        let (event_type, data) = execute(&args, &state).expect("show settings");

        assert_eq!(event_type, "settings_loaded");
        assert_eq!(data["relay_sync_enabled"], true);
        assert_eq!(data["gossip_sync_enabled"], true);
        assert_eq!(data["verbose_logging"], false);
        assert_eq!(data["relay_endpoint"], serde_json::Value::Null);
        assert_eq!(data["message_ttl_seconds"], 3600);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn settings_update_persists_single_field() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("settings-update");
        let state = test_state(&base_dir);

        let args = SettingsArgs {
            cmd: SettingsCmd::Update {
                relay_endpoint: Some("wss://relay.example".to_string()),
                verbose: None,
                relay_sync: None,
                gossip_sync: None,
                ttl: None,
            },
        };
        let (event_type, data) = execute(&args, &state).expect("update settings");

        assert_eq!(event_type, "settings_updated");
        assert_eq!(data["relay_endpoint"], "wss://relay.example");
        assert!(data.get("verbose").is_none());

        let show_args = SettingsArgs {
            cmd: SettingsCmd::Show,
        };
        let (_, loaded) = execute(&show_args, &state).expect("reload settings");
        assert_eq!(loaded["relay_endpoint"], "wss://relay.example");
        assert_eq!(loaded["relay_sync_enabled"], true);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn settings_reset_returns_defaults() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("settings-reset");
        let state = test_state(&base_dir);

        let update_args = SettingsArgs {
            cmd: SettingsCmd::Update {
                relay_endpoint: Some("wss://relay.example".to_string()),
                verbose: Some(true),
                relay_sync: Some(false),
                gossip_sync: Some(false),
                ttl: Some(120),
            },
        };
        execute(&update_args, &state).expect("seed settings");

        let reset_args = SettingsArgs {
            cmd: SettingsCmd::Reset,
        };
        let (event_type, data) = execute(&reset_args, &state).expect("reset settings");

        assert_eq!(event_type, "settings_reset");
        assert_eq!(data["relay_sync_enabled"], true);
        assert_eq!(data["gossip_sync_enabled"], true);
        assert_eq!(data["verbose_logging"], false);
        assert_eq!(data["relay_endpoint"], serde_json::Value::Null);
        assert_eq!(data["message_ttl_seconds"], 3600);

        let show_args = SettingsArgs {
            cmd: SettingsCmd::Show,
        };
        let (_, loaded) = execute(&show_args, &state).expect("reload defaults");
        assert_eq!(loaded, json_to_value(&AppSettings::default()));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    fn json_to_value(settings: &AppSettings) -> serde_json::Value {
        serde_json::json!({
            "relay_sync_enabled": settings.relay_sync_enabled,
            "gossip_sync_enabled": settings.gossip_sync_enabled,
            "verbose_logging": settings.verbose_logging,
            "relay_endpoint": settings.relay_endpoint,
            "message_ttl_seconds": settings.message_ttl_seconds,
        })
    }
}
