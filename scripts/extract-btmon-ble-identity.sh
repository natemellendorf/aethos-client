#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
usage: scripts/extract-btmon-ble-identity.sh --input <btmon.log> [--address <MAC>] [--scan-report] [--no-run-inspector]

Extracts the latest primary advertisement from btmon text output,
then (by default) runs `ble-identity-inspector` with the recovered bytes.

In v2 only the primary advertisement is needed (UUID-only wakeup hint).
Scan response data is no longer required.

When `--scan-report` is set, prints a summary of all scan-response reports for the
selected address (line number, event type, payload length, Aethos-UUID presence).

Examples:
  scripts/extract-btmon-ble-identity.sh --input tests/e2e-harness/artifacts/ble-proof/<run-id>/btmon.log
  scripts/extract-btmon-ble-identity.sh --input ./btmon.log --address AA:BB:CC:DD:EE:FF
  scripts/extract-btmon-ble-identity.sh --input ./btmon.log --address AA:BB:CC:DD:EE:FF --scan-report --no-run-inspector
EOF
}

INPUT_PATH=""
ADDRESS_FILTER=""
RUN_INSPECTOR=1
SCAN_REPORT=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --input)
      INPUT_PATH="$2"
      shift 2
      ;;
    --address)
      ADDRESS_FILTER="$2"
      shift 2
      ;;
    --no-run-inspector)
      RUN_INSPECTOR=0
      shift
      ;;
    --scan-report)
      SCAN_REPORT=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "${INPUT_PATH}" ]]; then
  echo "missing required --input" >&2
  usage >&2
  exit 1
fi

if [[ ! -f "${INPUT_PATH}" ]]; then
  echo "btmon log not found: ${INPUT_PATH}" >&2
  exit 1
fi

python3 - <<'PY' "$ROOT_DIR" "$INPUT_PATH" "$ADDRESS_FILTER" "$RUN_INSPECTOR" "$SCAN_REPORT"
import pathlib
import re
import subprocess
import sys

root_dir = pathlib.Path(sys.argv[1])
input_path = pathlib.Path(sys.argv[2])
address_filter = sys.argv[3].strip().upper()
run_inspector = sys.argv[4] == "1"
scan_report = sys.argv[5] == "1"

HEX_PAIR_RE = re.compile(r"^[0-9A-Fa-f]{2}$")
LINE_HEX_PAIRS_RE = re.compile(r"^\s*[0-9A-Fa-f]{2}(?:\s+[0-9A-Fa-f]{2})*\s*$")
ADDRESS_RE = re.compile(r"^\s*Address:\s*([0-9A-Fa-f:]{17})")
EVENT_TYPE_RE = re.compile(r"^\s*Event type:\s*(.+?)\s*$")
DATA_LINE_RE = re.compile(r"^\s*Data:\s*(.*?)\s*$")
DATA_LENGTH_RE = re.compile(r"^\s*Data length:\s*(?:0x[0-9A-Fa-f]+|\d+)")
AETHOS_UUID_LE_HEX = "4eee0dd26c0ef787f950295a85a51a18"


def hex_pairs_from_fragment(fragment: str):
    out = []
    for token in fragment.replace("\t", " ").split():
        if HEX_PAIR_RE.match(token):
            out.append(token.lower())
    return out


def leading_hex_pairs_from_line(line: str):
    out = []
    for token in line.strip().replace("\t", " ").split():
        if HEX_PAIR_RE.match(token):
            out.append(token.lower())
            continue
        break
    return out


def is_scan_response(event_type: str) -> bool:
    upper = event_type.upper()
    return "SCAN_RSP" in upper or "SCAN RESPONSE" in upper or "AUX_SCAN_RSP" in upper


def has_aethos_uuid(data_hex: str) -> bool:
    return AETHOS_UUID_LE_HEX in data_hex.lower()


def ad_types(data_hex: str):
    out = []
    raw = bytes.fromhex(data_hex) if data_hex else b""
    cursor = 0
    while cursor < len(raw):
        length = raw[cursor]
        cursor += 1
        if length == 0:
            break
        if cursor + length > len(raw):
            break
        ad_type = raw[cursor]
        out.append(ad_type)
        cursor += length
    return out


def format_ad_types(types):
    if not types:
        return "none"
    return ",".join([f"0x{value:02x}" for value in types])


def parse_reports(raw_lines):
    reports = []
    current = None
    collecting_data = False

    def flush():
        nonlocal current, collecting_data
        if current and current["data_hex"]:
            reports.append(current)
        current = None
        collecting_data = False

    for line_no, line in enumerate(raw_lines, start=1):
        if "LE Advertising Report" in line or "LE Extended Advertising Report" in line:
            flush()
            current = {
                "line_no": line_no,
                "event_type": "",
                "address": "",
                "data_hex": "",
            }
            continue

        if current is None:
            continue

        if line.startswith("> HCI Event:") or line.startswith("@ MGMT Event:"):
            flush()
            continue

        m = EVENT_TYPE_RE.match(line)
        if m:
            current["event_type"] = m.group(1).strip()
            collecting_data = False
            continue

        m = ADDRESS_RE.match(line)
        if m:
            current["address"] = m.group(1).upper()
            collecting_data = False
            continue

        m = DATA_LINE_RE.match(line)
        if m:
            pairs = hex_pairs_from_fragment(m.group(1))
            if pairs:
                current["data_hex"] += "".join(pairs)
                collecting_data = True
            continue

        if DATA_LENGTH_RE.match(line):
            collecting_data = True
            continue

        if collecting_data:
            pairs = leading_hex_pairs_from_line(line)
            if pairs:
                current["data_hex"] += "".join(pairs)
                continue

        collecting_data = False

    flush()
    return reports


raw = input_path.read_text(encoding="utf-8", errors="replace").splitlines()
reports = parse_reports(raw)
all_reports = reports

if address_filter:
    reports = [r for r in reports if r["address"] == address_filter]

if not reports:
    if all_reports and address_filter:
        seen_addresses = sorted({r["address"] for r in all_reports if r["address"]})
        print(
            f"No advertising reports matched --address {address_filter} in {input_path}",
            file=sys.stderr,
        )
        preview = ", ".join(seen_addresses[:8]) if seen_addresses else "(none)"
        print(
            f"Seen advertiser addresses in this capture ({len(seen_addresses)}): {preview}",
            file=sys.stderr,
        )
        print(
            "Tip: many devices use resolvable random addresses that rotate; run without --address first.",
            file=sys.stderr,
        )
        sys.exit(1)
    print(f"No advertising reports with raw Data bytes found in {input_path}", file=sys.stderr)
    print("Tip: ensure btmon text output includes `Data:` lines (for example: `sudo btmon | tee btmon.log`).", file=sys.stderr)
    sys.exit(1)

primary = None
primary_idx = -1
scan = None

for i in range(len(reports) - 1, -1, -1):
    candidate = reports[i]
    if is_scan_response(candidate["event_type"]):
        continue
    if has_aethos_uuid(candidate["data_hex"]):
        primary = candidate
        primary_idx = i
        break

if primary is None:
    for i in range(len(reports) - 1, -1, -1):
        candidate = reports[i]
        if not is_scan_response(candidate["event_type"]):
            primary = candidate
            primary_idx = i
            break

if primary is None:
    print("Could not locate a primary advertising report with raw Data bytes.", file=sys.stderr)
    sys.exit(1)

if primary["address"]:
    for j in range(primary_idx + 1, len(reports)):
        follower = reports[j]
        if follower["address"] != primary["address"]:
            continue
        if not is_scan_response(follower["event_type"]):
            continue
        if has_aethos_uuid(follower["data_hex"]):
            scan = follower
            break
        if scan is None:
            scan = follower

scan_hex = scan["data_hex"] if scan else ""

if primary and not has_aethos_uuid(primary["data_hex"]):
    print(
        "Warning: selected primary report does not include Aethos UUID bytes. "
        "Try --address <MAC> if you know your desktop advertiser address."
    )

print("btmon BLE extraction")
print(f"- input={input_path}")
if address_filter:
    print(f"- address_filter={address_filter}")
print(
    f"- selected_primary=line:{primary['line_no']} address:{primary['address'] or 'unknown'} event:{primary['event_type'] or 'unknown'} len:{len(primary['data_hex']) // 2}"
)
if scan:
    print(
        f"- selected_scan=line:{scan['line_no']} address:{scan['address'] or 'unknown'} event:{scan['event_type'] or 'unknown'} len:{len(scan_hex) // 2} (not used by v2 inspector)"
    )
else:
    print("- selected_scan=none (not required for v2 wakeup hint acceptance)")

print(f"- primary_hex={primary['data_hex']}")
print(f"- scan_hex={scan_hex}")
primary_types = ad_types(primary["data_hex"])
scan_types = ad_types(scan_hex)
print(f"- primary_ad_types={format_ad_types(primary_types)}")
print(f"- primary_has_0x07={0x07 in primary_types}")
print(f"- primary_has_0x21={0x21 in primary_types}")
print(f"- scan_ad_types={format_ad_types(scan_types)}")
print(f"- scan_has_0x07={0x07 in scan_types}")
print(f"- scan_has_0x21={0x21 in scan_types}")

scan_report_address = address_filter or primary["address"]
if scan_report and scan_report_address:
    matching = [
        r
        for r in reports
        if r["address"] == scan_report_address and is_scan_response(r["event_type"])
    ]
    print(f"- scan_report_address={scan_report_address}")
    print(f"- scan_report_count={len(matching)}")
    for idx, entry in enumerate(matching, start=1):
        data_hex = entry["data_hex"]
        short_hex = data_hex if len(data_hex) <= 64 else f"{data_hex[:64]}..."
        print(
            f"  scan[{idx}] line:{entry['line_no']} event:{entry['event_type'] or 'unknown'} len:{len(data_hex)//2} has_aethos_uuid:{has_aethos_uuid(data_hex)} ad_types:{format_ad_types(ad_types(data_hex))} data_hex:{short_hex}"
        )
elif scan_report:
    print("- scan_report_address=unknown")
    print("- scan_report_count=0")

inspector_cmd = [
    "cargo",
    "run",
    "--bin",
    "ble-identity-inspector",
    "--",
    "--primary",
    primary["data_hex"],
]
if primary["address"]:
    inspector_cmd.extend(["--address", primary["address"]])

print("- inspector_cmd=" + " ".join(inspector_cmd))

if run_inspector:
    print("\nRunning inspector...\n")
    completed = subprocess.run(inspector_cmd, cwd=root_dir)
    sys.exit(completed.returncode)
PY
