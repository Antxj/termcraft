# Development notes

## Toolchain

On Windows, use the MSVC toolchain. The GNU toolchain fails with
`error calling dlltool 'dlltool.exe': program not found`. To switch only
this project, run `rustup override set stable-msvc` in the repository root.
To switch every project, run `rustup default stable-msvc`. The SSH stack uses `ring` instead of `aws-lc-rs`,
so NASM is not required.

## Local SSH test server

`tc-devserver` is a small SSH server for development and integration tests.
It binds to **127.0.0.1 only** and serves the local shell (via `tc-pty`) and
exec requests, with password and public-key authentication. Its host key is
derived from a fixed seed, so a `known_hosts` entry stays valid across runs.

```sh
cargo run -p tc-devserver -- --port 2222
```

Test credentials (dev only, override with `TC_DEV_USER` / `TC_DEV_PASSWORD`):

| User | Password |
|---|---|
| `termcraft` | `termcraft-dev` |

Create a session for it:

```sh
cargo run -p tc-cli -- run session_add '{"session":{"name":"devserver","type":"ssh","host":"127.0.0.1","port":2222,"user":"termcraft"}}'
```

`cargo test -p tc-ssh` starts the same server in-process on a random port.
That run covers host-key trust and change detection, password and
public-key login, and a wrong password followed by cancel.

## UI snapshots (no input injection)

The app can render a scripted scene and save its own frame as a PNG. It
never touches the OS mouse or keyboard, and the window does not take focus.

```powershell
$env:TERMCRAFT_SNAPSHOT = "shot.png"           # required: output file
$env:TERMCRAFT_SNAPSHOT_SESSION = "devserver"  # optional: session to open
$env:TERMCRAFT_SNAPSHOT_PASSWORD = "..."       # optional: dev server password only
$env:TERMCRAFT_SNAPSHOT_DELAY_MS = "7000"      # optional: settle time
cargo run -p tc-ui
```

Other switches (see `crates/tc-ui/src/snapshot.rs`): `TERMCRAFT_SNAPSHOT_LOCAL_TABS`,
`TERMCRAFT_SNAPSHOT_WINDOW`, `TERMCRAFT_SNAPSHOT_TUNNELS`, `TERMCRAFT_SNAPSHOT_HOSTKEY`,
`TERMCRAFT_SNAPSHOT_TRAY` (adds the tray icon, iconifies and shows the window again).
The snapshot window stays below other windows and lets mouse clicks
pass through, so it cannot catch a click meant for another window.

Run it from a portable folder (with `termcraft.portable`) so your real
settings are never touched.

## Driving the GUI in tests

Prefer UI snapshots (above). Never inject OS mouse or keyboard events while
someone is using the machine: if another window is in front, the clicks and
keystrokes go to that window. The reference app is never executed (see
[CLEAN_ROOM.md](CLEAN_ROOM.md)).
