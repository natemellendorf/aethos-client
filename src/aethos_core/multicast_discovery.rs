use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use socket2::{Domain, Protocol, Socket, Type};

pub const AETHOS_MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);

pub const AETHOS_MULTICAST_BEACON: [u8; 4] = [0xAE, 0x74, 0x48, 0x53];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MulticastResolvedPeer {
    pub peer_addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MulticastDiscoveryEvent {
    ListenerStarted { group: Ipv4Addr, port: u16 },
    PeerDiscovered { peer_addr: SocketAddr },
    EndpointResolved(MulticastResolvedPeer),
    Error(String),
}

pub struct MulticastDiscovery {
    state: MulticastDiscoveryState,
}

enum MulticastDiscoveryState {
    Disabled {
        pending: VecDeque<MulticastDiscoveryEvent>,
    },
    Active(ActiveMulticastDiscovery),
}

struct ActiveMulticastDiscovery {
    socket: UdpSocket,
    multicast_addr: SocketAddr,
    local_addrs: Vec<IpAddr>,
    pending: VecDeque<MulticastDiscoveryEvent>,
    poll_count: u32,
}

impl MulticastDiscovery {
    pub fn new(port: u16, local_addrs: Vec<IpAddr>) -> Self {
        if multicast_is_disabled() {
            return Self {
                state: MulticastDiscoveryState::Disabled {
                    pending: VecDeque::new(),
                },
            };
        }

        match ActiveMulticastDiscovery::new(port, local_addrs) {
            Ok(active) => Self {
                state: MulticastDiscoveryState::Active(active),
            },
            Err(err) => Self {
                state: MulticastDiscoveryState::Disabled {
                    pending: VecDeque::from([MulticastDiscoveryEvent::Error(err)]),
                },
            },
        }
    }

    pub fn poll(&mut self) -> Vec<MulticastDiscoveryEvent> {
        match &mut self.state {
            MulticastDiscoveryState::Disabled { pending } => pending.drain(..).collect(),
            MulticastDiscoveryState::Active(active) => active.poll(),
        }
    }
}

impl ActiveMulticastDiscovery {
    fn new(port: u16, local_addrs: Vec<IpAddr>) -> Result<Self, String> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
            .map_err(|e| format!("multicast: socket create failed: {e}"))?;
        socket
            .set_reuse_address(true)
            .map_err(|e| format!("multicast: set_reuse_address failed: {e}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|e| format!("multicast: set_nonblocking failed: {e}"))?;
        let bind_addr: SocketAddr = format!("0.0.0.0:{port}")
            .parse()
            .expect("multicast: bind addr parse is infallible");
        socket
            .bind(&bind_addr.into())
            .map_err(|e| format!("multicast: bind failed: {e}"))?;

        let std_socket: UdpSocket = socket.into();

        let mut join_errors: Vec<String> = Vec::new();
        let mut joined = false;

        let ipv4_locals: Vec<Ipv4Addr> = local_addrs
            .iter()
            .filter_map(|a| match a {
                IpAddr::V4(v4) if !v4.is_loopback() => Some(*v4),
                _ => None,
            })
            .collect();

        if ipv4_locals.is_empty() {
            match std_socket.join_multicast_v4(&AETHOS_MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED) {
                Ok(()) => joined = true,
                Err(e) => join_errors.push(format!("multicast: join on UNSPECIFIED failed: {e}")),
            }
        } else {
            for iface in &ipv4_locals {
                match std_socket.join_multicast_v4(&AETHOS_MULTICAST_GROUP, iface) {
                    Ok(()) => joined = true,
                    Err(e) => {
                        join_errors.push(format!("multicast: join on {iface} failed: {e}"));
                    }
                }
            }
        }

        if !joined {
            let msg = join_errors.join("; ");
            return Err(format!(
                "multicast: could not join group on any interface: {msg}"
            ));
        }

        let multicast_addr: SocketAddr = format!("{}:{port}", AETHOS_MULTICAST_GROUP)
            .parse()
            .expect("multicast: multicast addr parse is infallible");

        let mut pending = VecDeque::new();
        for err_msg in join_errors {
            pending.push_back(MulticastDiscoveryEvent::Error(err_msg));
        }
        pending.push_back(MulticastDiscoveryEvent::ListenerStarted {
            group: AETHOS_MULTICAST_GROUP,
            port,
        });

        Ok(Self {
            socket: std_socket,
            multicast_addr,
            local_addrs,
            pending,
            poll_count: 0,
        })
    }

    fn poll(&mut self) -> Vec<MulticastDiscoveryEvent> {
        self.poll_count = self.poll_count.wrapping_add(1);

        if self.poll_count.is_multiple_of(5) {
            if let Err(err) = self
                .socket
                .send_to(&AETHOS_MULTICAST_BEACON, self.multicast_addr)
            {
                eprintln!("multicast: beacon send failed: {err}");
            }
        }

        let mut buf = [0u8; 1500];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, src)) => {
                    if self.local_addrs.contains(&src.ip()) {
                        continue;
                    }
                    if n >= 4 && buf[..4] == AETHOS_MULTICAST_BEACON {
                        self.pending
                            .push_back(MulticastDiscoveryEvent::PeerDiscovered { peer_addr: src });
                        self.pending
                            .push_back(MulticastDiscoveryEvent::EndpointResolved(
                                MulticastResolvedPeer { peer_addr: src },
                            ));
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    self.pending
                        .push_back(MulticastDiscoveryEvent::Error(format!(
                            "multicast: recv_from failed: {e}"
                        )));
                    break;
                }
            }
        }

        self.pending.drain(..).collect()
    }
}

fn multicast_is_disabled() -> bool {
    std::env::var("AETHOS_DISABLE_MULTICAST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_flag_prevents_active_state() {
        unsafe { std::env::set_var("AETHOS_DISABLE_MULTICAST", "1") };
        let mut bearer = MulticastDiscovery::new(47655, vec![]);
        let events = bearer.poll();
        unsafe { std::env::remove_var("AETHOS_DISABLE_MULTICAST") };
        assert!(events.is_empty());
        assert!(matches!(
            bearer.state,
            MulticastDiscoveryState::Disabled { .. }
        ));
    }

    #[test]
    fn disabled_flag_true_string() {
        unsafe { std::env::set_var("AETHOS_DISABLE_MULTICAST", "true") };
        let result = multicast_is_disabled();
        unsafe { std::env::remove_var("AETHOS_DISABLE_MULTICAST") };
        assert!(result);
    }

    #[test]
    fn multicast_group_address_is_correct() {
        assert_eq!(AETHOS_MULTICAST_GROUP, Ipv4Addr::new(224, 0, 0, 251));
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
    fn partial_interface_failure_is_non_fatal() {
        let local_addrs = [
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        ];
        let ipv4_locals: Vec<Ipv4Addr> = local_addrs
            .iter()
            .filter_map(|a| match a {
                IpAddr::V4(v4) if !v4.is_loopback() => Some(*v4),
                _ => None,
            })
            .collect();
        assert_eq!(ipv4_locals.len(), 2);
    }
}
