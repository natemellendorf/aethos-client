use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

const ROUTE_STALE_AFTER_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryBearer {
    Bonjour,
    IPv4Broadcast,
    Multicast,
}

impl DiscoveryBearer {
    pub fn priority(self) -> u8 {
        match self {
            Self::Multicast => 0,
            Self::IPv4Broadcast => 1,
            Self::Bonjour => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryCandidate {
    pub candidate_id: String,
    pub peer_endpoint: SocketAddr,
    pub peer_identity: Option<String>,
    pub discovered_via: DiscoveryBearer,
    pub observed_at_unix_ms: u64,
    pub first_observed_at_unix_ms: u64,
    pub last_observed_at_unix_ms: u64,
    pub route_priority: u8,
    pub route_stale: bool,
    pub signal_quality_hint: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
struct RouteObservation {
    first_seen_at_unix_ms: u64,
    last_seen_at_unix_ms: u64,
}

pub trait DiscoveryBearerSource {
    fn poll_candidates(&mut self) -> Vec<DiscoveryCandidate>;
}

pub struct DiscoveryCandidatePipeline {
    bearers: Vec<Box<dyn DiscoveryBearerSource>>,
    local_endpoints: Vec<IpAddr>,
    dedup_window_ms: u64,
    observations: HashMap<String, RouteObservation>,
    disabled: bool,
}

impl DiscoveryCandidatePipeline {
    pub fn new(bearers: Vec<Box<dyn DiscoveryBearerSource>>, local_endpoints: Vec<IpAddr>) -> Self {
        Self::new_inner(bearers, local_endpoints, lan_discovery_is_disabled())
    }

    pub fn new_inner(
        bearers: Vec<Box<dyn DiscoveryBearerSource>>,
        local_endpoints: Vec<IpAddr>,
        disabled: bool,
    ) -> Self {
        Self {
            bearers,
            local_endpoints,
            dedup_window_ms: 500,
            observations: HashMap::new(),
            disabled,
        }
    }

    pub fn poll(&mut self) -> Vec<DiscoveryCandidate> {
        if self.disabled {
            return Vec::new();
        }

        let now_ms = unix_now_ms();
        let mut selected_this_poll: HashMap<String, DiscoveryCandidate> = HashMap::new();

        for bearer in &mut self.bearers {
            let candidates = bearer.poll_candidates();
            for candidate in candidates {
                if self.local_endpoints.contains(&candidate.peer_endpoint.ip()) {
                    continue;
                }

                let id = candidate.candidate_id.clone();

                let last = self
                    .observations
                    .get(&id)
                    .map(|value| value.last_seen_at_unix_ms)
                    .unwrap_or(u64::MAX);
                if last != u64::MAX && now_ms.saturating_sub(last) < self.dedup_window_ms {
                    continue;
                }

                let observation = self
                    .observations
                    .get(&id)
                    .copied()
                    .unwrap_or(RouteObservation {
                        first_seen_at_unix_ms: candidate.observed_at_unix_ms,
                        last_seen_at_unix_ms: candidate.observed_at_unix_ms,
                    });
                let mut candidate = candidate;
                candidate.first_observed_at_unix_ms = observation.first_seen_at_unix_ms;
                candidate.last_observed_at_unix_ms = now_ms;
                candidate.route_priority = candidate.discovered_via.priority();
                candidate.route_stale =
                    now_ms.saturating_sub(observation.last_seen_at_unix_ms) >= ROUTE_STALE_AFTER_MS;

                match selected_this_poll.get(&id) {
                    Some(existing)
                        if existing.discovered_via.priority()
                            <= candidate.discovered_via.priority() => {}
                    _ => {
                        selected_this_poll.insert(id, candidate);
                    }
                }
            }
        }

        let mut results: Vec<DiscoveryCandidate> = selected_this_poll.into_values().collect();
        results.sort_by(|lhs, rhs| {
            lhs.discovered_via
                .priority()
                .cmp(&rhs.discovered_via.priority())
                .then_with(|| lhs.candidate_id.cmp(&rhs.candidate_id))
        });
        for candidate in &results {
            self.observations.insert(
                candidate.candidate_id.clone(),
                RouteObservation {
                    first_seen_at_unix_ms: candidate.first_observed_at_unix_ms,
                    last_seen_at_unix_ms: now_ms,
                },
            );
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
            first_observed_at_unix_ms: unix_now_ms(),
            last_observed_at_unix_ms: unix_now_ms(),
            route_priority: bearer.priority(),
            route_stale: false,
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
        assert_eq!(results[0].discovered_via, DiscoveryBearer::IPv4Broadcast);
        assert_eq!(
            results[0].route_priority,
            DiscoveryBearer::IPv4Broadcast.priority()
        );
    }

    #[test]
    fn multicast_candidate_is_preferred_over_bonjour_for_same_endpoint() {
        let bonjour = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Bonjour);
        let multicast = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Multicast);

        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![
            Box::new(StubBearer::new(vec![bonjour])),
            Box::new(StubBearer::new(vec![multicast])),
        ];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].discovered_via, DiscoveryBearer::Multicast);
        assert_eq!(
            results[0].route_priority,
            DiscoveryBearer::Multicast.priority()
        );
    }

    #[test]
    fn poll_results_are_sorted_by_bearer_priority() {
        let bonjour = make_candidate("192.168.1.22", 47655, DiscoveryBearer::Bonjour);
        let broadcast = make_candidate("192.168.1.21", 47655, DiscoveryBearer::IPv4Broadcast);
        let multicast = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Multicast);

        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![
            Box::new(StubBearer::new(vec![bonjour])),
            Box::new(StubBearer::new(vec![broadcast])),
            Box::new(StubBearer::new(vec![multicast])),
        ];

        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].discovered_via, DiscoveryBearer::Multicast);
        assert_eq!(results[1].discovered_via, DiscoveryBearer::IPv4Broadcast);
        assert_eq!(results[2].discovered_via, DiscoveryBearer::Bonjour);
    }

    #[test]
    fn candidate_retains_route_observation_timestamps() {
        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Multicast);
        let initial_observed = c.observed_at_unix_ms;
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];
        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].first_observed_at_unix_ms, initial_observed);
        assert!(results[0].last_observed_at_unix_ms >= initial_observed);
        assert!(!results[0].route_stale);
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
    fn bonjour_bearer_emits_bonjour_candidate() {
        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Bonjour);
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];
        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].discovered_via, DiscoveryBearer::Bonjour);
    }

    #[test]
    fn ipv4broadcast_bearer_emits_ipv4broadcast_candidate() {
        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::IPv4Broadcast);
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];
        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].discovered_via, DiscoveryBearer::IPv4Broadcast);
    }

    #[test]
    fn multicast_bearer_emits_multicast_candidate() {
        let c = make_candidate("192.168.1.20", 47655, DiscoveryBearer::Multicast);
        let bearers: Vec<Box<dyn DiscoveryBearerSource>> = vec![Box::new(StubBearer::new(vec![c]))];
        let mut pipeline = DiscoveryCandidatePipeline::new(bearers, vec![]);
        let results = pipeline.poll();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].discovered_via, DiscoveryBearer::Multicast);
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
