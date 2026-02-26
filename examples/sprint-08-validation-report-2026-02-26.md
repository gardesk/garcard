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

## Matrix Status
1. Baseline non-interactive rows updated in `examples/sprint-08-parity-matrix.md`.
2. Remaining rows are interactive/policy-dependent and still pending:
   - success/failure/cancel/timeout prompt-path parity via `pkcheck`
   - multi-identity and retention-choice scenarios
   - privileged polkit restart recovery path

## Next Actions
1. Run interactive parity pass:
   - `GARCARD_SPRINT08_RUN_INTERACTIVE=1 ./examples/validate-sprint-08-parity.sh`
2. Execute privileged recovery check:
   - `GARCARD_SPRINT07_POLKIT_RESTART_CMD='sudo systemctl restart polkit' ./examples/validate-sprint-07.sh`
3. Mark remaining matrix rows PASS/FAIL with log pointers.
