#!/usr/bin/env bash
set -euo pipefail

SOCKET_PATH="${GARCARD_SPRINT07_SOCKET:-${PWD}/target/garcard-sprint07.sock}"
BACKEND="${GARCARD_SPRINT07_BACKEND:-polkit}"
ACTION_ID="${GARCARD_SPRINT07_ACTION_ID:-com.mesonbuild.install.run}"
AUTH_CYCLES="${GARCARD_SPRINT07_AUTH_CYCLES:-3}"
LOG_FILE="${GARCARD_SPRINT07_LOG:-${PWD}/target/garcard-sprint07.log}"
RUN_PKCHECK="${GARCARD_SPRINT07_RUN_PKCHECK:-1}"
POLKIT_RESTART_CMD="${GARCARD_SPRINT07_POLKIT_RESTART_CMD:-}"

if command -v garcard >/dev/null 2>&1; then
  DAEMON_CMD=(garcard daemon)
else
  DAEMON_CMD=(cargo run -q -p garcard -- daemon)
fi

if command -v garcardctl >/dev/null 2>&1; then
  CTL_CMD=(garcardctl)
else
  CTL_CMD=(cargo run -q -p garcardctl --)
fi

DAEMON_PID=0

run_ctl() {
  GARCARD_SOCKET="${SOCKET_PATH}" "${CTL_CMD[@]}" "$@"
}

wait_for_daemon() {
  local tries=120
  while (( tries > 0 )); do
    if run_ctl ping >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
    tries=$((tries - 1))
  done
  echo "daemon did not become ready in time"
  return 1
}

start_daemon() {
  GARCARD_SOCKET="${SOCKET_PATH}" \
    GARCARD_AGENT_BACKEND="${BACKEND}" \
    "${DAEMON_CMD[@]}" >>"${LOG_FILE}" 2>&1 &
  DAEMON_PID=$!
  wait_for_daemon
}

stop_daemon() {
  if [[ "${DAEMON_PID}" -gt 0 ]] && kill -0 "${DAEMON_PID}" 2>/dev/null; then
    run_ctl quit >/dev/null 2>&1 || true
    wait "${DAEMON_PID}" 2>/dev/null || true
  fi
  DAEMON_PID=0
}

cleanup() {
  stop_daemon
  rm -f "${SOCKET_PATH}"
}
trap cleanup EXIT

mkdir -p "$(dirname "${SOCKET_PATH}")"
mkdir -p "$(dirname "${LOG_FILE}")"
rm -f "${SOCKET_PATH}" "${LOG_FILE}"

echo "Sprint 07 lifecycle validation"
echo "  socket: ${SOCKET_PATH}"
echo "  backend: ${BACKEND}"
echo "  action: ${ACTION_ID}"
echo "  log: ${LOG_FILE}"

start_daemon

echo "[1/5] Baseline lifecycle and diagnostics surface"
run_ctl status
run_ctl auth-summary
run_ctl diagnose
run_ctl temp-list || true

echo "[2/5] Repeated auth + revocation loop (${AUTH_CYCLES} iterations)"
if [[ "${RUN_PKCHECK}" == "1" ]] && command -v pkcheck >/dev/null 2>&1; then
  pkcheck --revoke-temp || true
  for i in $(seq 1 "${AUTH_CYCLES}"); do
    echo "  cycle ${i}: trigger auth"
    set +e
    pkcheck --allow-user-interaction --process "$$" --action-id "${ACTION_ID}"
    rc=$?
    set -e
    echo "  cycle ${i}: pkcheck rc=${rc}"
    run_ctl auth-summary || true
    run_ctl temp-list || true
    run_ctl temp-revoke-all || true
    run_ctl temp-list || true
  done
else
  echo "  skipped (set GARCARD_SPRINT07_RUN_PKCHECK=1 and install pkcheck)"
fi

echo "[3/5] Daemon restart resilience"
run_ctl quit >/dev/null
wait "${DAEMON_PID}" 2>/dev/null || true
DAEMON_PID=0
start_daemon
run_ctl status
run_ctl diagnose

echo "[4/5] Optional polkit restart check"
if [[ -n "${POLKIT_RESTART_CMD}" ]]; then
  echo "  running: ${POLKIT_RESTART_CMD}"
  set +e
  bash -lc "${POLKIT_RESTART_CMD}"
  restart_rc=$?
  set -e
  echo "  restart command rc=${restart_rc}"
  if [[ "${restart_rc}" -eq 0 ]]; then
    run_ctl quit >/dev/null
    wait "${DAEMON_PID}" 2>/dev/null || true
    DAEMON_PID=0
    start_daemon
    run_ctl diagnose
    run_ctl temp-list || true
  fi
else
  echo "  skipped (set GARCARD_SPRINT07_POLKIT_RESTART_CMD, e.g. 'sudo systemctl restart polkit')"
fi

echo "[5/5] Final summary snapshot"
run_ctl temp-revoke-all || true
run_ctl status
run_ctl auth-summary
run_ctl diagnose

echo "Validation complete. Log output:"
echo "  ${LOG_FILE}"
