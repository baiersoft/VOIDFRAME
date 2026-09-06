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

**Deterministic CS2 Telemetry. Zero Guesswork.**

[Installation](INSTALL.md) • [Releases](../../releases) • [License](LICENSE)

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
> 4. **Elevated, real system modification.** VOIDFRAME runs elevated and
>    edits real registry keys, power schemes, and CPU affinity to run its
>    benchmarks. Every change is journaled; an Emergency Restore path exists
>    if a run is interrupted mid-way. Still: understand what a tweak does
>    and keep a system restore point before your first run.
> 5. **Anti-cheat.** VOIDFRAME uses only official interfaces and **never
>    injects into or reads/modifies game memory**. It makes **no guarantee**
>    regarding Valve Anti-Cheat or any third-party anti-cheat.
> 6. **Unsigned alpha.** This build has no Authenticode code signature — a
>    deliberate choice, not an oversight. See [INSTALL.md](INSTALL.md) for
>    what to expect from Windows.
> 7. **Limitation of liability.** The authors and contributors assume
>    **zero liability** for system crashes, data loss, or any other
>    consequence of use.

---

## 🌌 Overview

**`baiersoft // VOIDFRAME`** is a self-orchestrating benchmark suite for
*Counter-Strike 2*, aimed at people who already tweak their systems and want
**measured proof on their own hardware** instead of trusting a claim.

- **Automated Benchmark Pipeline** — applies a tweak, launches CS2, captures
  frame times, and compares against an unmodified baseline.
- **Intel PresentMon Backend (bundled)** — frame times, 1% / 0.1% lows,
  render latency.
- **WCPS + Significance Testing** — a dimensionless, baseline-normalized
  score reported alongside a *better / worse / no measurable difference*
  verdict with confidence intervals, not just a raw number.
- **Journaled Safety Architecture** — every system change is journaled and
  can be rolled back; an Emergency Restore path exists if a run is
  interrupted mid-way.
- **Thermal Heat-Soak Protection** — via HWiNFO (bundled) shared memory,
  with inter-scenario cooldown so back-to-back runs aren't skewed by heat.
- **Obsidian Void Design** — deep `#010103` voids, glassmorphic HUD panels,
  Space Grotesk / JetBrains Mono, powered by **baiersoft**.

---

## 🎯 Tweak Catalog (Alpha)

The tweak catalog ships six selectable entries. Three are validated
end-to-end for this alpha:

- **Launch options** — CS2 launch argument changes
- **Power plan** — Windows power plan selection
- **Affinity: exclude core 0** — CPU affinity mask excluding the first
  logical core

The remaining three — Disable Core Parking, Disable CPU Idle States, and
P-Core Only Affinity — are implemented and selectable in the app, but have
not yet been independently validated end-to-end for this alpha. Treat their
results with more caution until they are.

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
to apply system tweaks). See [INSTALL.md](INSTALL.md) for the full
walkthrough.

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

- Expanding the validated tweak catalog beyond the three confirmed in this
  alpha
- Broader hardware/config coverage as more real-world runs come in
- Code signing is not currently planned for this alpha branch

---

## 🛠️ Third-Party Tools & Acknowledgements

VOIDFRAME bundles two third-party tools:

| Tool | Creator / Maintainer | Purpose | License |
| :--- | :--- | :--- | :--- |
| [HWiNFO](https://www.hwinfo.com/) | Martin Malík / REALiX | Hardware monitoring & thermal cooldown | Proprietary — bundled under a direct redistribution permission from the author |
| [PresentMon](https://www.presentmon.com/) | Intel Corporation (GameTechDev) | Frame-time / ETW capture | MIT |

Full license/notice text for both ships under `src-tauri/binaries/` in this
repository. *All trademarks and brand names are the property of their
respective owners.*

---

## 📂 Documentation

This is a public alpha mirror of a larger private development repo — some
source comments reference internal design documents that aren't included
here; they're artifacts of the private process and can be safely ignored.
For setup, see [INSTALL.md](INSTALL.md).

---

## 📜 License

MIT — see [LICENSE](LICENSE). Copyright © 2026 **baiersoft**.
*Architects of the unseen.*
