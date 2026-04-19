use clap::{Args, Subcommand};
use serde_json::json;

#[derive(Debug, Args)]
pub struct GossipArgs {
    #[command(subcommand)]
    pub cmd: GossipCmd,
}

#[derive(Debug, Subcommand)]
pub enum GossipCmd {
    Status,
    Announce,
    #[command(name = "store-stats")]
    StoreStats,
    /// Poll LAN for Bonjour/mDNS peers (Linux only)
    Discover {
        /// Seconds to listen for peer advertisements
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },
}

pub fn run(args: &GossipArgs, state: &crate::state::CliState) -> Result<(), String> {
    match &args.cmd {
        GossipCmd::Discover { timeout } => run_discover(*timeout),
        _ => {
            let (event_type, data) = execute(args, state)?;
            crate::output::emit_success(&event_type, data);
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
fn run_discover(timeout_secs: u64) -> Result<(), String> {
    use crate::aethos_core::bonjour_discovery::{BonjourDiscoveryEvent, BonjourLanDiscovery};
    use crate::aethos_core::gossip_sync::GOSSIP_LAN_PORT;
    use std::time::{Duration, Instant};

    let local_id = crate::aethos_core::identity_store::load_or_create_identity()
        .map(|id| id.wayfarer_id)
        .unwrap_or_else(|_| "unknown-local-peer".to_string());

    let mut discovery = BonjourLanDiscovery::new(&local_id, GOSSIP_LAN_PORT);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    crate::output::emit_success(
        "gossip_discover_started",
        json!({ "timeout_secs": timeout_secs }),
    );

    let mut peer_count: u64 = 0;
    while Instant::now() < deadline {
        for event in discovery.poll() {
            match event {
                BonjourDiscoveryEvent::AdvertisementStarted {
                    service_type,
                    instance_name,
                    domain,
                    port,
                } => {
                    crate::output::emit_event(
                        "bonjour_advertisement_started",
                        json!({
                            "service_type": service_type,
                            "instance_name": instance_name,
                            "domain": domain,
                            "port": port,
                        }),
                    );
                }
                BonjourDiscoveryEvent::PeerDiscovered { fullname } => {
                    crate::output::emit_event(
                        "bonjour_peer_discovered",
                        json!({ "fullname": fullname }),
                    );
                }
                BonjourDiscoveryEvent::EndpointResolved(peer) => {
                    peer_count += 1;
                    crate::output::emit_event(
                        "bonjour_endpoint_resolved",
                        json!({
                            "fullname": peer.fullname,
                            "endpoint": peer.endpoint.to_string(),
                        }),
                    );
                }
                BonjourDiscoveryEvent::Error(err) => {
                    crate::output::emit_error(&format!("bonjour_discovery_error: {err}"));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    crate::output::emit_success(
        "gossip_discover_complete",
        json!({ "peers_found": peer_count }),
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn run_discover(_timeout_secs: u64) -> Result<(), String> {
    crate::output::emit_success(
        "gossip_discover_complete",
        json!({
            "peers_found": 0,
            "note": "Bonjour/mDNS discovery is only available on Linux",
        }),
    );
    Ok(())
}

fn execute(
    args: &GossipArgs,
    state: &crate::state::CliState,
) -> Result<(String, serde_json::Value), String> {
    match &args.cmd {
        GossipCmd::Status => Ok((
            "gossip_status".to_string(),
            json!({
                "enabled": true,
                "running": false,
                "last_activity_ms": null,
            }),
        )),
        GossipCmd::Announce => Ok((
            "gossip_announce_sent".to_string(),
            json!({
                "status": "sent",
            }),
        )),
        GossipCmd::Discover { .. } => {
            unreachable!("discover is handled directly in run()")
        }
        GossipCmd::StoreStats => {
            let store_path = state.data_dir.join("gossip-object-store.sqlite3");
            let item_count = if store_path.exists() {
                let conn = rusqlite::Connection::open(&store_path)
                    .map_err(|err| format!("open gossip store {}: {err}", store_path.display()))?;
                conn.query_row("SELECT COUNT(*) FROM gossip_items", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|err| format!("count gossip store items: {err}"))?
            } else {
                0
            };

            Ok((
                "gossip_store_stats".to_string(),
                json!({
                    "item_count": item_count,
                    "data_dir": state.data_dir.display().to_string(),
                }),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{execute, GossipArgs, GossipCmd};
    use std::path::PathBuf;

    fn state(data_dir: &str) -> crate::state::CliState {
        crate::state::CliState::from_cli_args(Some(data_dir), None, false)
    }

    #[test]
    fn status_emits_static_jsonl_shape() {
        let state = state("/tmp/aethos-cli-gossip-status");
        let args = GossipArgs {
            cmd: GossipCmd::Status,
        };

        let (event_type, data) = execute(&args, &state).expect("gossip status");

        assert_eq!(event_type, "gossip_status");
        assert_eq!(data["enabled"], true);
        assert_eq!(data["running"], false);
        assert!(data["last_activity_ms"].is_null());
    }

    #[test]
    fn announce_emits_static_sent_event() {
        let state = state("/tmp/aethos-cli-gossip-announce");
        let args = GossipArgs {
            cmd: GossipCmd::Announce,
        };

        let (event_type, data) = execute(&args, &state).expect("gossip announce");

        assert_eq!(event_type, "gossip_announce_sent");
        assert_eq!(data["status"], "sent");
    }

    #[test]
    fn store_stats_returns_zero_for_missing_store() {
        let base_dir = PathBuf::from("/tmp/aethos-cli-gossip-empty");
        let _ = std::fs::remove_dir_all(&base_dir);
        let state = state(base_dir.to_str().expect("path str"));
        let args = GossipArgs {
            cmd: GossipCmd::StoreStats,
        };

        let (event_type, data) = execute(&args, &state).expect("gossip store stats");

        assert_eq!(event_type, "gossip_store_stats");
        assert_eq!(data["item_count"], 0);
        assert_eq!(data["data_dir"], base_dir.display().to_string());
    }

    #[test]
    fn store_stats_counts_existing_items() {
        let base_dir = PathBuf::from("/tmp/aethos-cli-gossip-populated");
        let _ = std::fs::remove_dir_all(&base_dir);
        std::fs::create_dir_all(&base_dir).expect("create base dir");
        let store_path = base_dir.join("gossip-object-store.sqlite3");
        let conn = rusqlite::Connection::open(&store_path).expect("open store");
        conn.execute_batch(
            "
            CREATE TABLE gossip_items (
                item_id TEXT PRIMARY KEY NOT NULL,
                envelope_b64 TEXT NOT NULL,
                expiry_unix_ms INTEGER NOT NULL,
                hop_count INTEGER NOT NULL,
                recorded_at_unix_ms INTEGER NOT NULL
            );
            INSERT INTO gossip_items (item_id, envelope_b64, expiry_unix_ms, hop_count, recorded_at_unix_ms)
            VALUES ('item-1', 'env-1', 10, 1, 1), ('item-2', 'env-2', 20, 2, 2);
            ",
        )
        .expect("seed store");

        let state = state(base_dir.to_str().expect("path str"));
        let args = GossipArgs {
            cmd: GossipCmd::StoreStats,
        };

        let (event_type, data) = execute(&args, &state).expect("gossip store stats");

        assert_eq!(event_type, "gossip_store_stats");
        assert_eq!(data["item_count"], 2);
        assert_eq!(data["data_dir"], base_dir.display().to_string());

        let _ = std::fs::remove_dir_all(&base_dir);
    }
}
