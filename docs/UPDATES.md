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

## Release pipeline (to build)

One-time setup:

```sh
termcraft-cli update keygen
# → TERMCRAFT_UPDATE_SECRET=…  (GitHub Actions secret, never committed)
# → TERMCRAFT_UPDATE_PUBKEY=…  (GitHub Actions variable, baked into builds)
```

Per release:

1. Build each platform with `TERMCRAFT_UPDATE_PUBKEY` set.
2. Write the manifest with `termcraft-cli update hash <file>` for each asset.
3. `TERMCRAFT_UPDATE_SECRET=… termcraft-cli update sign update-stable.json`
4. Upload the binaries, the manifest and the `.sig` to the release.

Keep the secret key offline, or in a protected GitHub environment that needs
manual approval. Anyone holding it can push code to every user.

## Not handled yet

- macOS `.app` bundles: the whole bundle has to be swapped and re-verified
  by Gatekeeper.
- Delta updates.
- Linux distro packages (`.deb`, `.rpm`, Flatpak) should update through
  their package manager. Self-update should be disabled in those builds.
