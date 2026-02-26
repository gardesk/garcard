# Sprint 08 Integration Certification

## Static Certification
1. `./examples/validate-sprint-08-integration.sh`

Expected:
1. Required integration checks pass for:
   - gar startup autostart default
   - user service install references
   - gargears garcard discovery/panel/adapter presence
2. Optional checks may emit warnings for pending UX/control-surface enhancements.

## Runtime Certification
1. `cargo run -q -p garcard -- daemon`
2. `cargo run -q -p garcardctl -- status`
3. `cargo run -q -p garcardctl -- diagnose`
4. `cargo run -q -p garcardctl -- temp-list`

Expected:
1. Daemon remains reachable and healthy through the full command set.
2. `status` and `diagnose` provide authority + subject visibility for operators.

## Service Lifecycle Certification
1. `systemctl --user restart garcard.service`
2. `garcardctl ping`
3. `garcardctl status`

Expected:
1. User service restart restores control path without stale-socket cleanup.
