# Canonical BLE Advertiser Proof (Desktop -> iOS)

This runbook validates that the Linux desktop client emits canonical BLE Discovery Identity Contract v1 bytes on-air and that an iOS scanner accepts them.

## Optional artifact helper

To scaffold a timestamped artifact bundle before running the manual steps:

```bash
scripts/collect-ble-proof-artifacts.sh
```

This runbook remains the canonical manual procedure.

## Preconditions

- Linux desktop with BlueZ and an adapter that supports LE advertising.
- iOS client build that logs BLE acceptance/rejection counters.
- Desktop client started from `spikes/tauri-desktop`.
- Legacy name-hint fallback disabled (default):
  - `AETHOS_BLE_ALLOW_LEGACY_NAME_HINTS=0`

## 1) Start desktop with canonical BLE advertising

```bash
cd spikes/tauri-desktop
AETHOS_BLE_ALLOW_LEGACY_NAME_HINTS=0 npm run tauri:dev
```

Expected desktop log line (example):

- `ble_advertiser_started ... primary_ad=11074eee0dd26c0ef787f950295a85a51a18 ... service_data_ad=1d214eee0dd26c0ef787f950295a85a51a18...`

## 2) Verify on-air bytes with `btmon`

In a separate terminal:

```bash
sudo btmon
```

After capturing output to a log file, you can extract likely primary/scan bytes and run the inspector automatically:

```bash
scripts/extract-btmon-ble-identity.sh --input ./btmon.log
```

Optional: pin to a specific advertiser MAC address:

```bash
scripts/extract-btmon-ble-identity.sh --input ./btmon.log --address AA:BB:CC:DD:EE:FF
```

Optional: list every scan-response report seen for that advertiser (helps diagnose missing/empty `SCAN_RSP`):

```bash
scripts/extract-btmon-ble-identity.sh --input ./btmon.log --address AA:BB:CC:DD:EE:FF --scan-report --no-run-inspector
```

Confirm the advertisement set includes:

- UUID list AD (`0x07` or `0x06`) containing `4e ee 0d d2 6c 0e f7 87 f9 50 29 5a 85 a5 1a 18`.
- Service Data AD (`0x21`) keyed by same UUID with exactly 12 payload bytes.

Optional: decode captured bytes with the local inspector:

```bash
cargo run --bin ble-identity-inspector -- \
  --primary 11074eee0dd26c0ef787f950295a85a51a18 \
  --scan 1d214eee0dd26c0ef787f950295a85a51a1801000100d6b6fc2bf0f08cdf
```

The inspector prints AD-structure breakdown and the exact fail-closed rejection reason code if bytes are non-conformant.

## 3) Verify scanner side with `bluetoothctl`

Optional Linux-side sanity check before iOS run:

```bash
bluetoothctl
scan on
```

Confirm remote devices expose the Aethos UUID/service data, and desktop logs avoid `missing primary service UUID` for valid peers.

## 4) Cross-client proof against iOS scanner

Run iOS scanner against the active desktop advertiser and capture logs showing:

- `accepted_count > 0`
- reduction in canonical-signal rejections
- absence of `missing primary service UUID` for the desktop advertiser observations
- BLE-triggered encounter activation event

## 5) Artifact checklist

Capture and archive:

- Desktop logs containing `ble_advertiser_started`, `ble_observation_accepted`, and encounter handoff events.
- iOS logs containing accepted Aethos peer and counter deltas.
- `btmon` trace showing UUID-list AD and `0x21` service-data AD bytes.
- Repro command lines and environment variables used for the run.
