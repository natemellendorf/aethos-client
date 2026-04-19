use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct CliState {
    pub data_dir: PathBuf,
    pub relay_endpoint: String,
    pub verbose: bool,
}

impl CliState {
    pub fn from_cli_args(
        data_dir_override: Option<&str>,
        relay_override: Option<&str>,
        verbose: bool,
    ) -> Self {
        Self {
            data_dir: crate::output::cli_data_dir(data_dir_override),
            relay_endpoint: relay_override
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("wss://aethos-relay.network")
                .to_string(),
            verbose,
        }
    }

    pub fn setup_env(&self) {
        crate::output::set_cli_data_dir(&self.data_dir);
        std::env::set_var("AETHOS_STATE_DIR", &self.data_dir);
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
