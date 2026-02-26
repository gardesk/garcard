# Garcardctl Operator Cookbook

## Reachability And Runtime
1. Ping daemon:
   - `garcardctl ping`
2. Runtime status and health surface:
   - `garcardctl status`
3. Extended diagnostics and remediation hints:
   - `garcardctl diagnose`
4. Version/protocol handshake:
   - `garcardctl version`

## Auth Lifecycle
1. Inspect current auth state:
   - `garcardctl auth-summary`
2. Trigger policy challenge manually:
   - `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`

## Temporary Authorization Controls
1. List temporary authorizations:
   - `garcardctl temp-list`
2. Revoke one authorization by id:
   - `garcardctl temp-revoke <authorization-id>`
3. Revoke all temporary authorizations:
   - `garcardctl temp-revoke-all`

## Service Control
1. Request daemon shutdown:
   - `garcardctl quit`
2. Start daemon (workspace run):
   - `cargo run -p garcard -- daemon`
3. Restart user service deployment:
   - `systemctl --user restart garcard.service`

## Standard Troubleshooting Sequence
1. `garcardctl ping`
2. `garcardctl status`
3. `garcardctl diagnose`
4. `garcardctl auth-summary`
5. `garcardctl temp-list`

## Operational Notes
1. IPC controls are same-UID restricted.
2. `status` now includes authority and subject health fields for control-surface consumers.
3. `diagnose` includes remediation hints for no-agent and denied-flow scenarios.
