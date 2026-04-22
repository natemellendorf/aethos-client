# ADR: Diagnostics Collector and Run-Scoped Observability v1

- Status: Accepted
- Date: 2026-04-21
- Deciders: Aethos Linux maintainers

## Context

Current diagnostics are mostly local freeform logs and ad-hoc CLI output. That is useful for human debugging but not sufficient for reliable agent-driven analysis of LAN gossip lifecycle behavior.

We need a single, queryable diagnostic plane that supports cross-process correlation by run and explains where protocol flow stopped (discovery, encounter, hello, summary, request, transfer, import, UI projection) without exposing sensitive payloads.

## Decision

Implement a separate diagnostics collector service in Rust with these properties:

1. **Transport/API**: Axum HTTP API
2. **Serialization**: `serde`/`serde_json` typed request/response models
3. **Storage**: `sqlx` + SQLite initially (future Postgres possible)
4. **Deployment**: single binary (`diagnostics-collector`)
5. **Scope boundary**: diagnostics plane remains separate from relay data plane

## API contract (v1)

The collector exposes exactly these endpoints:

- `POST /api/v1/diagnostics/runs`
- `POST /api/v1/diagnostics/events`
- `GET /api/v1/diagnostics/runs/{run_id}`
- `GET /api/v1/diagnostics/runs/{run_id}/timeline`
- `GET /api/v1/diagnostics/runs/{run_id}/summary`

## Event contract

Collector requests and responses are based on `docs/spec/diagnostics-event-schema-v1.md`.

Every event includes required correlation identifiers and platform metadata:

- `schema_version`, `run_id`, `session_id`, `encounter_id`, `event_id`, `timestamp_utc`
- `platform`, `app`, `build_sha`, `component`, `event_type`, `phase`, `result`

Optional context fields are accepted for richer forensics while keeping payloads privacy-safe.

## Summary and stall detection rules (server-side)

`GET /api/v1/diagnostics/runs/{run_id}/summary` computes:

- highest protocol phase reached (overall and per `item_id`)
- top errors grouped by `reason_code`
- missing transitions/stalls:
  - discovery without encounter
  - hello without summary
  - request without transfer
  - transfer without import
  - import without UI projection

## Privacy and safety policy

- Never capture private keys, session secrets, or decrypted plaintext content
- Never send plaintext message bodies by default
- Prefer IDs, hashes, sizes, counters, and protocol metadata
- Any verbose payload capture must be explicit local-only debug mode

## Retention policy

- Collector retains events for a bounded period (TTL configurable via environment)
- Expired rows are deleted by periodic cleanup job
- Summaries are always derived from retained run data; no separate sensitive cache

## Consequences

Positive:

- Agent-readable run timeline and summary for deterministic triage
- Cross-platform schema contract for desktop/iOS interoperability
- Better regression analysis from structured, queryable history

Trade-offs:

- New service/binary to run in dev/test pipelines
- Additional storage and retention lifecycle responsibilities
