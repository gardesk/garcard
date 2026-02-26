# GA Checklist

## Release Candidate Gate
1. `cargo test --workspace` passes.
2. `./examples/validate-sprint-08-parity.sh` completed.
3. `examples/sprint-08-parity-matrix.md` marked with scenario status and evidence links.
4. Incident/migration/operator runbooks reviewed:
   - `docs/runbooks/migrate-from-external-agent.md`
   - `docs/runbooks/incident-triage.md`
   - `docs/runbooks/garcardctl-cookbook.md`
5. `RELEASE_NOTES.md` reviewed and approved.

## Integration Certification
1. Gar startup default path validated for daemon availability.
2. User-service lifecycle validated (`enable`, `restart`, `disable`).
3. Status/diagnostics surface validated for external control-plane consumers.

## Tagging
1. Candidate tag:
   - `git tag -a v0.1.0-rc1 -m "garcard 0.1.0-rc1"`
2. Push tag:
   - `git push origin v0.1.0-rc1`

## GA Signoff
1. No unresolved critical gaps in parity matrix.
2. Rollback plan reviewed:
   - `docs/runbooks/rollback-plan.md`
3. Operator handoff includes:
   - parity report
   - daemon logs for interactive scenarios
   - final command cookbook
