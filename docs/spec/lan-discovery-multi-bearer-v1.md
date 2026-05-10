# LAN Discovery Multi-Bearer v1

- Status: Final

## Summary

This spec captures the cutover state for desktop LAN discovery after the multi-bearer encounter migration. Discovery candidates may arrive from Bonjour, IPv4 broadcast, or multicast, while canonical encounter orchestration owns follow-up control-exchange and transfer decisions.

## Supersedes

- `ac-mbe-01-audit-current-desktop-encounter-paths.md` — audited the legacy desktop encounter/discovery/runtime paths before canonical cutover.
- `ac-mbe-02-rust-adoption-of-canonical-bearer-orchestration-models.md` — mapped canonical bearer and orchestration concepts into the Rust desktop runtime.
- `ac-mbe-03-desktop-encounter-manager-integration-plan.md` — defined encounter-manager ownership boundaries and staged integration points.
- `ac-mbe-04-discovery-bootstrap-bearer-adoption-plan.md` — specified discovery/bootstrap bearer responsibilities and handoff rules.
- `ac-mbe-05-transfer-bearer-upgrade-downgrade-plan.md` — planned upgrade/downgrade behavior between control and transfer bearers.
- `ac-mbe-06-desktop-telemetry-diagnostics-explainability-plan.md` — defined the explainability and telemetry contract for multi-bearer encounters.
- `ac-mbe-07-mixed-bearer-fixture-consumption-plan.md` — mapped authoritative fixture families into desktop test inputs.
- `ac-mbe-08-mixed-bearer-integration-test-plan.md` — defined mixed-bearer integration scenarios and scheduler verification gates.
- `ac-mbe-09-cutover-and-cleanup-plan.md` — described the final cutover and cleanup steps for superseded encounter logic.
