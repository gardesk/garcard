# Sprint 07 Validation Checklist

Run these checks from an active desktop user session.

## Automated Baseline
1. `cargo test --workspace`
2. `./examples/validate-sprint-07.sh`

Expected:
1. Tests pass.
2. `status`, `auth-summary`, `diagnose`, and temp-authorization commands all respond.

## Fresh Login / Session Restart
1. Log out and back in.
2. Start daemon: `cargo run -q -p garcard -- daemon`
3. Run:
   - `cargo run -q -p garcardctl -- status`
   - `cargo run -q -p garcardctl -- diagnose`

Expected:
1. `status.subject_kind` should normally be `unix-session`.
2. `diagnose.authority_connected` should be `true` when polkit is reachable.

## Polkit Restart Recovery
1. Run: `GARCARD_SPRINT07_POLKIT_RESTART_CMD='sudo systemctl restart polkit' ./examples/validate-sprint-07.sh`
2. If sudo is unavailable, restart polkit manually and rerun:
   - `garcardctl diagnose`
   - `garcardctl temp-list`

Expected:
1. Daemon can be restarted cleanly after polkit restart.
2. Diagnostics and temp-authorization controls recover without socket cleanup.

## Repeated Authorization + Revocation Cycles
1. Run: `GARCARD_SPRINT07_BACKEND=polkit GARCARD_SPRINT07_RUN_PKCHECK=1 ./examples/validate-sprint-07.sh`
2. During prompts, exercise both denied and successful authentication outcomes.
3. After successful auth, verify:
   - `garcardctl temp-list`
   - `garcardctl temp-revoke-all`
   - `garcardctl temp-list`

Expected:
1. Repeated cycles do not wedge auth state.
2. Temporary authorizations are introspectable and revocable in-session.
3. `diagnose.hints` includes useful remediation guidance for denied/no-agent paths.
