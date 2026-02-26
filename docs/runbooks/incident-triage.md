# Incident Triage Playbook

## 1) No Agent Available
Symptom:
1. `pkcheck` returns: `Authorization requires authentication but no agent is available.`

Checklist:
1. Verify daemon socket/control path:
   - `garcardctl ping`
   - `garcardctl status`
2. Check diagnostics:
   - `garcardctl diagnose`
3. If daemon is down, start/restart:
   - `systemctl --user restart garcard.service`
4. If polkit was restarted, restart garcard after polkit recovery:
   - `garcardctl quit`
   - relaunch daemon/service

Escalate if:
1. `diagnose.authority_connected` remains `false` after daemon restart.

## 2) Repeated Denied Authentication
Symptom:
1. Prompt loops with denied results despite retries.

Checklist:
1. Inspect last outcome and phase:
   - `garcardctl auth-summary`
2. Verify operator account authorization policy (wheel/admin membership and action policy rules).
3. Trigger a controlled challenge and inspect logs:
   - `RUST_LOG=garcard=debug garcard daemon`
   - `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`
4. Confirm deny path vs transport failure in logs (`FAILURE` vs transport errors).

Escalate if:
1. Correct credentials consistently map to denied outcomes across multiple actions.

## 3) Authority Disconnect / DBus Errors
Symptom:
1. `diagnose` reports authority connectivity errors.
2. Temp-authorization commands fail with dbus/authority errors.

Checklist:
1. Check diagnostics:
   - `garcardctl diagnose`
2. Restart daemon:
   - `garcardctl quit`
   - relaunch daemon/service
3. Re-check:
   - `garcardctl status`
   - `garcardctl temp-list`
4. If still failing, restart polkit (system policy permitting) and restart daemon.

Escalate if:
1. Authority connectivity remains down after polkit + daemon restart.

## Evidence To Capture For Any Incident
1. `garcardctl status`
2. `garcardctl auth-summary`
3. `garcardctl diagnose`
4. Relevant daemon logs with timestamps
5. Exact `pkcheck` command and stderr/stdout
