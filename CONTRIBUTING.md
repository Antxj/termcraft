# Contributing to TermCraft

TermCraft is written in Rust (edition 2024). The engine crates have no UI
dependency; the desktop app (`tc-ui`) is a thin `egui` layer on top.

## Build and run

```sh
cargo run -p tc-ui                 # desktop app
cargo run -p tc-cli -- commands    # list every command
cargo run -p tc-cli -- mcp         # MCP server on stdio (for AI agents)
cargo run -p tc-devserver          # loopback SSH server for manual tests (docs/DEV.md)
```

On Windows use the MSVC toolchain (`rustup default stable-msvc`). On Linux the
GUI needs `libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev
libxcb-xfixes0-dev libwayland-dev`.

## Checks before a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Layout

| Crate | Role |
|---|---|
| `tc-core` | Sessions, settings, storage paths, portable mode |
| `tc-commands` | Command enum, catalog, `Host` trait, `execute` |
| `tc-term` | Terminal emulation, charsets, highlighting, key encoding |
| `tc-pty` | Local shells over ConPTY / PTY |
| `tc-ssh` | Native SSH client: auth, host keys, shell, SFTP / SCP, tunnels, X11, agent |
| `tc-net` | Telnet, serial, rlogin, FTP / FTPS and S3 backends |
| `tc-servers` | TFTP and HTTP servers of the "Servers" window |
| `tc-devserver` | Loopback SSH server for tests |
| `tc-vault` | Saved passwords: OS keychain or Master-Password-encrypted file |
| `tc-import` | MobaXterm, PuTTY, OpenSSH config and WSL importers |
| `tc-update` | Signed self-update |
| `tc-mcp` | MCP server |
| `tc-cli` | `termcraft-cli` |
| `tc-ui` | `termcraft` desktop app |

**Everything is a command.** A user-facing action is a variant of
`tc_commands::Command` plus a `CommandSpec` in `catalog.rs`; the menus,
`termcraft-cli run` and the MCP tools come from that single list.

## Packages

```sh
cargo build --release -p tc-ui -p tc-cli
cargo xtask package          # portable zip (+ .deb on Linux, .app on macOS)
cargo xtask brand --settings defaults.toml --sessions team.mxtsessions
cargo xtask icons            # README icons (docs/icons)
```

Releases are built by `.github/workflows/release.yml` when a `v*` tag is
pushed; see [docs/UPDATES.md](docs/UPDATES.md) for update signing.

## Clean room

TermCraft is an independent implementation. Do not decompile or copy code,
artwork or text from other products; see [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md).
