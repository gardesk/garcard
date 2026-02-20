# Sprint 04 Validation Report (2026-02-20)

## Scope
1. Hardening regression checks after Sprint 04 code changes.
2. Automated reliability checks for daemon restart resilience.
3. Runtime race validation for active prompt interruption paths.

## Commands
1. `cargo test --workspace`
2. `./examples/validate-sprint-04.sh` (executed with default `stub` backend)
3. `./examples/validate-sprint-04-runtime.sh` (executed with `polkit` backend)

## Results
1. Workspace tests passed (`39` garcard tests + workspace crates).
2. `validate-sprint-04.sh` passed baseline and restart loop checks:
   - daemon reachable via `ping`/`status`
   - restart loop completed (`3` stop/start iterations)
   - post-restart status and auth summary remained healthy (`idle`)
3. Optional interactive `pkcheck` loop was intentionally skipped in this run:
   - requires live polkit challenge flow and operator interaction.
4. Runtime race harness passed for both previously manual checks:
   - active prompt + daemon restart (`garcardctl quit`)
   - active prompt + `SIGTERM`
5. Runtime log evidence (`target/garcard-sprint04-runtime.log`) confirms:
   - auth request reached active processing before interruption
   - daemon shutdown/termination unregistered cleanly
   - relaunch succeeded with healthy `status` and `auth-summary`

## Hardening Outcomes Confirmed
1. IPC control path now validates same-UID peer credentials.
2. Prompt UI runtime path no longer relies on panic/`expect` for color parsing.
3. Helper response buffers are scrubbed after sending to helper socket.

## Remaining Manual Sprint 04 Checks
1. Optional interactive acceptance pass (enter valid credentials, wrong-then-retry, explicit cancel) in full desktop session.
