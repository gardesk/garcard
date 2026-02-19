# Sprint 03 Integration Validation Report (2026-02-18)

## Scope
1. Static ecosystem wiring checks against parent Gardesk repo.
2. Runtime challenge-flow probes for logind and NetworkManager policy actions.

## Command
1. `./examples/validate-sprint-03-integration.sh ..`

## Static Check Results
1. Gar default init autostarts `garcard`.
2. External Polkit fallback notes remain present/commented.
3. Installer contains `garcard` component flag and install flow.
4. Installer provisions user service and config scaffold wiring.
5. Uninstaller removes `garcard` binaries, service, and config directory.
6. Root repo includes `config/garcard/config.toml` and `specs/garcard.spec`.
7. `gargears` includes `garcard` discovery socket mapping and panel component.
8. Script exited successfully with all checks passing.

## Runtime Probe Results
1. Daemon run mode:
   - `RUST_LOG=garcard=debug GARCARD_AGENT_BACKEND=polkit cargo run -p garcard -- daemon`
2. logind probe:
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.login1.manage`
   - Result: exit `1` (`Not authorized.`) with daemon callback logs:
     - `Started active polkit auth request action_id=org.freedesktop.login1.manage ...`
     - `Processing polkit auth request action_id=org.freedesktop.login1.manage ...`
3. NetworkManager probes:
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.NetworkManager.settings.modify.system`
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.NetworkManager.settings.modify.global-dns`
   - Result: authorized (`polkit.result=yes`) in this host policy context; no challenge callback required.
4. Policy inspection (`pkaction --verbose`) confirms host/session policy variance:
   - multiple NetworkManager actions resolve to active `yes` in this environment even when defaults are `auth_admin_keep`.
5. Host policy root-cause confirmation:
   - `/etc/static/polkit-1/rules.d/10-nixos.rules` contains:
     - `subject.isInGroup("networkmanager")` + `org.freedesktop.NetworkManager.*` -> `polkit.Result.YES`
   - session user groups include `networkmanager` (`id` output), so NetworkManager probes bypass challenge by design.

## Deferred Caveat Closure Plan
1. Use `examples/force-networkmanager-auth-admin.rules` as temporary override.
2. Install and reload policy:
   - `sudo install -m644 examples/force-networkmanager-auth-admin.rules /etc/polkit-1/rules.d/00-garcard-networkmanager-auth.rules`
   - `sudo systemctl restart polkit`
3. Re-run probe while daemon is active:
   - `pkcheck --allow-user-interaction --process $$ --action-id org.freedesktop.NetworkManager.settings.modify.system`
4. Expect callback logs from `garcard` auth request processing path.
5. Remove override and restart polkit after validation.

## Conclusion
1. Sprint 03 static integration wiring is in place.
2. logind-side runtime challenge callback is verified with live daemon.
3. NetworkManager challenge suppression cause is identified and reproducible.
4. A deterministic override path is documented to force and validate the callback path on this host.
