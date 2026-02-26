# Sprint 08 Parity Matrix

Use this matrix to certify behavior against mature desktop PolicyKit agents.

## Automated Prerequisites
1. `cargo test --workspace`
2. `./examples/validate-sprint-08-parity.sh`

## Matrix
| Scenario | Procedure | Expected Result | Status | Evidence |
| --- | --- | --- | --- | --- |
| Success path | Trigger `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run` and enter correct password | Prompt completes, auth is authorized, no failure flash | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`cycle 1/2`, `last_outcome: success`) |
| Failure path | Trigger same `pkcheck` and enter wrong password | Prompt flashes error, reprompts in place, `auth-summary.last_outcome=failure` before retry | PASS (targeted) | 2026-02-26 deterministic wrong-password capture (`last_outcome: failure`, `pkcheck rc=1`) |
| Cancel path | Trigger `pkcheck`, cancel prompt | Request exits cleanly, `auth-summary.last_outcome=canceled` | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`cycle 3`, `last_outcome: canceled`) |
| Timeout path | Set short timeout (`GARCARD_PROMPT_TIMEOUT_SECS=2`), trigger auth, do not respond | Request times out, `auth-summary.last_outcome=timeout` | PASS (targeted) | 2026-02-26 deterministic timeout capture (`last_outcome: timeout`, `pkcheck rc=1`) |
| Multi-identity flow | Trigger policy requiring identity choice | Identity list rendered, selected identity is honored | PASS (targeted) | 2026-02-26 targeted capture prompt listed `mfwolffe` + `garcardqa`; helper connected as selected `garcardqa` |
| Retention choice flow | Trigger policy exposing retention options | Retention choice accepted and recorded in `auth-summary` | PASS (targeted) | 2026-02-26 targeted capture showed retention prompt (`One-shot`, `Keep for session`); `auth-summary.last_retention_policy=keep-session` |
| Temp auth introspection | Run `garcardctl temp-list` after successful retained auth | Active temporary authorization entries are listed | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`tmpauthz0/tmpauthz1` listed) |
| Temp auth revoke single | Run `garcardctl temp-revoke <id>` | Target authorization removed | PASS (targeted) | 2026-02-26 single-id revoke (`tmpauthz0` present before, revoked true, absent after) |
| Temp auth revoke all | Run `garcardctl temp-revoke-all` | All temporary authorizations removed | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`revoked_count: 1` after cycle 1/2) |
| Daemon restart during lifecycle | Restart daemon and rerun status/diag/temp commands | Control plane recovers without stale socket state | PASS (baseline) | `target/sprint-08-parity-evidence.md` (`validate-sprint-07.sh` section) |
| Polkit restart recovery | Restart polkit and relaunch daemon | Diagnostics recover, control commands remain responsive | PASS (manual) | 2026-02-26 manual `sudo systemctl restart polkit` + post-check `validate-sprint-07.sh` (`polkit` backend healthy) |

## Signoff
1. Date: 2026-02-26 (baseline run)
2. Operator: mfwolffe/codex
3. Result (`PASS`/`FAIL`): PASS
4. Blocking gaps:
   - none
