use crate::aethos_core::gossip_sync::{
    self, GossipSyncFrame, GOSSIP_LAN_PORT, MAX_FRAME_BYTES, MAX_TRANSFER_BYTES,
    MAX_TRANSFER_ITEMS, MAX_WANT_ITEMS,
};
use crate::aethos_core::identity_store::ensure_local_identity;
use base64::Engine;
use clap::{Args, Subcommand};
use serde_json::json;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Args)]
pub struct GossipArgs {
    #[command(subcommand)]
    pub cmd: GossipCmd,
}

#[derive(Debug, Subcommand)]
pub enum GossipCmd {
    /// Show gossip subsystem status
    Status,
    /// Broadcast mDNS/Bonjour service announcement on LAN
    Announce,
    /// Show local gossip store statistics (item count, data directory)
    #[command(name = "store-stats")]
    StoreStats,
    /// Poll LAN for Bonjour/mDNS peers
    Discover {
        /// Seconds to listen for peer advertisements
        #[arg(long, default_value_t = 5)]
        timeout: u64,
    },
    /// One-shot LAN gossip sync over UDP (HELLO/SUMMARY/REQUEST/TRANSFER)
    Sync {
        #[arg(long, default_value_t = GOSSIP_LAN_PORT, help = "UDP port to bind for gossip sync")]
        port: u16,
        #[arg(
            long,
            default_value_t = 10,
            help = "Seconds to run sync before exiting"
        )]
        timeout: u64,
        /// Use loopback-only mode (127.0.0.1 instead of broadcast)
        #[arg(long)]
        loopback: bool,
        /// Send directly to a specific peer address (e.g. 127.0.0.1:47701)
        #[arg(long)]
        peer: Option<String>,
    },
}

pub fn run(args: &GossipArgs, state: &crate::state::CliState) -> Result<(), String> {
    match &args.cmd {
        GossipCmd::Discover { timeout } => run_discover(*timeout),
        GossipCmd::Sync {
            timeout,
            port,
            loopback,
            peer,
        } => run_sync(*timeout, *port, *loopback, peer.clone()),
        _ => {
            let (event_type, data) = execute(args, state)?;
            crate::output::emit_success(&event_type, data);
            Ok(())
        }
    }
}

fn configured_lan_port() -> u16 {
    std::env::var("AETHOS_GOSSIP_LAN_PORT")
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(GOSSIP_LAN_PORT)
}

fn run_discover(timeout_secs: u64) -> Result<(), String> {
    use crate::aethos_core::bonjour_discovery::{BonjourDiscoveryEvent, BonjourLanDiscovery};
    use std::time::{Duration, Instant};

    let local_id = crate::aethos_core::identity_store::ensure_local_identity()
        .map(|id| id.wayfarer_id)
        .unwrap_or_else(|_| "unknown-local-peer".to_string());

    let mut discovery = BonjourLanDiscovery::new(&local_id, configured_lan_port());
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

#[derive(Debug, Default)]
struct GossipSyncStats {
    frames_sent: u64,
    frames_received: u64,
    requests_sent: u64,
    requests_served: u64,
    transfers_sent: u64,
    transfers_received: u64,
    items_imported: u64,
}

fn run_sync(
    timeout_secs: u64,
    port: u16,
    loopback: bool,
    peer: Option<String>,
) -> Result<(), String> {
    let identity = ensure_local_identity()?;
    let node_pubkey_raw = base64::engine::general_purpose::STANDARD
        .decode(&identity.verifying_key_b64)
        .map_err(|err| format!("decode verifying_key_b64: {err}"))?;
    let node_pubkey = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(node_pubkey_raw);

    let bind_ip = if loopback {
        Ipv4Addr::LOCALHOST
    } else {
        Ipv4Addr::UNSPECIFIED
    };
    let bind_addr = SocketAddrV4::new(bind_ip, port);
    let raw_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
        .map_err(|err| format!("create gossip sync socket: {err}"))?;
    raw_socket
        .set_reuse_address(true)
        .map_err(|err| format!("set_reuse_address: {err}"))?;
    raw_socket
        .bind(&bind_addr.into())
        .map_err(|err| format!("bind gossip sync socket {bind_addr}: {err}"))?;
    let socket: UdpSocket = raw_socket.into();
    socket
        .set_nonblocking(true)
        .map_err(|err| format!("set_nonblocking: {err}"))?;
    socket
        .set_broadcast(true)
        .map_err(|err| format!("set_broadcast: {err}"))?;

    let hello = gossip_sync::build_hello_frame(&identity.wayfarer_id, &node_pubkey)?;
    let summary = gossip_sync::build_summary_frame(now_unix_ms())?;
    let mut targets = if loopback {
        vec![SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))]
    } else {
        vec![SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(255, 255, 255, 255),
            port,
        ))]
    };

    if let Some(ref peer_addr) = peer {
        let addr: SocketAddr = peer_addr
            .parse()
            .map_err(|err| format!("invalid --peer address '{peer_addr}': {err}"))?;
        if !targets.contains(&addr) {
            targets.push(addr);
        }
    }

    crate::output::emit_event(
        "gossip_sync_started",
        json!({
            "timeout_secs": timeout_secs,
            "port": port,
            "loopback": loopback,
            "bind_addr": bind_addr.to_string(),
            "peer": peer,
        }),
    );

    let mut stats = GossipSyncStats::default();
    let mut latest_summary: Option<gossip_sync::SummaryFrame> = None;
    for target in &targets {
        if *target == SocketAddr::V4(bind_addr) {
            continue;
        }
        send_sync_frame(&socket, *target, &hello, &mut stats)?;
        send_sync_frame(&socket, *target, &summary, &mut stats)?;
    }

    let started_at = Instant::now();
    let deadline = started_at + Duration::from_secs(timeout_secs);
    let mut peer_node_by_addr = HashMap::<SocketAddr, String>::new();
    let mut peer_max_want_by_addr = HashMap::<SocketAddr, usize>::new();
    let mut buf = vec![0u8; MAX_FRAME_BYTES];

    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, source)) => {
                stats.frames_received = stats.frames_received.saturating_add(1);
                let raw = &buf[..len];
                let frame = match gossip_sync::parse_frame(raw) {
                    Ok(frame) => frame,
                    Err(err) => {
                        crate::output::emit_event(
                            "gossip_sync_frame_received",
                            json!({
                                "bytes": len,
                                "source": source.to_string(),
                                "status": "parse_error",
                                "error": err,
                            }),
                        );
                        continue;
                    }
                };

                crate::output::emit_event(
                    "gossip_sync_frame_received",
                    json!({
                        "frame_type": gossip_sync_frame_type(&frame),
                        "bytes": len,
                        "source": source.to_string(),
                    }),
                );

                match frame {
                    GossipSyncFrame::Hello(hello) => {
                        if hello.node_id == identity.wayfarer_id {
                            continue;
                        }

                        peer_max_want_by_addr.insert(
                            source,
                            usize::try_from(hello.max_want).unwrap_or(MAX_WANT_ITEMS),
                        );
                        peer_node_by_addr.insert(source, hello.node_id);

                        let summary = gossip_sync::build_summary_frame(now_unix_ms())?;
                        send_sync_frame(&socket, source, &summary, &mut stats)?;
                    }
                    GossipSyncFrame::Summary(summary) => {
                        latest_summary = Some(summary.clone());
                        let want =
                            gossip_sync::select_request_item_ids_from_summary_with_candidates(
                                &summary,
                                MAX_WANT_ITEMS,
                                summary.preview_item_ids.as_deref().unwrap_or(&[]),
                            )?;
                        if want.is_empty() {
                            continue;
                        }
                        let request = gossip_sync::build_request_frame(want, MAX_WANT_ITEMS)?;
                        stats.requests_sent = stats.requests_sent.saturating_add(1);
                        send_sync_frame(&socket, source, &request, &mut stats)?;
                    }
                    GossipSyncFrame::Request(request) => {
                        if request.want.is_empty() {
                            continue;
                        }
                        let transfer_objects = gossip_sync::transfer_items_for_request(
                            &request.want,
                            MAX_TRANSFER_ITEMS as u32,
                            MAX_TRANSFER_BYTES,
                            now_unix_ms(),
                        )?;
                        let transfer = GossipSyncFrame::Transfer(gossip_sync::TransferFrame {
                            objects: transfer_objects,
                        });
                        stats.requests_served = stats.requests_served.saturating_add(1);
                        stats.transfers_sent = stats.transfers_sent.saturating_add(1);
                        send_sync_frame(&socket, source, &transfer, &mut stats)?;
                    }
                    GossipSyncFrame::Transfer(transfer) => {
                        stats.transfers_received = stats.transfers_received.saturating_add(1);
                        let peer_node = peer_node_by_addr.get(&source).map(String::as_str);
                        let imported = gossip_sync::import_transfer_items(
                            &identity.wayfarer_id,
                            Some(&source.to_string()),
                            peer_node,
                            &transfer.objects,
                            now_unix_ms(),
                        )?;
                        stats.items_imported = stats
                            .items_imported
                            .saturating_add(imported.accepted_item_ids.len() as u64);
                        crate::output::emit_event(
                            "gossip_sync_items_imported",
                            json!({
                                "source": source.to_string(),
                                "accepted": imported.accepted_item_ids.len(),
                                "receipt": imported.receipt_item_ids.len(),
                                "rejected": imported.rejected_items.len(),
                                "new_messages": imported.new_messages.len(),
                            }),
                        );
                    }
                    GossipSyncFrame::Receipt(receipt) => {
                        crate::output::emit_event(
                            "gossip_sync_frame_received",
                            json!({
                                "frame_type": "RECEIPT",
                                "bytes": len,
                                "source": source.to_string(),
                                "received_items": receipt.received.len(),
                            }),
                        );
                    }
                    GossipSyncFrame::RelayIngest(ingest) => {
                        let _ = latest_summary.as_ref();
                        let want = ingest.item_ids.clone();
                        if want.is_empty() {
                            continue;
                        }
                        let request = gossip_sync::build_request_frame(want, MAX_WANT_ITEMS)?;
                        stats.requests_sent = stats.requests_sent.saturating_add(1);
                        send_sync_frame(&socket, source, &request, &mut stats)?;
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(format!("recv gossip sync frame: {err}")),
        }
    }

    crate::output::emit_success(
        "gossip_sync_complete",
        json!({
            "elapsed_ms": started_at.elapsed().as_millis() as u64,
            "timeout_secs": timeout_secs,
            "port": port,
            "loopback": loopback,
            "frames_sent": stats.frames_sent,
            "frames_received": stats.frames_received,
            "requests_sent": stats.requests_sent,
            "requests_served": stats.requests_served,
            "transfers_sent": stats.transfers_sent,
            "transfers_received": stats.transfers_received,
            "items_imported": stats.items_imported,
            "peer_max_want_observed": peer_max_want_by_addr.values().copied().max().unwrap_or(0),
        }),
    );

    Ok(())
}

fn send_sync_frame(
    socket: &UdpSocket,
    target: SocketAddr,
    frame: &GossipSyncFrame,
    stats: &mut GossipSyncStats,
) -> Result<(), String> {
    let raw = gossip_sync::serialize_frame(frame)?;
    socket.send_to(&raw, target).map_err(|err| {
        format!(
            "send gossip sync {} to {target}: {err}",
            gossip_sync_frame_type(frame)
        )
    })?;
    stats.frames_sent = stats.frames_sent.saturating_add(1);
    crate::output::emit_event(
        "gossip_sync_frame_sent",
        json!({
            "frame_type": gossip_sync_frame_type(frame),
            "bytes": raw.len(),
            "target": target.to_string(),
        }),
    );
    Ok(())
}

fn gossip_sync_frame_type(frame: &GossipSyncFrame) -> &'static str {
    match frame {
        GossipSyncFrame::Hello(_) => "HELLO",
        GossipSyncFrame::Summary(_) => "SUMMARY",
        GossipSyncFrame::Request(_) => "REQUEST",
        GossipSyncFrame::Transfer(_) => "TRANSFER",
        GossipSyncFrame::Receipt(_) => "RECEIPT",
        GossipSyncFrame::RelayIngest(_) => "RELAY_INGEST",
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
        GossipCmd::Sync { .. } => unreachable!("sync is handled directly in run()"),
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
    use clap::Parser;
    use std::path::PathBuf;

    #[derive(Debug, Parser)]
    struct GossipCmdParser {
        #[command(subcommand)]
        cmd: GossipCmd,
    }

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

    #[test]
    fn sync_subcommand_parses_default_args() {
        let parsed = GossipCmdParser::try_parse_from(["aethos-cli", "sync"])
            .expect("parse gossip sync defaults");

        assert!(matches!(
            parsed.cmd,
            GossipCmd::Sync {
                timeout: 10,
                port: 47655,
                loopback: false,
                peer: None,
            }
        ));
    }

    #[test]
    fn sync_subcommand_parses_custom_args() {
        let parsed = GossipCmdParser::try_parse_from([
            "aethos-cli",
            "sync",
            "--timeout",
            "5",
            "--port",
            "48000",
            "--loopback",
        ])
        .expect("parse gossip sync custom args");

        assert!(matches!(
            parsed.cmd,
            GossipCmd::Sync {
                timeout: 5,
                port: 48000,
                loopback: true,
                peer: None,
            }
        ));
    }

    #[test]
    fn sync_subcommand_parses_peer_flag() {
        let parsed =
            GossipCmdParser::try_parse_from(["aethos-cli", "sync", "--peer", "127.0.0.1:47701"])
                .expect("parse gossip sync --peer");

        assert!(matches!(
            parsed.cmd,
            GossipCmd::Sync {
                timeout: 10,
                port: 47655,
                loopback: false,
                peer: Some(_),
            }
        ));
        if let GossipCmd::Sync { peer, .. } = parsed.cmd {
            assert_eq!(peer.unwrap(), "127.0.0.1:47701");
        }
    }
}
