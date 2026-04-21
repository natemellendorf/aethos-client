use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct CliState {
    pub data_dir: PathBuf,
    pub relay_endpoint: String,
    pub verbose: bool,
    pub run_id: Option<String>,
    pub diagnostics_collector_url: Option<String>,
}

impl CliState {
    pub fn from_cli_args(
        data_dir_override: Option<&str>,
        relay_override: Option<&str>,
        verbose: bool,
    ) -> Self {
        Self::from_cli_args_with_diagnostics(data_dir_override, relay_override, verbose, None, None)
    }

    pub fn from_cli_args_with_diagnostics(
        data_dir_override: Option<&str>,
        relay_override: Option<&str>,
        verbose: bool,
        run_id: Option<&str>,
        diagnostics_collector_url: Option<&str>,
    ) -> Self {
        Self {
            data_dir: crate::output::cli_data_dir(data_dir_override),
            relay_endpoint: relay_override
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("wss://aethos-relay.network")
                .to_string(),
            verbose,
            run_id: run_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            diagnostics_collector_url: diagnostics_collector_url
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_end_matches('/').to_string()),
        }
    }

    pub fn setup_env(&self) {
        crate::output::set_cli_data_dir(&self.data_dir);
        std::env::set_var("AETHOS_STATE_DIR", &self.data_dir);
        std::env::set_var("AETHOS_APP_NAME", "aethos-cli");
        if let Some(run_id) = &self.run_id {
            std::env::set_var("AETHOS_DIAGNOSTICS_RUN_ID", run_id);
        }
        if let Some(url) = &self.diagnostics_collector_url {
            std::env::set_var("AETHOS_DIAGNOSTICS_COLLECTOR_URL", url);
        }
        crate::aethos_core::diagnostics::attach_current_run("cli", "aethos-cli");
        crate::aethos_core::diagnostics::emit_app_lifecycle(
            "cli",
            "start",
            Some("cli command started"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::CliState;
    use std::path::PathBuf;

    #[test]
    fn resolves_explicit_data_dir_override() {
        let state = CliState::from_cli_args(Some("/tmp/test"), None, false);

        assert_eq!(state.data_dir, PathBuf::from("/tmp/test"));
    }

    #[test]
    fn uses_default_relay_endpoint() {
        let state = CliState::from_cli_args(None, None, false);

        assert_eq!(state.relay_endpoint, "wss://aethos-relay.network");
    }

    #[test]
    fn respects_custom_relay_endpoint() {
        let state = CliState::from_cli_args(None, Some("wss://custom.relay"), false);

        assert_eq!(state.relay_endpoint, "wss://custom.relay");
    }
}
