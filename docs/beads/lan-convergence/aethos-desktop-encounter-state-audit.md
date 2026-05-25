# aethos-desktop-encounter-state-audit

Status: complete

## Scope

This note audits the live desktop LAN runtime in the Tauri backend at `spikes/tauri-desktop/src-tauri/src/main.rs` plus the shared discovery/core modules in `src/aethos_core/`.

The primary finding is that the desktop runtime still treats a LAN encounter as a short bounded reconciliation round keyed partly by peer identity and partly by endpoint/IP. When a UDP encounter reaches a stop condition, the runtime immediately prunes both encounter state and the peer route maps. That makes transfer completion look like peer completion and leaves async app-layer follow-up work such as `/ping -> /pong` dependent on later rediscovery.

## Current runtime map

### Discovery and route candidate creation

- `spikes/tauri-desktop/src-tauri/src/main.rs:create_discovery_candidate_pipeline`
  - Builds the shared candidate pipeline with three bearers:
    - `BonjourDiscoveryCandidateSource`
    - `IPv4BroadcastDiscoveryCandidateSource`
    - `MulticastDiscoveryCandidateSource`
- `src/aethos_core/discovery_candidate_pipeline.rs`
  - Dedupes candidates by `candidate_id`, which is currently the endpoint string.
- `src/aethos_core/bonjour_discovery.rs`
  - Browses `_aethos._udp.local.`, resolves the first IPv4 endpoint, and emits `EndpointResolved`.
- `src/aethos_core/multicast_discovery.rs`
  - Joins the multicast group, emits `PeerDiscovered`/`EndpointResolved`, and currently resolves peers as raw `SocketAddr` endpoints.

### Encounter creation and discovery-to-HELLO handoff

- `spikes/tauri-desktop/src-tauri/src/main.rs` background gossip loop
  - Creates `lan_encounters: HashMap<String, EncounterManager>` keyed by `candidate.candidate_id`.
  - Creates `udp_encounters: HashMap<String, GossipEncounterState>` keyed by source IP string.
  - For each discovery candidate, calls `EncounterManager::observe_discovery(...)` and then `maybe_trigger_bonjour_hello(...)`.
- `maybe_trigger_bonjour_hello`
  - Clears old address-to-peer mappings for the resolved endpoint.
  - Sends a HELLO to the resolved endpoint.
  - Applies a short per-endpoint cooldown.

### HELLO / SUMMARY / REQUEST / TRANSFER / RECEIPT handling

- `handle_gossip_frame`
  - `HELLO`
    - Maps `source_key` and `source_ip_key` to `hello.node_id` in `peer_node_by_addr`.
    - Maps `peer_addr_by_node[node_id] = source`.
    - Ensures a `udp_encounter` keyed by source IP.
    - Starts control exchange in `lan_encounters` if the exact endpoint-keyed encounter exists.
    - Sends `SUMMARY` and `RELAY_INGEST` immediately.
  - `SUMMARY`
    - Ensures the UDP encounter.
    - Stores the latest summary in `udp_latest_summary_by_peer[source_ip]`.
    - Calls `drain_udp_peer_reconciliation_round(...)`.
  - `RELAY_INGEST`
    - Selects missing item IDs.
    - Sends a `REQUEST` unless the encounter already hit a stop condition.
  - `REQUEST`
    - Serves transfer over TCP if supported, else UDP fallback.
  - `TRANSFER`
    - Imports objects via `import_transfer_items(...)`.
    - Merges inbound messages into chat state with `merge_pulled_messages(...)`.
    - May queue follow-up outbound work such as `/pong` via `queue_auto_pong_message(...)`.
    - If accepted items increased, immediately tries another reconciliation round via `drain_udp_peer_reconciliation_round(..., "udp_transfer_followup")`.
    - Sends `RECEIPT` when `receipt_item_ids` exist.
  - `RECEIPT`
    - Currently ignored for convergence purposes in the UDP path.

## Required output

### 1. Where encounters are created

- Discovery-facing encounter tracking:
  - `lan_encounters.entry(candidate.candidate_id.clone())` in the gossip worker loop.
- UDP reconciliation encounter tracking:
  - `ensure_udp_peer_encounter(...)`
  - `new_udp_peer_encounter(...)`
- BLE discovery uses a separate `EncounterManager` map and is not the source of the LAN teardown bug.

### 2. Where encounters are keyed

- `lan_encounters`
  - Keyed by discovery `candidate_id`, currently the resolved endpoint string such as `ip:port`.
- `udp_encounters`
  - Keyed by source IP string only.
- `peer_node_by_addr`
  - Mixed map keyed by both `ip:port` and `ip`, with values set to `HELLO.node_id`.
- `peer_addr_by_node`
  - Keyed by `HELLO.node_id`, value is the latest resolved `SocketAddr`.

**Conclusion:** peer identity is learned from HELLO, but operational state is still split across endpoint-keyed and IP-keyed maps. Endpoint identity is still authoritative for discovery encounter ownership and prune behavior.

### 3. Where encounters are discarded

- `GossipEncounterState::finish(...)`
  - Marks a stop reason.
- `prune_finished_udp_encounters(...)`
  - Immediately removes:
    - `udp_encounters`
    - `udp_last_seen_by_peer`
    - `udp_latest_summary_by_peer`
    - `udp_relay_ingest_candidates_by_peer`
    - `peer_tcp_capable_by_ip`
    - `tcp_backoff_until_by_ip`
    - `recent_served_request_by_peer`
    - `recent_outbound_request_by_peer`
    - `peer_node_by_addr` entries for the peer
    - `peer_addr_by_node` entries for the peer
  - Also closes the matching `EncounterManager` and, when bytes were imported, records `set_transfer_bearer(...)`, `mark_transfer_completed(...)`, then `close(...)`.

### 4. Which events currently trigger discard

Indirectly, any event path that causes `GossipEncounterState::finish(...)` and then reaches the next prune pass:

- `drain_udp_peer_reconciliation_round(...)`
  - `NoMoreWanted`
  - `RoundBudgetExceeded`
  - `TimeBudgetExceeded`
- `handle_gossip_frame` `TRANSFER` branch
  - `PeerReturnedNoUsefulItems`
  - `ByteBudgetExceeded`
- `handle_gossip_frame` `RELAY_INGEST` branch
  - `NoMoreWanted`
- `ensure_udp_peer_encounter(...)` and `prune_finished_udp_encounters(...)`
  - `PeerTimeout`
- `GossipEncounterState::should_stop(...)`
  - `NoProgressStreakExceeded`
  - `GossipDisabled`

**Important:** there is no explicit `on_transfer_completed() { discard_encounter(peer) }` function, but the runtime still closes and prunes immediately after a transfer-driven stop condition, which is equivalent at the lifecycle level.

### 5. Whether discard is immediate or delayed

- Effectively immediate.
- `prune_finished_udp_encounters(...)` runs:
  - once at the top of every gossip worker loop iteration
  - again immediately after every handled inbound UDP frame
- There is no idle grace or quiet convergence state between `finish(...)` and route/encounter removal.

### 6. Whether outbox mutation can resume an encounter

- Not directly.
- Local outbound work creation (`send_message_blocking`, `queue_auto_pong_message`) calls:
  - `gossip_record_local_payload(...)`
  - `runtime.force_announce.store(true, ...)`
  - `set_gossip_event("announce queued")`
- The worker responds by broadcasting inventory/HELLO on the next loop.
- There is **no direct path** that:
  - reopens an existing converged encounter,
  - cancels a pending idle disconnect,
  - or targets a still-valid active peer encounter as a first-class resume.

If the prior encounter was pruned, delivery depends on periodic broadcast or discovery plus fresh HELLO. The runtime does not currently treat outbox mutation as peer-specific convergence work.

### 7. Whether peer identity or endpoint identity is authoritative

- Peer identity is authoritative for protocol meaning only after `HELLO.node_id` is learned.
- Endpoint identity is still authoritative in several runtime control paths:
  - discovery candidates are keyed by endpoint string
  - `lan_encounters` are keyed by endpoint string
  - Bonjour re-HELLO behavior clears endpoint-derived route state
  - prune removes all peer routing state by IP/endpoint immediately

**Conclusion:** the current implementation is hybrid, not peer-authoritative.

### 8. Where `/ping -> /pong` currently fails or could fail

- `merge_pulled_messages(...)` detects `/ping` and calls `queue_auto_pong_message(...)`.
- `queue_auto_pong_message(...)` records a new outbound `/pong` item and forces an announce.
- The same inbound transfer path may still finish the UDP encounter for `NoMoreWanted`, `PeerReturnedNoUsefulItems`, or other bounded-stop reasons.
- `prune_finished_udp_encounters(...)` can then immediately remove the peer route maps and close the endpoint-keyed `EncounterManager`.

Result:

1. `/ping` is received.
2. `/pong` is queued asynchronously at app-layer merge time.
3. Encounter state can still be marked finished in the same lifecycle.
4. Prune clears the active route/encounter association.
5. `/pong` depends on later broadcast/discovery/HELLO instead of the still-live peer session.

This is the concrete desktop version of the failure described in the bead.

### 9. Whether the Tauri frontend/app layer can enqueue follow-up work after receipt handling

- Yes, but in the currently traced `/ping -> /pong` path the follow-up is created inside the Rust backend, not by a frontend callback.
- `merge_pulled_messages(...)` queues `/pong` from backend logic after inbound message classification.
- User-driven outbound work also enters through Tauri commands like `send_message` / `send_message_blocking`.
- The event bridge itself (`emit_chat_snapshot_event*`, sound events) is projection-only in the current runtime and does not create outbound items.

### 10. Whether the Rust backend sees those mutations soon enough to resume convergence

- The backend sees the outbox mutation immediately because it performs the write itself.
- The problem is not visibility of the mutation.
- The problem is lifecycle semantics:
  - outbox mutation only forces a new generic announce,
  - while encounter teardown/pruning can happen in the same or next worker iteration,
  - so the mutation does not extend the existing encounter.

## Discovery bearer notes

- Bonjour, broadcast, and multicast all currently feed the same endpoint-keyed candidate pipeline.
- `maybe_trigger_bonjour_hello(...)` is reused for all three bearers despite the name.
- Multicast is present but not yet clearly primary; all bearers currently collapse into the same resolved-endpoint HELLO flow.
- Route freshness is not modeled explicitly beyond recent dedupe and current peer maps.

## Tauri/frontend follow-up notes

- The frontend bridge does matter indirectly because the desktop app runtime owns persistence, chat projection, and UI event emission in the same Tauri backend.
- However, the specific follow-up work examined here is backend-originated. The missing piece is not frontend awareness; it is convergence-aware encounter retention after backend-created follow-up.

## Minimal follow-up code paths that must change

1. `GossipEncounterState` in `spikes/tauri-desktop/src-tauri/src/main.rs`
   - Replace simple bounded-stop semantics with explicit convergence/quiet/idle state.
2. `drain_udp_peer_reconciliation_round(...)`
   - Stop treating one empty request result as encounter completion.
3. `handle_gossip_frame(...)` transfer and receipt paths
   - Feed convergence state and app-follow-up state instead of relying on stop+prune.
4. `prune_finished_udp_encounters(...)`
   - Stop immediate route/encounter discard after item-level completion.
   - Introduce idle grace and delayed disconnect only after quiet convergence.
5. Outbox mutation hooks
   - `send_message_blocking(...)`
   - `queue_auto_pong_message(...)`
   - any media-control enqueue path using `runtime.force_announce`
   - These need peer-targeted encounter resume/extension semantics, not just broadcast.
6. Diagnostics in `src/aethos_core/diagnostics.rs` and LAN logging sites
   - Separate transfer completion from encounter completion.
   - Emit explicit convergence/idle/resume/disconnect reasons.

## Compatibility note

This audit aligns with the existing protocol/docs direction already present in the repo:

- HELLO establishes peer identity.
- Bonjour metadata is not identity.
- LAN encounters should drain through multiple rounds until convergence.

The desktop bug is an implementation-lifecycle gap, not a protocol gap.
