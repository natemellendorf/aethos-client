use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
pub struct LogArgs {
    #[command(subcommand)]
    pub cmd: LogCmd,
}

#[derive(Debug, Subcommand)]
pub enum LogCmd {
    /// Show recent log entries
    Show {
        #[arg(
            long,
            default_value_t = 50,
            help = "Number of recent lines to display [default: 50]"
        )]
        lines: usize,
    },
    /// Clear the log file
    Clear,
    /// Print the log file path
    Path,
}

pub fn run(args: &LogArgs, state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(args, state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(
    args: &LogArgs,
    state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    let path = resolve_log_path(state);

    match args.cmd {
        LogCmd::Path => Ok((
            "app_log_path".to_string(),
            json!({"log_path": path.display().to_string()}),
        )),
        LogCmd::Show { lines } => {
            let content = match std::fs::read_to_string(&path) {
                Ok(raw) => tail_lines(&raw, lines),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(err) => {
                    return Err(format!(
                        "failed reading app log at {}: {err}",
                        path.display()
                    ));
                }
            };

            Ok((
                "app_log".to_string(),
                json!({"lines": lines, "content": content}),
            ))
        }
        LogCmd::Clear => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    format!(
                        "failed creating app log directory {}: {err}",
                        parent.display()
                    )
                })?;
            }
            std::fs::write(&path, "")
                .map_err(|err| format!("failed clearing app log at {}: {err}", path.display()))?;

            Ok((
                "app_log_cleared".to_string(),
                json!({"log_path": path.display().to_string()}),
            ))
        }
    }
}

fn resolve_log_path(state: &crate::state::CliState) -> std::path::PathBuf {
    let path = crate::aethos_core::logging::app_log_file_path();
    if path.exists() {
        path
    } else {
        state.data_dir.join("aethos.log")
    }
}

fn tail_lines(raw: &str, lines: usize) -> String {
    let all_lines = raw.lines().collect::<Vec<_>>();
    let start = all_lines.len().saturating_sub(lines);
    all_lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::{execute, LogArgs, LogCmd};
    use std::path::PathBuf;

    fn test_state(base_dir: &str) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(Some(base_dir), None, false)
    }

    fn log_path(state: &crate::state::CliState) -> PathBuf {
        super::resolve_log_path(state)
    }

    #[test]
    fn path_command_emits_log_path() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let state = test_state("/tmp/aethos-cli-log-path");
        state.setup_env();
        std::env::set_var("XDG_STATE_HOME", "/tmp/aethos-cli-log-path-state");

        let args = LogArgs { cmd: LogCmd::Path };
        let (event_type, data) = execute(&args, &state).expect("path command");

        assert_eq!(event_type, "app_log_path");
        assert!(data["log_path"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));

        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn show_missing_log_is_empty() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let state = test_state("/tmp/aethos-cli-log-show");
        state.setup_env();
        std::env::set_var("XDG_STATE_HOME", "/tmp/aethos-cli-log-show-state");

        let args = LogArgs {
            cmd: LogCmd::Show { lines: 10 },
        };
        let (event_type, data) = execute(&args, &state).expect("show command");

        assert_eq!(event_type, "app_log");
        assert_eq!(data["lines"], 10);
        assert_eq!(data["content"], "");

        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn clear_creates_empty_log_file() {
        let _guard = crate::global_test_env_lock().lock().expect("lock env");
        let state = test_state("/tmp/aethos-cli-log-clear");
        state.setup_env();
        std::env::set_var("XDG_STATE_HOME", "/tmp/aethos-cli-log-clear-state");

        let path = log_path(&state);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create log parent");
        }
        std::fs::write(&path, "one\ntwo\nthree\n").expect("seed log");

        let args = LogArgs { cmd: LogCmd::Clear };
        let (event_type, data) = execute(&args, &state).expect("clear command");

        assert_eq!(event_type, "app_log_cleared");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read cleared log"),
            ""
        );
        assert_eq!(
            data["log_path"].as_str().unwrap_or(""),
            path.display().to_string()
        );

        std::env::remove_var("XDG_STATE_HOME");
        let _ = std::fs::remove_dir_all("/tmp/aethos-cli-log-clear-state");
    }
}
