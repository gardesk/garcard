# garcard 0.1.0-rc1

## Highlights
1. Polkit authentication agent backend with queue-aware auth state tracking.
2. Built-in gartk prompt path with timeout/cancel behavior and ask-password fallback.
3. Daemon health/reconnect loop with forced reconnect support (`SIGHUP` + maintenance pass).
4. `garcardctl` operational commands: `ping`, `status`, `version`, `auth-summary`, `quit`.

## Hardening Included In Sprint 04
1. Same-UID enforcement for local IPC control clients.
2. Reduced panic surface in prompt color setup paths.
3. Best-effort scrubbing of helper prompt response buffers after use.
4. Reduced prompt credential lifetime by moving submitted input without cloning and scrubbing prompt output buffers.
5. Added built-in prompt feedback tones for auth success/error visual feedback.
6. Reused the same built-in prompt window across helper callbacks so failed auth can flash and reprompt without tearing down the modal.
7. Removed daemon-level same-cookie retry loop; retries now follow helper/PAM flow to avoid stale-cookie false failures.
8. Backend maintenance now uses ping-only health checks instead of periodic re-registration to avoid invalidating in-flight auth cookies.

## Validation Coverage
1. Sprint 02 live callback and reconnect validation:
   - `examples/sprint-02-validation-report-2026-02-18.md`
2. Sprint 03 ecosystem + runtime probes:
   - `examples/sprint-03-validation-report-2026-02-18.md`
3. Sprint 04 reliability harness/checklist:
   - `examples/validate-sprint-04.sh`
   - `examples/validate-sprint-04-runtime.sh`
   - `examples/sprint-04-validation.md`

## Known Limitations
1. Challenge prompting depends on host polkit policy; some actions may auto-authorize.
2. Scope is logged-in user sessions (X11), not greeter/session-manager flows.
3. Full panel controls in `gargears` remain limited to discovery/visibility for now.
