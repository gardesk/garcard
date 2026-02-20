# Sprint 04 Validation Checklist

Run these checks from an active X11 user session.

## Automated Baseline
1. `cargo test --workspace`
2. `./examples/validate-sprint-04.sh`

Expected:
1. All tests pass.
2. Daemon survives restart loop and remains reachable over IPC.

## Interactive Challenge Loop
1. `GARCARD_SPRINT04_BACKEND=polkit ./examples/validate-sprint-04.sh`
2. Complete two `pkcheck` prompts (cancel/deny/success combinations).

Expected:
1. Prompt appears for challenge actions.
2. `garcardctl auth-summary` updates and remains responsive across iterations.

## Daemon Restart During Active Prompt
1. Start daemon in one terminal:
   - `RUST_LOG=garcard=debug GARCARD_AGENT_BACKEND=polkit cargo run -p garcard -- daemon`
2. Trigger challenge in another terminal:
   - `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`
3. While prompt is visible, restart daemon:
   - `cargo run -q -p garcardctl -- quit`
   - relaunch daemon command from step 1.
4. Re-run the same `pkcheck` command.

Expected:
1. Active prompt interruption does not wedge daemon state.
2. Relaunched daemon accepts new requests with clean `auth-summary`.

## Session Shutdown/Logout Race
1. Start daemon with debug logs.
2. Trigger an auth prompt.
3. Send `SIGTERM` to daemon PID while request is active.
4. Relaunch daemon and run `garcardctl status`.

Expected:
1. Daemon exits cleanly without stale socket.
2. Relaunch succeeds without manual socket cleanup.

## Security Spot Checks
1. Verify secret response handling does not log plaintext values.
2. Confirm IPC control socket mode remains owner-only in production (`600`).
3. Validate only same-UID peers can control daemon IPC.
