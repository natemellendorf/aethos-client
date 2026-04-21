use serde_json::json;

pub fn run(state: &crate::state::CliState) -> Result<(), String> {
    let (event_type, data) = execute(state)?;
    crate::output::emit_success(&event_type, data);
    Ok(())
}

fn execute(state: &crate::state::CliState) -> Result<(String, serde_json::Value), String> {
    let log_file_path = {
        let path = crate::aethos_core::logging::app_log_file_path();
        if path.exists() {
            path
        } else {
            state.data_dir.join("aethos.log")
        }
    };

    Ok((
        "app_diagnostics".to_string(),
        json!({
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "data_dir": state.data_dir.display().to_string(),
            "log_file_path": log_file_path.display().to_string(),
            "verbose": state.verbose,
            "run_id": state.run_id,
            "diagnostics_collector_url": state.diagnostics_collector_url,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::execute;

    #[test]
    fn diagnostics_payload_includes_required_fields() {
        let state = crate::state::CliState::from_cli_args_with_diagnostics(
            Some("/tmp/aethos-cli-diag"),
            None,
            true,
            Some("run-123"),
            Some("http://127.0.0.1:9774"),
        );
        let (event_type, data) = execute(&state).expect("diagnostics payload");

        assert_eq!(event_type, "app_diagnostics");
        assert_eq!(
            data["platform"].as_str().unwrap_or(""),
            std::env::consts::OS
        );
        assert_eq!(data["arch"].as_str().unwrap_or(""), std::env::consts::ARCH);
        assert_eq!(
            data["version"].as_str().unwrap_or(""),
            env!("CARGO_PKG_VERSION")
        );
        assert!(matches!(
            data["profile"].as_str(),
            Some("debug") | Some("release")
        ));
        assert_eq!(data["data_dir"], "/tmp/aethos-cli-diag");
        assert_eq!(data["verbose"], true);
        assert_eq!(data["run_id"], "run-123");
        assert_eq!(data["diagnostics_collector_url"], "http://127.0.0.1:9774");
    }
}
