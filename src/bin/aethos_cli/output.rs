use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

const APP_DIR_NAME: &str = "aethos-linux";
const MAC_APP_DIR_NAME: &str = "aethos";

pub fn emit_event(event_type: &str, data: serde_json::Value) {
    let mut stdout = io::stdout().lock();
    let record = render_event_line(event_type, data);

    if writeln!(stdout, "{}", record).is_ok() {
        let _ = stdout.flush();
    }
}

pub fn emit_error(message: &str) {
    emit_event("error", json!({"message": message}));

    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{}", message);
    let _ = stderr.flush();
}

pub fn emit_success(event_type: &str, data: serde_json::Value) {
    emit_event(event_type, data);
}

pub fn cli_data_dir(data_dir_override: Option<&str>) -> PathBuf {
    if let Some(override_dir) = data_dir_override.filter(|value| !value.trim().is_empty()) {
        return PathBuf::from(override_dir);
    }

    if let Some(xdg_data_home) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(xdg_data_home).join(APP_DIR_NAME);
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(MAC_APP_DIR_NAME);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(APP_DIR_NAME);
        }
    }

    env::temp_dir().join(APP_DIR_NAME)
}

pub fn set_cli_data_dir(path: &Path) {
    let xdg_data_home = if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == APP_DIR_NAME)
    {
        path.parent().unwrap_or(path)
    } else {
        path
    };

    env::set_var("XDG_DATA_HOME", xdg_data_home);
}

fn event_record(event_type: &str, data: serde_json::Value) -> serde_json::Value {
    json!({
        "ts": unix_ms_now(),
        "event": event_type,
        "data": data,
    })
}

fn render_event_line(event_type: &str, data: serde_json::Value) -> String {
    serde_json::to_string(&event_record(event_type, data)).expect("serialize event record")
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{cli_data_dir, render_event_line, unix_ms_now};
    use serde_json::json;

    #[test]
    fn event_record_has_expected_json_shape() {
        let line = render_event_line("hello", json!({"value": 7}));
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("parse event json");

        assert!(parsed["ts"].as_u64().unwrap_or(0) > 0);
        assert_eq!(parsed["event"], "hello");
        assert_eq!(parsed["data"]["value"], 7);
    }

    #[test]
    fn unix_ms_now_is_positive() {
        assert!(unix_ms_now() > 0);
    }

    #[test]
    fn cli_data_dir_without_override_is_not_empty() {
        let path = cli_data_dir(None);
        assert!(!path.as_os_str().is_empty());
    }
}
