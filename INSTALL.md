# Installation

## Requirements

- Windows 10 version 21H2 or later, or Windows 11 — 64-bit only
- Administrator rights (VOIDFRAME runs elevated to apply power-plan,
  registry, and affinity tweaks)
- Steam, with Counter-Strike 2 already installed
- No Visual C++ Redistributable needed — the binary is statically linked

## 1. Download

Grab the latest installer (`VOIDFRAME_x.y.z_x64-setup.exe`) from the
[Releases page](../../releases). A `SHA256SUMS.txt` is published alongside
it if you want to verify the download yourself:

```
certutil -hashfile VOIDFRAME_x.y.z_x64-setup.exe SHA256
```

Compare the output against the matching line in `SHA256SUMS.txt`.

## 2. Expect two warnings — both normal for an unsigned app

**"Windows protected your PC" (SmartScreen)**, when you first run the
installer: click **More info**, then **Run anyway**. This appears for any
unsigned Windows installer — it's not specific to VOIDFRAME and doesn't
indicate a problem with the download.

**"Do you want to allow this app to make changes to your device?" with
"Unknown publisher"**, on every launch: this is the standard Windows UAC
elevation prompt. It shows "Unknown publisher" because the app isn't
code-signed (a deliberate choice for this alpha — see the README). Click
**Yes**. VOIDFRAME needs administrator rights to apply the system tweaks it
benchmarks.

## 3. Install

Run the installer. It installs to `C:\Program Files\VOIDFRAME` by default
and bundles everything needed (PresentMon and HWiNFO are included — no
separate downloads required).

## 4. First run

VOIDFRAME ships with a ready-to-run **"Alpha Test — Exclude Core 0"**
project already seeded into a fresh install. Launch the app, select it from
Project Explorer, and run it to confirm everything works end-to-end before
building your own test scenarios.

## Uninstalling

Uninstall from **Settings → Apps** (or **Control Panel → Programs and
Features**) like any other Windows application. This removes the installed
program files and, if you choose "also delete app data" during uninstall,
your saved projects and run history under
`%LOCALAPPDATA%\baiersoft\VOIDFRAME`.

## Logs & troubleshooting

Logs live at `%LOCALAPPDATA%\baiersoft\VOIDFRAME\logs`. Include the
relevant log file when reporting an issue.
