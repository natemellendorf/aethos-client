pub mod aeth_discovery_packet;
pub mod ble_discovery;
pub mod bonjour_discovery;
pub mod diagnostics;
pub mod discovery_candidate_pipeline;
pub mod encounter_orchestration;
pub mod encounter_scheduler;
pub mod gossip_store_sqlite;
pub mod gossip_sync;
pub mod identity_store;
pub mod ipv4_broadcast_discovery;
pub mod logging;
pub mod multicast_discovery;
pub mod protocol;
#[cfg(test)]
pub mod vectors;
