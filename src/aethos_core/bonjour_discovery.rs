use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, SocketAddr};

use flume::Receiver;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

pub const AETHOS_BONJOUR_SERVICE_TYPE: &str = "_aethos._udp.local.";
pub const AETHOS_BONJOUR_DOMAIN: &str = "local.";

const AETHOS_BONJOUR_TXT_VERSION: &str = "1";
const AETHOS_BONJOUR_API: &str = "gossipv1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BonjourResolvedPeer {
    pub fullname: String,
    pub endpoint: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BonjourDiscoveryEvent {
    AdvertisementStarted {
        service_type: String,
        instance_name: String,
        domain: String,
        port: u16,
    },
    PeerDiscovered {
        fullname: String,
    },
    EndpointResolved(BonjourResolvedPeer),
    Error(String),
}

pub struct BonjourLanDiscovery {
    state: BonjourLanDiscoveryState,
}

enum BonjourLanDiscoveryState {
    Disabled {
        pending: VecDeque<BonjourDiscoveryEvent>,
    },
    Active(ActiveBonjourLanDiscovery),
}

struct ActiveBonjourLanDiscovery {
    _daemon: ServiceDaemon,
    browse_rx: Receiver<ServiceEvent>,
    pending: VecDeque<BonjourDiscoveryEvent>,
}

impl BonjourLanDiscovery {
    pub fn new(local_wayfarer_id: &str, port: u16) -> Self {
        if bonjour_is_disabled() {
            return Self {
                state: BonjourLanDiscoveryState::Disabled {
                    pending: VecDeque::new(),
                },
            };
        }

        match ActiveBonjourLanDiscovery::new(local_wayfarer_id, port) {
            Ok(active) => Self {
                state: BonjourLanDiscoveryState::Active(active),
            },
            Err(err) => Self {
                state: BonjourLanDiscoveryState::Disabled {
                    pending: VecDeque::from([BonjourDiscoveryEvent::Error(err)]),
                },
            },
        }
    }

    pub fn poll(&mut self) -> Vec<BonjourDiscoveryEvent> {
        match &mut self.state {
            BonjourLanDiscoveryState::Disabled { pending } => pending.drain(..).collect(),
            BonjourLanDiscoveryState::Active(active) => active.poll(),
        }
    }
}

impl ActiveBonjourLanDiscovery {
    fn new(_local_wayfarer_id: &str, port: u16) -> Result<Self, String> {
        let daemon =
            ServiceDaemon::new().map_err(|err| format!("bonjour daemon init failed: {err}"))?;
        let browse_rx = daemon
            .browse(AETHOS_BONJOUR_SERVICE_TYPE)
            .map_err(|err| format!("bonjour browse failed: {err}"))?;

        let instance_name = build_instance_name();
        let hostname = format!("{}.local.", build_hostname_label());
        let properties = [
            ("txtvers", AETHOS_BONJOUR_TXT_VERSION),
            ("api", AETHOS_BONJOUR_API),
        ];
        let service = ServiceInfo::new(
            AETHOS_BONJOUR_SERVICE_TYPE,
            &instance_name,
            &hostname,
            "",
            port,
            &properties[..],
        )
        .map_err(|err| format!("bonjour service info failed: {err}"))?
        .enable_addr_auto();

        daemon
            .register(service)
            .map_err(|err| format!("bonjour register failed: {err}"))?;

        Ok(Self {
            _daemon: daemon,
            browse_rx,
            pending: VecDeque::from([BonjourDiscoveryEvent::AdvertisementStarted {
                service_type: AETHOS_BONJOUR_SERVICE_TYPE.to_string(),
                instance_name,
                domain: AETHOS_BONJOUR_DOMAIN.to_string(),
                port,
            }]),
        })
    }

    fn poll(&mut self) -> Vec<BonjourDiscoveryEvent> {
        while let Ok(event) = self.browse_rx.try_recv() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    let fullname = service_fullname(&info);
                    self.pending
                        .push_back(BonjourDiscoveryEvent::PeerDiscovered {
                            fullname: fullname.clone(),
                        });

                    if let Some(endpoint) = first_ipv4_endpoint(&info) {
                        self.pending
                            .push_back(BonjourDiscoveryEvent::EndpointResolved(
                                BonjourResolvedPeer { fullname, endpoint },
                            ));
                    }
                }
                ServiceEvent::SearchStopped(service_type) => {
                    self.pending.push_back(BonjourDiscoveryEvent::Error(format!(
                        "bonjour browse stopped unexpectedly: service_type={service_type}"
                    )));
                }
                _ => {}
            }
        }
        self.pending.drain(..).collect()
    }
}

fn bonjour_is_disabled() -> bool {
    std::env::var("AETHOS_DISABLE_BONJOUR")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || std::env::var("AETHOS_DISABLE_MDNS")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
}

fn build_instance_name() -> String {
    let base = read_hostname()
        .map(|value| format!("{value} Aethos"))
        .unwrap_or_else(|| "Aethos Desktop".to_string());
    truncate_utf8(&base, 63)
}

fn build_hostname_label() -> String {
    let fallback = "aethos-desktop".to_string();
    let source = read_hostname().unwrap_or(fallback);
    let sanitized = source
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        "aethos-desktop".to_string()
    } else {
        truncate_utf8(&sanitized, 63)
    }
}

fn read_hostname() -> Option<String> {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| fs::read_to_string("/etc/hostname").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if out.len() + ch.len_utf8() > max_bytes {
            break;
        }
        out.push(ch);
    }
    out
}

fn service_fullname(info: &ResolvedService) -> String {
    info.get_fullname().to_string()
}

fn first_ipv4_endpoint(info: &ResolvedService) -> Option<SocketAddr> {
    info.get_addresses_v4()
        .into_iter()
        .next()
        .map(|ipv4| SocketAddr::new(IpAddr::V4(ipv4), info.get_port()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonjour_hostname_label_is_dns_safe() {
        std::env::set_var("HOSTNAME", "Desk Top_01.local");
        let label = build_hostname_label();
        std::env::remove_var("HOSTNAME");
        assert_eq!(label, "desk-top-01-local");
    }

    #[test]
    fn bonjour_instance_name_is_capped_to_txt_safe_length() {
        let input = "a".repeat(90);
        let output = truncate_utf8(&input, 63);
        assert_eq!(output.len(), 63);
    }

    #[test]
    fn bonjour_txt_metadata_omits_peer_identity() {
        let properties = [("txtvers", "1"), ("api", "gossipv1")];
        let info = ServiceInfo::new(
            AETHOS_BONJOUR_SERVICE_TYPE,
            "Aethos Desktop",
            "desktop.local.",
            "127.0.0.1",
            47655,
            &properties[..],
        )
        .expect("service info");
        assert!(info.get_property_val_str("peer").is_none());
        assert_eq!(info.get_property_val_str("txtvers"), Some("1"));
        assert_eq!(info.get_property_val_str("api"), Some("gossipv1"));
    }
}
