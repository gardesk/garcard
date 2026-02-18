#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="${1:-..}"
FAILURES=0

pass() {
    echo "PASS: $1"
}

fail() {
    echo "FAIL: $1"
    FAILURES=$((FAILURES + 1))
}

require_file() {
    local path="$1"
    local label="$2"
    if [[ -f "$path" ]]; then
        pass "$label"
    else
        fail "$label (missing: $path)"
    fi
}

require_pattern() {
    local path="$1"
    local pattern="$2"
    local label="$3"
    if [[ ! -f "$path" ]]; then
        fail "$label (missing: $path)"
        return
    fi

    if grep -Eq "$pattern" "$path"; then
        pass "$label"
    else
        fail "$label (pattern not found in $path)"
    fi
}

echo "Validating Sprint 03 static integration against: $ROOT_DIR"

require_pattern "$ROOT_DIR/config/gar/init.lua" 'gar\.exec_once\("garcard daemon"\)' \
    "Gar default init autostarts garcard"
require_pattern "$ROOT_DIR/config/gar/init.lua" 'polkit-kde-authentication-agent-1' \
    "Gar init keeps external fallback note for KDE agent"
require_pattern "$ROOT_DIR/config/gar/init.lua" 'polkit-gnome-authentication-agent-1' \
    "Gar init keeps external fallback note for GNOME agent"

require_pattern "$ROOT_DIR/install.sh" 'INSTALL_GARCARD=false' \
    "Installer exposes garcard component flag"
require_pattern "$ROOT_DIR/install.sh" 'install_garcard\(\)' \
    "Installer defines garcard install function"
require_pattern "$ROOT_DIR/install.sh" 'garcard\.service' \
    "Installer provisions garcard systemd user unit"
require_pattern "$ROOT_DIR/install.sh" '\.config/garcard/config\.toml' \
    "Installer handles garcard config scaffold"

require_pattern "$ROOT_DIR/uninstall.sh" 'garcard garcardctl' \
    "Uninstaller removes garcard binaries"
require_pattern "$ROOT_DIR/uninstall.sh" 'garcard\.service' \
    "Uninstaller removes garcard service unit"
require_pattern "$ROOT_DIR/uninstall.sh" '~/.config/garcard' \
    "Uninstaller references garcard config directory"

require_file "$ROOT_DIR/config/garcard/config.toml" \
    "Root config scaffold includes garcard default config"
require_file "$ROOT_DIR/specs/garcard.spec" \
    "Root specs include garcard package manifest"

require_pattern "$ROOT_DIR/gargears/gargears/src/ipc/discovery.rs" 'garcard\.sock' \
    "gargears discovery maps garcard socket"
require_pattern "$ROOT_DIR/gargears/gargears/src/panels/mod.rs" 'Component::Garcard' \
    "gargears includes garcard panel component"

if [[ "$FAILURES" -eq 0 ]]; then
    echo "All static integration checks passed."
    exit 0
fi

echo "Static integration checks failed: $FAILURES"
exit 1
