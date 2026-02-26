# Sprint 08 Parity Matrix

Use this matrix to certify behavior against mature desktop PolicyKit agents.

## Automated Prerequisites
1. `cargo test --workspace`
2. `./examples/validate-sprint-08-parity.sh`

## Matrix
| Scenario | Procedure | Expected Result | Status | Evidence |
| --- | --- | --- | --- | --- |
| Success path | Trigger `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run` and enter correct password | Prompt completes, auth is authorized, no failure flash | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`cycle 1/2`, `last_outcome: success`) |
| Failure path | Trigger same `pkcheck` and enter wrong password | Prompt flashes error, reprompts in place, `auth-summary.last_outcome=failure` before retry | Pending | daemon log + `garcardctl auth-summary` |
| Cancel path | Trigger `pkcheck`, cancel prompt | Request exits cleanly, `auth-summary.last_outcome=canceled` | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`cycle 3`, `last_outcome: canceled`) |
| Timeout path | Set short timeout (`GARCARD_PROMPT_TIMEOUT_SECS=2`), trigger auth, do not respond | Request times out, `auth-summary.last_outcome=timeout` | Pending | daemon log + `garcardctl auth-summary` |
| Multi-identity flow | Trigger policy requiring identity choice | Identity list rendered, selected identity is honored | Pending | prompt capture + daemon log |
| Retention choice flow | Trigger policy exposing retention options | Retention choice accepted and recorded in `auth-summary` | Pending | `garcardctl auth-summary` |
| Temp auth introspection | Run `garcardctl temp-list` after successful retained auth | Active temporary authorization entries are listed | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`tmpauthz0/tmpauthz1` listed) |
| Temp auth revoke single | Run `garcardctl temp-revoke <id>` | Target authorization removed | Pending interactive retained auth | `temp-list` before/after |
| Temp auth revoke all | Run `garcardctl temp-revoke-all` | All temporary authorizations removed | PASS (interactive) | `target/sprint-08-parity-evidence.md` (`revoked_count: 1` after cycle 1/2) |
| Daemon restart during lifecycle | Restart daemon and rerun status/diag/temp commands | Control plane recovers without stale socket state | PASS (baseline) | `target/sprint-08-parity-evidence.md` (`validate-sprint-07.sh` section) |
| Polkit restart recovery | Restart polkit and relaunch daemon | Diagnostics recover, control commands remain responsive | PASS (manual) | 2026-02-26 manual `sudo systemctl restart polkit` + post-check `validate-sprint-07.sh` (`polkit` backend healthy) |

## Signoff
1. Date: 2026-02-26 (baseline run)
2. Operator: mfwolffe/codex
3. Result (`PASS`/`FAIL`): IN PROGRESS
4. Blocking gaps:
   - failure-path parity (`last_outcome: failure`) on explicit wrong-password flow
   - timeout-path parity (`last_outcome: timeout`) under interactive challenge
   - multi-identity and retention-choice scenarios on policies that expose those options
   - temp-revoke single-id scenario
