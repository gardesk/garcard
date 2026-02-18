#!/usr/bin/env bash
set -euo pipefail

ACTION_ID="${1:-org.freedesktop.login1.power-off}"

if ! command -v pkcheck >/dev/null 2>&1; then
  echo "pkcheck not found; install polkit tools to run live auth validation"
  exit 1
fi

if command -v garcardctl >/dev/null 2>&1; then
  GARCARDCTL=(garcardctl)
else
  GARCARDCTL=(cargo run -q -p garcardctl --)
fi

run_garcardctl() {
  "${GARCARDCTL[@]}" "$@"
}

echo "[1/5] Check daemon connectivity"
run_garcardctl ping

echo "[2/5] Check daemon status"
run_garcardctl status

echo "[3/5] Check pre-auth summary"
run_garcardctl auth-summary

echo "[4/5] Trigger interactive policy check"
echo "Action ID: ${ACTION_ID}"
echo "Expected: garcard prompt should appear in your X11 session."
set +e
pkcheck --allow-user-interaction --process "$$" --action-id "${ACTION_ID}"
PKCHECK_RC=$?
set -e
echo "pkcheck exit code: ${PKCHECK_RC}"

echo "[5/5] Check post-auth summary"
run_garcardctl auth-summary

cat <<'EOF'
Exit code hints:
  0   authorized
  1   not authorized or canceled
  2   no such action or action unavailable in this context

Next manual checks:
  - Run again and press Esc to verify cancel behavior.
  - Run `garcard prompt --mode secret --message "Timeout check" --timeout-secs 5`
    and wait to verify timeout handling (exit code 124).
EOF
