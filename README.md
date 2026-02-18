# garcard

`garcard` is the in-progress Polkit authentication agent for the gar desktop suite.

## Workspace
1. `garcard`: daemon runtime
2. `garcard-ipc`: shared protocol types
3. `garcardctl`: control/debug CLI

## Quick Start
1. `cargo run -p garcard -- daemon`
2. `cargo run -p garcardctl -- status`

## Config
Default config path: `~/.config/garcard/config.toml`

Environment overrides:
1. `GARCARD_SOCKET`
2. `GARCARD_SOCKET_MODE`
3. `GARCARD_CONFIG`
4. `GARCARD_AGENT_BACKEND`
5. `GARCARD_POLKIT_OBJECT_PATH`
6. `GARCARD_LOCALE`

See `examples/config.toml` for a starter file.
