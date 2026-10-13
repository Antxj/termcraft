# Reimplementation policy

TermCraft aims for **1:1 parity** with MobaXterm: the same menus, dialogs,
shortcuts, settings and behavior, with its own code and visual identity.
This follows the model used by storytold/photocraft for Photoshop.

TermCraft is an independent project for personal use. It is not affiliated
with Mobatek.

## Allowed sources

- **Observing the running app:** screens, menus, dialogs, layouts, flows,
  and what each option does.
- **Static inspection of the installed or portable files, without running
  them:** configuration files (`MobaXterm.ini`), exported sessions, the list
  of bundled components, the documented shortcut list, and **strings
  embedded in the executable** (menu labels, option names, messages) to build
  inventories of menus, settings and dialogs.
- **Output oracles:** files produced by the original (exported sessions,
  `.ini` files, logs), used as test fixtures in a corpus that is kept
  outside git.
- Public documentation, open standards and RFCs (SSH, SFTP, Telnet, RDP
  MS-RDPBCGR, VNC RFC 6143, X11).
- **Bundled open-source components** (PuTTY, Xorg/VcXsrv, Cygwin tools),
  under their own licenses and with attribution.

## Not done

- Decompiling or disassembling proprietary code, or translating its
  algorithms. Inventories come from strings and resources, never from logic.
- Shipping or committing Mobatek icons, logos, images, sounds, or the
  MobaXterm name and branding. TermCraft uses its own icons.
- Copying long-form text such as help pages or the EULA. Short UI labels
  (menu items, option names) may match, because that is what parity means.
- Committing proprietary files. The original app and its extracted files
  live in `reference/`, which is git-ignored.

## Reference workspace

`reference/` (git-ignored) holds the downloaded original, its extraction,
and the generated inventories. Tools in `xtask` read from there and write
checklists into `docs/`.
