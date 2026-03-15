use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const APP_LOG_FILE_NAME: &str = "aethos-linux.log";

static VERBOSE_LOGGING_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_verbose_logging_enabled(enabled: bool) {
    VERBOSE_LOGGING_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn verbose_logging_enabled() -> bool {
    VERBOSE_LOGGING_ENABLED.load(Ordering::SeqCst)
}

pub fn log_info(message: &str) {
    if let Err(err) = append_local_log_inner(message) {
        eprintln!("local log warning: {err}");
    }
}

pub fn log_verbose(message: &str) {
    if verbose_logging_enabled() {
        log_info(message);
    }
}

pub fn app_log_file_path() -> PathBuf {
    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(APP_LOG_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(APP_LOG_FILE_NAME);
    }

    std::env::temp_dir().join(APP_LOG_FILE_NAME)
}

fn append_local_log_inner(message: &str) -> Result<(), String> {
    let path = app_log_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating app log directory: {err}"))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("failed opening app log file at {}: {err}", path.display()))?;

    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    };

    writeln!(file, "[{now}] {message}")
        .map_err(|err| format!("failed writing app log file at {}: {err}", path.display()))
}
