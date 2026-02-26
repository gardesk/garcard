# garcard 0.1.0-rc1

## Highlights
1. Polkit authentication agent backend with queue-aware auth state tracking.
2. Built-in gartk prompt path with timeout/cancel behavior and ask-password fallback.
3. Daemon health/reconnect loop with forced reconnect support (`SIGHUP` + maintenance pass).
4. Lifecycle controls in `garcardctl`: `ping`, `status`, `diagnose`, `version`, `auth-summary`, `temp-list`, `temp-revoke`, `temp-revoke-all`, `quit`.
5. Session helper child lifecycle handling and improved helper-protocol fallback behavior.
6. Auth lifecycle metadata and retention mapping exposed via `auth-summary`.
7. Status health surface now includes authority connectivity and subject-kind fields for control-surface consumers.

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
4. Sprint 07 authorization lifecycle coverage:
   - `examples/validate-sprint-07.sh`
   - `examples/sprint-07-validation.md`
5. Sprint 08 parity matrix scaffolding:
   - `examples/sprint-08-parity-matrix.md`
   - `examples/validate-sprint-08-parity.sh`
6. Sprint 08 parity certification and targeted captures:
   - `examples/sprint-08-validation-report-2026-02-26.md`
   - `examples/sprint-08-parity-matrix.md` (`PASS`, blockers: none)
   - `target/sprint-08-parity-evidence.md`

## GA Gate Summary (2026-02-26)
1. Release-candidate gate checklist completed: `docs/release/ga-checklist.md`.
2. Interactive and targeted parity scenarios pass (success/failure/cancel/timeout, multi-identity, retention, temp auth lifecycle).
3. Integration certification and post-polkit-restart recovery validated in Sprint 08 report.

## Explicit Out-Of-Scope For 0.1.0
1. Challenge prompting depends on host polkit policy; some actions may auto-authorize.
2. Scope is logged-in user sessions (X11), not greeter/session-manager flows.
3. `gargears` integration is limited to command/control-surface parity contracts; native UI parity is tracked separately.
4. Multi-seat/remote-session policy nuances are not fully certified in this release cycle.
