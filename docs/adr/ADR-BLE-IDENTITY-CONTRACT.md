# ADR: BLE Discovery Identity Contract v1 Placement and Budget

- Status: **Superseded** — replaced by v2 wakeup-hint-only semantics
- Date: 2026-04-07
- Superseded by: `https://github.com/natemellendorf/aethos/blob/main/docs/adr/ADR-BLE-IDENTITY-V2.md`

## Context

BLE legacy advertising uses separate 31-byte budgets for primary advertisement and scan response.

Aethos discovery needs both:

1. deterministic Aethos-only filtering by service UUID, and
2. identity payload bytes suitable for runtime dedupe/capability hints.

With Service Data AD type `0x21`, 16 bytes are consumed by the 128-bit UUID before payload bytes. Requiring both UUID-list filtering and full service-data payload in one 31-byte packet is fragile and can fail across stacks.

## Decision

`docs/protocol/ble-identity-v1.md` is the canonical contract for BLE identity bytes. It freezes:

1. Primary UUID in AD type `0x07` (or `0x06`) UUID lists for filtering in primary advertisement.
2. Identity payload in AD type `0x21` in scan response for legacy mode, with extended-advertising allowance in primary advertising data.
3. Fixed payload length of 12 bytes (`version`, `flags`, `capabilities`, `identity_ref`).
4. `identity_ref` length of 8 bytes with stable and rotating derivation modes.
5. Fail-closed parser requirements for length/version/reserved-flag/all-zero-id checks.

Extended advertising MAY place the same AD type `0x21` bytes in primary advertisement when space allows, but bytes and parsing are unchanged.

## Consequences

1. Aethos filtering remains deterministic via UUID list scanning.
2. Legacy packet-size limits are respected without reducing parser strictness.
3. Identity bytes remain minimal and privacy-aware.
4. Implementations have one frozen v1 wire shape and conformance fixture suite.
