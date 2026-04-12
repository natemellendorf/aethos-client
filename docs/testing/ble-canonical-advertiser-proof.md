# Canonical BLE Advertiser Proof (Desktop -> iOS)

This runbook validates that the Linux desktop client emits a v2 BLE wakeup hint (UUID-only, no identity payload) on-air and that a scanner accepts it.

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

## 1) Start desktop with canonical BLE advertising

```bash
cd spikes/tauri-desktop
npm run tauri:dev
```

Expected desktop log line (example):

- `ble_advertiser_started source=bluez mode=uuid-only-0x07 uuid=181aa585-... primary_ad=11074eee0dd26c0ef787f950295a85a51a18`

In v2 the log no longer includes `service_data_ad`, `payload`, or `identity_ref` fields.

## 2) Verify on-air bytes with `btmon`

In a separate terminal:

```bash
sudo btmon
```

After capturing output to a log file, you can extract likely primary bytes and run the inspector:

```bash
scripts/extract-btmon-ble-identity.sh --input ./btmon.log
```

Optional: pin to a specific advertiser MAC address:

```bash
scripts/extract-btmon-ble-identity.sh --input ./btmon.log --address AA:BB:CC:DD:EE:FF
```

Confirm the advertisement includes:

- UUID list AD (`0x07` or `0x06`) containing `4e ee 0d d2 6c 0e f7 87 f9 50 29 5a 85 a5 1a 18`.
- **No** Service Data AD (`0x21`) keyed by Aethos UUID (v2 forbids it per §5.2).

Optional: decode captured bytes with the local inspector:

```bash
cargo run --bin ble-identity-inspector -- \
  --primary 11074eee0dd26c0ef787f950295a85a51a18
```

The inspector prints AD-structure breakdown and the exact rejection reason code if bytes are non-conformant.

## 3) Verify scanner side with `bluetoothctl`

Optional Linux-side sanity check before iOS run:

```bash
bluetoothctl
scan on
```

Confirm remote devices expose the Aethos UUID, and desktop logs show `ble_wakeup_hint_accepted` for valid peers.

## 4) Cross-client proof against iOS scanner

Run iOS scanner against the active desktop advertiser and capture logs showing:

- `accepted_count > 0`
- absence of `missing primary service UUID` for the desktop advertiser observations
- BLE-triggered encounter activation event

## 5) Artifact checklist

Capture and archive:

- Desktop logs containing `ble_advertiser_started`, `ble_wakeup_hint_accepted`, and encounter handoff events.
- iOS logs containing accepted Aethos peer and counter deltas.
- `btmon` trace showing UUID-list AD bytes (no `0x21` service data expected).
- Repro command lines and environment variables used for the run.
