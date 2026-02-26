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

## Matrix Status
1. Baseline non-interactive rows updated in `examples/sprint-08-parity-matrix.md`.
2. Interactive/passive coverage now includes:
   - success and canceled prompt paths
   - temp-list and temp-revoke-all with concrete temporary authorization ids
   - manual privileged polkit-restart recovery
3. Remaining rows are policy/path specific and still pending:
   - explicit wrong-password failure path (`last_outcome: failure`)
   - timeout path under live challenge (`last_outcome: timeout`)
   - multi-identity and retention-choice scenarios
   - temp-revoke single-id scenario

## Next Actions
1. Run one focused wrong-password parity capture (`failure` outcome) with debug logs.
2. Run one focused timeout capture using short prompt timeout on `polkit` backend.
3. Add one targeted single-id revoke capture (`temp-revoke <authorization-id>`) and finalize matrix signoff.
