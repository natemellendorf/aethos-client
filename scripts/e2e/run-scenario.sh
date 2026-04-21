#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DESKTOP_DIR="${ROOT_DIR}/spikes/tauri-desktop"
SCENARIO="clean"
MODE="peer"
RUN_ID="run-$(date +%s)-$RANDOM"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scenario)
      SCENARIO="$2"
      shift 2
      ;;
    --mode)
      MODE="$2"
      shift 2
      ;;
    --run-id)
      RUN_ID="$2"
      shift 2
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

ARTIFACT_DIR="${ROOT_DIR}/tests/e2e-harness/artifacts/${RUN_ID}"
E2E_WORKDIR="${DESKTOP_DIR}/e2e/workdir/${RUN_ID}"
mkdir -p "${ARTIFACT_DIR}"
mkdir -p "${E2E_WORKDIR}"

export AETHOS_E2E_RUN_ID="${RUN_ID}"
export AETHOS_E2E_TEST_CASE_ID="${MODE}-${SCENARIO}"
export AETHOS_E2E_SCENARIO="${SCENARIO}"
export AETHOS_E2E_ARTIFACT_DIR="${ARTIFACT_DIR}"
export AETHOS_E2E_WORKDIR="${E2E_WORKDIR}"
export AETHOS_STRUCTURED_LOGS="${AETHOS_STRUCTURED_LOGS:-1}"
export AETHOS_E2E_BURST_COUNT="${AETHOS_E2E_BURST_COUNT:-1}"
export AETHOS_E2E_BURST_INTERVAL_MS="${AETHOS_E2E_BURST_INTERVAL_MS:-1000}"
export AETHOS_E2E_BURST_RECEIVE_TIMEOUT_MS="${AETHOS_E2E_BURST_RECEIVE_TIMEOUT_MS:-600000}"
export AETHOS_E2E_TEST_TIMEOUT_MS="${AETHOS_E2E_TEST_TIMEOUT_MS:-1200000}"

if [[ -n "${AETHOS_DIAGNOSTICS_COLLECTOR_URL:-}" ]]; then
  export AETHOS_DIAGNOSTICS_COLLECTOR_URL="${AETHOS_DIAGNOSTICS_COLLECTOR_URL%/}"
  export AETHOS_DIAGNOSTICS_RUN_ID="${RUN_ID}"
  if ! python3 - "${AETHOS_DIAGNOSTICS_COLLECTOR_URL}" "${RUN_ID}" "${SCENARIO}" "${MODE}" <<'PY'
import json
import sys
import urllib.request

base, run_id, scenario, mode = sys.argv[1:5]
payload = {
    "requested_run_id": run_id,
    "app": "aethos-desktop",
    "platform": "linux",
    "scenario": scenario,
    "test_case_id": f"{mode}-{scenario}",
    "metadata": {"mode": mode, "source": "scripts/e2e/run-scenario.sh"},
    "ttl_seconds": 86400,
}
req = urllib.request.Request(
    f"{base}/api/v1/diagnostics/runs",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=2.0) as resp:
    body = json.load(resp)
if body.get("run_id") != run_id:
    raise SystemExit(f"collector returned unexpected run_id: {body}")
PY
  then
    echo "warning: diagnostics collector unavailable during run creation" >&2
  fi
fi

case "${MODE}" in
  relay)
    export AETHOS_E2E_DISABLE_RELAY="0"
    export AETHOS_E2E_LOOPBACK_ONLY="0"
    export AETHOS_E2E_LOCALHOST_FANOUT="0"
    export AETHOS_E2E_EAGER_UNICAST="0"
    export AETHOS_E2E_DISABLE_LAN_TCP="0"
    ;;
  peer)
    export AETHOS_E2E_DISABLE_RELAY="1"
    export AETHOS_E2E_LOOPBACK_ONLY="1"
    export AETHOS_E2E_LOCALHOST_FANOUT="1"
    export AETHOS_E2E_EAGER_UNICAST="1"
    export AETHOS_E2E_DISABLE_LAN_TCP="1"
    ;;
  mixed)
    export AETHOS_E2E_DISABLE_RELAY="0"
    export AETHOS_E2E_LOOPBACK_ONLY="1"
    export AETHOS_E2E_LOCALHOST_FANOUT="1"
    export AETHOS_E2E_EAGER_UNICAST="1"
    export AETHOS_E2E_DISABLE_LAN_TCP="1"
    ;;
  *)
    echo "invalid mode: ${MODE} (expected relay|peer|mixed)" >&2
    exit 1
    ;;
esac

if [[ -z "${AETHOS_E2E_RELAY_ENDPOINT:-}" && ( "${MODE}" == "relay" || "${MODE}" == "mixed" ) ]]; then
  AETHOS_E2E_RELAY_ENDPOINT="http://127.0.0.1:8082"
fi
if [[ -n "${AETHOS_E2E_RELAY_ENDPOINT:-}" ]]; then
  export AETHOS_E2E_RELAY_ENDPOINT
fi

PRECHECK_EXIT=0

if [[ "${SCENARIO}" != "clean" ]]; then
  if [[ -n "${AETHOS_E2E_TOXIPROXY_URL:-}" ]]; then
    if [[ ! -f "${ROOT_DIR}/tests/e2e-harness/config/scenarios/${SCENARIO}.json" ]]; then
      echo "scenario file missing: tests/e2e-harness/config/scenarios/${SCENARIO}.json" >&2
      exit 1
    fi
    if ! python3 "${ROOT_DIR}/scripts/e2e/toxiproxy_apply.py" \
      --scenario-file "${ROOT_DIR}/tests/e2e-harness/config/scenarios/${SCENARIO}.json" \
      --toxiproxy-url "${AETHOS_E2E_TOXIPROXY_URL}" \
      --strict; then
      PRECHECK_EXIT=2
      python3 "${ROOT_DIR}/scripts/e2e/index_artifacts.py" \
        --artifact-dir "${ARTIFACT_DIR}" \
        --run-id "${RUN_ID}" \
        --scenario "${SCENARIO}" \
        --mode "${MODE}" \
        --exit-code "${PRECHECK_EXIT}"
      cat > "${ARTIFACT_DIR}/failure-summary.json" <<EOF
{
  "run_id": "${RUN_ID}",
  "scenario": "${SCENARIO}",
  "mode": "${MODE}",
  "failure": "toxiproxy unavailable for strict scenario setup",
  "toxiproxy_url": "${AETHOS_E2E_TOXIPROXY_URL}"
}
EOF
      exit ${PRECHECK_EXIT}
    fi
  fi
fi

set +e
cd "${DESKTOP_DIR}"
npm run e2e
EXIT_CODE=$?
set -e

python3 "${ROOT_DIR}/scripts/e2e/index_artifacts.py" \
  --artifact-dir "${ARTIFACT_DIR}" \
  --run-id "${RUN_ID}" \
  --scenario "${SCENARIO}" \
  --mode "${MODE}" \
  --exit-code "${EXIT_CODE}"

python3 "${ROOT_DIR}/scripts/e2e/summarize_logs.py" \
  --artifact-dir "${ARTIFACT_DIR}" || true

if [[ -n "${AETHOS_DIAGNOSTICS_COLLECTOR_URL:-}" ]]; then
  if ! python3 - "${AETHOS_DIAGNOSTICS_COLLECTOR_URL}" "${RUN_ID}" "${ARTIFACT_DIR}" <<'PY'
import json
import pathlib
import sys
import urllib.request

base, run_id, artifact_dir = sys.argv[1:4]
artifact_root = pathlib.Path(artifact_dir)
artifact_root.mkdir(parents=True, exist_ok=True)

def fetch_json(url: str):
    with urllib.request.urlopen(url, timeout=4.0) as resp:
        return json.load(resp)

run = fetch_json(f"{base}/api/v1/diagnostics/runs/{run_id}")
timeline = fetch_json(f"{base}/api/v1/diagnostics/runs/{run_id}/timeline")
summary = fetch_json(f"{base}/api/v1/diagnostics/runs/{run_id}/summary")

(artifact_root / "diagnostics-run.json").write_text(json.dumps(run, indent=2), encoding="utf-8")
(artifact_root / "diagnostics-timeline.json").write_text(json.dumps(timeline, indent=2), encoding="utf-8")
(artifact_root / "diagnostics-summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

bundle = {
    "run_id": run_id,
    "protocol_phase_reached": summary.get("highest_protocol_phase"),
    "item_ids_sent": summary.get("item_ids_sent", []),
    "item_ids_received": summary.get("item_ids_received", []),
    "missing_transitions": summary.get("missing_transitions", []),
    "top_errors": summary.get("top_errors", []),
}
(artifact_root / "diagnostics-result-bundle.json").write_text(
    json.dumps(bundle, indent=2),
    encoding="utf-8",
)
PY
  then
    echo "warning: failed fetching diagnostics timeline/summary" >&2
  fi
fi

exit "${EXIT_CODE}"
