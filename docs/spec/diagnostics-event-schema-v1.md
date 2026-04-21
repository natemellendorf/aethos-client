# Diagnostics Event Schema v1

- Version: 1.0.0
- Status: **Draft**

## Overview

This schema defines the structure for diagnostic events collected by aethos-linux clients. It ensures compatibility across different relay implementations and automated triage tools.

## Event Structure

### Required Fields
- `event_id`: UUIDv4 - Unique identifier for the event.
- `run_id`: UUIDv4 - Reference to the diagnostic run.
- `timestamp`: ISO8601 (UTC) - When the event occurred.
- `name`: String - The event type name (see Event Catalog).

### Optional Fields
- `payload`: Object - Arbitrary JSON data specific to the event type.
- `severity`: Enum (DEBUG, INFO, WARN, ERROR, CRITICAL) - Default: INFO.
- `subsystem`: String - The component that emitted the event (e.g., "ble", "relay", "gui").

## Event Catalog

The following event names are reserved and must be supported by the collector and relay:

- `APP_START`: Client application initialization.
- `APP_STOP`: Graceful application shutdown.
- `RELAY_CONNECT`: Successful connection to an Aethos relay.
- `RELAY_DISCONNECT`: Disconnection (clean or unexpected) from a relay.
- `HEARTBEAT`: Periodic liveness signal from the client.
- `GOSSIP_SYNC_START`: Beginning of a LAN gossip encounter.
- `GOSSIP_SYNC_COMPLETE`: Successful completion of a gossip sync round.
- `ENVELOPE_SEND`: Attempt to send an Aethos envelope.
- `ENVELOPE_RECEIVE`: Receipt of an Aethos envelope from a peer or relay.
- `STALL_DETECTED`: Synthetic event triggered by internal watchdog.
- `GUI_STALL`: Detection of unresponsive UI thread.
- `CRASH_REPORT`: Forensics from a previous session crash.

## Stall Detection Rules

- **Interval Watchdog**: Any event with an expected cadence (e.g., `HEARTBEAT`) triggers a `STALL_DETECTED` event if the gap between occurrences exceeds 1.5x the expected interval.
- **UI Liveness**: The GUI thread must emit a internal heartbeat every 1000ms. If 3 consecutive heartbeats are missed, a `GUI_STALL` event is dispatched.

## Privacy Rules

- **No PII**: Events must never include Wayfarer secret keys, passwords, or personal user data (name, email).
- **ID Hashing**: Wayfarer IDs in `payload` fields must be truncated or salted-hashed if the run is marked as "Public".
- **Opt-in**: Diagnostics collection is disabled by default and requires explicit user consent.

## Retention Guidance

- **Standard Runs**: Relays should retain full event timelines for 7 days.
- **Summary Data**: Aggregated run summaries should be retained for 30 days.
- **Critical/Crash**: Events with severity `CRITICAL` or `ERROR` may be pinned for 90 days.
