use std::time::{SystemTime, UNIX_EPOCH};

use clap::Args;
use serde_json::json;

#[derive(Debug, Args)]
/// Compose and record a message to the local gossip store for delivery
pub struct SendArgs {
    #[arg(long, help = "Recipient wayfarer_id (64 lowercase hex chars)")]
    pub to: String,
    #[arg(long, help = "Message body text")]
    pub text: String,
    #[arg(long, default_value_t = 3600, help = "Message time-to-live in seconds")]
    pub ttl: u64,
}

pub fn run(args: &SendArgs, state: &crate::state::CliState) -> Result<(), String> {
    let data = execute(args, state)?;
    crate::output::emit_success("send_ok", data);
    Ok(())
}

fn execute(args: &SendArgs, _state: &crate::state::CliState) -> Result<serde_json::Value, String> {
    let recipient = args.to.trim();
    if !crate::aethos_core::protocol::is_valid_wayfarer_id(recipient) {
        let message = "invalid --to; expected 64 lowercase hex chars";
        crate::output::emit_error(message);
        return Err(message.to_string());
    }

    let text = args.text.trim();
    if text.is_empty() {
        let message = "empty --text is not allowed";
        crate::output::emit_error(message);
        return Err(message.to_string());
    }

    let identity = crate::aethos_core::identity_store::ensure_local_identity()?;
    let signing_seed = crate::aethos_core::identity_store::load_local_signing_key_seed()?;
    let created_at_unix_ms = unix_ms_now();
    let payload_b64 = crate::aethos_core::protocol::build_wayfarer_chat_envelope_payload_b64(
        recipient,
        text,
        &signing_seed,
        created_at_unix_ms as i64,
    )?;
    let expiry_ms = created_at_unix_ms.saturating_add(args.ttl.saturating_mul(1000));
    let item_id = crate::aethos_core::gossip_sync::record_local_payload(&payload_b64, expiry_ms)?;

    Ok(json!({
        "item_id": item_id,
        "wayfarer_id": identity.wayfarer_id,
        "recipient": recipient,
        "relay_sync_attempted": false,
    }))
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{execute, SendArgs};
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
    fn send_valid_input_records_item_in_gossip_store() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("send-success");
        let state = test_state(&base_dir);
        state.setup_env();

        let recipient = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let args = SendArgs {
            to: recipient.to_string(),
            text: "hello from cli".to_string(),
            ttl: 3600,
        };

        let data = execute(&args, &state).expect("send should succeed");
        let item_id = data["item_id"].as_str().expect("item id").to_string();
        let wayfarer_id = data["wayfarer_id"]
            .as_str()
            .expect("wayfarer id")
            .to_string();

        state.setup_env();

        assert_eq!(data["recipient"], recipient);
        assert_eq!(data["relay_sync_attempted"], false);
        assert_eq!(wayfarer_id.len(), 64);

        let stored = crate::aethos_core::gossip_store_sqlite::get_existing_items_for_ids(
            std::slice::from_ref(&item_id),
        )
        .expect("load stored item");
        let record = stored.get(&item_id).expect("stored item record exists");
        assert_eq!(record.item_id, item_id);

        let decoded =
            crate::aethos_core::protocol::decode_envelope_payload_b64(&record.envelope_b64)
                .expect("decode stored envelope");
        assert_eq!(decoded.to_wayfarer_id_hex, recipient);
        assert_eq!(decoded.author_wayfarer_id_hex, wayfarer_id);

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn send_rejects_invalid_wayfarer_id() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("send-invalid-to");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = SendArgs {
            to: "not-a-wayfarer-id".to_string(),
            text: "hello".to_string(),
            ttl: 3600,
        };

        let error = execute(&args, &state).expect_err("invalid recipient should fail");
        assert_eq!(error, "invalid --to; expected 64 lowercase hex chars");

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn send_rejects_empty_text() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let base_dir = unique_temp_dir("send-empty-text");
        let state = test_state(&base_dir);
        state.setup_env();

        let args = SendArgs {
            to: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            text: "   \n\t  ".to_string(),
            ttl: 3600,
        };

        let error = execute(&args, &state).expect_err("empty text should fail");
        assert_eq!(error, "empty --text is not allowed");

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
