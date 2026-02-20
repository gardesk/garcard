#!/usr/bin/env bash
set -euo pipefail

SOCKET_PATH="${GARCARD_SPRINT04_RUNTIME_SOCKET:-${PWD}/target/garcard-sprint04-runtime.sock}"
LOG_FILE="${GARCARD_SPRINT04_RUNTIME_LOG:-${PWD}/target/garcard-sprint04-runtime.log}"
ACTION_ID="${GARCARD_SPRINT04_RUNTIME_ACTION_ID:-com.mesonbuild.install.run}"
PROMPT_TIMEOUT_SECS="${GARCARD_SPRINT04_RUNTIME_PROMPT_TIMEOUT_SECS:-20}"

if ! command -v pkcheck >/dev/null 2>&1; then
  echo "pkcheck not found; install polkit tools to run runtime validation"
  exit 1
fi

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
PROBE_PID=0

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
    GARCARD_AGENT_BACKEND=polkit \
    GARCARD_PROMPT_TIMEOUT_SECS="${PROMPT_TIMEOUT_SECS}" \
    RUST_LOG=garcard=debug \
    "${DAEMON_CMD[@]}" >>"${LOG_FILE}" 2>&1 &
  DAEMON_PID=$!
  wait_for_daemon
}

stop_daemon_graceful() {
  if [[ "${DAEMON_PID}" -gt 0 ]] && kill -0 "${DAEMON_PID}" 2>/dev/null; then
    run_ctl quit >/dev/null 2>&1 || true
    wait "${DAEMON_PID}" 2>/dev/null || true
  fi
  DAEMON_PID=0
}

stop_daemon_sigterm() {
  if [[ "${DAEMON_PID}" -gt 0 ]] && kill -0 "${DAEMON_PID}" 2>/dev/null; then
    kill -TERM "${DAEMON_PID}" 2>/dev/null || true
    wait "${DAEMON_PID}" 2>/dev/null || true
  fi
  DAEMON_PID=0
}

cleanup() {
  stop_daemon_graceful
  rm -f "${SOCKET_PATH}"
}
trap cleanup EXIT

wait_for_active_request() {
  local tries=120
  while (( tries > 0 )); do
    local summary
    summary="$(run_ctl auth-summary 2>/dev/null || true)"
    if [[ "${summary}" == *'"active_requests": 1'* ]]; then
      return 0
    fi
    sleep 0.2
    tries=$((tries - 1))
  done
  echo "auth request never became active"
  return 1
}

run_probe_background() {
  local out_file="$1"
  local err_file="$2"
  pkcheck --allow-user-interaction --process "$$" --action-id "${ACTION_ID}" \
    >"${out_file}" 2>"${err_file}" &
  PROBE_PID=$!
}

wait_probe() {
  local probe_pid="$1"
  local rc_file="$2"
  set +e
  wait "${probe_pid}"
  local rc=$?
  set -e
  echo "${rc}" > "${rc_file}"
}

mkdir -p "$(dirname "${SOCKET_PATH}")"
mkdir -p "$(dirname "${LOG_FILE}")"
rm -f "${SOCKET_PATH}" "${LOG_FILE}"

echo "Sprint 04 runtime validation"
echo "  socket: ${SOCKET_PATH}"
echo "  log: ${LOG_FILE}"
echo "  action: ${ACTION_ID}"

echo "[0/3] Reset temporary authorizations"
pkcheck --revoke-temp || true

echo "[1/3] Active prompt + daemon restart"
start_daemon
probe1_out="${PWD}/target/sprint04-probe-restart.out"
probe1_err="${PWD}/target/sprint04-probe-restart.err"
probe1_rc="${PWD}/target/sprint04-probe-restart.rc"
run_probe_background "${probe1_out}" "${probe1_err}"
wait_for_active_request
run_ctl quit >/dev/null
wait "${DAEMON_PID}" 2>/dev/null || true
DAEMON_PID=0
wait_probe "${PROBE_PID}" "${probe1_rc}"
start_daemon
run_ctl status >/dev/null
run_ctl auth-summary >/dev/null
stop_daemon_graceful

echo "[2/3] Active prompt + SIGTERM race"
start_daemon
probe2_out="${PWD}/target/sprint04-probe-sigterm.out"
probe2_err="${PWD}/target/sprint04-probe-sigterm.err"
probe2_rc="${PWD}/target/sprint04-probe-sigterm.rc"
run_probe_background "${probe2_out}" "${probe2_err}"
wait_for_active_request
stop_daemon_sigterm
wait_probe "${PROBE_PID}" "${probe2_rc}"
start_daemon
run_ctl status
run_ctl auth-summary
stop_daemon_graceful

echo "[3/3] Summary"
echo "  restart probe rc=$(cat "${probe1_rc}")"
echo "  sigterm probe rc=$(cat "${probe2_rc}")"
echo "  restart probe stderr: $(tr '\n' ' ' < "${probe1_err}")"
echo "  sigterm probe stderr: $(tr '\n' ' ' < "${probe2_err}")"
echo "Validation complete."
