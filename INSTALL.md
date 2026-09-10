# Installation

## Requirements

- Windows 10 version 21H2 or later, or Windows 11 — 64-bit only
- Administrator rights (VOIDFRAME runs elevated to apply power-plan,
  registry, affinity and CS2-config tweaks, and to reboot the machine for
  reboot-required scenarios)
- Steam, with Counter-Strike 2 already installed
- For the **Dust2 Workshop** benchmark kind (the default): a subscription to
  the Dust2 benchmark Workshop map (`3240880604`). Pre-flight checks for it.
  The **AveYo `benchmark.cfg` v2** kind needs no Workshop map — the config
  is bundled and written into CS2's `cfg\` folder when a run starts.
- For **reboot-required scenarios** (HAGS): Windows AutoLogon, so the
  machine logs back in without a password prompt. See the
  [AutoLogon setup guide](docs/guides/autologon-setup.md). Pre-flight refuses
  to start a reboot run without it.
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
and bundles everything needed (PresentMon, HWiNFO and AveYo's benchmark
config are included — no separate downloads required).

## 4. First run

VOIDFRAME ships with a ready-to-run **"Alpha Test — Exclude Core 0"**
project already seeded into a fresh install. Launch the app, select it from
Project Explorer, and run it to confirm everything works end-to-end before
building your own test scenarios. It uses the Dust2 Workshop benchmark, so
subscribe to the map first (see Requirements).

When you create your own project, **New Project** lets you pick the
benchmark kind. Each kind seeds its own capture length and measurement-loop
default (Dust2: 3 loops; AveYo: 5 loops — AveYo's shorter capture needs
more loops for a stable verdict). You can raise the loop count afterwards;
more loops means a longer run and a more confident result.

## 5. Before your first reboot run

A project with a HAGS scenario reboots your PC unattended — at least twice
per reboot scenario (once to apply, once to revert). Read the
[Unattended reboots](README.md#-unattended-reboots--read-before-your-first-hags-run)
section of the README once, then check off:

1. **AutoLogon is configured** ([guide](docs/guides/autologon-setup.md)).
   Open **Pre-Flight** from the header bar before starting: it shows the
   AutoLogon result and any warnings (Microsoft account / Windows Hello,
   plaintext `DefaultPassword`, BitLocker, Secure Boot).
2. **Decide on auto-shutdown.** The run-countdown dialog has a
   **"Shut down when the run completes"** checkbox (off by default; the
   default can be changed in Settings). With it on, the final restore reboot
   becomes a shutdown — the machine is already back at stock and the
   results are on disk. You can also toggle it from the Live Monitor while
   the run is going, and cancel the 60-second shutdown notice from the
   Results view if you're at the keyboard when it fires.
3. **Post-boot settle time.** After each reboot VOIDFRAME waits (default
   180 s, **Settings → Post-boot Settle**) before measuring, so Windows'
   post-boot maintenance doesn't leak into the capture. The Live Monitor
   shows the countdown. Raise it on a machine with a lot of startup
   software.
4. **Leave the install alone while a run is in progress.** The resume and
   deadman scheduled tasks point at the installed `voidframe.exe`. Don't
   move, rename, uninstall or update VOIDFRAME mid-run; finish or cancel
   the run first.
5. **Know where the restore script is.** For the duration of a run,
   `VOIDFRAME_RESTORE.bat` sits on your Desktop (and in
   `%LOCALAPPDATA%\baiersoft\VOIDFRAME\recovery`). Running it by hand
   reverts every journaled change of the current run, even if VOIDFRAME
   itself won't start. It is deleted when the run ends cleanly.

After the reboot, VOIDFRAME relaunches itself at logon and opens directly
on the Live Monitor with the run's log restored. Just leave it alone.

## If something goes wrong

- **The app shows a crash-recovery banner on launch.** A previous run
  didn't end cleanly. If VOIDFRAME never came back after a reboot, the
  deadman task (about ten minutes after boot) already reverted every
  journaled change on its own and removed the scheduled tasks — the run's
  folder under `%LOCALAPPDATA%\baiersoft\VOIDFRAME\runs` then contains a
  `recovered.json` marker. Your system is back at stock; the run's partial
  results, if any, are still viewable.
- **A run is stuck or was interrupted and something is still applied.**
  Use **Emergency Restore** in the app, or run `VOIDFRAME_RESTORE.bat` from
  your Desktop.
- **A scenario shows as "unstable".** The machine did not boot cleanly
  while that tweak was applied (bugcheck, power loss, or an extra reboot
  VOIDFRAME didn't ask for). The scenario was reverted and the run
  continued; the reason is shown next to the badge.
- **A partially-completed run.** The Results view shows every scenario that
  finished, even if the run failed later.

## Uninstalling

Finish or cancel any run in progress first (see step 4 above), then
uninstall from **Settings → Apps** (or **Control Panel → Programs and
Features**) like any other Windows application. This removes the installed
program files and, if you choose "also delete app data" during uninstall,
your saved projects and run history under
`%LOCALAPPDATA%\baiersoft\VOIDFRAME`.

## Logs & troubleshooting

Logs live at `%LOCALAPPDATA%\baiersoft\VOIDFRAME\logs`. Each run also keeps
its own `run.log` under `%LOCALAPPDATA%\baiersoft\VOIDFRAME\runs\<run id>`.
Include the relevant app log and the run's `run.log` when reporting an
issue.
