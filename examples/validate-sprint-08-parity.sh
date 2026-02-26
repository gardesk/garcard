#!/usr/bin/env bash
set -euo pipefail

REPORT_FILE="${GARCARD_SPRINT08_REPORT_FILE:-${PWD}/target/sprint-08-parity-evidence.md}"
RUN_INTERACTIVE="${GARCARD_SPRINT08_RUN_INTERACTIVE:-0}"
ACTION_ID="${GARCARD_SPRINT08_ACTION_ID:-com.mesonbuild.install.run}"

mkdir -p "$(dirname "${REPORT_FILE}")"

timestamp() {
  date -u +"%Y-%m-%dT%H:%M:%SZ"
}

append_section() {
  local heading="$1"
  {
    echo
    echo "## ${heading}"
    echo
  } >>"${REPORT_FILE}"
}

{
  echo "# Sprint 08 Parity Evidence"
  echo
  echo "- generated_at: $(timestamp)"
  echo "- host: $(hostname)"
  echo "- action_id: ${ACTION_ID}"
  echo
} >"${REPORT_FILE}"

append_section "Workspace Tests"
cargo test --workspace | tee -a "${REPORT_FILE}"

append_section "Sprint 04 Reliability Baseline"
./examples/validate-sprint-04.sh | tee -a "${REPORT_FILE}"

append_section "Sprint 07 Lifecycle Baseline (Non-Interactive)"
GARCARD_SPRINT07_RUN_PKCHECK=0 ./examples/validate-sprint-07.sh | tee -a "${REPORT_FILE}"

if [[ "${RUN_INTERACTIVE}" == "1" ]]; then
  append_section "Sprint 07 Lifecycle Interactive Loop"
  if command -v pkcheck >/dev/null 2>&1; then
    GARCARD_SPRINT07_RUN_PKCHECK=1 \
      GARCARD_SPRINT07_ACTION_ID="${ACTION_ID}" \
      ./examples/validate-sprint-07.sh | tee -a "${REPORT_FILE}"
  else
    echo "pkcheck not found; interactive loop skipped" | tee -a "${REPORT_FILE}"
  fi
else
  append_section "Interactive Loop Status"
  echo "Skipped interactive parity loop (set GARCARD_SPRINT08_RUN_INTERACTIVE=1 to enable)." \
    | tee -a "${REPORT_FILE}"
fi

append_section "Next Manual Matrix Steps"
{
  echo "1. Open examples/sprint-08-parity-matrix.md."
  echo "2. Record PASS/FAIL and attach evidence pointers from this report."
  echo "3. Add daemon log references for success/failure/cancel/timeout and retention coverage."
} | tee -a "${REPORT_FILE}"

echo "Sprint 08 parity baseline complete."
echo "Evidence report: ${REPORT_FILE}"
