# Sprint 02 Validation Report (2026-02-18)

## Environment
1. Host: `mizu` (NixOS)
2. Daemon run mode: `GARCARD_AGENT_BACKEND=polkit`
3. Socket: `/run/user/1000/garcard.sock`

## Baseline Checks
1. `garcardctl ping` -> success.
2. `garcardctl status` -> running with `agent_backend: polkit`.
3. `garcardctl auth-summary` -> `idle`.

## Live Policy Checks
1. `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.login1.power-off`
Result:
1. Exit code `0`.
2. No challenge emitted (policy already authorized in this session context).

2. `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`
Result:
1. Exit code `1` (`Not authorized.`).
2. Daemon logs showed live challenge callback:
   - `Started active polkit auth request ...`
   - `Processing polkit auth request ...`
   - `Starting helper authentication dialog ...`
3. `garcardctl auth-summary` after call -> `timeout`.

## Reconnect Validation
1. `kill -HUP <garcard-pid>` (forced reconnect path).
2. Daemon logs:
   - `Received SIGHUP; forcing backend reconnect`
   - `Unregistered polkit authentication agent`
   - `Registered polkit authentication agent`
3. Post-check:
   - `garcardctl status` remained responsive.
   - `garcardctl auth-summary` remained consistent.

## Optional Root-Level Disruption Check
1. Attempted `systemctl restart polkit`.
2. Result: `Access denied` (no root/system permission from this validation context).
3. Root-level service restart scenario remains for host-owner execution if desired.

## Conclusion
1. Sprint 02 auth callback path is live and receiving real polkit challenge requests.
2. Timeout state is observable and propagated through IPC summary.
3. Backend reconnect behavior is validated through forced reconnect workflow.
