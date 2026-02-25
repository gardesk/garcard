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
1. Workspace tests passed (`41` garcard tests + workspace crates).
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
6. Acceptance behavior update (2026-02-24):
   - wrong-password path verified (`auth-summary: failure`)
   - cancel path verified (`auth-summary: canceled`)
   - helper diagnostics classification tightened to avoid treating plaintext helper lines as protocol errors
7. Regression coverage update (2026-02-25):
   - added helper callback-path tests for explicit `SUCCESS`/`FAILURE` outcomes.
   - added helper diagnostic-then-success test to guard against false failure signaling on success.
   - added agent-level mocked retry conversation test (first failure, second success) to verify recoverable in-session retry behavior.
   - workspace test baseline now includes `51` `garcard` tests.

## Hardening Outcomes Confirmed
1. IPC control path now validates same-UID peer credentials.
2. Prompt UI runtime path no longer relies on panic/`expect` for color parsing.
3. Helper response buffers are scrubbed after sending to helper socket.
4. Prompt input handling now moves submitted secrets without cloning and scrubs prompt/output buffers after use.
5. Prompt feedback tones are wired for auth outcomes (success/error), with error flash behavior in built-in prompt mode.
6. Built-in prompt reuses a persistent modal so auth failure feedback can flash inline and reprompt without window teardown.

## Remaining Manual Sprint 04 Checks
1. Final interactive success confirmation in desktop session (correct password should return `pkcheck` exit `0` and show success feedback).
