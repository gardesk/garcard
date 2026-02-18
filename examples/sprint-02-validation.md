# Sprint 02 Validation Checklist

Run these checks from an active X11 user session with `garcard daemon` running.

## Baseline
1. `garcardctl ping`
2. `garcardctl status`
3. `garcardctl auth-summary`

Expected:
1. Daemon reachable over IPC.
2. `agent_backend` is `polkit` in normal session use.
3. Auth summary starts in `idle`.

## Prompt Flow: Success
1. `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.login1.power-off`
2. Enter valid password in garcard prompt.
3. `garcardctl auth-summary`

Expected:
1. Prompt appears.
2. `pkcheck` exits `0`.
3. Auth summary reaches `success` then returns to `idle` on next request cycle.

## Prompt Flow: Wrong Password Then Recovery
1. Trigger same `pkcheck` command.
2. Enter wrong password once, then correct password on retry.
3. `garcardctl auth-summary`

Expected:
1. No daemon restart required.
2. Failure path is visible (`failure`) and recoverable in the same session.

## Prompt Flow: Cancel
1. Trigger same `pkcheck` command.
2. Press `Esc` in prompt.
3. Inspect `pkcheck` exit code and `garcardctl auth-summary`.

Expected:
1. `pkcheck` is denied/canceled (non-zero).
2. Auth summary reaches `canceled`.

## Prompt Flow: Timeout
1. `garcard prompt --mode secret --message "Timeout check" --timeout-secs 5`
2. Wait without typing.

Expected:
1. Command exits with code `124`.
2. Daemon-side prompt handling can represent timeout state.

## Queue Behavior
1. Start two checks close together in separate terminals:
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.login1.power-off`
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.login1.reboot`
2. Watch `garcardctl auth-summary` while completing prompts.

Expected:
1. One active request at a time.
2. Additional requests queue and process FIFO.
3. No deadlock after cancel/failure/success.
