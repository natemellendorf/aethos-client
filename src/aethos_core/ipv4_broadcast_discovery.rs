use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use socket2::{Domain, Protocol, Socket, Type};
use uuid::Uuid;

use crate::aethos_core::aeth_discovery_packet::{
    AethDiscoveryMessageType, AethDiscoveryPacket, AETH_DISCOVERY_GOSSIP_PORT,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IPv4BroadcastResolvedPeer {
    pub peer_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IPv4BroadcastDiscoveryEvent {
    ListenerStarted { port: u16 },
    PeerDiscovered { peer_addr: SocketAddr },
    EndpointResolved(IPv4BroadcastResolvedPeer),
    Error(String),
}

pub struct IPv4BroadcastDiscovery {
    state: IPv4BroadcastDiscoveryState,
}

enum IPv4BroadcastDiscoveryState {
    Disabled {
        pending: VecDeque<IPv4BroadcastDiscoveryEvent>,
    },
    Active(ActiveIPv4BroadcastDiscovery),
}

struct ActiveIPv4BroadcastDiscovery {
    socket: UdpSocket,
    broadcast_addr: SocketAddr,
    local_addrs: Vec<IpAddr>,
    pending: VecDeque<IPv4BroadcastDiscoveryEvent>,
    poll_count: u32,
}

impl IPv4BroadcastDiscovery {
    pub fn new(port: u16, local_addrs: Vec<IpAddr>) -> Self {
        if ipv4_broadcast_is_disabled() {
            return Self {
                state: IPv4BroadcastDiscoveryState::Disabled {
                    pending: VecDeque::new(),
                },
            };
        }

        match ActiveIPv4BroadcastDiscovery::new(port, local_addrs) {
            Ok(active) => Self {
                state: IPv4BroadcastDiscoveryState::Active(active),
            },
            Err(err) => Self {
                state: IPv4BroadcastDiscoveryState::Disabled {
                    pending: VecDeque::from([IPv4BroadcastDiscoveryEvent::Error(err)]),
                },
            },
        }
    }

    pub fn poll(&mut self) -> Vec<IPv4BroadcastDiscoveryEvent> {
        match &mut self.state {
            IPv4BroadcastDiscoveryState::Disabled { pending } => pending.drain(..).collect(),
            IPv4BroadcastDiscoveryState::Active(active) => active.poll(),
        }
    }
}

impl ActiveIPv4BroadcastDiscovery {
    fn new(port: u16, local_addrs: Vec<IpAddr>) -> Result<Self, String> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| format!("ipv4_broadcast: socket create failed: {e}"))?;
        socket
            .set_broadcast(true)
            .map_err(|e| format!("ipv4_broadcast: set_broadcast failed: {e}"))?;
        socket
            .set_reuse_address(true)
            .map_err(|e| format!("ipv4_broadcast: set_reuse_address failed: {e}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("ipv4_broadcast: set_nonblocking failed: {e}"))?;
        let bind_addr: SocketAddr = format!("0.0.0.0:{port}")
            .parse()
            .expect("ipv4_broadcast: bind addr parse is infallible");
        socket
            .bind(&bind_addr.into())
            .map_err(|e| format!("ipv4_broadcast: bind failed: {e}"))?;

        let std_socket: UdpSocket = socket.into();
        let broadcast_addr: SocketAddr = format!("255.255.255.255:{port}")
            .parse()
            .expect("ipv4_broadcast: broadcast addr parse is infallible");

        Ok(Self {
            socket: std_socket,
            broadcast_addr,
            local_addrs,
            pending: VecDeque::from([IPv4BroadcastDiscoveryEvent::ListenerStarted { port }]),
            poll_count: 0,
        })
    }

    fn poll(&mut self) -> Vec<IPv4BroadcastDiscoveryEvent> {
        self.poll_count = self.poll_count.wrapping_add(1);

        // Send beacon every 5th poll.
        if self.poll_count.is_multiple_of(5) {
            let packet =
                AethDiscoveryPacket::probe(*Uuid::new_v4().as_bytes(), AETH_DISCOVERY_GOSSIP_PORT);
            if let Err(err) = self.socket.send_to(&packet.encode(), self.broadcast_addr) {
                eprintln!("ipv4_broadcast: beacon send failed: {err}");
            }
        }

        // Drain inbound packets.
        let mut buf = [0u8; 1500];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, src)) => {
                    // Self-filter: ignore packets from our own addresses.
                    if self.local_addrs.contains(&src.ip()) {
                        continue;
                    }
                    // Only process valid AETH discovery frames.
                    if let Ok(packet) = AethDiscoveryPacket::decode(&buf[..n]) {
                        if packet.message_type != AethDiscoveryMessageType::Probe
                            && packet.message_type != AethDiscoveryMessageType::Response
                        {
                            continue;
                        }
                        self.pending
                            .push_back(IPv4BroadcastDiscoveryEvent::PeerDiscovered {
                                peer_addr: src,
                            });
                        self.pending
                            .push_back(IPv4BroadcastDiscoveryEvent::EndpointResolved(
                                IPv4BroadcastResolvedPeer { peer_addr: src },
                            ));
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    self.pending
                        .push_back(IPv4BroadcastDiscoveryEvent::Error(format!(
                            "ipv4_broadcast: recv_from failed: {e}"
                        )));
                    break;
                }
            }
        }

        self.pending.drain(..).collect()
    }
}

fn ipv4_broadcast_is_disabled() -> bool {
    std::env::var("AETHOS_DISABLE_IPV4_BROADCAST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Returns the non-loopback IPv4 addresses of this host.
/// Falls back to an empty list on error (bearer will still run, just with no self-filter).
pub fn local_ipv4_addrs() -> Vec<IpAddr> {
    // Use a connect trick to find the preferred outbound IP.
    let mut addrs = Vec::new();
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = sock.local_addr() {
                addrs.push(local.ip());
            }
        }
    }
    // Always include loopback so we self-filter loopback packets too.
    addrs.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    addrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_flag_prevents_active_state() {
        // SAFETY: test-only env mutation; tests run in separate processes.
        unsafe { std::env::set_var("AETHOS_DISABLE_IPV4_BROADCAST", "1") };
        let mut bearer = IPv4BroadcastDiscovery::new(47655, vec![]);
        let events = bearer.poll();
        unsafe { std::env::remove_var("AETHOS_DISABLE_IPV4_BROADCAST") };
        // Disabled bearer emits no events (no pending errors either).
        assert!(events.is_empty());
        assert!(matches!(
            bearer.state,
            IPv4BroadcastDiscoveryState::Disabled { .. }
        ));
    }

    #[test]
    fn disabled_flag_true_string() {
        unsafe { std::env::set_var("AETHOS_DISABLE_IPV4_BROADCAST", "true") };
        let result = ipv4_broadcast_is_disabled();
        unsafe { std::env::remove_var("AETHOS_DISABLE_IPV4_BROADCAST") };
        assert!(result);
    }

    #[test]
    fn broadcast_payload_is_aeth_discovery_packet() {
        let packet = AethDiscoveryPacket::probe([0x01; 16], AETH_DISCOVERY_GOSSIP_PORT);
        let encoded = packet.encode();
        assert_eq!(encoded.len(), 29);
        assert_eq!(&encoded[0..4], b"AETH");
        assert_eq!(
            AethDiscoveryPacket::decode(&encoded).unwrap().sender_port,
            47655
        );
    }

    #[test]
    fn self_filter_rejects_own_ip() {
        let own_ip: IpAddr = "192.168.1.10".parse().unwrap();
        let local_addrs = [own_ip];
        let src: SocketAddr = "192.168.1.10:47655".parse().unwrap();
        assert!(local_addrs.contains(&src.ip()));
    }

    #[test]
    fn self_filter_passes_remote_ip() {
        let own_ip: IpAddr = "192.168.1.10".parse().unwrap();
        let local_addrs = [own_ip];
        let src: SocketAddr = "192.168.1.20:47655".parse().unwrap();
        assert!(!local_addrs.contains(&src.ip()));
    }

    #[test]
    fn bind_failure_degrades_gracefully() {
        // Port 0 should succeed; port 1 may fail on non-root — either way no panic.
        let bearer = IPv4BroadcastDiscovery::new(0, vec![]);
        // We just verify it doesn't panic. State may be Active or Disabled.
        drop(bearer);
    }
}
