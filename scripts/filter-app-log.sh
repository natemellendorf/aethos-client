#!/usr/bin/env bash
set -euo pipefail

PATTERN=""
FOLLOW=0
MODE=""

usage() {
  cat <<'EOF'
usage: scripts/filter-app-log.sh [--pattern <regex>] [--mode ble|error|relay] [--follow]

Filters Aethos desktop app log with grep-friendly presets.

Options:
  --pattern <regex>    Custom grep pattern (extended regex)
  --mode <name>        Preset pattern: ble | error | relay
  --follow             Follow log file as it grows
  -h, --help           Show this help

Examples:
  scripts/filter-app-log.sh --mode ble
  scripts/filter-app-log.sh --mode error --follow
  scripts/filter-app-log.sh --pattern "ble_advertiser_started|ble_advertiser_error"
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --pattern)
      [[ $# -ge 2 ]] || { echo "--pattern requires a value" >&2; exit 1; }
      PATTERN="$2"
      shift 2
      ;;
    --mode)
      [[ $# -ge 2 ]] || { echo "--mode requires a value" >&2; exit 1; }
      MODE="$2"
      shift 2
      ;;
    --follow)
      FOLLOW=1
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

if [[ -z "$PATTERN" ]]; then
  case "$MODE" in
    ble)
      PATTERN='ble_advertiser_started|ble_advertiser_error|ble_observation_(accepted|rejected|ignored_self)|encounter_ble_handoff'
      ;;
    error)
      PATTERN='\[ERROR\]|failed|error|rejected|timeout|Did not receive a reply'
      ;;
    relay)
      PATTERN='relay_worker_|relay_session_|relay_encounter_|relay_frame_(send|recv)|relay_socket_'
      ;;
    "")
      echo "provide --pattern or --mode" >&2
      usage >&2
      exit 1
      ;;
    *)
      echo "unsupported --mode: $MODE" >&2
      exit 1
      ;;
  esac
fi

LOG_PATH=""
if [[ -n "${XDG_STATE_HOME:-}" ]]; then
  LOG_PATH="${XDG_STATE_HOME}/aethos-linux/aethos-linux.log"
elif [[ -n "${HOME:-}" ]]; then
  LOG_PATH="${HOME}/.local/state/aethos-linux/aethos-linux.log"
else
  LOG_PATH="/tmp/aethos-linux.log"
fi

if [[ ! -f "$LOG_PATH" ]]; then
  echo "app log not found: $LOG_PATH" >&2
  exit 1
fi

if [[ "$FOLLOW" == "1" ]]; then
  tail -f "$LOG_PATH" | grep -E --line-buffered "$PATTERN"
else
  grep -E "$PATTERN" "$LOG_PATH"
fi
