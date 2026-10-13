# Notes for AI agents and contributors

## Ground rules

- **Clean room.** Read [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md) first. Never
  decompile or disassemble proprietary software. Never copy proprietary
  assets or text.
- **Engine first.** `tc-core`, `tc-commands`, `tc-term`, `tc-import` and
  `tc-update` must not depend on `egui`/`eframe`. UI code lives only in `tc-ui`.
- **Everything is a command.** A new user-facing action is a variant of
  `tc_commands::Command` plus a `CommandSpec` in `catalog.rs`, with a valid
  `example_args`. The GUI menu, `termcraft-cli run` and the MCP tool all come
  from that single entry.
- **Parity list.** Update [docs/PARITY.md](docs/PARITY.md) in the same change
  that implements or partially implements a feature.

## Layout

| Crate | Role |
|---|---|
| `tc-core` | Sessions, settings, storage paths, portable mode |
| `tc-commands` | Command enum, catalog, `Host` trait, `execute` |
| `tc-term` | VT emulation wrapper, palette, key encoding |
| `tc-pty` | Local shell over ConPTY / PTY |
| `tc-import` | MobaXterm (and later PuTTY, ssh_config) importers |
| `tc-update` | Signed self-update |
| `tc-mcp` | MCP server over stdio |
| `tc-cli` | `termcraft-cli` binary |
| `tc-ui` | `termcraft` desktop binary (egui) |

## Reference workflow (parity)

1. Put the original portable zip in `reference/download/`. The folder is
   git-ignored and nothing in it is ever executed.
2. Run `cargo xtask extract` to get forms (`.dfm` text), embedded files and
   strings in `reference/extracted/`.
3. Run `cargo xtask inventory` to regenerate `docs/inventory/`, and
   `cargo xtask schema` to regenerate `crates/tc-core/src/schema_gen.rs`
   (session parameter schema). Never edit generated files by hand.
4. Implement against the inventory: same menus, labels, shortcuts, dialog
   layout and parameters. Use TermCraft's own icons.

## Checks before you finish

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

On Windows, use the MSVC toolchain (`rustup default stable-msvc`). The GNU
toolchain needs `dlltool`.
