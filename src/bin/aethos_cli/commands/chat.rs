use std::collections::BTreeMap;
use std::fs;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::json;

const CHAT_HISTORY_FILE_NAME: &str = "chat-history.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatMessage {
    pub id: String,
    pub from_wayfarer_id: String,
    pub to_wayfarer_id: String,
    pub body_text: String,
    pub sent_at_ms: u64,
    pub outbound: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PersistedChatState {
    pub threads: BTreeMap<String, Vec<ChatMessage>>,
}

#[derive(Debug, Args)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub cmd: ChatCmd,
}

#[derive(Debug, Subcommand)]
pub enum ChatCmd {
    /// Show all chat threads summary
    Snapshot,
    /// Show message history for a specific contact
    History {
        #[arg(long, help = "Wayfarer ID of the contact")]
        contact: String,
    },
    /// Delete chat history for a contact (requires --confirm)
    Clear(ChatClearArgs),
}

#[derive(Debug, Args)]
pub struct ChatClearArgs {
    #[arg(long, help = "Wayfarer ID of the contact to clear")]
    pub contact: String,

    #[arg(long, help = "Required safety flag to confirm deletion")]
    pub confirm: bool,
}

pub fn run(args: &ChatArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn load_chat_state(state: &crate::state::CliState) -> Result<PersistedChatState, String> {
    let path = state.data_dir.join(CHAT_HISTORY_FILE_NAME);
    if !path.exists() {
        return Ok(PersistedChatState::default());
    }
    let raw = fs::read_to_string(&path)
        .map_err(|err| format!("failed reading chat history {}: {err}", path.display()))?;
    serde_json::from_str(&raw)
        .map_err(|err| format!("failed parsing chat history {}: {err}", path.display()))
}

fn save_chat_state(
    state: &crate::state::CliState,
    chat_state: &PersistedChatState,
) -> Result<(), String> {
    let path = state.data_dir.join(CHAT_HISTORY_FILE_NAME);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed creating chat history dir {}: {err}",
                parent.display()
            )
        })?;
    }
    let raw = serde_json::to_string(chat_state)
        .map_err(|err| format!("failed serializing chat history {}: {err}", path.display()))?;
    fs::write(&path, raw)
        .map_err(|err| format!("failed writing chat history {}: {err}", path.display()))
}

fn execute(
    args: &ChatArgs,
    state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match &args.cmd {
        ChatCmd::Snapshot => {
            let chat_state = load_chat_state(state)?;
            Ok((
                "chat_snapshot".to_string(),
                json!({ "threads": chat_state.threads }),
            ))
        }
        ChatCmd::History { contact } => {
            let trimmed = contact.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                let message = "contact id must not be empty";
                crate::output::emit_error(message);
                return Err(message.to_string());
            }
            let chat_state = load_chat_state(state)?;
            let messages = chat_state
                .threads
                .get(&trimmed)
                .cloned()
                .unwrap_or_default();
            Ok((
                "chat_history".to_string(),
                json!({ "contact": trimmed, "messages": messages }),
            ))
        }
        ChatCmd::Clear(clear_args) => {
            let trimmed = clear_args.contact.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                let message = "contact id must not be empty";
                crate::output::emit_error(message);
                return Err(message.to_string());
            }
            if !clear_args.confirm {
                let message = "use --confirm to clear chat history";
                crate::output::emit_error(message);
                return Err(message.to_string());
            }

            let mut chat_state = load_chat_state(state)?;
            let cleared_count = chat_state
                .threads
                .remove(&trimmed)
                .map_or(0, |messages| messages.len());
            save_chat_state(state, &chat_state)?;

            Ok((
                "chat_cleared".to_string(),
                json!({ "contact": trimmed, "cleared_count": cleared_count }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, ChatArgs, ChatCmd, PersistedChatState};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aethos-cli-chat-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn test_state(base_dir: &std::path::Path) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(base_dir.to_str(), None, false)
    }

    #[test]
    fn snapshot_empty_state_returns_empty_threads() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("snapshot");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ChatArgs {
            cmd: ChatCmd::Snapshot,
        };
        let (event_type, data) = execute(&args, &state).expect("chat snapshot");

        assert_eq!(event_type, "chat_snapshot");
        assert_eq!(data["threads"], serde_json::json!({}));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn history_empty_state_returns_empty_messages() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("history-empty");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ChatArgs {
            cmd: ChatCmd::History {
                contact: "abc123".to_string(),
            },
        };
        let (event_type, data) = execute(&args, &state).expect("chat history");

        assert_eq!(event_type, "chat_history");
        assert_eq!(data["contact"], "abc123");
        assert_eq!(data["messages"], serde_json::json!([]));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn snapshot_loads_persisted_threads() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("snapshot-persisted");
        std::fs::create_dir_all(&base_dir).expect("create dir");
        let state = test_state(&base_dir);
        state.setup_env();

        let chat_state = PersistedChatState {
            threads: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "peer1".to_string(),
                    vec![super::ChatMessage {
                        id: "msg1".to_string(),
                        from_wayfarer_id: "peer1".to_string(),
                        to_wayfarer_id: "me".to_string(),
                        body_text: "hello".to_string(),
                        sent_at_ms: 1000,
                        outbound: false,
                    }],
                );
                m
            },
        };
        let path = base_dir.join("chat-history.json");
        std::fs::write(&path, serde_json::to_string(&chat_state).unwrap()).unwrap();

        let args = ChatArgs {
            cmd: ChatCmd::Snapshot,
        };
        let (event_type, data) = execute(&args, &state).expect("snapshot with data");

        assert_eq!(event_type, "chat_snapshot");
        assert!(data["threads"]["peer1"].is_array());
        assert_eq!(data["threads"]["peer1"][0]["body_text"], "hello");

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn history_empty_contact_returns_error() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("history-empty-contact");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ChatArgs {
            cmd: ChatCmd::History {
                contact: "  ".to_string(),
            },
        };
        let err = execute(&args, &state).expect_err("should fail with empty contact");
        assert!(err.contains("empty"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn clear_requires_confirmation() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("clear-confirm");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = ChatArgs {
            cmd: ChatCmd::Clear(super::ChatClearArgs {
                contact: "peer1".to_string(),
                confirm: false,
            }),
        };
        let err = execute(&args, &state).expect_err("clear requires confirm");
        assert!(err.contains("--confirm"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn clear_removes_contact_messages_and_persists() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("clear-persisted");
        std::fs::create_dir_all(&base_dir).expect("create dir");
        let state = test_state(&base_dir);
        state.setup_env();

        let chat_state = PersistedChatState {
            threads: {
                let mut m = std::collections::BTreeMap::new();
                m.insert(
                    "peer1".to_string(),
                    vec![super::ChatMessage {
                        id: "msg1".to_string(),
                        from_wayfarer_id: "peer1".to_string(),
                        to_wayfarer_id: "me".to_string(),
                        body_text: "hello".to_string(),
                        sent_at_ms: 1000,
                        outbound: false,
                    }],
                );
                m.insert(
                    "peer2".to_string(),
                    vec![super::ChatMessage {
                        id: "msg2".to_string(),
                        from_wayfarer_id: "peer2".to_string(),
                        to_wayfarer_id: "me".to_string(),
                        body_text: "hi".to_string(),
                        sent_at_ms: 2000,
                        outbound: false,
                    }],
                );
                m
            },
        };
        let path = base_dir.join("chat-history.json");
        std::fs::write(&path, serde_json::to_string(&chat_state).unwrap()).unwrap();

        let args = ChatArgs {
            cmd: ChatCmd::Clear(super::ChatClearArgs {
                contact: " PEER1 ".to_string(),
                confirm: true,
            }),
        };
        let (event_type, data) = execute(&args, &state).expect("clear chat");

        assert_eq!(event_type, "chat_cleared");
        assert_eq!(data["contact"], "peer1");
        assert_eq!(data["cleared_count"], 1);

        let saved: PersistedChatState =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read saved chat history"))
                .expect("parse saved history");
        assert!(!saved.threads.contains_key("peer1"));
        assert!(saved.threads.contains_key("peer2"));

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
