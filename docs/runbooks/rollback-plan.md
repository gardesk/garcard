# Rollback Plan

## Trigger Conditions
1. Critical auth regressions (correct password denied, prompt deadlock, no-agent after restart).
2. Authority connectivity failures not recoverable by daemon/polkit restart.
3. Control-plane failures in `garcardctl` lifecycle commands.

## Immediate Containment
1. Stop garcard user service:
   - `systemctl --user disable --now garcard.service`
2. Re-enable prior known-good external agent:
   - `systemctl --user enable --now <previous-agent>.service`
3. Verify authentication fallback path:
   - `pkcheck --allow-user-interaction --process $$ --action-id com.mesonbuild.install.run`

## Artifact Rollback
1. Revert to prior release tag/commit in deployment repo.
2. Reinstall previous binaries/packages.
3. Restart user service stack for session.

## Verification
1. `pkcheck` challenge opens and accepts valid credentials.
2. Previous agent remains stable across repeated auth attempts.
3. Incident evidence captured for postmortem:
   - daemon logs
   - `garcardctl` outputs
   - exact rollback commit/tag

## Recovery Path Back To Garcard
1. Resolve root cause in staging.
2. Re-run Sprint 08 parity script and matrix.
3. Reattempt controlled rollout with rollback gate in place.
