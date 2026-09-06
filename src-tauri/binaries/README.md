# Bundled binaries

This directory holds the sidecar binaries `tauri.conf.json`'s `bundle.externalBin` wires
into the installer.

| File | What it is | License / terms |
| :--- | :--- | :--- |
| `presentmon-x86_64-pc-windows-msvc.exe` | `PresentMon.exe`, pinned **v2.5.1** (Intel/GameTechDev) | MIT — full text in [`PresentMon-LICENSE.txt`](./PresentMon-LICENSE.txt), verbatim from <https://github.com/GameTechDev/PresentMon> |
| `hwinfo64-x86_64-pc-windows-msvc.exe` | `HWiNFO64.exe` (Martin Malík / REALiX, <https://www.hwinfo.com/>) | Proprietary — bundled under a direct redistribution permission granted by the author, **not** an open-source license. See [`HWiNFO64-NOTICE.txt`](./HWiNFO64-NOTICE.txt) — no formal license file was provided by HWiNFO's official download, so none is reproduced here. |

`src-tauri/resources/hwinfo/HWiNFO64.ini` (bundled as a `resources` entry, not an
`externalBin`) ships alongside `hwinfo64-x86_64-pc-windows-msvc.exe` with Shared Memory
Support pre-enabled (`SensorsSM=1`), captured from a real running HWiNFO64 instance per
`crates/voidframe-engine/src/system/windows/hwinfo.rs`'s live-verification discipline.

`src-tauri/build.rs` fails any `--release` build if either `.exe` here isn't a real PE
binary (checks for the `MZ` magic bytes) — a safeguard against ever shipping a stale or
placeholder file, not something you need to think about as long as real binaries are here.
