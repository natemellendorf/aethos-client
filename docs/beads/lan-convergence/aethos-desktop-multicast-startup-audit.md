# aethos-desktop-multicast-startup-audit

Status: complete

## Scope

This note audits the current desktop multicast startup path implemented in:

- `src/aethos_core/multicast_discovery.rs`
- `src/aethos_core/discovery_candidate_pipeline.rs`
- `spikes/tauri-desktop/src-tauri/src/main.rs`

## Current startup behavior

### Socket creation and bind

- Desktop creates an IPv4 UDP datagram socket with `socket2`.
- The runtime enables:
  - `SO_REUSEADDR`
  - nonblocking mode
- The multicast socket binds `0.0.0.0:<port>` where `<port>` is the stable Aethos LAN listener port (`AETHOS_GOSSIP_LAN_PORT`, default `47655`).

### Group join strategy

- The runtime targets multicast group `224.0.0.251` on the stable LAN port.
- It collects non-loopback IPv4 local addresses and attempts `join_multicast_v4` on each interface.
- If no non-loopback IPv4 local addresses are found, it falls back to joining on `INADDR_ANY` / `UNSPECIFIED`.
- Partial interface join failure is non-fatal:
  - join errors are queued as `MulticastDiscoveryEvent::Error`
  - startup still succeeds if at least one interface join succeeds
- Total join failure is fatal to the multicast bearer only:
  - the bearer falls back to disabled state
  - the error is surfaced instead of crashing the app

### Announcement and receive behavior

- The bearer emits a 4-byte Aethos beacon (`[0xAE, 0x74, 0x48, 0x53]`) every fifth poll.
- Inbound datagrams are drained in nonblocking mode.
- The bearer self-filters packets from local IPs before emitting discovery events.
- Valid beacon frames emit:
  - `PeerDiscovered`
  - `EndpointResolved`

### Runtime integration

- The Tauri gossip worker builds a shared `DiscoveryCandidatePipeline` with:
  1. Bonjour
  2. IPv4 broadcast
  3. Multicast
- The shared pipeline now prefers the strongest bearer per endpoint during one poll:
  1. multicast
  2. IPv4 broadcast
  3. Bonjour
- The gossip worker still uses one stable UDP listener and one stable TCP listener on the same LAN port family.

## Failure visibility

- Multicast startup failures surface as `MulticastDiscoveryEvent::Error` and are logged through the discovery source wrapper.
- Runtime restart behavior already exists in the Tauri gossip worker:
  - on multicast discovery error, restart is scheduled after 5 seconds
  - the app does not crash or stop the rest of LAN discovery

## Platform and test notes

- The current implementation is IPv4-only.
- Loopback self-filtering exists, but there is no explicit loopback-enable toggle specific to multicast beyond the broader local-address/self-filter behavior.
- The current tests already cover:
  - disable flag behavior
  - self-filter behavior
  - partial interface failure non-fatal behavior
  - candidate pipeline multicast preference over Bonjour for the same endpoint

## Current conclusion

The desktop runtime already has a stable multicast startup path that:

- binds the stable Aethos LAN port
- attempts interface-specific group joins
- degrades gracefully on partial failure
- surfaces total startup failure clearly
- integrates multicast into the shared candidate pipeline

The major remaining gap is not basic startup; it is richer route freshness/lifecycle policy above startup.
