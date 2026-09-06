//! Tier-2 (log-tail) detection — the only implemented tier for M1, per
//! `spike/findings.md` §1/§7.4 (tier 1/netcon confirmed dead on the
//! current CS2 build; tier 3/fixed-window isn't needed since tier 2 works
//! reliably). Tails `console.log` (populated by `-condebug`, an
//! engine-reserved launch arg — see `cs2::keybind_cfg::reserved_tokens`)
//! and matches the confirmed regex candidates from `spike/findings.md` §2.

use crate::error::{Error, Result};
use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader, SeekFrom};

#[derive(Debug, Clone, Deserialize)]
pub struct Signatures {
    pub map_loaded: String,
    pub benchmark_started: String,
    pub benchmark_ended: String,
    pub vprof_fps: String,
    /// Confirmed live (not from the verification spike,
    /// `docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §12
    /// — found during `docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`'s
    /// own live verification): CS2 loads its own main-menu background scene
    /// (`ui/xpshop_item`, a local server connection — `<player> connected`,
    /// `[Prediction] Added prediction for player slot 0`, etc.) on every
    /// boot, before the game is actually processing keyboard input. That
    /// sequence ends with a Steam Datagram Relay connectivity check
    /// (`[SteamNetSockets] Ping measurement completed...` then `SDR
    /// RelayNetworkStatus: ...`) — observed directly to coincide with the
    /// game becoming genuinely interactive, unlike the Win32 "window is
    /// visible" signal alone, which fires several seconds earlier (still
    /// mid-intro/loading). This is the real readiness signal `reissue_map`
    /// needs; matching on the LAST of the two related SteamNetSockets lines
    /// (RelayNetworkStatus, not the ping-completed line before it) for the
    /// most conservative timing.
    pub menu_ready: String,
}

impl Signatures {
    pub fn from_file(path: &Path) -> Result<Signatures> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
        serde_json::from_str(&text).map_err(Error::from)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetectionEvent {
    MapLoaded {
        map: String,
        addon: String,
    },
    BenchmarkStarted,
    BenchmarkEnded,
    /// A secondary, informational signal — never blocks the state machine
    /// (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.3 step 5: "logged for cross-check", not the primary
    /// end-of-run marker; `BenchmarkEnded` from the disconnect line is).
    VProfFps {
        avg: f64,
        p1: f64,
    },
    /// CS2's main-menu background scene has finished loading and its
    /// Steam Datagram Relay connectivity check has completed — the real
    /// signal that the game is processing input, confirmed live (see
    /// `Signatures::menu_ready`'s own doc comment). Fires once per CS2
    /// launch, before any workshop map is ever loaded.
    MenuReady,
}

#[allow(clippy::too_many_arguments)]
fn classify(
    line: &str,
    re_map: &Regex,
    re_start: &Regex,
    re_end: &Regex,
    re_fps: &Regex,
    re_menu_ready: &Regex,
) -> Option<DetectionEvent> {
    if let Some(c) = re_map.captures(line) {
        return Some(DetectionEvent::MapLoaded {
            map: c.name("map")?.as_str().to_string(),
            addon: c.name("addon")?.as_str().to_string(),
        });
    }
    if re_start.is_match(line) {
        return Some(DetectionEvent::BenchmarkStarted);
    }
    if re_end.is_match(line) {
        return Some(DetectionEvent::BenchmarkEnded);
    }
    if let Some(c) = re_fps.captures(line) {
        return Some(DetectionEvent::VProfFps {
            avg: c.name("avg")?.as_str().parse().ok()?,
            p1: c.name("p1")?.as_str().parse().ok()?,
        });
    }
    if re_menu_ready.is_match(line) {
        return Some(DetectionEvent::MenuReady);
    }
    None
}

/// The CS2-readiness detection channel `run_scenario_inner`/`launch.rs`
/// wait on — real callers always get [`RealLogTail`], genuinely tailing
/// `console.log`; `voidframe_engine::mock_harness::MockLogTail` (built for
/// `RunConfig::mock_cs2_log`, the E2E mock-run mode) is the only other
/// implementation, and answers any `accept` predicate instantly without
/// touching a real file. Object-safe (`&mut dyn FnMut` rather than `impl
/// FnMut`) specifically so `run_scenario_inner`/`launch.rs` can hold either
/// implementation behind one `Box<dyn Cs2LogDetector>` — the same
/// reasoning `CaptureRunner` already applies at capture time.
#[async_trait]
pub trait Cs2LogDetector: Send {
    /// See [`RealLogTail::wait_for`] for the full contract (both
    /// implementations honor it identically from a caller's perspective —
    /// `MockLogTail` just never actually times out or reads a stale line).
    async fn wait_for(
        &mut self,
        timeout: Duration,
        accept: &mut (dyn for<'r> FnMut(&'r DetectionEvent) -> bool + Send),
    ) -> Result<Option<DetectionEvent>>;

    /// Diagnostics only — see [`RealLogTail::lines_read`].
    fn lines_read(&self) -> u64;

    /// Diagnostics only — see [`RealLogTail::last_line_seen`].
    fn last_line_seen(&self) -> Option<&str>;
}

/// Tails a file from its *current* end (not from the start — a scenario's
/// detection channel opens once per §7.2 step 4 and should only see lines
/// written after that point, not a prior session's stale content still on
/// disk).
pub struct RealLogTail {
    /// Kept so `wait_for`'s EOF branch can re-resolve by path — see
    /// `created`'s own doc comment for why this matters.
    path: PathBuf,
    reader: BufReader<File>,
    /// How many bytes into the file the reader's logical cursor currently
    /// sits at — tracked ourselves (rather than re-querying the stream)
    /// since `BufReader` doesn't expose its own effective position
    /// directly. Only ever updated at a point where the internal buffer is
    /// known to be empty (right after `open`'s own seek, and right after a
    /// successful `read_line`), so it always matches the underlying
    /// `File`'s real cursor at the moments it's compared against the
    /// file's on-disk length.
    pos: u64,
    /// The creation timestamp of the file `reader` currently has open, when
    /// the filesystem reports one. Confirmed live: CS2's `-conclearlog`
    /// does NOT truncate `console.log` in place — it deletes and recreates
    /// it. An already-open Windows file handle (opened with the default
    /// sharing flags `tokio`/`std` use) keeps referring to the OLD, now-
    /// unlinked file's data forever after that; querying THAT handle's own
    /// metadata reports the old file's frozen state, not what's actually at
    /// the path now — a size-only truncation check (this file's own
    /// earlier version) can't tell "the same file grew" from "a different
    /// file replaced it and happens to be a similar size". `Metadata::
    /// created()` is a stable, cross-platform proxy for file identity here
    /// (the NTFS file index/inode equivalent would be more direct, but
    /// `MetadataExt::file_index()` is still unstable on this toolchain,
    /// confirmed via a real compile error): a delete+recreate gets a fresh
    /// creation time, while an in-place truncate (`SetEndOfFile`-style,
    /// what `CREATE_ALWAYS`-over-an-existing-file typically does on NTFS)
    /// preserves the original one.
    created: Option<SystemTime>,
    re_map: Regex,
    re_start: Regex,
    re_end: Regex,
    re_fps: Regex,
    re_menu_ready: Regex,
    /// Diagnostics only — how many lines `wait_for` has actually read
    /// (classified or not) since `open`, and the most recent one seen.
    /// Lets a caller distinguish "nothing was ever read" (a truncation/seek
    /// bug — genuinely nothing reached this tail) from "lines were read but
    /// never matched" (a regex/signature problem) without needing to
    /// reproduce a live failure to find out which.
    lines_read: u64,
    last_line: String,
}

/// How long `open` retries a not-yet-existing `console.log` before giving
/// up. CS2 may not have created the file the instant its process becomes
/// discoverable — a real possibility, not a hypothetical — so a first
/// `NotFound` isn't treated as fatal immediately.
const OPEN_RETRY_BUDGET: Duration = Duration::from_secs(5);
const OPEN_RETRY_POLL: Duration = Duration::from_millis(200);

/// The creation timestamp of an already-opened `tokio::fs::File`, if the
/// filesystem/metadata call reports one (Windows filesystems universally
/// do; a `None` here — a transient race, or a filesystem that genuinely
/// doesn't support it — just means callers fall back to a size-based
/// heuristic instead of treating it as fatal).
async fn created_of(file: &File) -> Option<SystemTime> {
    file.metadata().await.ok()?.created().ok()
}

impl RealLogTail {
    pub async fn open(path: &Path, sigs: &Signatures) -> Result<RealLogTail> {
        let deadline = tokio::time::Instant::now() + OPEN_RETRY_BUDGET;
        let mut file = loop {
            match File::open(path).await {
                Ok(f) => break f,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(OPEN_RETRY_POLL).await;
                    continue;
                }
                Err(e) => {
                    return Err(Error::msg(format!("opening {}: {e}", path.display())));
                }
            }
        };
        let created = created_of(&file).await;
        // `seek` returns the new (post-seek) stream position, which for
        // `SeekFrom::End(0)` is exactly the file's current length — that
        // doubles as this tail's starting `pos`.
        let pos = file
            .seek(SeekFrom::End(0))
            .await
            .map_err(|e| Error::msg(format!("seeking {}: {e}", path.display())))?;
        Ok(RealLogTail {
            path: path.to_path_buf(),
            reader: BufReader::new(file),
            pos,
            created,
            re_map: Regex::new(&sigs.map_loaded).map_err(|e| Error::msg(e.to_string()))?,
            re_start: Regex::new(&sigs.benchmark_started).map_err(|e| Error::msg(e.to_string()))?,
            re_end: Regex::new(&sigs.benchmark_ended).map_err(|e| Error::msg(e.to_string()))?,
            re_fps: Regex::new(&sigs.vprof_fps).map_err(|e| Error::msg(e.to_string()))?,
            re_menu_ready: Regex::new(&sigs.menu_ready).map_err(|e| Error::msg(e.to_string()))?,
            lines_read: 0,
            last_line: String::new(),
        })
    }
}

#[async_trait]
impl Cs2LogDetector for RealLogTail {
    /// Diagnostics only (see the field's own doc comment) — how many lines
    /// have been read since `open`, regardless of whether any matched.
    fn lines_read(&self) -> u64 {
        self.lines_read
    }

    /// Diagnostics only — the most recent line read, if any.
    fn last_line_seen(&self) -> Option<&str> {
        if self.last_line.is_empty() {
            None
        } else {
            Some(&self.last_line)
        }
    }

    /// Reads new lines as they're appended (`console.log` grows as CS2
    /// runs), classifying each; returns the first matching `DetectionEvent`
    /// that is a "stopping" kind of event for this call (i.e. every call
    /// site chooses which events matter for the wait it's doing — pass a
    /// filter closure so e.g. waiting for `BenchmarkStarted` doesn't
    /// terminate early on a `VProfFps` line that happens to appear first in
    /// a pathological ordering). `Ok(None)` on timeout — not an error, the
    /// caller (the watchdog, §7.3 step 6) decides what a timeout means.
    async fn wait_for(
        &mut self,
        timeout: Duration,
        accept: &mut (dyn for<'r> FnMut(&'r DetectionEvent) -> bool + Send),
    ) -> Result<Option<DetectionEvent>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut line = String::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            line.clear();
            let read = tokio::time::timeout(
                remaining.min(Duration::from_millis(250)),
                self.reader.read_line(&mut line),
            )
            .await;
            match read {
                Ok(Ok(0)) => {
                    // EOF for now — genuinely nothing new yet, OR
                    // console.log was deleted and recreated out from under
                    // us. Confirmed live (not a hypothetical): CS2's
                    // `-conclearlog` does NOT truncate the file in place —
                    // it replaces it — and an already-open Windows handle
                    // keeps referring to the OLD, now-unlinked file's data
                    // forever afterward, unable to ever see anything
                    // written to the NEW file at the same path. A size-only
                    // check (comparing the stale handle's own metadata)
                    // cannot tell "the same file grew" from "a different,
                    // similarly-sized file replaced it" — re-resolve by
                    // PATH and compare file IDENTITY (creation timestamp)
                    // against what this tail currently holds. Any I/O
                    // failure here (a transient race opening/stat'ing the
                    // path) is treated the same as "no replacement
                    // detected" — fall through to the normal poll-and-retry
                    // rather than erroring the whole wait over it.
                    if let Ok(fresh) = File::open(&self.path).await {
                        let fresh_created = created_of(&fresh).await;
                        let fresh_len = fresh.metadata().await.ok().map(|m| m.len());
                        // Two distinct ways the content we're holding can go
                        // stale, both handled the same way (adopt the fresh
                        // handle, restart from 0): the file at this path is
                        // now a genuinely DIFFERENT file (delete+recreate —
                        // different creation timestamp), or it's the SAME
                        // file but shorter than what we've read (an
                        // in-place truncate — same creation time,
                        // `SetEndOfFile`-style). A creation-time match alone
                        // does NOT mean "nothing to do" — it only rules out
                        // the replacement case; the shrink check below still
                        // has to run either way.
                        let different_file = matches!(
                            (self.created, fresh_created),
                            (Some(cur), Some(new)) if cur != new
                        );
                        let shrunk = fresh_len.is_some_and(|len| len < self.pos);
                        let replaced = different_file || shrunk;
                        if replaced {
                            let mut fresh = fresh;
                            fresh.seek(SeekFrom::Start(0)).await.map_err(|e| {
                                Error::msg(format!("seeking freshly-reopened console.log: {e}"))
                            })?;
                            self.reader = BufReader::new(fresh);
                            self.pos = 0;
                            self.created = fresh_created;
                            continue;
                        }
                    }
                    // Not an error; brief poll delay, then retry
                    // (tokio::fs has no native inotify-style follow, so
                    // this polls — 250ms matches the per-iteration timeout
                    // slice above, keeping the loop responsive to the
                    // overall deadline without a tight busy-spin).
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                Ok(Ok(n)) => {
                    self.pos += n as u64;
                    self.lines_read += 1;
                    self.last_line = line.trim_end().to_string();
                    if let Some(ev) = classify(
                        &line,
                        &self.re_map,
                        &self.re_start,
                        &self.re_end,
                        &self.re_fps,
                        &self.re_menu_ready,
                    ) && accept(&ev)
                    {
                        return Ok(Some(ev));
                    }
                }
                Ok(Err(e)) => return Err(Error::msg(format!("reading console.log: {e}"))),
                Err(_) => continue, // this slice's timeout elapsed, outer loop re-checks the real deadline
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sigs() -> Signatures {
        Signatures {
            map_loaded: r#"Loading map "(?P<map>[^"]+)" \(addon '(?P<addon>\d+)'\)"#.to_string(),
            benchmark_started: r"\[Server\] BeginMatch".to_string(),
            benchmark_ended: r"\[Client\] Disconnected from server:".to_string(),
            vprof_fps: r"\[VProf\] FPS: Avg=(?P<avg>[\d.]+), P1=(?P<p1>[\d.]+)".to_string(),
            menu_ready: r"\[SteamNetSockets\] SDR RelayNetworkStatus:".to_string(),
        }
    }

    #[test]
    fn classifies_every_confirmed_regex_from_the_spike() {
        let s = sigs();
        let (re_map, re_start, re_end, re_fps, re_menu_ready) = (
            Regex::new(&s.map_loaded).unwrap(),
            Regex::new(&s.benchmark_started).unwrap(),
            Regex::new(&s.benchmark_ended).unwrap(),
            Regex::new(&s.vprof_fps).unwrap(),
            Regex::new(&s.menu_ready).unwrap(),
        );

        assert_eq!(
            classify(
                r#"Loading map "de_dust2" (addon '3240880604')"#,
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            Some(DetectionEvent::MapLoaded {
                map: "de_dust2".into(),
                addon: "3240880604".into()
            })
        );
        assert_eq!(
            classify(
                "[02:48:40.757] 09/01 02:48:40 [Server] BeginMatch",
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            Some(DetectionEvent::BenchmarkStarted)
        );
        assert_eq!(
            classify(
                "[02:50:50.731] 09/01 02:50:50 [Client] Disconnected from server: NETWORK_DISCONNECT_DISCONNECT_BY_USER",
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            Some(DetectionEvent::BenchmarkEnded)
        );
        assert_eq!(
            classify(
                "[02:50:50.734] 09/01 02:50:50 [VProf] FPS: Avg=923.4, P1=356.1",
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            Some(DetectionEvent::VProfFps {
                avg: 923.4,
                p1: 356.1
            })
        );
        assert_eq!(
            classify(
                "09/01 19:45:11 [SteamNetSockets] SDR RelayNetworkStatus:  avail=OK  config=OK  anyrelay=OK",
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            Some(DetectionEvent::MenuReady)
        );
        assert_eq!(
            classify(
                "unrelated log noise",
                &re_map,
                &re_start,
                &re_end,
                &re_fps,
                &re_menu_ready,
            ),
            None
        );
    }

    #[tokio::test]
    async fn wait_for_only_sees_lines_written_after_open_not_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.log");
        // Stale line is the SAME event type being awaited below, and nothing
        // is appended after open — so this only passes if `RealLogTail::open`
        // genuinely seeks to EOF. If the seek were ever removed, this stale
        // line would be seen immediately and the call would return
        // `Ok(Some(BenchmarkEnded))` instead of timing out.
        tokio::fs::write(&path, "[Client] Disconnected from server: STALE\n")
            .await
            .unwrap();

        let mut tail = RealLogTail::open(&path, &sigs()).await.unwrap();

        let result = tail
            .wait_for(Duration::from_millis(300), &mut |ev| {
                matches!(ev, DetectionEvent::BenchmarkEnded)
            })
            .await;
        assert_eq!(result.unwrap(), None);
    }

    #[tokio::test]
    async fn wait_for_times_out_returning_ok_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.log");
        tokio::fs::write(&path, "").await.unwrap();
        let mut tail = RealLogTail::open(&path, &sigs()).await.unwrap();
        let result = tail
            .wait_for(Duration::from_millis(300), &mut |ev| {
                matches!(ev, DetectionEvent::BenchmarkStarted)
            })
            .await
            .unwrap();
        assert_eq!(result, None);
    }

    async fn append_line(path: &Path, line: &str) {
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .await
            .unwrap();
        f.write_all(line.as_bytes()).await.unwrap();
        f.flush().await.unwrap();
    }

    /// `-conclearlog` (an engine-reserved launch arg) makes CS2
    /// truncate/recreate `console.log` shortly after launch — *after*
    /// `RealLogTail::open`'s own seek-to-EOF already happened. Without
    /// truncation detection, the stream position would sit past the new,
    /// shorter file's actual end and `wait_for` would poll forever without
    /// ever seeing the fresh content. This writes an ignorable line, then
    /// truncates the file to empty (simulating `-conclearlog`), then
    /// writes the real target line — matching this module's established
    /// `tokio::join!` concurrent-writer test pattern.
    #[tokio::test]
    async fn wait_for_recovers_from_a_mid_tail_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.log");
        tokio::fs::write(&path, "").await.unwrap();
        let mut tail = RealLogTail::open(&path, &sigs()).await.unwrap();

        let path2 = path.clone();
        let writer = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            append_line(&path2, "some ignorable line before truncation\n").await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            // Simulates -conclearlog's own startup truncation.
            tokio::fs::write(&path2, "").await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            append_line(&path2, "[Server] BeginMatch\n").await;
        };

        let mut accept = |ev: &DetectionEvent| matches!(ev, DetectionEvent::BenchmarkStarted);
        let (result, ()) =
            tokio::join!(tail.wait_for(Duration::from_secs(5), &mut accept), writer,);
        assert_eq!(
            result.unwrap(),
            Some(DetectionEvent::BenchmarkStarted),
            "wait_for must detect the truncation and keep reading from the fresh file"
        );
    }

    /// The actual live-observed bug (found *after* the truncation-detection
    /// fix above shipped, during real `--controller windows` testing): CS2's
    /// `-conclearlog` doesn't necessarily truncate `console.log` in place
    /// (`wait_for_recovers_from_a_mid_tail_truncation`'s scenario, via
    /// `tokio::fs::write` over the same path — which reuses the file on
    /// NTFS rather than allocating a new one) — it can genuinely DELETE and
    /// RECREATE the file, which a size-only truncation check cannot detect
    /// at all if the new file happens to already be at-or-past the old
    /// read position by the time it's checked. This simulates that
    /// specifically: remove the file, then create a brand-new one at the
    /// same path (not an overwrite of the existing handle's target) with
    /// content that would already be past the tail's `pos` if the new
    /// file's growth were (wrongly) measured against the old file's stale
    /// handle.
    #[tokio::test]
    async fn wait_for_recovers_from_a_delete_and_recreate_not_just_an_in_place_truncate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.log");
        // Starts non-trivially sized so `pos` (captured at `open`'s
        // seek-to-EOF) is a real, non-zero offset into the ORIGINAL file —
        // proving the fix isn't just "happens to start from 0 anyway".
        tokio::fs::write(&path, "pre-existing content padding out the file\n")
            .await
            .unwrap();
        let mut tail = RealLogTail::open(&path, &sigs()).await.unwrap();

        let path2 = path.clone();
        let writer = async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            // Genuine delete + recreate, not an overwrite of the same file.
            tokio::fs::remove_file(&path2).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            tokio::fs::write(&path2, "[Server] BeginMatch\n")
                .await
                .unwrap();
        };

        let mut accept = |ev: &DetectionEvent| matches!(ev, DetectionEvent::BenchmarkStarted);
        let (result, ()) =
            tokio::join!(tail.wait_for(Duration::from_secs(5), &mut accept), writer,);
        assert_eq!(
            result.unwrap(),
            Some(DetectionEvent::BenchmarkStarted),
            "wait_for must detect the file was replaced (not just shrunk) and read the new one"
        );
    }

    /// `console.log` may not exist yet at the instant CS2's
    /// process becomes discoverable — `open` must retry briefly rather
    /// than failing on the very first `NotFound`.
    #[tokio::test]
    async fn open_retries_briefly_for_a_not_yet_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console.log");
        // Deliberately not created yet.

        let path2 = path.clone();
        let creator = async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            tokio::fs::write(&path2, "").await.unwrap();
        };

        let s = sigs();
        let (result, ()) = tokio::join!(RealLogTail::open(&path, &s), creator);
        assert!(
            result.is_ok(),
            "open should retry until the file appears rather than failing immediately"
        );
    }
}
