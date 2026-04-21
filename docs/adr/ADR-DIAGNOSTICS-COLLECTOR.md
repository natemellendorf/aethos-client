# ADR: Diagnostics Collector and Unified Eventing v1

- Status: **Accepted**
- Date: 2026-04-21
- Authors: aethos-linux team

## Context

As aethos-linux transitions into multi-bearer discovery and complex relay session management, we need a standard way to collect and exfiltrate diagnostic data without polluting primary protocol frames. We lack a unified sink for performance metrics, connection lifecycle events, and crash/error forensics.

Existing telemetry is scattered across local logs and UI state, making remote triage and regression analysis difficult for the decentralized node network.

## Decision

We will implement a centralized Diagnostics Collector on the Linux client that pushes to a standardized Diagnostics Relay API.

### 1. Collector Responsibility

The collector is a singleton service within the client that:
- Buffers events in-memory with a configurable overflow drop policy.
- Batches events for exfiltration over HTTPS.
- Manages "Diagnostic Runs" representing a single app session or a specific troubleshooting window.

### 2. Required API Endpoints

The collector must communicate with a relay supporting these exact endpoints:

- `POST /api/v1/diagnostics/runs`: Initialize a new diagnostic run.
- `POST /api/v1/diagnostics/events`: Push a batch of events associated with a run.
- `GET /api/v1/diagnostics/runs/{run_id}`: Retrieve run metadata.
- `GET /api/v1/diagnostics/runs/{run_id}/timeline`: Retrieve ordered event stream for a run.
- `GET /api/v1/diagnostics/runs/{run_id}/summary`: Retrieve aggregated run statistics (event counts, stall flags).

### 3. Stall Detection

The collector must implement stall detection to flag unresponsive subsystems:
- **Rule**: If a high-priority event (e.g., `HEARTBEAT`, `RELAY_CONNECT`) is not received within 150% of its expected interval, the collector generates a synthetic `STALL_DETECTED` event.
- **Rule**: If the UI thread heartbeat misses 3 consecutive 1s intervals, a `GUI_STALL` event is recorded.

## Consequences

- Improved observability during multi-bearer cutover.
- Standardized forensic data for bug reports.
- Increased memory overhead for event buffering (capped at 50MB by default).
- Privacy risk: collector must strictly follow privacy rules defined in the schema spec.
