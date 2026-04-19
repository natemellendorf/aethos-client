#![allow(dead_code)]

#[path = "../../aethos_core/mod.rs"]
mod aethos_core;
#[path = "../../relay/mod.rs"]
mod relay;

mod commands;
mod output;
mod state;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "aethos-cli", version, about = "Aethos CLI scaffold")]
struct Cli {
    #[arg(long)]
    relay: Option<String>,

    #[arg(long, value_name = "PATH")]
    data_dir: Option<String>,

    #[arg(long)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Identity(commands::identity::IdentityArgs),
    Send(commands::send::SendArgs),
    Listen(commands::listen::ListenArgs),
    Contacts(commands::contacts::ContactsArgs),
    Relay(commands::relay::RelayArgs),
    Gossip(commands::gossip::GossipArgs),
    Diagnostics,
    Log(commands::log_cmd::LogArgs),
    Settings(commands::settings::SettingsArgs),
    Chat(commands::chat::ChatArgs),
    Share(commands::share::ShareArgs),
    Encounter(commands::encounter::EncounterArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let state =
        state::CliState::from_cli_args(cli.data_dir.as_deref(), cli.relay.as_deref(), cli.verbose);
    state.setup_env();

    let result = match cli.command {
        Commands::Identity(args) => commands::identity::run(&args, &state),
        Commands::Send(args) => commands::send::run(&args, &state),
        Commands::Listen(args) => {
            let result = commands::listen::run(&args, &state);
            if result.is_ok() && commands::listen::take_requested_exit_code() == 2 {
                std::process::exit(2);
            }
            result
        }
        Commands::Contacts(args) => commands::contacts::run(&args, &state),
        Commands::Relay(args) => commands::relay::run(&args, &state),
        Commands::Gossip(args) => commands::gossip::run(&args, &state),
        Commands::Diagnostics => commands::diagnostics::run(&state),
        Commands::Log(args) => commands::log_cmd::run(&args, &state),
        Commands::Settings(args) => commands::settings::run(&args, &state),
        Commands::Chat(args) => commands::chat::run(&args, &state),
        Commands::Share(args) => commands::share::run(&args, &state),
        Commands::Encounter(_args) => commands::encounter::run_status(&state),
    };

    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
pub(crate) fn global_test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::{commands, Cli, Commands};
    use clap::Parser;

    #[test]
    fn parses_identity_create_subcommand() {
        let cli = Cli::try_parse_from(["aethos-cli", "identity", "create"])
            .expect("parse identity create");

        assert!(matches!(
            cli.command,
            Commands::Identity(commands::identity::IdentityArgs {
                cmd: commands::identity::IdentityCmd::Create
            })
        ));
        assert!(cli.relay.is_none());
        assert!(cli.data_dir.is_none());
        assert!(!cli.verbose);
    }

    #[test]
    fn parses_identity_reset_confirm_flag() {
        let cli = Cli::try_parse_from(["aethos-cli", "identity", "reset", "--confirm"])
            .expect("parse identity reset confirm");

        assert!(matches!(
            cli.command,
            Commands::Identity(commands::identity::IdentityArgs {
                cmd: commands::identity::IdentityCmd::Reset { confirm: true }
            })
        ));
    }

    #[test]
    fn parses_settings_update_subcommand() {
        let cli = Cli::try_parse_from([
            "aethos-cli",
            "settings",
            "update",
            "--relay-endpoint",
            "wss://custom.relay",
            "--verbose",
            "true",
            "--relay-sync",
            "false",
            "--gossip-sync",
            "true",
            "--ttl",
            "7200",
        ])
        .expect("parse settings update");

        assert!(matches!(
            cli.command,
            Commands::Settings(commands::settings::SettingsArgs {
                cmd: commands::settings::SettingsCmd::Update {
                    relay_endpoint,
                    verbose: Some(true),
                    relay_sync: Some(false),
                    gossip_sync: Some(true),
                    ttl: Some(7200),
                }
            }) if relay_endpoint.as_deref() == Some("wss://custom.relay")
        ));
    }

    #[test]
    fn parses_diagnostics_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "diagnostics"]).expect("parse diagnostics");

        assert!(matches!(cli.command, Commands::Diagnostics));
    }

    #[test]
    fn parses_send_command_args() {
        let cli = Cli::try_parse_from([
            "aethos-cli",
            "send",
            "--to",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "--text",
            "hello",
            "--ttl",
            "90",
        ])
        .expect("parse send command");

        assert!(matches!(
            cli.command,
            Commands::Send(commands::send::SendArgs { to, text, ttl })
                if to == "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    && text == "hello"
                    && ttl == 90
        ));
    }

    #[test]
    fn parses_listen_command_args() {
        let cli = Cli::try_parse_from([
            "aethos-cli",
            "listen",
            "--timeout",
            "15",
            "--relay",
            "localhost:8082",
            "--filter-from",
            "abc123",
        ])
        .expect("parse listen command");

        assert!(matches!(
            cli.command,
            Commands::Listen(commands::listen::ListenArgs {
                timeout: Some(15),
                relay,
                filter_from,
            }) if relay.as_deref() == Some("localhost:8082")
                && filter_from.as_deref() == Some("abc123")
        ));
    }

    #[test]
    fn parses_relay_health_command() {
        let cli =
            Cli::try_parse_from(["aethos-cli", "relay", "health"]).expect("parse relay health");

        assert!(matches!(
            cli.command,
            Commands::Relay(commands::relay::RelayArgs {
                cmd: commands::relay::RelayCmd::Health
            })
        ));
    }

    #[test]
    fn parses_relay_diagnostics_command_with_override() {
        let cli = Cli::try_parse_from([
            "aethos-cli",
            "relay",
            "diagnostics",
            "--relay",
            "localhost:19999",
        ])
        .expect("parse relay diagnostics");

        assert!(matches!(
            cli.command,
            Commands::Relay(commands::relay::RelayArgs {
                cmd: commands::relay::RelayCmd::Diagnostics { relay }
            }) if relay.as_deref() == Some("localhost:19999")
        ));
    }

    #[test]
    fn parses_relay_sync_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "relay", "sync"]).expect("parse relay sync");

        assert!(matches!(
            cli.command,
            Commands::Relay(commands::relay::RelayArgs {
                cmd: commands::relay::RelayCmd::Sync
            })
        ));
    }

    #[test]
    fn parses_log_show_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "log", "show", "--lines", "12"])
            .expect("parse log show");

        assert!(matches!(
            cli.command,
            Commands::Log(commands::log_cmd::LogArgs {
                cmd: commands::log_cmd::LogCmd::Show { lines: 12 }
            })
        ));
    }

    #[test]
    fn parses_gossip_status_command() {
        let cli =
            Cli::try_parse_from(["aethos-cli", "gossip", "status"]).expect("parse gossip status");

        assert!(matches!(
            cli.command,
            Commands::Gossip(commands::gossip::GossipArgs {
                cmd: commands::gossip::GossipCmd::Status
            })
        ));
    }

    #[test]
    fn parses_gossip_announce_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "gossip", "announce"])
            .expect("parse gossip announce");

        assert!(matches!(
            cli.command,
            Commands::Gossip(commands::gossip::GossipArgs {
                cmd: commands::gossip::GossipCmd::Announce
            })
        ));
    }

    #[test]
    fn parses_gossip_store_stats_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "gossip", "store-stats"])
            .expect("parse gossip store stats");

        assert!(matches!(
            cli.command,
            Commands::Gossip(commands::gossip::GossipArgs {
                cmd: commands::gossip::GossipCmd::StoreStats
            })
        ));
    }

    #[test]
    fn parses_gossip_discover_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "gossip", "discover", "--timeout", "10"])
            .expect("parse gossip discover");

        assert!(matches!(
            cli.command,
            Commands::Gossip(commands::gossip::GossipArgs {
                cmd: commands::gossip::GossipCmd::Discover { timeout: 10 }
            })
        ));
    }

    #[test]
    fn parses_gossip_discover_default_timeout() {
        let cli = Cli::try_parse_from(["aethos-cli", "gossip", "discover"])
            .expect("parse gossip discover default");

        assert!(matches!(
            cli.command,
            Commands::Gossip(commands::gossip::GossipArgs {
                cmd: commands::gossip::GossipCmd::Discover { timeout: 5 }
            })
        ));
    }

    #[test]
    fn parses_chat_clear_command() {
        let cli = Cli::try_parse_from([
            "aethos-cli",
            "chat",
            "clear",
            "--contact",
            "peer1",
            "--confirm",
        ])
        .expect("parse chat clear");

        assert!(matches!(
            cli.command,
            Commands::Chat(commands::chat::ChatArgs {
                cmd: commands::chat::ChatCmd::Clear(commands::chat::ChatClearArgs {
                    contact,
                    confirm: true,
                })
            }) if contact == "peer1"
        ));
    }

    #[test]
    fn parses_encounter_status_command() {
        let cli = Cli::try_parse_from(["aethos-cli", "encounter", "status"])
            .expect("parse encounter status");

        assert!(matches!(
            cli.command,
            Commands::Encounter(commands::encounter::EncounterArgs {
                command: commands::encounter::EncounterCmd::Status
            })
        ));
    }
}
