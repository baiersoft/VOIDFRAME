//! HWiNFO Shared Memory Support reading + process lifecycle control.
//!
//! Process name, shared-memory name, and the binary layout below were all
//! confirmed live against a real running HWiNFO64.exe instance (see
//! `docs/superpowers/plans/2026-09-04-thermal-cooldown.md`, 2026-09-04)
//! -- `study/research/scripts/hwinfo_shared_memory_probe.ps1`'s
//! recorded findings, not assumed from community documentation alone
//! (`docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md`
//! §A1 names this as a required live-verification step).

use crate::error::{Error, Result};
use crate::system::HwinfoSensorSnapshot;
use std::path::Path;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile,
    VirtualQuery,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

/// Confirmed live -- the 64-bit build's real image name.
const HWINFO_PROCESS_NAME: &str = "HWiNFO64.exe";

/// Confirmed live: `OpenFileMapping` succeeded on this, the only
/// candidate tried. An unprefixed (session-namespace) fallback name existed
/// here previously -- removed: it was never actually needed on the
/// verification machine, and this app runs elevated while the session
/// namespace is reachable by any unprivileged process in the same session --
/// including, during `start_sync`'s up-to-15s readiness poll, one that
/// squats the fallback name with a small forged mapping before the real
/// HWiNFO publishes the `Global\` one. Trying only the `Global\` name closes
/// that path outright rather than just bounding the damage (see
/// [`map_shared_memory_bytes`]'s own size-clamping fix for the
/// defense-in-depth half of that same finding).
const SHARED_MEM_NAME: &str = r"Global\HWiNFO_SENS_SM2";

/// `dwSignature`'s confirmed live value -- ASCII "HWiS" read as a
/// little-endian `u32`.
const HWIS_SIGNATURE: u32 = 0x5369_5748;

/// Sub-offsets within one reading element (`SharedMemHeader::size_of_reading_element`,
/// confirmed 460 bytes/element) -- confirmed live.
const READING_LABEL_OFFSET: usize = 12;
const READING_LABEL_LEN: usize = 128;
const READING_VALUE_OFFSET: usize = 284;

/// Upper bound on how many bytes [`map_shared_memory_bytes`] will ever copy
/// out of the mapped view, even if the header claims a much larger reading
/// section than expected -- guards a corrupt or unexpected header from
/// driving an unbounded read. Comfortably above the confirmed real size
/// (`48 + 420 * 460` = 193,248 bytes, ~188.7 KiB, confirmed live), with headroom
/// for a future HWiNFO version reporting more sensors.
const MAX_SHARED_MEM_READ_BYTES: usize = 16 * 1024 * 1024;

/// HWiNFO's Shared Memory Support v2 header -- 48 bytes, confirmed live.
/// Only `signature` (validated) and the reading-section fields
/// are actually read; the sensor-section fields and `_reserved` exist
/// purely to keep every field after them at its confirmed real byte offset
/// (`#[repr(C, packed)]` lays fields out sequentially with no padding, so
/// omitting any of them would shift everything after it).
#[repr(C, packed)]
struct SharedMemHeader {
    signature: u32,
    version: u32,
    revision: u32,
    poll_time: i64,
    offset_of_sensor_section: u32,
    size_of_sensor_element: u32,
    num_sensor_elements: u32,
    offset_of_reading_section: u32,
    size_of_reading_element: u32,
    num_reading_elements: u32,
    /// Live verification confirmed `dwOffsetOfSensorSection` = 48, not 44 (where the
    /// community-documented 10-field header would otherwise end) -- these
    /// 4 bytes are real and structurally required for every offset below
    /// them to land correctly, even though their own meaning is unknown.
    _reserved: u32,
}

/// CPU temperature reading labels to match against -- confirmed exact label
/// text. `"CPU Package"` is the community-documented Intel label
/// (never observed live on this session's verification machine);
/// `"CPU (Tctl/Tdie)"` is the label actually confirmed live there (AMD
/// Ryzen 7 9800X3D). Matching either covers both major CPU vendors, not
/// just the one machine this was verified on. The scan below matches
/// whichever of these is encountered first in HWiNFO's own reading-array
/// order, not this list's order -- on real hardware only one of the two
/// ever appears (a CPU is Intel or AMD, never both), so this has no
/// practical effect today.
const CPU_TEMP_LABELS: &[&str] = &["CPU Package", "CPU (Tctl/Tdie)"];
/// Confirmed exact label text.
const GPU_TEMP_LABELS: &[&str] = &["GPU Temperature"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decodes a NUL-terminated Latin-1 (ISO-8859-1) byte buffer to a `String`.
/// HWiNFO's label/unit fields aren't pure ASCII -- live verification confirmed a
/// degree-sign byte (`0xB0`) in the adjacent `szUnit` field -- and Latin-1's
/// first 256 codepoints map 1:1 onto the same Unicode scalar values, so this
/// is correct without pulling in `encoding_rs` (not already a workspace
/// dependency).
fn latin1_to_string(buf: &[u8]) -> String {
    buf.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect()
}

/// Extracts one reading element's label (`szLabelOrig`) and current `Value`
/// out of `buf`, starting at byte offset `start`.
fn read_reading_element(buf: &[u8], start: usize) -> Result<(String, f64)> {
    let needed_end = start + READING_VALUE_OFFSET + 8;
    if needed_end > buf.len() {
        return Err(Error::msg(format!(
            "HWiNFO shared memory buffer too short for a reading element at offset {start} \
             (need {needed_end} bytes, have {})",
            buf.len()
        )));
    }
    let label_bytes =
        &buf[start + READING_LABEL_OFFSET..start + READING_LABEL_OFFSET + READING_LABEL_LEN];
    let label = latin1_to_string(label_bytes);

    let value_bytes: [u8; 8] = buf[start + READING_VALUE_OFFSET..start + READING_VALUE_OFFSET + 8]
        .try_into()
        .expect("slice is exactly 8 bytes, sliced from the bounds just checked above");
    let value = f64::from_le_bytes(value_bytes);

    Ok((label, value))
}

/// `Err` unless `signature` is the confirmed `dwSignature` value -- shared
/// by [`parse_shared_memory`] and [`map_shared_memory_bytes`] so both
/// reject a wrong/corrupt/mismatched section the same way, and (for
/// `map_shared_memory_bytes`) so this check runs *before* any other header
/// field is trusted for anything, not after.
fn validate_signature(signature: u32) -> Result<()> {
    if signature != HWIS_SIGNATURE {
        return Err(Error::msg(format!(
            "HWiNFO shared memory signature mismatch: expected {HWIS_SIGNATURE:#010x}, found \
             {signature:#010x} -- this isn't HWiNFO's Shared Memory Support v2 layout"
        )));
    }
    Ok(())
}

/// Parses a raw Shared Memory Support v2 buffer into a [`HwinfoSensorSnapshot`].
/// A pure function over `buf` so it's testable against a synthetic buffer,
/// without a live HWiNFO instance.
fn parse_shared_memory(buf: &[u8]) -> Result<HwinfoSensorSnapshot> {
    if buf.len() < std::mem::size_of::<SharedMemHeader>() {
        return Err(Error::msg(
            "HWiNFO shared memory buffer shorter than its own header".into(),
        ));
    }
    // SAFETY: `buf` was just checked above to be at least
    // `size_of::<SharedMemHeader>()` bytes, and `SharedMemHeader` is
    // `#[repr(C, packed)]` with only integer fields (no padding, no
    // pointers, no alignment requirement stricter than 1) -- reading it by
    // value out of an arbitrary byte offset is sound.
    let header: SharedMemHeader =
        unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const SharedMemHeader) };

    validate_signature(header.signature)?;

    let reading_size = header.size_of_reading_element as usize;
    let reading_base = header.offset_of_reading_section as usize;
    let num_readings = header.num_reading_elements as usize;

    let mut cpu_temp: Option<f64> = None;
    let mut gpu_temp: Option<f64> = None;
    for i in 0..num_readings {
        if cpu_temp.is_some() && gpu_temp.is_some() {
            break;
        }
        let start = reading_base + i * reading_size;
        let (label, value) = read_reading_element(buf, start)?;
        if cpu_temp.is_none() && CPU_TEMP_LABELS.contains(&label.as_str()) {
            cpu_temp = Some(value);
        }
        if gpu_temp.is_none() && GPU_TEMP_LABELS.contains(&label.as_str()) {
            gpu_temp = Some(value);
        }
    }

    Ok(HwinfoSensorSnapshot {
        cpu_temp_celsius: cpu_temp.ok_or_else(|| {
            Error::msg(format!(
                "no reading matched any of {CPU_TEMP_LABELS:?} -- CPU temperature sensor not \
                 found in HWiNFO's shared memory"
            ))
        })?,
        gpu_temp_celsius: gpu_temp,
    })
}

/// Opens HWiNFO's Shared Memory Support mapping by its confirmed `Global\`
/// name only -- see [`SHARED_MEM_NAME`]'s own doc comment for why the
/// previously-tried unprefixed fallback name was removed rather than kept.
fn open_shared_memory() -> Result<HANDLE> {
    let wide_name = wide(SHARED_MEM_NAME);
    // SAFETY: `wide_name` is a local, NUL-terminated `Vec<u16>` that
    // outlives this call (dropped only after `OpenFileMappingW`
    // returns); `FILE_MAP_READ.0` requests read-only access to an
    // already-existing mapping -- this call never creates one.
    let opened = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(wide_name.as_ptr())) };
    opened.map_err(|_| {
        Error::msg(format!(
            "OpenFileMappingW failed for \"{SHARED_MEM_NAME}\" -- HWiNFO's own \"Shared Memory \
             Support\" setting is probably not enabled (VOIDFRAME never enables it on the user's \
             behalf)"
        ))
    })
}

/// Opens + maps HWiNFO's shared memory, copies out exactly the bytes the
/// mapping's own header says it needs (header plus the whole reading
/// section, bounded by [`MAX_SHARED_MEM_READ_BYTES`]), then unmaps and
/// closes the handles on every path out.
fn map_shared_memory_bytes() -> Result<Vec<u8>> {
    let handle = open_shared_memory()?;
    let result = (|| -> Result<Vec<u8>> {
        // SAFETY: `handle` is a live file-mapping handle just returned by
        // `OpenFileMappingW` inside `open_shared_memory` above.
        // `dwFileOffsetHigh`/`dwFileOffsetLow` = 0 and
        // `dwNumberOfBytesToMap` = 0 map the view from the mapping's own
        // start through to its end, as documented for `MapViewOfFile`.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            return Err(Error::msg(format!(
                "MapViewOfFile({SHARED_MEM_NAME}) failed: {}",
                windows::core::Error::from_thread()
            )));
        }
        let read_result = (|| -> Result<Vec<u8>> {
            // SAFETY: `view.Value` was just checked non-null above, and
            // points at a live mapped view of a real HWiNFO section, which
            // `OpenFileMappingW` only ever opens against an
            // already-created mapping -- confirmed live to be at
            // least header-sized. `SharedMemHeader` is `#[repr(C,
            // packed)]` with only integer fields, so an unaligned read is
            // sound regardless of the pointer's actual alignment. This is
            // the ONLY thing read from `view.Value` before the signature
            // check just below rejects a wrong/corrupt/mismatched section
            // -- nothing here yet trusts any header field for sizing a
            // further read.
            let header: SharedMemHeader =
                unsafe { std::ptr::read_unaligned(view.Value as *const SharedMemHeader) };

            // Validated before anything below trusts `offset_of_reading_section`/
            // `num_reading_elements`/`size_of_reading_element` for a second,
            // larger read -- mirrors `parse_shared_memory`'s own
            // bounds-before-trust ordering. Without this check here (not
            // just in `parse_shared_memory`, which only ever sees bytes
            // this function already read), a same-named section from a
            // different HWiNFO version, a corrupted mid-poll write, or an
            // unrelated process squatting the `Global\` namespace could
            // have its header-derived fields drive the `slice::from_raw_parts`
            // call below past the section's actual extent before ever
            // getting rejected.
            validate_signature(header.signature)?;

            let needed = (header.offset_of_reading_section as usize).saturating_add(
                (header.num_reading_elements as usize)
                    .saturating_mul(header.size_of_reading_element as usize),
            );
            if needed == 0 || needed > MAX_SHARED_MEM_READ_BYTES {
                return Err(Error::msg(format!(
                    "HWiNFO shared memory header reports an implausible reading-section extent \
                     ({needed} bytes) -- refusing to read past {MAX_SHARED_MEM_READ_BYTES} bytes"
                )));
            }

            // `needed` above is computed entirely from fields written by
            // whatever process created this mapping -- trusting it alone
            // would only be an assumption about the producer, not a checked
            // invariant (a same-named section from a different HWiNFO
            // version, a corrupted mid-poll write, or -- before the
            // `Global\`-only fix on `open_shared_memory` -- a same-session
            // process squatting the mapping name could all claim a
            // reading-section extent larger than what's actually mapped).
            // `VirtualQuery` asks the OS directly for the real size of the
            // region backing `view.Value`, which the header cannot lie
            // about, and is what actually bounds the read below.
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: `view.Value` is the same live mapped-view pointer
            // checked non-null above; `&mut mbi` is a local, live for the
            // duration of this call, whose size exactly matches
            // `size_of::<MEMORY_BASIC_INFORMATION>()` as passed below.
            // `VirtualQuery` only reads page/region metadata about the
            // address range -- it never dereferences `view.Value` itself.
            let mbi_len = unsafe {
                VirtualQuery(
                    Some(view.Value),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if mbi_len == 0 {
                return Err(Error::msg(format!(
                    "VirtualQuery({SHARED_MEM_NAME}) failed: {}",
                    windows::core::Error::from_thread()
                )));
            }
            if needed > mbi.RegionSize {
                return Err(Error::msg(format!(
                    "HWiNFO shared memory header claims a {needed}-byte reading section, but the \
                     mapped view is only {} bytes -- refusing to read past the view's real extent",
                    mbi.RegionSize
                )));
            }

            // SAFETY: `view.Value` points at a live mapped view backed by a
            // real HWiNFO section, confirmed above (not merely assumed from
            // the section's own self-reported header) to be `HWiS`-signed
            // and to actually have at least `needed` bytes mapped --
            // `VirtualQuery` just verified `needed <= mbi.RegionSize`, the
            // OS's own report of how many bytes are genuinely backed by
            // this mapping starting at `view.Value`. Also capped above by
            // `MAX_SHARED_MEM_READ_BYTES` regardless of what either source
            // reports.
            let slice = unsafe { std::slice::from_raw_parts(view.Value as *const u8, needed) };
            Ok(slice.to_vec())
        })();
        // SAFETY: `view` is the same `MEMORY_MAPPED_VIEW_ADDRESS` returned
        // by `MapViewOfFile` above, unmapped here exactly once regardless
        // of whether the read above succeeded.
        let _ = unsafe { UnmapViewOfFile(view) };
        read_result
    })();
    // SAFETY: `handle` is the same file-mapping handle opened above, closed
    // here exactly once on every path out of this function.
    let _ = unsafe { CloseHandle(handle) };
    result
}

fn read_sensors_sync() -> Result<HwinfoSensorSnapshot> {
    let bytes = map_shared_memory_bytes()?;
    parse_shared_memory(&bytes)
}

/// Interval between shared-memory readiness checks in [`poll_until_ready`] --
/// matches `input.rs`'s own poll-loop interval
/// (`wait_for_visible_window`/`wait_for_visible_window_by_title`) exactly.
const READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Upper bound on how long [`start_sync`] will wait, after a cold `spawn()`
/// returns, for HWiNFO's shared-memory section to actually become readable.
/// Live A/B testing (`voidframe-cli hwinfo-check` diagnostic task,
/// 2026-09-04) confirmed a cold-started instance's shared memory reliably
/// isn't valid to read yet immediately after `spawn()` returns, but reliably
/// is a few seconds later -- an immediate read fails with
/// `map_shared_memory_bytes`'s "Shared Memory Support is probably not
/// enabled" error even though it is enabled, purely because the process
/// hasn't finished its own startup. 15s gives real margin over the confirmed
/// "several seconds" gap -- the same proportion of margin `steam.rs`'s own
/// `WINDOW_APPEAR_TIMEOUT` (10s) keeps over its confirmed real delay --
/// without leaving a genuinely broken launch (crashed, shared memory setting
/// somehow not actually enabled) hanging for an unreasonable time.
const SHARED_MEMORY_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Polls `is_ready` every [`READY_POLL_INTERVAL`] until it returns `true` or
/// `timeout` elapses, returning whether it ever succeeded. Same bounded
/// `Instant`-deadline loop shape as `input.rs`'s
/// `wait_for_visible_window`/`wait_for_visible_window_by_title`. Generic
/// over the check (rather than hardcoding `map_shared_memory_bytes`) purely
/// so the timeout path is deterministically testable without a real HWiNFO
/// process -- see the tests below.
fn poll_until_ready(timeout: Duration, mut is_ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if is_ready() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(READY_POLL_INTERVAL);
    }
}

/// Launches via `ShellExecuteW`, not `std::process::Command::spawn()` --
/// live-confirmed (2026-09-04, with UAC re-enabled after earlier testing had
/// it off) that a plain `spawn()` (which uses `CreateProcess` under the
/// hood) fails with `ERROR_ELEVATION_REQUIRED` (Win32 740) launching HWiNFO,
/// even from this already-elevated process -- `CreateProcess` alone cannot
/// satisfy a target's manifest-requested elevation the way `ShellExecuteW`
/// can (the same reason `steam::launch_sync` already uses it for Steam,
/// though for an unrelated reason there -- preserving Steam's own
/// integrity-level inheritance rather than an elevation requirement). `path`
/// is the caller's responsibility (see [`SystemController::start_hwinfo`]'s
/// own doc comment) -- this function never guesses one.
///
/// Launches with its working directory (`lpDirectory`) set to `path`'s own
/// parent directory, not inherited from the calling process. HWiNFO resolves
/// its portable-mode `HWiNFO64.INI` (including the "Shared Memory Support"
/// setting this whole module depends on, plus the user's other portable
/// settings -- auto-update disabled, sensor-only minimized start, etc.)
/// relative to *its own process's cwd at launch*, which without this would
/// simply be whatever cwd the launch inherits from the calling process --
/// not necessarily the directory the portable INI actually lives in.
/// Confirmed live via `voidframe-cli hwinfo-check`: a launch that inherited
/// VOIDFRAME's own cwd instead left a freshly-started HWiNFO with Shared
/// Memory Support off, so every `read_hwinfo_sensors` call right after
/// `start_hwinfo` failed.
///
/// Does not return until the shared-memory section is actually confirmed
/// readable (or [`SHARED_MEMORY_READY_TIMEOUT`] is reached) -- a cold launch
/// returning is not sufficient on its own (see `SHARED_MEMORY_READY_TIMEOUT`'s
/// doc comment); every caller of [`SystemController::start_hwinfo`] already
/// relies on "sensors are readable right after this returns" as this
/// function's whole contract, so the wait belongs here rather than in each
/// caller.
///
/// On a readiness timeout, the just-spawned process is killed (and reaped,
/// by name -- `ShellExecuteW` returns only a pseudo-`HINSTANCE` for error
/// checking, not a real process handle, so there is no `Child` to hold onto
/// the way `spawn()` gave one) before returning `Err`. Every existing caller
/// of [`SystemController::start_hwinfo`] was written under the contract that
/// `Err` means nothing was left running. Without this, a misconfigured
/// HWiNFO (e.g. "Shared Memory Support" not enabled) would leak an orphaned
/// process on every failed start, silently interfering with later capture
/// windows and permanently disabling thermal mode for the rest of the
/// session (`hwinfo_already_running()` would report `true` from then on).
///
/// [`SystemController::start_hwinfo`]: crate::system::SystemController::start_hwinfo
fn start_sync(path: &Path) -> Result<()> {
    // Unlike `CreateProcess` (what `std::process::Command::spawn()` used
    // before this fix), `ShellExecuteW` does not reliably resolve a
    // relative `lpFile` against the calling process's own cwd -- confirmed
    // live: the default `--hwinfo-path` (`spike/tools/HWiNFO64.exe`, a
    // relative path) failed with `SE_ERR_FNF` (code 2, "file not found")
    // even though the exact same relative path worked fine under the old
    // `spawn()`-based launch. Resolved to an absolute path here, once, so
    // both `lpFile` and `lpDirectory` below are unambiguous regardless of
    // what the caller passed in; `path.display()` in error messages below
    // still shows the caller's original (possibly relative) path, since
    // that's what the user actually configured.
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let file_wide = wide(&absolute_path.to_string_lossy());
    let verb_wide = wide("open");
    let dir_wide = absolute_path.parent().map(|d| wide(&d.to_string_lossy()));

    // SAFETY: `file_wide`/`verb_wide`/`dir_wide` are local, NUL-terminated
    // `Vec<u16>` buffers that all outlive this call (dropped only after
    // `ShellExecuteW` returns); `hwnd = None` is a documented-valid "no
    // owner window" per `ShellExecuteW`'s own contract; `lpParameters` is
    // null (HWiNFO needs no arguments); `nShowCmd = SW_SHOWNORMAL` is a
    // plain, always-valid show-command constant, same as
    // `steam::launch_sync`'s own identical choice.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb_wide.as_ptr()),
            PCWSTR(file_wide.as_ptr()),
            PCWSTR::null(),
            dir_wide
                .as_ref()
                .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success (a fake HINSTANCE, per
    // the documented, if archaic, convention) and <= 32 on failure -- same
    // check `steam::launch_sync` already uses.
    if (result.0 as isize) <= 32 {
        return Err(Error::msg(format!(
            "ShellExecuteW failed to launch HWiNFO at {}: code {}",
            path.display(),
            result.0 as isize
        )));
    }

    // `read_sensors_sync().is_ok()`, not just `map_shared_memory_bytes().is_ok()`:
    // live-verifying an earlier fix (this file's own
    // `live_start_already_running_and_close_round_trip` test) found the
    // mapping itself becomes valid -- `map_shared_memory_bytes` succeeds --
    // *before* the reading section's labels are actually populated, so a
    // read right after that first success still failed to find a CPU
    // temperature reading (`parse_shared_memory`'s "no reading matched"
    // error, not the mapping error this was originally written to poll
    // past). The full parse is what every caller's "sensors are readable
    // afterward" contract actually needs proven, so that's what's polled.
    if poll_until_ready(SHARED_MEMORY_READY_TIMEOUT, || read_sensors_sync().is_ok()) {
        Ok(())
    } else {
        // Find + kill the process this call just launched -- see this
        // function's own doc comment for why an `Err` here must leave
        // nothing running, and why this is by-name rather than by handle.
        // Best-effort throughout: if the process already exited on its own
        // (e.g. it crashed rather than merely being slow), a lookup/kill
        // failure here is not itself a reason to change the error reported
        // below.
        if let Ok(Some(handle)) = super::process::find_by_name_sync_pub(HWINFO_PROCESS_NAME) {
            let _ = super::process::kill_tree_sync_pub(handle.pid);
        }
        Err(Error::msg(format!(
            "HWiNFO launched at {} but its shared memory never became readable within \
             {SHARED_MEMORY_READY_TIMEOUT:?} -- the process started, but its \"Shared Memory \
             Support\" mapping never became valid (either it isn't actually enabled, or it's \
             taking far longer than the confirmed-live norm to initialize); the process has been \
             terminated",
            path.display()
        )))
    }
}

pub async fn already_running() -> Result<bool> {
    Ok(super::process::find_by_name(HWINFO_PROCESS_NAME)
        .await?
        .is_some())
}

pub async fn start(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || start_sync(&path))
        .await
        .map_err(|e| Error::msg(format!("start_hwinfo task panicked: {e}")))?
}

/// A plain `kill_process_tree` against HWiNFO's own pid -- no window-close
/// nuance needed (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A1: no anti-cheat-style termination-blocking
/// concern, and no "user was already looking at it" state to preserve,
/// since this is only ever called on an instance VOIDFRAME itself
/// started). A no-op, not an error, if HWiNFO isn't running.
pub async fn close() -> Result<()> {
    match super::process::find_by_name(HWINFO_PROCESS_NAME).await? {
        Some(handle) => super::process::kill_tree(handle.pid).await,
        None => Ok(()),
    }
}

pub async fn read_sensors() -> Result<HwinfoSensorSnapshot> {
    tokio::task::spawn_blocking(read_sensors_sync)
        .await
        .map_err(|e| Error::msg(format!("read_hwinfo_sensors task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real bundled HWiNFO binary/ini this repo ships (see
    /// `src-tauri/binaries/README.md`) -- used by this module's live,
    /// `#[ignore]`d tests below instead of a hardcoded developer-machine
    /// path, so the test source carries no machine-specific path and runs
    /// unmodified from any clone of either the private or public repo.
    fn bundled_hwinfo_exe_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src-tauri/binaries/hwinfo64-x86_64-pc-windows-msvc.exe")
    }

    fn bundled_hwinfo_ini_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/resources/hwinfo/HWiNFO64.ini")
    }

    /// Builds a minimal buffer matching the confirmed layout: a 48-byte
    /// header (with the reading section starting immediately after it, no
    /// sensor section) followed by exactly one 460-byte reading element
    /// carrying `label` at the confirmed label sub-offset and `value` at
    /// the confirmed value sub-offset.
    fn synthetic_shared_memory_with_one_cpu_temp_reading(label: &str, value: f64) -> Vec<u8> {
        const HEADER_LEN: usize = 48;
        const READING_SIZE: usize = 460;
        let mut buf = vec![0u8; HEADER_LEN + READING_SIZE];

        buf[0..4].copy_from_slice(&HWIS_SIGNATURE.to_le_bytes());
        buf[4..8].copy_from_slice(&2u32.to_le_bytes()); // version
        buf[8..12].copy_from_slice(&1u32.to_le_bytes()); // revision
        buf[12..20].copy_from_slice(&0i64.to_le_bytes()); // poll_time
        buf[20..24].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes()); // offset_of_sensor_section
        buf[24..28].copy_from_slice(&0u32.to_le_bytes()); // size_of_sensor_element
        buf[28..32].copy_from_slice(&0u32.to_le_bytes()); // num_sensor_elements
        buf[32..36].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes()); // offset_of_reading_section
        buf[36..40].copy_from_slice(&(READING_SIZE as u32).to_le_bytes()); // size_of_reading_element
        buf[40..44].copy_from_slice(&1u32.to_le_bytes()); // num_reading_elements
        // offset 44..48: _reserved, left zeroed

        let elem = HEADER_LEN;
        buf[elem..elem + 4].copy_from_slice(&0u32.to_le_bytes()); // dwReadingID
        buf[elem + 4..elem + 8].copy_from_slice(&0u32.to_le_bytes()); // dwSensorIndex
        buf[elem + 8..elem + 12].copy_from_slice(&0x0100_0000u32.to_le_bytes()); // tReading (temp)
        let label_bytes = label.as_bytes();
        buf[elem + READING_LABEL_OFFSET..elem + READING_LABEL_OFFSET + label_bytes.len()]
            .copy_from_slice(label_bytes);
        buf[elem + READING_VALUE_OFFSET..elem + READING_VALUE_OFFSET + 8]
            .copy_from_slice(&value.to_le_bytes());

        buf
    }

    #[test]
    fn parses_a_cpu_package_temperature_reading_by_label() {
        let buf = synthetic_shared_memory_with_one_cpu_temp_reading("CPU Package", 61.5);
        let snapshot = parse_shared_memory(&buf).unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 61.5);
        assert_eq!(snapshot.gpu_temp_celsius, None);
    }

    #[test]
    fn parses_the_confirmed_amd_cpu_label() {
        let buf = synthetic_shared_memory_with_one_cpu_temp_reading("CPU (Tctl/Tdie)", 45.0);
        let snapshot = parse_shared_memory(&buf).unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 45.0);
    }

    #[test]
    fn errors_clearly_when_no_matching_cpu_label_is_found() {
        let buf = synthetic_shared_memory_with_one_cpu_temp_reading("Some Unrelated Sensor", 12.0);
        let err = parse_shared_memory(&buf).unwrap_err();
        assert!(err.to_string().contains("CPU"), "error was: {err}");
    }

    #[test]
    fn errors_clearly_on_a_signature_mismatch() {
        let mut buf = synthetic_shared_memory_with_one_cpu_temp_reading("CPU Package", 61.5);
        buf[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let err = parse_shared_memory(&buf).unwrap_err();
        assert!(err.to_string().contains("signature"), "error was: {err}");
    }

    /// Direct coverage of the extracted helper both `parse_shared_memory`
    /// and `map_shared_memory_bytes` now share -- see the doc comment on
    /// `map_shared_memory_bytes` for why the real Win32 path needed this
    /// check to run *before* trusting any other header field, not just in
    /// `parse_shared_memory`.
    #[test]
    fn validate_signature_accepts_the_confirmed_value_and_rejects_others() {
        assert!(validate_signature(HWIS_SIGNATURE).is_ok());
        let err = validate_signature(0xDEAD_BEEF).unwrap_err();
        assert!(err.to_string().contains("signature"), "error was: {err}");
    }

    #[test]
    fn errors_clearly_when_the_buffer_is_shorter_than_the_header() {
        let buf = vec![0u8; 10];
        let err = parse_shared_memory(&buf).unwrap_err();
        assert!(err.to_string().contains("header"), "error was: {err}");
    }

    /// Two reading elements: a GPU temperature followed by a CPU one --
    /// proves both labels are found in the same pass, not just whichever
    /// one happens to be first.
    #[test]
    fn finds_both_cpu_and_gpu_temperature_in_the_same_snapshot() {
        const HEADER_LEN: usize = 48;
        const READING_SIZE: usize = 460;
        let mut buf = vec![0u8; HEADER_LEN + READING_SIZE * 2];
        buf[0..4].copy_from_slice(&HWIS_SIGNATURE.to_le_bytes());
        buf[20..24].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        buf[32..36].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        buf[36..40].copy_from_slice(&(READING_SIZE as u32).to_le_bytes());
        buf[40..44].copy_from_slice(&2u32.to_le_bytes());

        let elem0 = HEADER_LEN;
        let label0 = b"GPU Temperature";
        buf[elem0 + READING_LABEL_OFFSET..elem0 + READING_LABEL_OFFSET + label0.len()]
            .copy_from_slice(label0);
        buf[elem0 + READING_VALUE_OFFSET..elem0 + READING_VALUE_OFFSET + 8]
            .copy_from_slice(&32.0f64.to_le_bytes());

        let elem1 = HEADER_LEN + READING_SIZE;
        let label1 = b"CPU (Tctl/Tdie)";
        buf[elem1 + READING_LABEL_OFFSET..elem1 + READING_LABEL_OFFSET + label1.len()]
            .copy_from_slice(label1);
        buf[elem1 + READING_VALUE_OFFSET..elem1 + READING_VALUE_OFFSET + 8]
            .copy_from_slice(&61.5f64.to_le_bytes());

        let snapshot = parse_shared_memory(&buf).unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 61.5);
        assert_eq!(snapshot.gpu_temp_celsius, Some(32.0));
    }

    #[test]
    fn latin1_to_string_decodes_a_non_ascii_byte_like_the_degree_sign() {
        // 0xB0 is Latin-1's degree sign (live verification confirmed this exact byte
        // in HWiNFO's own szUnit field) -- must decode to U+00B0, not be
        // rejected or corrupted the way strict UTF-8 decoding would.
        let buf = [b'C', 0xB0, b'C', 0];
        assert_eq!(latin1_to_string(&buf), "C\u{B0}C");
    }

    #[test]
    fn latin1_to_string_stops_at_the_first_nul() {
        let buf = [b'C', b'P', b'U', 0, b'X', b'X'];
        assert_eq!(latin1_to_string(&buf), "CPU");
    }

    #[test]
    fn poll_until_ready_times_out_rather_than_blocking_forever() {
        // A check that never succeeds exercises the real retry loop's
        // timeout path end to end -- confirms it actually polls (returns
        // `false` only after roughly the requested duration, not instantly)
        // and does terminate (not an infinite loop). Same shape as
        // `input.rs`'s `wait_for_visible_window_times_out_rather_than_blocking_forever`.
        let start = Instant::now();
        let ready = poll_until_ready(Duration::from_millis(600), || false);
        let elapsed = start.elapsed();
        assert!(!ready);
        assert!(
            elapsed >= Duration::from_millis(600),
            "returned before the requested timeout elapsed: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "took far longer than the requested timeout: {elapsed:?}"
        );
    }

    #[test]
    fn poll_until_ready_returns_true_as_soon_as_the_check_succeeds() {
        // Proves the loop doesn't wait out the full timeout once the check
        // starts succeeding -- the second call (after one poll interval)
        // is what flips `ready`, so this should return well before the 5s
        // timeout below.
        let mut calls = 0;
        let start = Instant::now();
        let ready = poll_until_ready(Duration::from_secs(5), || {
            calls += 1;
            calls >= 2
        });
        let elapsed = start.elapsed();
        assert!(ready);
        assert_eq!(calls, 2);
        assert!(
            elapsed < Duration::from_secs(2),
            "should have returned as soon as the check succeeded, not waited near the timeout: {elapsed:?}"
        );
    }

    /// Live round trip against a real running HWiNFO64.exe instance with
    /// its "Shared Memory Support" setting enabled -- not portable/CI-safe
    /// (same reasoning as `power_plan.rs`'s/`powercfg.rs`'s own real-rig
    /// `#[ignore]`d tests), so this is `#[ignore]`d and run manually.
    /// Confirms `open_shared_memory`/`map_shared_memory_bytes`/
    /// `parse_shared_memory` work end-to-end against the real OS, not just
    /// the synthetic fixture above.
    #[test]
    #[ignore = "needs a real running HWiNFO64.exe with Shared Memory Support enabled -- run \
                manually: `cargo test -p voidframe-engine --lib \
                system::windows::hwinfo::tests::live_read_against_a_real_running_hwinfo_instance \
                -- --ignored --exact --nocapture`"]
    fn live_read_against_a_real_running_hwinfo_instance() {
        let snapshot = read_sensors_sync().expect(
            "read_sensors_sync failed -- is HWiNFO64.exe running with Shared Memory Support \
             enabled in its own Settings?",
        );
        println!("live HWiNFO snapshot: {snapshot:?}");
        assert!(
            snapshot.cpu_temp_celsius > 0.0 && snapshot.cpu_temp_celsius < 150.0,
            "cpu_temp_celsius out of a plausible range: {}",
            snapshot.cpu_temp_celsius
        );
        if let Some(gpu) = snapshot.gpu_temp_celsius {
            assert!(
                gpu > 0.0 && gpu < 150.0,
                "gpu_temp_celsius out of a plausible range: {gpu}"
            );
        }
    }

    /// Live round trip for the process-control primitives, against the
    /// real confirmed HWiNFO64.exe copy that live verification was run
    /// against (the bundled binary at `bundled_hwinfo_exe_path()`) -- not
    /// portable/CI-safe (a real path, a real spawned process, and it
    /// terminates that process), so `#[ignore]`d and run manually, same
    /// precedent as this module's other live test above.
    #[tokio::test]
    #[ignore = "spawns and kills a real HWiNFO64.exe -- run manually: `cargo test -p \
                voidframe-engine --lib \
                system::windows::hwinfo::tests::live_start_already_running_and_close_round_trip \
                -- --ignored --exact --nocapture`"]
    async fn live_start_already_running_and_close_round_trip() {
        let path = bundled_hwinfo_exe_path();
        start(&path).await.expect("start() failed");
        // This is this fix's own live-verification: no manual sleep between
        // start() and this read. `start_sync`'s internal readiness poll
        // (`poll_until_ready` against `read_sensors_sync` -- not the
        // cheaper `map_shared_memory_bytes`, which was tried first and
        // found insufficient; see `start_sync`'s own doc comment) is what
        // must have already confirmed a full reading was available before
        // `start()` returned. Before the fix, reading immediately after a
        // cold `start()` reliably failed with `map_shared_memory_bytes`'s
        // "Shared Memory Support is probably not enabled" error even though
        // it is enabled -- the process just wasn't ready yet (confirmed
        // live, `voidframe-cli hwinfo-check` diagnostic task, 2026-09-04).
        read_sensors().await.expect(
            "read_sensors() failed immediately after start() returned -- start_sync's readiness \
             poll should have already confirmed the shared memory was mapped before returning",
        );
        assert!(
            already_running().await.unwrap(),
            "already_running() should report true right after start()"
        );
        close().await.expect("close() failed");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(
            !already_running().await.unwrap(),
            "already_running() should report false right after close()"
        );
    }

    /// RAII guard for [`live_start_sync_kills_the_process_it_spawned_on_a_readiness_timeout`]
    /// below: temporarily flips the real portable `HWiNFO64.INI`'s
    /// `SensorsSM` setting off (forcing `start_sync`'s readiness poll to
    /// time out) and restores the original content on drop -- including on
    /// a panic (e.g. a failed assertion mid-test), since this is the user's
    /// real working HWiNFO config, not a throwaway fixture, and a permanent
    /// unattended test needs to be safe about it rather than "restore at
    /// the end" with no panic protection.
    struct DisabledSharedMemoryIni {
        path: std::path::PathBuf,
        original: String,
    }

    impl DisabledSharedMemoryIni {
        fn install(path: std::path::PathBuf) -> Self {
            let original = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            let disabled = original.replace("SensorsSM=1", "SensorsSM=0");
            assert_ne!(
                disabled,
                original,
                "expected to find \"SensorsSM=1\" in {} to flip off -- has the real INI's \
                 format changed?",
                path.display()
            );
            std::fs::write(&path, &disabled)
                .unwrap_or_else(|e| panic!("failed to write {}: {e}", path.display()));
            Self { path, original }
        }
    }

    impl Drop for DisabledSharedMemoryIni {
        fn drop(&mut self) {
            // Best-effort and must never itself panic (a panic inside
            // `drop` during an existing unwind aborts the process instead
            // of just failing the test) -- but loud in the test output
            // either way, since a failure here leaves the user's real
            // working config altered.
            if let Err(e) = std::fs::write(&self.path, &self.original) {
                eprintln!(
                    "FAILED TO RESTORE {} to its original content -- restore it by hand \
                     (SensorsSM should be 1): {e}",
                    self.path.display()
                );
            }
        }
    }

    /// Fix 1's own live verification, checked into the suite rather than
    /// only ever having been confirmed once by hand (session verification,
    /// 2026-09-04): with HWiNFO's own "Shared Memory Support" setting
    /// disabled (via [`DisabledSharedMemoryIni`] above), `start_sync`'s
    /// readiness poll must time out -- `start()` returns `Err` -- and the
    /// process it just spawned must already be gone by the time it
    /// returns, never left orphaned. Not portable/CI-safe (edits the real
    /// portable INI next to a real `HWiNFO64.exe`, spawns and kills a real
    /// process, and takes the real ~15s readiness timeout to elapse), so
    /// `#[ignore]`d and run manually, same precedent as this module's other
    /// live tests above.
    #[tokio::test]
    #[ignore = "edits the real HWiNFO64.INI to disable Shared Memory Support, spawns a real \
                HWiNFO64.exe, and waits out the real ~15s readiness timeout -- run manually: \
                `cargo test -p voidframe-engine --lib \
                system::windows::hwinfo::tests::live_start_sync_kills_the_process_it_spawned_on_a_readiness_timeout \
                -- --ignored --exact --nocapture`"]
    async fn live_start_sync_kills_the_process_it_spawned_on_a_readiness_timeout() {
        let path = bundled_hwinfo_exe_path();
        let _ini_guard = DisabledSharedMemoryIni::install(bundled_hwinfo_ini_path());

        let err = start(&path)
            .await
            .expect_err("start() should have timed out with Shared Memory Support disabled");
        println!("live start() timeout error (expected): {err}");

        assert!(
            !already_running().await.unwrap(),
            "start_sync must kill the process it spawned before returning Err on a readiness \
             timeout -- this is Fix 1's whole point"
        );
    }
}
