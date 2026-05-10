# ADR: Multi-Bearer LAN Discovery

- Status: Accepted

## Context

Desktop LAN discovery previously mixed legacy UDP peer-interaction state with newer multi-bearer orchestration work. The cutover consolidates discovery around normalized LAN candidates and canonical encounter ownership so follow-up behavior is no longer planned in parallel documents.

## Decision

Use the multi-bearer LAN discovery flow as the canonical desktop behavior:

1. Discovery candidates are normalized from Bonjour, IPv4 broadcast, and multicast sources.
2. Encounter orchestration owns discovery observation, control-exchange start, and transfer-bearer selection.
3. Legacy `UdpPeerInteraction` state and its duplicate encounter gating are removed.

## Consequences

- Desktop LAN discovery behavior is described by one spec/ADR pair instead of active planning shards.
- Historical `ac-mbe-*` planning documents remain available for audit context but are no longer authoritative.

## Appendix: Supersedes

- `ac-mbe-01-audit-current-desktop-encounter-paths.md` — legacy path audit for encounter, discovery, and gossip session behavior.
- `ac-mbe-02-rust-adoption-of-canonical-bearer-orchestration-models.md` — Rust adoption map for canonical bearer/orchestration interfaces.
- `ac-mbe-03-desktop-encounter-manager-integration-plan.md` — encounter-manager ownership and staged integration blueprint.
- `ac-mbe-04-discovery-bootstrap-bearer-adoption-plan.md` — discovery/bootstrap bearer contract and handoff plan.
- `ac-mbe-05-transfer-bearer-upgrade-downgrade-plan.md` — transfer-bearer transition strategy.
- `ac-mbe-06-desktop-telemetry-diagnostics-explainability-plan.md` — telemetry and diagnostics explainability plan.
- `ac-mbe-07-mixed-bearer-fixture-consumption-plan.md` — authoritative mixed-bearer fixture consumption mapping.
- `ac-mbe-08-mixed-bearer-integration-test-plan.md` — integration-test and scheduler verification plan.
- `ac-mbe-09-cutover-and-cleanup-plan.md` — cutover and cleanup plan for superseded desktop encounter logic.
