# garcard

`garcard` is the in-progress Polkit authentication agent for the gar desktop suite.

## Workspace
1. `garcard`: daemon runtime
2. `garcard-ipc`: shared protocol types
3. `garcardctl`: control/debug CLI

## Quick Start
1. `cargo run -p garcard -- daemon`
2. `cargo run -p garcardctl -- status`
3. `cargo run -p garcard -- prompt --mode secret --message "Validation prompt"`

## Lifecycle Commands
1. `cargo run -q -p garcardctl -- diagnose`
2. `cargo run -q -p garcardctl -- temp-list`
3. `cargo run -q -p garcardctl -- temp-revoke <authorization-id>`
4. `cargo run -q -p garcardctl -- temp-revoke-all`

## User Service
1. Install unit file:
   - `install -Dm644 garcard.service ~/.config/systemd/user/garcard.service`
2. Enable and start:
   - `systemctl --user daemon-reload`
   - `systemctl --user enable --now garcard`
3. Check health:
   - `cargo run -q -p garcardctl -- status`

## Config
Default config path: `~/.config/garcard/config.toml`

Environment overrides:
1. `GARCARD_SOCKET`
2. `GARCARD_SOCKET_MODE`
3. `GARCARD_CONFIG`
4. `GARCARD_AGENT_BACKEND`
5. `GARCARD_POLKIT_OBJECT_PATH`
6. `GARCARD_LOCALE`
7. `GARCARD_POLKIT_HELPER_SOCKET`
8. `GARCARD_PROMPT_COMMAND`
9. `GARCARD_PROMPT_TIMEOUT_SECS`
10. `GARCARD_BACKEND_HEALTHCHECK_SECS`

Default scaffold file for packaging/integration: `config/garcard/config.toml`

See `examples/config.toml` for a minimal local starter file.

`GARCARD_PROMPT_COMMAND` is optional. If unset, `garcard` runs the built-in
gartk prompt path with a persistent in-process modal session and falls back to
`systemd-ask-password` when the X11 prompt backend is unavailable.

## Validation Docs
1. `examples/sprint-02-validation.md`
2. `examples/sprint-03-validation-report-2026-02-18.md`
3. `examples/sprint-04-validation.md`
4. `examples/validate-sprint-02.sh`
5. `examples/validate-sprint-03-integration.sh`
6. `examples/validate-sprint-04.sh`
7. `examples/validate-sprint-04-runtime.sh`
8. `examples/sprint-07-validation.md`
9. `examples/validate-sprint-07.sh`

## Troubleshooting
1. `Authorization requires authentication but no agent is available`
   - ensure daemon is running: `cargo run -q -p garcardctl -- ping`
   - inspect authority and subject health: `cargo run -q -p garcardctl -- diagnose`
   - restart daemon after polkit restart: `cargo run -q -p garcardctl -- quit` then relaunch
2. `failed to connect to garcard daemon ...`
   - check socket path from `garcardctl status`
   - if using custom socket, export the same `GARCARD_SOCKET` for both daemon and ctl
3. Prompt did not open in X11
   - run with debug logs: `RUST_LOG=garcard=debug cargo run -p garcard -- daemon`
   - verify fallback path by setting `GARCARD_PROMPT_COMMAND` explicitly

## Known Limitations
1. Policy results are host-specific; some actions may auto-authorize and not trigger prompts.
2. Current implementation targets logged-in user sessions on X11.
