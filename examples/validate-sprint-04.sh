#!/usr/bin/env bash
set -euo pipefail

SOCKET_PATH="${GARCARD_SPRINT04_SOCKET:-${PWD}/target/garcard-sprint04.sock}"
BACKEND="${GARCARD_SPRINT04_BACKEND:-stub}"
RESTART_LOOPS="${GARCARD_SPRINT04_RESTART_LOOPS:-3}"
PKCHECK_LOOPS="${GARCARD_SPRINT04_PKCHECK_LOOPS:-2}"
ACTION_ID="${GARCARD_SPRINT04_ACTION_ID:-com.mesonbuild.install.run}"
LOG_FILE="${GARCARD_SPRINT04_LOG:-${PWD}/target/garcard-sprint04.log}"

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
  local tries=100
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

echo "Sprint 04 validation"
echo "  socket: ${SOCKET_PATH}"
echo "  backend: ${BACKEND}"
echo "  log: ${LOG_FILE}"

mkdir -p "$(dirname "${SOCKET_PATH}")"
mkdir -p "$(dirname "${LOG_FILE}")"
rm -f "${SOCKET_PATH}" "${LOG_FILE}"
start_daemon

echo "[1/4] Baseline daemon health"
run_ctl ping
run_ctl status
run_ctl auth-summary

echo "[2/4] Restart resilience loop (${RESTART_LOOPS} iterations)"
for i in $(seq 1 "${RESTART_LOOPS}"); do
  echo "  iteration ${i}: stop/start daemon"
  run_ctl quit >/dev/null
  wait "${DAEMON_PID}" 2>/dev/null || true
  DAEMON_PID=0
  start_daemon
  run_ctl ping >/dev/null
done

echo "[3/4] Post-restart health"
run_ctl status
run_ctl auth-summary

echo "[4/4] Optional interactive auth loop"
if [[ "${BACKEND}" == "polkit" ]] && command -v pkcheck >/dev/null 2>&1; then
  echo "  running ${PKCHECK_LOOPS} pkcheck attempts for action ${ACTION_ID}"
  for i in $(seq 1 "${PKCHECK_LOOPS}"); do
    set +e
    pkcheck --allow-user-interaction --process "$$" --action-id "${ACTION_ID}"
    rc=$?
    set -e
    echo "  pkcheck[${i}] rc=${rc}"
    run_ctl auth-summary || true
  done
else
  echo "  skipped (requires BACKEND=polkit and pkcheck installed)"
fi

echo "Validation complete. Log output:"
echo "  ${LOG_FILE}"
