#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="${1:-..}"
FAILURES=0
WARNINGS=0

pass() {
  echo "PASS: $1"
}

fail() {
  echo "FAIL: $1"
  FAILURES=$((FAILURES + 1))
}

warn() {
  echo "WARN: $1"
  WARNINGS=$((WARNINGS + 1))
}

require_pattern() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if [[ ! -f "${path}" ]]; then
    fail "${label} (missing: ${path})"
    return
  fi

  if grep -Eq "${pattern}" "${path}"; then
    pass "${label}"
  else
    fail "${label} (pattern not found in ${path})"
  fi
}

optional_pattern() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if [[ ! -f "${path}" ]]; then
    warn "${label} (missing: ${path})"
    return
  fi

  if grep -Eq "${pattern}" "${path}"; then
    pass "${label}"
  else
    warn "${label} (pattern not found in ${path})"
  fi
}

echo "Sprint 08 integration certification checks against: ${ROOT_DIR}"

require_pattern "${ROOT_DIR}/config/gar/init.lua" 'gar\.exec_once\("garcard daemon"\)' \
  "Gar startup defaults autostart garcard"
require_pattern "${ROOT_DIR}/install.sh" 'garcard\.service' \
  "Installer wires garcard user service unit"
require_pattern "${ROOT_DIR}/install.sh" 'garcardctl status' \
  "Installer docs include garcardctl status guidance"

require_pattern "${ROOT_DIR}/gargears/gargears/src/ipc/discovery.rs" 'garcard\.sock' \
  "gargears discovery includes garcard socket"
require_pattern "${ROOT_DIR}/gargears/gargears/src/panels/mod.rs" 'Component::Garcard' \
  "gargears panel registry includes garcard component"
require_pattern "${ROOT_DIR}/gargears/gargears/src/ipc/adapters/garcard.rs" 'pub struct GarcardAdapter' \
  "gargears has garcard IPC adapter"

optional_pattern "${ROOT_DIR}/install.sh" 'garcardctl diagnose' \
  "Installer docs surface diagnostics command"
optional_pattern "${ROOT_DIR}/gargears/gargears/src/ipc/adapters/garcard.rs" 'diagnose|temp-list|temp-revoke' \
  "gargears adapter exposes lifecycle control methods"

if [[ "${FAILURES}" -gt 0 ]]; then
  echo "Integration certification checks failed: ${FAILURES} failure(s), ${WARNINGS} warning(s)."
  exit 1
fi

echo "Integration certification checks passed with ${WARNINGS} warning(s)."
exit 0
