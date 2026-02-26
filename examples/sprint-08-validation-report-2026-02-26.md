# Sprint 08 Validation Report (2026-02-26)

## Scope
1. Close static ecosystem integration warnings from Sprint 08 certification.
2. Execute parity baseline harness and capture evidence.
3. Verify temporary-authorization lifecycle commands against authority call contracts.

## Commands
1. `./examples/validate-sprint-08-integration.sh ..`
2. `./examples/validate-sprint-08-parity.sh`
3. `GARCARD_SPRINT07_BACKEND=stub GARCARD_SPRINT07_RUN_PKCHECK=0 ./examples/validate-sprint-07.sh`
4. `cargo test -p garcard`
5. `cargo test --workspace`
6. targeted failure capture with deterministic wrong-password prompt command (`GARCARD_PROMPT_COMMAND='printf "wrong-password\n"'`)
7. targeted timeout capture with deterministic timeout prompt command (`GARCARD_PROMPT_COMMAND='exit 124'`)
8. targeted single-id revoke capture (`temp-list` -> `temp-revoke <id>` -> `temp-list`)
9. manual privileged restart + verification:
   - `sudo systemctl restart polkit`
   - `GARCARD_SPRINT07_BACKEND=polkit GARCARD_SPRINT07_RUN_PKCHECK=0 ./examples/validate-sprint-07.sh`

## Results
1. Integration certification script now passes with zero warnings:
   - installer guidance includes `garcardctl diagnose`
   - gargears adapter exposes `diagnose` and temp-authorization lifecycle methods
2. Parity baseline harness completed successfully and generated evidence:
   - `target/sprint-08-parity-evidence.md`
3. Temporary-authorization DBus contract issue fixed:
   - previous `InvalidArgs` (`(sa{sv})` vs `((sa{sv}))`) no longer appears
   - `temp-list` and `temp-revoke-all` return clean baseline results
4. Workspace tests pass after lifecycle-call marshaling fix.
5. Interactive parity loop executed via `GARCARD_SPRINT08_RUN_INTERACTIVE=1 ./examples/validate-sprint-08-parity.sh`:
   - successful auth path observed (`last_outcome: success`)
   - canceled auth path observed (`last_outcome: canceled`)
   - temporary authorizations created and revoked in-loop (`revoked_count: 1`)
6. Privileged polkit-restart recovery executed manually on 2026-02-26:
   - operator ran `sudo systemctl restart polkit`
   - post-restart lifecycle verification on `polkit` backend passed (`validate-sprint-07.sh`)
7. Targeted failure-path capture passed:
   - `pkcheck rc=1` with `Not authorized`
   - `auth-summary.last_outcome=failure`
8. Targeted timeout-path capture passed:
   - `pkcheck rc=1` with `Not authorized`
   - `auth-summary.last_outcome=timeout`
9. Targeted temp-revoke single-id capture passed:
   - temporary authorization id observed: `tmpauthz0`
   - `temp-revoke tmpauthz0` returned `revoked: true`
   - follow-up `temp-list` returned empty authorizations
10. Runtime capability probe findings:
   - multi-identity not exposed on tested host/action (`identity_count=1`)
   - retention options for tested action resolve to `one-shot` only

## Matrix Status
1. Baseline non-interactive rows updated in `examples/sprint-08-parity-matrix.md`.
2. Interactive/passive coverage now includes:
   - success and canceled prompt paths
   - temp-list and temp-revoke-all with concrete temporary authorization ids
   - manual privileged polkit-restart recovery
3. Targeted deterministic coverage now includes:
   - explicit wrong-password failure path (`last_outcome: failure`)
   - timeout path (`last_outcome: timeout`)
   - temp-revoke single-id scenario
4. Remaining blocked rows are host policy dependent:
   - multi-identity scenario (requires >1 eligible identity)
   - retention-choice scenario (requires session/always retention options from policy details)

## Next Actions
1. If full parity signoff is required on this host, provision a second eligible identity and an action that exposes retention session/always metadata.
2. Otherwise mark remaining blocked rows as environment-limited and proceed with GA checklist gate review.
