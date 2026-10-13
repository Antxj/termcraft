# Auto-update

TermCraft updates itself from signed GitHub Releases. The same mechanism serves
the portable zip and installed copies.

## Client flow (`tc-update`)

1. Fetch `update-<channel>.json` and `update-<channel>.json.sig` from
   `releases/latest/download/`.
2. Verify the ed25519 signature against the public key(s) compiled into the
   binary (`TERMCRAFT_UPDATE_PUBKEY`, comma-separated to allow key rotation).
   Dev builds have no key, so updates are disabled.
3. Reject a manifest for another channel, and any version that is not
   strictly newer (no downgrades).
4. Download the asset for `<os>-<arch>` and check its exact size and SHA-256
   while streaming.
5. Replace the running executable (`self-replace`). The new version runs on
   next start.

Settings (`settings.toml`, `[update]`): `check_on_startup` (default `true`),
`auto_install` (default `false`), `channel` (`stable` | `beta`).

## Manifest

```json
{
  "channel": "stable",
  "version": "0.2.0",
  "notes": "Release notes…",
  "assets": [
    { "platform": "windows-x86_64", "url": "https://…/termcraft-windows-x86_64.exe",
      "sha256": "…", "size": 12345678 },
    { "platform": "linux-x86_64",   "url": "…", "sha256": "…", "size": 0 },
    { "platform": "macos-aarch64",  "url": "…", "sha256": "…", "size": 0 }
  ]
}
```

## Release pipeline

`.github/workflows/release.yml` runs when a `v*` tag is pushed:

1. It builds `termcraft` and `termcraft-cli` for `windows-x86_64`,
   `macos-aarch64`, `macos-x86_64` and `linux-x86_64`. The public key
   (`TERMCRAFT_UPDATE_PUBKEY`) and the update URL
   (`TERMCRAFT_UPDATE_BASE_URL`) are baked into each binary.
2. It packs a Windows portable zip containing both executables and a
   `termcraft.portable` marker.
3. It writes `update-stable.json` with
   `termcraft-cli update manifest --version … --base-url … platform=file…`
   and signs it with `termcraft-cli update sign`.
4. It publishes the GitHub release with every file. Tags containing `-`
   become prereleases.

One-time setup:

```sh
termcraft-cli update keygen
# TERMCRAFT_UPDATE_PUBKEY=…  → repository variable
# TERMCRAFT_UPDATE_SECRET=…  → secret of the "release" environment
```

Protect the `release` environment with required reviewers. Anyone holding
the secret can push code to every user.

## Not handled yet

- macOS `.app` bundles: the whole bundle has to be swapped and re-verified
  by Gatekeeper.
- Delta updates.
- Beta channel: clients read `releases/latest`, which skips prereleases, so
  `update-beta.json` needs its own location.
- Linux distro packages (`.deb`, `.rpm`, Flatpak) should update through
  their package manager. Self-update should be disabled in those builds.
