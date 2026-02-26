# GA Checklist

## Release Candidate Gate (2026-02-26)
1. `cargo test --workspace` passes.
   - Status: PASS
   - Evidence: `examples/sprint-08-validation-report-2026-02-26.md`
2. `./examples/validate-sprint-08-parity.sh` completed.
   - Status: PASS
   - Evidence: `target/sprint-08-parity-evidence.md`
3. `examples/sprint-08-parity-matrix.md` marked with scenario status and evidence links.
   - Status: PASS
   - Evidence: `examples/sprint-08-parity-matrix.md`
4. Incident/migration/operator runbooks reviewed:
   - `docs/runbooks/migrate-from-external-agent.md`
   - `docs/runbooks/incident-triage.md`
   - `docs/runbooks/garcardctl-cookbook.md`
   - Status: PASS
5. `RELEASE_NOTES.md` reviewed and approved.
   - Status: PASS

## Integration Certification (2026-02-26)
1. Gar startup default path validated for daemon availability.
   - Status: PASS
   - Evidence: `examples/sprint-08-validation-report-2026-02-26.md`
2. User-service lifecycle validated (`enable`, `restart`, `disable`).
   - Status: PASS
   - Evidence: `examples/sprint-08-validation-report-2026-02-26.md`
3. Status/diagnostics surface validated for external control-plane consumers.
   - Status: PASS
   - Evidence: `examples/validate-sprint-08-integration.sh`

## Tagging
1. Candidate tag:
   - `git tag -a v0.1.0-rc1 -m "garcard 0.1.0-rc1"`
2. Push tag:
   - `git push origin v0.1.0-rc1`

## GA Signoff (2026-02-26)
1. No unresolved critical gaps in parity matrix.
   - Status: PASS (`examples/sprint-08-parity-matrix.md`)
2. Rollback plan reviewed:
   - `docs/runbooks/rollback-plan.md`
   - Status: PASS
3. Operator handoff includes:
   - parity report
   - daemon logs for interactive scenarios
   - final command cookbook
   - Status: PASS
