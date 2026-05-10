# LAN Discovery — Multi-Bearer v1 (Desktop)

## Overview
The aethos-client desktop discovery system uses a multi-bearer approach to ensure reliable peer encounter detection across diverse network environments. By running three distinct discovery mechanisms concurrently, the client avoids reliance on a single protocol like Bonjour, which can be inconsistent or unavailable on certain Linux distributions and restricted corporate networks.

The primary bearer is Bonjour (mDNS/DNS-SD), which provides rich service advertisement and discovery. This is supplemented by IPv4 broadcast as a universal LAN fallback and UDP multicast as a reliable middle ground. Together, these three bearers provide a robust discovery layer that remains functional even if specific protocols are blocked or unconfigured on the local segment.

## Service Addresses
Discovery traffic is standardized across the following service addresses:

- **Bonjour**: Service type `_aethos._udp.local.`, Port `47655`
- **IPv4 Broadcast**: `255.255.255.255:47655`
- **Multicast**: Group `224.0.0.251`, Port `47655`

## Discovery Candidate Normalization
All raw packets received from the three bearers are funneled into a unified `DiscoveryCandidatePipeline`. This pipeline normalizes incoming data into a standard `DiscoveryCandidate` structure.

### DiscoveryCandidate Fields
- `candidate_id`: A unique identifier for the endpoint, formatted as `"{ip}:{port}"`.
- `bearer_type`: The source protocol (Bonjour, Broadcast, or Multicast).
- `payload`: The raw discovery advertisement data.
- `timestamp`: The arrival time of the packet.

### Deduplication and Filtering
- **Dedup Window**: 500ms. If the same endpoint (identified by `candidate_id`) is detected via multiple bearers within this window, only one `DiscoveryCandidate` event is emitted.
- **Self-Filter**: The pipeline automatically discards packets originating from the client's own interface IP addresses to prevent self-discovery.

## Bearer Failure Isolation Policy
Bearer reliability is managed through strict isolation. A failure in one bearer (e.g., a socket error or an unavailable network interface) does not affect the operation of the other active bearers.

- Each bearer independently monitors its own health.
- If a bearer encounters a fatal error, it degrades to a `Disabled` state and emits an Error event for diagnostic logging.
- The discovery pipeline continues to operate using the remaining functional bearers.

## Runtime Toggles
Discovery behavior can be tuned using the following environment variables:

- `AETHOS_DISABLE_LAN_DISCOVERY=1`: Disables all three discovery bearers entirely.
- `AETHOS_DISABLE_BONJOUR=1`: Disables the Bonjour/mDNS bearer.
- `AETHOS_DISABLE_IPV4_BROADCAST=1`: Disables the IPv4 broadcast bearer.
- `AETHOS_DISABLE_MULTICAST=1`: Disables the multicast bearer.
- `AETHOS_GOSSIP_LAN_PORT`: Overrides the default discovery port (47655).

## iOS vs Desktop Parity Matrix

| Behavior | iOS Status | Desktop Status | Notes |
|---|---|---|---|
| Bonjour/mDNS discovery | ✅ Implemented | ✅ Implemented | Both use `_aethos._udp` |
| IPv4 broadcast discovery | ✅ Implemented | ✅ Implemented (this spec) | Same port 47655 |
| Multicast discovery (224.0.0.251) | ✅ Implemented | ✅ Implemented (this spec) | |
| Discovery candidate pipeline with dedup | ✅ Implemented | ✅ Implemented (this spec) | 500ms dedup window |
| Self-discovery filter | ✅ Implemented | ✅ Implemented | Filter own interface IPs |
| Bearer failure isolation | ✅ Implemented | ✅ Implemented | Per-bearer degradation |
| Sleep/wake bearer restart | ✅ Implemented | 🔄 Partial (poll detects stale) | Full restart on SIGUSR1 |
| Multicast entitlement | ⚠️ Pending Apple approval | N/A | Not needed on macOS/Linux |
| EncounterManager for LAN peers | ✅ Implemented | ✅ Implemented (ac-impl-06) | |
| Per-bearer disable env flags | ✅ Implemented | ✅ Implemented | Different flag names |

## Supersedes
This specification provides the canonical definition for multi-bearer discovery and supersedes the following planning documents:

- `docs/beads/multi-bearer-encounter/ac-mbe-01-audit-current-desktop-encounter-paths.md`: Initial audit of existing discovery paths.
- `docs/beads/multi-bearer-encounter/ac-mbe-02-rust-adoption-of-canonical-bearer-orchestration-models.md`: Modeling of bearer state machines.
- `docs/beads/multi-bearer-encounter/ac-mbe-03-desktop-encounter-manager-integration-plan.md`: Integration plan for EncounterManager.
- `docs/beads/multi-bearer-encounter/ac-mbe-04-discovery-bootstrap-bearer-adoption-plan.md`: Planning for bootstrap bearer implementation.
- `docs/beads/multi-bearer-encounter/ac-mbe-05-transfer-bearer-upgrade-downgrade-plan.md`: Logic for switching between discovery and transfer states.
- `docs/beads/multi-bearer-encounter/ac-mbe-06-desktop-telemetry-diagnostics-explainability-plan.md`: Telemetry and logging requirements.
- `docs/beads/multi-bearer-encounter/ac-mbe-07-mixed-bearer-fixture-consumption-plan.md`: Test fixture definitions.
- `docs/beads/multi-bearer-encounter/ac-mbe-08-mixed-bearer-integration-test-plan.md`: Integration testing strategy.
- `docs/beads/multi-bearer-encounter/ac-mbe-09-cutover-and-cleanup-plan.md`: Plan for removing legacy single-bearer logic.
