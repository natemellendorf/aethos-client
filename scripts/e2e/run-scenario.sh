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

exit "${EXIT_CODE}"
