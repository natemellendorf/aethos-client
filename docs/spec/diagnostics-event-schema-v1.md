# Diagnostics Event Schema v1

- Version: 1
- Status: Final
- Date: 2026-04-21

## 1. Overview

This document defines the canonical diagnostics event contract for Aethos desktop and iOS clients and the Rust diagnostics collector.

The schema is run-scoped and designed for agent-driven analysis of protocol progression while avoiding sensitive payload exposure.

## 2. Required event fields

All events MUST contain the following fields:

| Field | Type | Description |
|---|---|---|
| `schema_version` | string | Schema contract version (for v1: `"1"`) |
| `run_id` | string | Correlates all events in one diagnostic run |
| `session_id` | string | Correlates a user/app session within a run |
| `encounter_id` | string | Correlates one encounter lifecycle |
| `event_id` | string | Unique event identifier |
| `timestamp_utc` | string | Event time in UTC (RFC3339 recommended) |
| `platform` | string | E.g. `linux`, `ios` |
| `app` | string | E.g. `aethos-desktop`, `aethos-ios` |
| `build_sha` | string | Build/source revision identifier |
| `component` | string | Subsystem name (`discovery`, `relay`, `ui`, etc.) |
| `event_type` | string | Canonical event catalog key |
| `phase` | string | Protocol/app phase marker |
| `result` | string | Outcome marker (`started`, `succeeded`, `failed`, etc.) |

## 3. Optional event fields

Events MAY include:

| Field | Type | Description |
|---|---|---|
| `peer_id` | string | Local peer identifier |
| `remote_peer_id` | string | Remote peer identifier |
| `item_id` | string | Content/item identifier |
| `bearer` | string | Bearer context (`ble`, `bonjour`, `relay`, etc.) |
| `reason_code` | string | Stable machine-readable failure/status code |
| `message` | string | Human-readable diagnostic note (non-sensitive) |
| `fields` | object | Structured metadata map for event-specific attributes |

## 4. Event catalog v1

Implementations MUST support these event types:

- `app.start`
- `app.stop`
- `diag.run.attached`
- `discovery.started`
- `discovery.signal.detected`
- `discovery.signal.ignored`
- `bearer.selected`
- `encounter.opened`
- `encounter.closed`
- `hello.sent`
- `hello.received`
- `summary.sent`
- `summary.received`
- `request.planned`
- `request.sent`
- `request.received`
- `transfer.sent`
- `transfer.received`
- `receipt.sent`
- `receipt.received`
- `inbox.import.started`
- `inbox.import.succeeded`
- `inbox.import.failed`
- `ui.projection.started`
- `ui.projection.succeeded`
- `ui.projection.failed`
- `relay.connected`
- `relay.disconnected`
- `error`

## 5. Canonical phase guidance

Recommended `phase` values:

- `app`
- `discovery`
- `encounter`
- `hello`
- `summary`
- `request`
- `transfer`
- `receipt`
- `import`
- `ui_projection`
- `relay`
- `error`

## 6. Example event

```json
{
  "schema_version": "1",
  "run_id": "run-1775656013003",
  "session_id": "desktop-a",
  "encounter_id": "enc-4b4d3e",
  "event_id": "evt-01J8M9W1YQ5R",
  "timestamp_utc": "2026-04-21T18:48:32Z",
  "platform": "linux",
  "app": "aethos-desktop",
  "build_sha": "4a3b2c1d",
  "component": "gossip_sync",
  "event_type": "transfer.received",
  "phase": "transfer",
  "result": "succeeded",
  "peer_id": "peer-a",
  "remote_peer_id": "peer-b",
  "item_id": "item-92c5",
  "bearer": "bonjour",
  "fields": {
    "bytes": 4096,
    "chunk_count": 4
  }
}
```

## 7. Privacy and security rules

- MUST NOT include private keys, session secrets, or decrypted plaintext message bodies
- SHOULD prefer IDs, hashes, lengths, counters, timing, and protocol metadata
- `message` MUST be safe for logs and issue trackers
- Verbose payload capture MUST be opt-in and local-only debug mode

## 8. Retention guidance

- Keep event retention bounded by TTL (default 7 days recommended for local diagnostics)
- Purge expired rows periodically
- Keep summaries derived from retained event data; avoid parallel sensitive caches

## 9. Compatibility and evolution

- Additive fields are allowed; unknown fields MUST be ignored by consumers
- Breaking changes require a new schema version
- Producers SHOULD continue to emit v1 until all consumers support a newer version
