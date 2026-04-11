#!/usr/bin/env bash
set -euo pipefail

ADAPTER_PATH="/org/bluez/hci0"
DO_PKILL=1
DO_POWER_CYCLE=1
DO_RESTART_DAEMON=1

usage() {
  cat <<'EOF'
usage: scripts/reset-ble-advertising.sh [--adapter hci0] [--no-pkill] [--no-power-cycle] [--no-restart-daemon]

Resets local BLE advertising preconditions for Aethos desktop debugging.

What it does:
  1) Stops local desktop app processes (pkill patterns)
  2) Power-cycles adapter via bluetoothctl (off/on)
  3) Prints BlueZ advertising capacity via busctl

Options:
  --adapter <name>    Adapter name (default: hci0)
  --no-pkill          Skip process cleanup
  --no-power-cycle    Skip bluetoothctl power off/on
  --no-restart-daemon Skip `systemctl restart bluetooth`
  -h, --help          Show this help

Examples:
  scripts/reset-ble-advertising.sh
  scripts/reset-ble-advertising.sh --adapter hci1
EOF
}

log() {
  printf '[ble-reset] %s\n' "$*"
}

fail() {
  printf '[ble-reset] ERROR: %s\n' "$*" >&2
  exit 1
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --adapter)
        [[ $# -ge 2 ]] || fail "--adapter requires a value"
        ADAPTER_PATH="/org/bluez/$2"
        shift 2
        ;;
      --no-pkill)
        DO_PKILL=0
        shift
        ;;
      --no-power-cycle)
        DO_POWER_CYCLE=0
        shift
        ;;
      --no-restart-daemon)
        DO_RESTART_DAEMON=0
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        fail "unknown option: $1"
        ;;
    esac
  done
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

cleanup_processes() {
  log "stopping local desktop processes"
  pkill -f target/debug/aethos >/dev/null 2>&1 || true
  pkill -f "tauri dev" >/dev/null 2>&1 || true
  sleep 1
}

power_cycle_adapter() {
  log "power cycling adapter via bluetoothctl"
  sudo bluetoothctl power off >/dev/null || true
  sleep 1
  sudo bluetoothctl power on >/dev/null || true
}

restart_bluetooth_daemon() {
  log "restarting bluetooth daemon"
  sudo systemctl restart bluetooth
}

print_capacity() {
  local active supported active_count available_count
  active="$(busctl get-property org.bluez "$ADAPTER_PATH" org.bluez.LEAdvertisingManager1 ActiveInstances 2>/dev/null || true)"
  supported="$(busctl get-property org.bluez "$ADAPTER_PATH" org.bluez.LEAdvertisingManager1 SupportedInstances 2>/dev/null || true)"

  if [[ -z "$active" || -z "$supported" ]]; then
    fail "failed reading LEAdvertisingManager1 properties for $ADAPTER_PATH"
  fi

  log "active_instances:    $active"
  log "available_instances: $supported"

  active_count="${active##* }"
  available_count="${supported##* }"
  if [[ "$available_count" =~ ^[0-9]+$ && "$active_count" =~ ^[0-9]+$ ]]; then
    if [[ "$available_count" -eq 0 ]]; then
      fail "adapter reports no available advertiser slots (active=${active_count}, available=${available_count})"
    fi
  fi
}

main() {
  parse_args "$@"
  require_cmd pkill
  require_cmd bluetoothctl
  require_cmd busctl
  require_cmd sudo
  require_cmd systemctl

  if [[ "$DO_PKILL" == "1" ]]; then
    cleanup_processes
  else
    log "skipping process cleanup"
  fi

  if [[ "$DO_POWER_CYCLE" == "1" ]]; then
    power_cycle_adapter
  else
    log "skipping adapter power cycle"
  fi

  if [[ "$DO_RESTART_DAEMON" == "1" ]]; then
    restart_bluetooth_daemon
  else
    log "skipping bluetooth daemon restart"
  fi

  print_capacity
  log "done"
}

main "$@"
