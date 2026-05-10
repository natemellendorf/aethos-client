# ADR: Multi-Bearer LAN Discovery for aethos-client Desktop

## Status
Accepted

## Date
2026-05-10

## Context
The aethos-client desktop implementation originally relied exclusively on Bonjour (mDNS/DNS-SD) for local network discovery. While Bonjour provides a rich discovery experience, it has several limitations:
- It is not universally available or configured on all Linux distributions (often requiring Avahi).
- It is frequently blocked by corporate or restrictive network firewalls.
- It can be difficult to troubleshoot when service advertisement fails.

To match the reliability of the aethos-ios client, which successfully implemented a three-bearer discovery model (Bonjour + IPv4 Broadcast + Multicast), the desktop client requires a similar architecture. The goal is to ensure that LAN discovery works out-of-the-box in the widest possible range of network environments without a single point of failure.

## Decision
We will implement a multi-bearer LAN discovery architecture for the desktop client:
- Add IPv4 broadcast and UDP multicast bearers alongside the existing Bonjour bearer.
- All three bearers will run concurrently on port `47655`.
- A unified `DiscoveryCandidatePipeline` will consume packets from all active bearers.
- The pipeline will normalize packets into `DiscoveryCandidate` events and apply a 500ms deduplication window to prevent redundant notifications for the same peer.
- Successful discovery events will flow into the `EncounterManager`, aligning the LAN discovery path with existing BLE and Relay encounter flows.

## Alternatives Considered
- **Bonjour-only (Rejected)**: Deemed insufficiently robust for the variety of Linux environments the client supports.
- **Multicast-only (Rejected)**: While more efficient than broadcast, multicast is sometimes blocked or poorly routed on consumer-grade hardware where broadcast still functions.
- **Broadcast-only (Rejected)**: Lacks the efficiency and grouping capabilities of multicast/Bonjour and is considered a fallback rather than a primary mechanism.

## Consequences
- The client will maintain three concurrent UDP sockets per LAN interface (where supported).
- Sockets must be bound using `SO_REUSEADDR` to allow port 47655 to be shared across bearers.
- The introduction of the `DiscoveryCandidatePipeline` adds a clean abstraction layer in `src/aethos_core/` for handling multi-source discovery data.
- Increased CPU and network usage for discovery traffic, though the total volume remains negligible for typical LAN environments.

## Supersedes
This ADR formally supersedes the following planning documents from the multi-bearer encounter series:

| Document | Covered | Status |
|---|---|---|
| `docs/beads/multi-bearer-encounter/ac-mbe-01-audit-current-desktop-encounter-paths.md` | Audit of legacy discovery paths | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-02-rust-adoption-of-canonical-bearer-orchestration-models.md` | State machine architecture | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-03-desktop-encounter-manager-integration-plan.md` | Integration with EncounterManager | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-04-discovery-bootstrap-bearer-adoption-plan.md` | Implementation strategy for new bearers | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-05-transfer-bearer-upgrade-downgrade-plan.md` | Peer state transition logic | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-06-desktop-telemetry-diagnostics-explainability-plan.md` | Logging and diagnostics | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-07-mixed-bearer-fixture-consumption-plan.md` | Testing fixtures | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-08-mixed-bearer-integration-test-plan.md` | Integration testing | Superseded by this ADR |
| `docs/beads/multi-bearer-encounter/ac-mbe-09-cutover-and-cleanup-plan.md` | Deprecation of old logic | Superseded by this ADR |
