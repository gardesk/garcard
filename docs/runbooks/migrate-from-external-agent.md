# Migration Runbook: External Agent To Garcard

## Scope
Move from another desktop polkit agent (for example `polkit-kde-agent-1`, `lxqt-policykit-agent`, `mate-polkit`) to `garcard`.

## Preconditions
1. `garcard` and `garcardctl` are installed or runnable from this workspace.
2. You have a user session with access to polkit prompts.
3. Existing external polkit agent service is known.

## Steps
1. Stop and disable the existing agent for the user session.
   - Example:
     - `systemctl --user disable --now lxqt-policykit-agent.service`
     - `systemctl --user disable --now polkit-kde-authentication-agent-1.service`
2. Install and enable `garcard` user service.
   - `install -Dm644 garcard.service ~/.config/systemd/user/garcard.service`
   - `systemctl --user daemon-reload`
   - `systemctl --user enable --now garcard.service`
3. Verify control plane reachability.
   - `garcardctl ping`
   - `garcardctl status`
   - `garcardctl diagnose`
4. Trigger an auth challenge.
   - `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`
5. Validate lifecycle controls.
   - `garcardctl auth-summary`
   - `garcardctl temp-list`
   - `garcardctl temp-revoke-all`

## Success Criteria
1. Only garcard owns the active auth-agent role in session.
2. Prompt appears and accepts valid credentials.
3. `garcardctl` lifecycle commands respond without permission/socket errors.

## Rollback
1. Disable garcard user service:
   - `systemctl --user disable --now garcard.service`
2. Re-enable prior agent service:
   - `systemctl --user enable --now <previous-agent>.service`
3. Re-run `pkcheck --allow-user-interaction ...` to confirm prior behavior is restored.
