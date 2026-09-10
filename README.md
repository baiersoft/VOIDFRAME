<div align="center">

# `baiersoft // VOIDFRAME`
### *Autonomous CS2 Benchmark Orchestrator*

[![License: MIT](https://img.shields.io/badge/License-MIT-emerald?style=for-the-badge)](LICENSE)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-f5b544?style=for-the-badge)](#this-is-an-alpha--what-that-means)
[![Tauri 2](https://img.shields.io/badge/Tauri-2.x-24c8db?style=for-the-badge&logo=tauri&logoColor=white)](https://tauri.app/)
[![Rust](https://img.shields.io/badge/Rust-1.98+-dea584?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![React 19](https://img.shields.io/badge/React-19-61dafb?style=for-the-badge&logo=react&logoColor=black)](https://react.dev/)
[![Tailwind CSS v4](https://img.shields.io/badge/Tailwind_CSS-v4-38bdf8?style=for-the-badge&logo=tailwindcss&logoColor=white)](https://tailwindcss.com/)
[![Intel PresentMon](https://img.shields.io/badge/Intel_PresentMon-v2.5.1_ETW-0071c5?style=for-the-badge&logo=intel&logoColor=white)](https://www.presentmon.com/)
[![HWiNFO](https://img.shields.io/badge/HWiNFO-bundled-6aa84f?style=for-the-badge)](https://www.hwinfo.com/)
[![Windows](https://img.shields.io/badge/Windows-10_%7C_11_(64--Bit)-0078d4?style=for-the-badge&logo=windows&logoColor=white)](https://microsoft.com/windows)

**Autonomous Reboot Pipelines. Deterministic CS2 Telemetry. Zero Guesswork.**

[Installation](INSTALL.md) • [Guides](#-guides) • [Releases](../../releases) • [License](LICENSE)

</div>

---

## ⚠️ Important Disclaimer — Alpha Software, Use At Your Own Risk

> [!WARNING]
> **NO WARRANTY. NO GUARANTEE OF ANY OUTCOME.**
>
> 1. **Alpha hobby / research project.** `baiersoft // VOIDFRAME` is an
>    independent project built primarily for the author's own CS2 testing,
>    published for transparency and for other **expert users / tweakers**.
>    It is **not** a polished consumer product.
> 2. **AI-assisted development.** The codebase, specifications and UI were
>    developed collaboratively with agentic AI coding assistants.
> 3. **Deliberately limited scope.** This alpha ships a small,
>    confirmed-working set of system tweaks rather than a large, unverified
>    one — see below. Windows 10/11 desktop only, CS2 only.
> 4. **Elevated, real system modification — including unattended reboots.**
>    VOIDFRAME runs elevated and edits real registry keys, power schemes,
>    CPU affinity and your CS2 video config to run its benchmarks. A
>    scenario that needs a reboot (HAGS) **reboots your PC on its own** and
>    resumes after Windows logs back in; you can also opt in to having the
>    machine **shut itself down** when the run is complete. Every change is
>    journaled, a standalone restore script is written to your Desktop for
>    the duration of the run, and an Emergency Restore path exists if a run
>    is interrupted mid-way. Still: read
>    [Unattended reboots](#-unattended-reboots--read-before-your-first-hags-run)
>    below, understand what a tweak does, and keep a system restore point
>    before your first run.
> 5. **Custom scripts run as administrator.** The Custom Script tweak runs
>    *your* `.bat` / `.cmd` / `.ps1` files elevated. VOIDFRAME records their
>    hashes and runs your revert script, but it cannot know what your script
>    did — such results are flagged "script-reverted, unverified".
> 6. **Anti-cheat.** VOIDFRAME uses only official interfaces and **never
>    injects into or reads/modifies game memory**. It makes **no guarantee**
>    regarding Valve Anti-Cheat or any third-party anti-cheat.
> 7. **Unsigned alpha.** This build has no Authenticode code signature — a
>    deliberate choice, not an oversight. See [INSTALL.md](INSTALL.md) for
>    what to expect from Windows.
> 8. **Limitation of liability.** The authors and contributors assume
>    **zero liability** for system crashes, data loss, or any other
>    consequence of use.

---

## 🌌 Overview

**`baiersoft // VOIDFRAME`** is a self-orchestrating benchmark suite for
*Counter-Strike 2*, aimed at people who already tweak their systems and want
**measured proof on their own hardware** instead of trusting a claim.

- **Automated Benchmark Pipeline** — applies a tweak, launches CS2, captures
  frame times, reverts the tweak, and compares against an unmodified
  baseline. Two benchmark kinds: the Dust2 Workshop benchmark map, or
  AveYo's `benchmark.cfg` v2 (bundled, no Workshop subscription needed).
- **Autonomous Reboot Pipelines** — reboot-required tweaks apply, reboot,
  resume after AutoLogon with elevated privileges via a scheduled task, wait
  for Windows to settle, measure, revert, and reboot back to stock. Start it
  in the evening, wake up to results — optionally with the PC powered off.
- **Intel PresentMon Backend (bundled)** — frame times, 1% / 0.1% lows,
  render latency.
- **WCPS + Significance Testing** — a dimensionless, baseline-normalized
  score reported alongside a *better / worse / no measurable difference*
  verdict with confidence intervals, not just a raw number. See
  [Understanding your benchmark results](docs/guides/understanding-your-benchmark-results.md).
- **Journaled Safety Architecture** — every system change is journaled and
  rolled back; a deadman task and a standalone `VOIDFRAME_RESTORE.bat` on
  your Desktop cover the case where VOIDFRAME itself never comes back after
  a reboot; an Emergency Restore path exists if a run is interrupted mid-way.
- **Thermal Heat-Soak Protection** — via HWiNFO (bundled) shared memory,
  with inter-scenario cooldown so back-to-back runs aren't skewed by heat.
- **Obsidian Void Design** — deep `#010103` voids, glassmorphic HUD panels,
  Space Grotesk / JetBrains Mono, powered by **baiersoft**.

---

## 🎯 Tweak Catalog (Alpha)

The builder offers eight tweaks in this alpha, each confirmed end-to-end on
real hardware:

| Tweak | What it changes | Reboot? |
| :--- | :--- | :---: |
| **Launch options** | CS2 launch arguments | — |
| **Power plan** | Windows power plan selection | — |
| **Affinity: exclude core 0** | CPU affinity mask excluding the first logical core | — |
| **Hardware-Accelerated GPU Scheduling (HAGS)** | `HwSchMode` registry value | **Yes** |
| **Windows Game Mode** | `AutoGameModeEnabled` registry value | — |
| **Win32PrioritySeparation** | Foreground/background scheduling quantum (twelve named presets) | — |
| **CS2 Video Config** | Curated `cs2_video.txt` settings (display mode, resolution, VSync, quality presets, Reflex, …) plus raw key/value overrides; whole-file snapshot and restore | — |
| **Custom Script** *(expert)* | Your own `.bat` / `.cmd` / `.ps1` apply script, with a **mandatory** revert script | optional |

A HAGS scenario on a machine that already has HAGS in the requested state is
blocked at pre-flight rather than burning a reboot for a no-op — the builder
greys out the choice that matches your live value.

Three more entries — Disable Core Parking, Disable CPU Idle States, and
P-Core Only Affinity — are implemented in the engine but hidden from the
builder until they've been independently validated end-to-end.

---

## 🔁 Unattended reboots — read before your first HAGS run

This is what an unattended run does, so your own machine rebooting or
shutting down overnight is not a surprise:

1. **Pre-flight refuses to start a reboot run without AutoLogon.** Windows
   must log you back in without a password prompt, or the run stalls at the
   lock screen. Set it up first:
   [AutoLogon setup guide](docs/guides/autologon-setup.md). A Microsoft
   account or Windows Hello sign-in, a plaintext `DefaultPassword` in the
   registry, BitLocker, and Secure Boot each produce a warning you should
   read. Sleep is inhibited automatically for the duration of the run.
2. **The run-countdown dialog shows the reboot count** for your project and
   a **"Shut down when the run completes"** checkbox (default configurable
   in Settings). This is your last chance to cancel.
3. **When a scenario needs a reboot,** the Live Monitor shows a
   *reboot pending* phase and the machine reboots about ten seconds later.
   Before it does, VOIDFRAME registers two scheduled tasks — one that
   relaunches it at your logon, one deadman task — and rewrites
   `VOIDFRAME_RESTORE.bat` on your Desktop.
4. **After logon, VOIDFRAME reopens itself on the Live Monitor** and
   counts down the **post-boot settle wait** (default 180 s, adjustable in
   Settings) so Windows' post-boot maintenance doesn't leak into the
   capture. It then checks how the boot went, measures, reverts the tweak,
   and reboots once more to return to stock.
5. **A crash while the tweak is applied** (bugcheck, power loss, an extra
   reboot VOIDFRAME didn't ask for) is detected on the next boot: the
   scenario is reverted, marked *unstable* on the Results view with the
   reason, and the run continues with the next scenario.
6. **If VOIDFRAME never comes back after a reboot** (the resume task could
   not launch it), the deadman task fires about ten minutes after boot,
   reverts every journaled change, and removes both tasks. The next launch
   shows the crash-recovery banner for that run, and the run's folder holds
   a `recovered.json` marker recording that the deadman did the revert.
7. **Auto-shutdown**, if enabled, replaces the final restore reboot: the
   machine is already back at stock and the results are on disk. It fires
   only after a successful rollback, with a 60-second notice and a cancel
   button on the Results view. It never fires if a revert failed — that
   must be seen by a human.

Do not move, rename or uninstall VOIDFRAME while a reboot run is in
progress: both scheduled tasks point at the installed `voidframe.exe`.

---

## 🧰 Tech Stack

| Layer | Stack |
| :--- | :--- |
| Shell | Tauri 2.x (WebView2) |
| Backend | Rust 1.98 — `windows`, `tokio`, `serde`, `tracing` |
| Frontend | React 19, TypeScript 5.7 (strict), Tailwind CSS v4, Zustand |
| Charts | uPlot (frame-time timelines) + Recharts (bar/delta) |
| Benchmark engine | Intel PresentMon CLI v2.5.1 (ETW), bundled |
| Thermal telemetry | HWiNFO shared memory, bundled |

---

## 📋 Requirements

Windows 10 (21H2+) or Windows 11, 64-bit, with Steam and Counter-Strike 2
already installed. Administrator rights are required (the app runs elevated
to apply system tweaks). Reboot-required scenarios additionally need
Windows AutoLogon configured. The Dust2 Workshop benchmark needs a
subscription to the Workshop map; the AveYo benchmark kind does not. See
[INSTALL.md](INSTALL.md) for the full walkthrough.

---

## 📖 Guides

- [Installation](INSTALL.md) — download, the two unsigned-app warnings,
  first run, what to do before a reboot run, and where the recovery
  artifacts live.
- [AutoLogon setup](docs/guides/autologon-setup.md) — the three ways to make
  Windows log in on its own for unattended reboot runs, and the one way not
  to.
- [Understanding your benchmark results](docs/guides/understanding-your-benchmark-results.md)
  — what every number on the Results screen means and which two a
  competitive player should actually look at.

---

## This is an alpha — what that means

- **Unsigned**: this build has no Authenticode code signature (no
  commercial certificate, no self-signed root — a deliberate choice, not an
  oversight). Windows will show a SmartScreen warning on the installer and
  an "Unknown publisher" UAC prompt on launch. Both are expected for every
  unsigned Windows application, not a sign of tampering. Verify your
  download against the `SHA256SUMS.txt` published alongside each release if
  you want to confirm integrity yourself.
- **No auto-updater**: check the [Releases page](../../releases) for new
  versions.
- **Single-author project**: response times on issues will vary.

---

## 🗺️ Roadmap

- Expanding the validated tweak catalog beyond the eight confirmed in this
  alpha (the three hidden power/affinity entries first)
- GPU / driver-level tweaks and driver automation (NVIDIA)
- Broader hardware/config coverage as more real-world runs come in
- Code signing is not currently planned for this alpha branch

---

## 🛠️ Third-Party Tools & Acknowledgements

VOIDFRAME bundles two third-party tools and one third-party benchmark
config:

| Tool | Creator / Maintainer | Purpose | License |
| :--- | :--- | :--- | :--- |
| [HWiNFO](https://www.hwinfo.com/) | Martin Malík / REALiX | Hardware monitoring & thermal cooldown | Proprietary — bundled under a direct redistribution permission from the author |
| [PresentMon](https://www.presentmon.com/) | Intel Corporation (GameTechDev) | Frame-time / ETW capture | MIT |
| [AveYo `benchmark.cfg` v2](https://github.com/AveYo/Gaming/tree/main/CS2) | AveYo | Alternative CS2 benchmark kind, bundled byte-for-byte and written into CS2's `cfg\` folder when selected | MIT |

Full license/notice text for the tools ships under `src-tauri/binaries/`,
and for the benchmark config under `data/cs2-benchmarks/aveyo/`, in this
repository. *All trademarks and brand names are the property of their
respective owners.*

---

## 📂 Documentation

This is a public alpha mirror of a larger private development repo — some
source comments reference internal design documents that aren't included
here; they're artifacts of the private process and can be safely ignored.
The tester-facing guides under [`docs/guides/`](docs/guides/) are included.
For setup, see [INSTALL.md](INSTALL.md).

---

## 📜 License

MIT — see [LICENSE](LICENSE). Copyright © 2026 **baiersoft**.
*Architects of the unseen.*
