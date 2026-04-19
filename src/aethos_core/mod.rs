pub mod ble_discovery;
#[cfg(target_os = "linux")]
pub mod bonjour_discovery;
pub mod encounter_orchestration;
pub mod encounter_scheduler;
pub mod gossip_store_sqlite;
pub mod gossip_sync;
pub mod identity_store;
pub mod logging;
pub mod protocol;
#[cfg(test)]
pub mod vectors;
