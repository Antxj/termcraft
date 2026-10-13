# TermCraft

An open-source, all-in-one terminal and remote-access toolbox written from
scratch in pure Rust, for Windows, macOS and Linux. It provides tabs, local
shells, SSH, SFTP, tunnels, RDP/VNC, serial, X11 and MultiExec. It is drivable
by humans, scripts and AI agents.

> **Early alpha.** The local shell, saved sessions, MobaXterm session import,
> the CLI, the MCP server and the signed updater work. SSH currently goes
> through the system `ssh` client. Native SSH/SFTP is next. See
> [docs/PARITY.md](docs/PARITY.md).

## Run

```sh
cargo run -p tc-ui                 # desktop app
cargo run -p tc-cli -- commands    # list every command
cargo run -p tc-cli -- import-mobaxterm sessions.mxtsessions
cargo run -p tc-cli -- mcp         # MCP server on stdio
```

**Portable mode.** Put an empty `termcraft.portable` file next to the
executable. All data then lives in `termcraft-data/` beside it.

## Use from an AI agent (MCP)

```json
{ "mcpServers": { "termcraft": { "command": "termcraft-cli", "args": ["mcp"] } } }
```

## Design

- The engine comes first, with a thin `egui` UI on top. The core crates have
  no UI dependency.
- One command registry serves the GUI, the CLI and MCP, so they never drift
  apart.
- Updates use signed manifests (ed25519) plus SHA-256 checks. See
  [docs/UPDATES.md](docs/UPDATES.md).
- The project follows a clean-room policy. See
  [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE),
at your option.

TermCraft is an independent project. It is not affiliated with, sponsored by,
or endorsed by Mobatek or MobaXterm. Product names are used only to describe
compatibility (for example, importing exported session files).
