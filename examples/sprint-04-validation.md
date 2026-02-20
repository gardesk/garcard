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
1. Preferred automated execution:
   - `./examples/validate-sprint-04-runtime.sh`
2. Manual fallback:
   - start daemon with `GARCARD_AGENT_BACKEND=polkit`
   - trigger `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`
   - while prompt is visible, issue `garcardctl quit`, relaunch daemon, and retry probe.

Expected:
1. Active prompt interruption does not wedge daemon state.
2. Relaunched daemon accepts new requests with clean `auth-summary`.

## Session Shutdown/Logout Race
1. Preferred automated execution:
   - `./examples/validate-sprint-04-runtime.sh`
2. Manual fallback:
   - start daemon with debug logs
   - trigger auth prompt
   - send `SIGTERM` to daemon PID while request is active
   - relaunch daemon and confirm `garcardctl status`.

Expected:
1. Daemon exits cleanly without stale socket.
2. Relaunch succeeds without manual socket cleanup.

## Security Spot Checks
1. Verify secret response handling does not log plaintext values.
2. Confirm IPC control socket mode remains owner-only in production (`600`).
3. Validate only same-UID peers can control daemon IPC.
