use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryBearer {
    Bonjour,
    IPv4Broadcast,
    Multicast,
}

#[derive(Debug, Clone)]
pub struct DiscoveryCandidate {
    pub candidate_id: String,
    pub peer_endpoint: SocketAddr,
    pub peer_identity: Option<String>,
    pub discovered_via: DiscoveryBearer,
    pub observed_at_unix_ms: u64,
    pub signal_quality_hint: Option<f32>,
}

pub trait DiscoveryBearerSource {
    fn poll_candidates(&mut self) -> Vec<DiscoveryCandidate>;
}

pub struct DiscoveryCandidatePipeline {
    bearers: Vec<Box<dyn DiscoveryBearerSource>>,
    local_endpoints: Vec<IpAddr>,
    dedup_window_ms: u64,
    last_seen: HashMap<String, u64>,
    disabled: bool,
}

impl DiscoveryCandidatePipeline {
    pub fn new(bearers: Vec<Box<dyn DiscoveryBearerSource>>, local_endpoints: Vec<IpAddr>) -> Self {
        Self::new_inner(bearers, local_endpoints, lan_discovery_is_disabled())
    }

    fn new_inner(
        bearers: Vec<Box<dyn DiscoveryBearerSource>>,
        local_endpoints: Vec<IpAddr>,
        disabled: bool,
    ) -> Self {
        Self {
            bearers,
            local_endpoints,
            dedup_window_ms: 500,
            last_seen: HashMap::new(),
            disabled,
        }
    }

    pub fn poll(&mut self) -> Vec<DiscoveryCandidate> {
        if self.disabled {
            return Vec::new();
        }

        let now_ms = unix_now_ms();
        let mut results: Vec<DiscoveryCandidate> = Vec::new();
        let mut seen_this_poll: HashMap<String, bool> = HashMap::new();

        for bearer in &mut self.bearers {
            let candidates = bearer.poll_candidates();
            for candidate in candidates {
                if self.local_endpoints.contains(&candidate.peer_endpoint.ip()) {
                    continue;
                }

                let id = &candidate.candidate_id;

                if seen_this_poll.contains_key(id) {
                    continue;
                }

                let last = self.last_seen.get(id).copied().unwrap_or(u64::MAX);
                if last != u64::MAX && now_ms.saturating_sub(last) < self.dedup_window_ms {
                    continue;
                }

                seen_this_poll.insert(id.clone(), true);
                self.last_seen.insert(id.clone(), now_ms);
                results.push(candidate);
            }
        }

        results
    }
}

fn lan_discovery_is_disabled() -> bool {
    std::env::var("AETHOS_DISABLE_LAN_DISCOVERY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubBearer {
        candidates: Vec<DiscoveryCandidate>,
    }

    impl StubBearer {
        fn new(candidates: Vec<DiscoveryCandidate>) -> Self {
            Self { candidates }
        }
    }

    impl DiscoveryBearerSource for StubBearer {
        fn poll_candidates(&mut self) -> Vec<DiscoveryCandidate> {
            self.candidates.clone()
        }
    }

    fn make_candidate(ip: &str, port: u16, bearer: DiscoveryBearer) -> DiscoveryCandidate {
        let peer_endpoint: SocketAddr = format!("{ip}:{port}").parse().unwrap();
        DiscoveryCandidate {
            candidate_id: format!("{ip}:{port}"),
            peer_endpoint,
            peer_identity: None,
            discovered_via: bearer,
            observed_at_unix_ms: unix_now_ms(),
            signal_quality_hint: None,
        }
    }

    #[test]
    fn multi_bearer_dedup_yields_one_candidate() {
        let c1 = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Bonjour);
        let c2 = make_candidate("192.168.1.20", 47655, DiscoveryBearer::IPv4Broadcast);

        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![
            Box::new(StubBearer::new(vec![c1])),
            Box::new(StubBearer::new(vec![c2])),
        ];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn self_filter_removes_own_endpoint() {
        let own_ip: IpAddr = "192.168.1.10".parse().unwrap();
        let c = make_candidate("192.168.1.10", 47655, DiscoveryBearer::IPv4Broadcast);

        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![own_ip]);
        let results = pipeline.poll();
        assert!(results.is_empty());
    }

    #[test]
    fn disabled_flag_yields_no_candidates() {
        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Bonjour);
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];
        let mut pipeline = DiscoveryCandidatePipeline::new_inner(bearers, vec![], true);
        let results = pipeline.poll();
        assert!(results.is_empty());
    }

    #[test]
    fn bearer_failure_isolation_other_bearers_still_yield() {
        struct FailingBearer;
        impl DiscoveryBearerSource for FailingBearer {
            fn poll_candidates(&mut self) -> Vec<DiscoveryCandidate> {
                vec![]
            }
        }

        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Multicast);
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> =
            vec![Box::new(FailingBearer), Box::new(StubBearer::new(vec![c]))];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn two_distinct_peers_yield_two_candidates() {
        let c1 = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Bonjour);
        let c2 = make_candidate("192.168.1.21", 47655, DiscoveryBearer::Bonjour);

        let bearers: Vec<Box<dyn DiscoveryBearerSource>> =
            vec![Box::new(StubBearer::new(vec![c1, c2]))];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 2);
    }
}
