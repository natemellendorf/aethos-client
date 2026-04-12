# BLE Discovery Identity Contract v1

> **Deprecated.** This contract has been superseded by v2 (wakeup-hint-only semantics).
> See the upstream v2 spec: `https://github.com/natemellendorf/aethos/blob/main/docs/protocol/ble-identity-v2.md`
> and ADR: `https://github.com/natemellendorf/aethos/blob/main/docs/adr/ADR-BLE-IDENTITY-V2.md`
>
> This file is retained as a historical reference only.

Status: **deprecated** — replaced by v2 BLE wakeup hint contract.

## 1. Normative language and authority

The key words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as in RFC 2119.

Per `docs/adr/ADR-0001-protocol-contract-source-of-truth.md`, this document is canonical for active BLE discovery identity behavior.

This contract defines only BLE discovery identity advertisement bytes. It does **not** redefine Gossip V1 frame encoding (CBOR remains authoritative for Gossip frames per `docs/protocol/gossip.md` and `docs/protocol/frames.md`).

## 2. Frozen constants (v1)

### 2.1 Primary service UUID

- Primary Aethos Discovery Service UUID (128-bit):
  - `181aa585-5a29-50f9-87f7-0e6cd20dee4e`
- Derivation (frozen): UUIDv5 (RFC 4122) with DNS namespace `6ba7b810-9dad-11d1-80b4-00c04fd430c8` and name `"aethos.ble.discovery.identity.v1"`.
- This is the **primary** UUID for scanner filtering.

### 2.2 Secondary UUIDs

- Additional service UUIDs MAY be present for non-Aethos purposes.
- They MUST NOT replace the primary UUID above for Aethos discovery.
- Scanners implementing this contract MUST key Aethos discovery on the primary UUID above.

## 3. On-air placement and packet budget

Legacy BLE advertising/scan-response PDUs are constrained to 31 bytes each. To preserve filterability and fit identity bytes:

1. Primary advertisement **MUST** include the Aethos primary service UUID in AD type `0x07` (Complete List of 128-bit Service UUIDs) or AD type `0x06` (Incomplete List of 128-bit Service UUIDs).
2. In AD type `0x06`/`0x07`, UUID bytes are concatenated 16-byte entries; list length rule is `(Length - 1) % 16 == 0`.
3. Identity payload bytes **MUST** be carried in AD type `0x21` (Service Data - 128-bit UUID) keyed by the Aethos UUID.
4. For legacy advertising, AD type `0x21` with identity payload **SHOULD** be in scan response.
5. For extended advertising, AD type `0x21` with identity payload MAY be in primary advertising data.
6. Parsers **MUST** search scan response for Aethos AD type `0x21` first, then primary advertising data.
7. Implementations **MUST NOT** require UUID-list AD and service-data AD to coexist in the same 31-byte PDU.

Rationale: AD type `0x21` includes a 16-byte UUID prefix; with legacy 31-byte constraints this leaves at most 13 payload bytes, so v1 freezes a 12-byte payload.

## 4. AD structure encoding

### 4.1 Endianness for UUID bytes in AD structures

For AD type `0x06`, AD type `0x07`, and AD type `0x21`, UUID octets on air are little-endian BLE order.

- UUID canonical text: `181aa585-5a29-50f9-87f7-0e6cd20dee4e`
- UUID bytes in AD payload: `4e ee 0d d2 6c 0e f7 87 f9 50 29 5a 85 a5 1a 18`

### 4.2 Primary advertisement UUID-list requirement

Primary advertisement MUST contain an AD structure:

- `Type`: `0x07` (preferred) or `0x06` (accepted)
- `Length`: MUST satisfy `(Length - 1) % 16 == 0`
- `Data`: one or more concatenated 16-byte little-endian UUIDs
- At least one UUID entry MUST equal the Aethos UUID

Example (single UUID, hex):

`11 07 4e ee 0d d2 6c 0e f7 87 f9 50 29 5a 85 a5 1a 18`

Example (multi-UUID list with Aethos present, hex):

`21 07 ff ee dd cc bb aa 99 88 77 66 55 44 33 22 11 00 4e ee 0d d2 6c 0e f7 87 f9 50 29 5a 85 a5 1a 18`

### 4.3 Identity service-data requirement (`0x21`)

Advertising set (primary + optional scan response) MUST contain Aethos v1 identity bytes in AD type `0x21`:

- `Length`: `0x1D` (29 bytes = 1 type + 16 UUID + 12 payload)
- `Type`: `0x21`
- `Data[0..15]`: 16-byte little-endian Aethos UUID
- `Data[16..27]`: 12-byte identity payload (Section 5)

Uniqueness and duplicate handling:

1. Scanners **MUST** inspect scan response first, then primary advertising data.
2. If no Aethos AD type `0x21` is found across both, parser **MUST** reject.
3. If more than one Aethos AD type `0x21` appears in the same PDU, parser **MUST** reject.
4. If exactly one Aethos AD type `0x21` appears in each PDU (one in scan response + one in primary), payload bytes **MUST** be identical; mismatch **MUST** reject.
5. When both are present and identical, scanner **MUST** parse the scan-response copy as authoritative.

## 5. Identity payload wire format (fixed 12 bytes)

Payload bytes are carried immediately after the 16-byte UUID inside AD type `0x21`.

| Offset | Size | Field | Encoding |
| --- | --- | --- | --- |
| 0 | 1 | `version` | `u8`, MUST be `0x01` |
| 1 | 1 | `flags` | `u8` bitfield |
| 2..3 | 2 | `capabilities` | `u16` little-endian |
| 4..11 | 8 | `identity_ref` | opaque 8-byte value |

Total payload bytes: 12.

### 5.1 `flags` bit assignment (u8)

- Bit 0: `identity_rotating`
- Bit 1: `identity_private`
- Bits 2..7: reserved, MUST be zero in v1

Reserved-flag handling is strict for v1: if bits 2..7 are nonzero, scanners **MUST** reject the payload.

### 5.2 `capabilities` bit assignment (u16 little-endian)

- Bit 0: LAN capability
- Bit 1: MPC capability
- Bit 2: RELAY capability
- Bits 3..15: reserved for future capability assignments

Capability reserved-bit rule (forward compatible):

1. Advertisers **MUST** set reserved bits (3..15) to zero in v1.
2. Scanners **MUST NOT** ascribe meaning to unknown/reserved bits.
3. Scanners **MUST NOT** reject solely because reserved capability bits are set.
4. For dedupe/linkage decisions, scanners SHOULD normalize capabilities as `(capabilities & 0x0007)` and treat bits 3..15 as zero.

### 5.3 `identity_ref` constraints

- Exactly 8 bytes.
- MUST NOT be all zero (`00 00 00 00 00 00 00 00`).
- MUST be treated as opaque by scanners.

## 6. Identity reference derivation

`identity_ref` MUST NOT contain raw public keys, user names, phone numbers, IP addresses, LAN SSIDs, relay URLs, or other directly identifying plaintext fields.

Context bytes for both derivations are ASCII with trailing NUL:

- `"aethos:ble:idref:v1\0"`

Authoritative derivation vectors are in:

- `Fixtures/BLE/identity-v1/vector-stable-derivation.json`
- `Fixtures/BLE/identity-v1/vector-rotating-derivation.json`

### 6.1 Stable mode (`identity_rotating = 0`)

`identity_ref = Trunc8(SHA-256(context || wayfarer_id_bytes))`

Where:

1. `wayfarer_id_bytes` are the 32 raw bytes obtained by hex-decoding the canonical WayfarerID.
2. WayfarerID in active Aethos contracts is lowercase hex SHA-256 of author public key bytes (64 lowercase hex characters).
3. Stable-mode derivation input uses those 32 raw digest bytes directly; implementations MUST NOT hash the UTF-8 hex string characters.
4. `Trunc8` means the first 8 bytes of the 32-byte digest.

### 6.2 Rotating mode (`identity_rotating = 1`)

`identity_ref = Trunc8(HMAC-SHA256(k_ble, context || LE64(epoch)))`

Where:

1. `k_ble` is a local device secret and MUST NOT be broadcast.
2. `epoch = floor(unix_time_seconds / 900)` (15-minute window).
3. `LE64(epoch)` is 8-byte little-endian epoch encoding.
4. `Trunc8` means first 8 bytes of HMAC output.

## 7. Privacy, rotation, and anti-fingerprinting

1. Implementations that require stronger unlinkability SHOULD use rotating mode.
2. Advertisers setting `identity_private=1` SHOULD also set `identity_rotating=1`.
3. When `identity_private=1`, scanners SHOULD avoid long-term linkage beyond short operational windows.
4. Scanners SHOULD avoid persisting or logging raw identity payload bytes when `identity_private=1`.
5. Implementations SHOULD avoid adding stable identifying AD structures (for example static local names) alongside this contract when privacy is requested.
6. Deduplication SHOULD treat `identity_ref` as short-lived and use a rolling 60-second correlation window.
7. Rotating identities MUST be expected to change over time and MUST NOT be treated as permanent node identity.

## 8. Authentication/tag statement for v1

v1 has **no** authentication tag/MAC/signature field inside the BLE discovery payload.

- Clients **MUST NOT** invent extra auth/tag bytes for v1.
- Clients **MUST** follow the exact 12-byte payload layout and parse behavior in this contract.

## 9. Fail-closed parsing behavior

Scanner/parser behavior for v1 MUST fail closed when any required condition is violated.

### 9.1 Required acceptance checks

1. Primary advertisement includes AD type `0x06` or `0x07` whose UUID list contains the exact Aethos UUID.
2. For AD type `0x06`/`0x07`, `(Length - 1) % 16 == 0`.
3. At least one AD type `0x21` with Aethos UUID exists in scan response or primary advertising data.
4. No PDU contains multiple Aethos AD type `0x21` entries.
5. If both scan response and primary contain Aethos AD type `0x21`, payload bytes are identical.
6. AD type `0x21` payload length after UUID is exactly 12 bytes.
7. `version == 0x01`.
8. `flags` reserved bits (2..7) are all zero.
9. `identity_ref` is not all zero.

If any check fails, parser MUST reject this BLE identity payload.

### 9.2 Unknown version handling

Unknown or unsupported `version` values MUST be rejected (fail closed). Parsers MUST NOT attempt heuristic fallback.

## 10. Conformance guidance

A conforming implementation MUST:

1. Advertisers: include Aethos UUID in AD type `0x07` or `0x06` UUID lists and set reserved capability bits (3..15) to zero.
2. Advertisers: emit AD type `0x21` with exact Aethos UUID + 12-byte payload bytes.
3. Scanners: enforce fail-closed checks in Section 9.
4. Scanners: interpret capability bits 0..2 as defined, tolerate unknown capability bits for forward compatibility, and mask bits 3..15 to zero for dedupe/linkage.
5. Keep BLE payload minimal and non-CBOR; explicit version byte governs parsing.

Authoritative conformance vectors are in `Fixtures/BLE/identity-v1/`.
