//! CS2 session preparation (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.2): build-freeze re-check, launch-args
//! reconciliation, Steam readiness, path/keybind resolution, signature
//! load, stale-cs2 cleanup, and the CS2 launch itself -- everything
//! `run_scenario_inner` needs before it can open the detection channel and
//! start iterating. Also owns the menu-ready wait `run_scenario_inner`'s
//! own iteration loop needs again on the watchdog relaunch retry.

use super::RunContext;
use super::signatures::load_signatures;
use super::steam::{
    ensure_steam_running, graceful_kill_cs2, kill_process_tree, wait_for_process,
    wait_until_steam_closed, wait_until_steam_ready,
};
use crate::cs2::detection::{Cs2LogDetector, DetectionEvent, Signatures};
use crate::error::{Error, Result};
use crate::model::Module;
use crate::model::project::Scenario;
use crate::run::{ControlMsg, EngineEvent};
use crate::system::{Cs2LaunchSpec, SystemController};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// What [`prepare_cs2_session`] hands back to `run_scenario_inner` once CS2
/// is confirmed running: the resolved `console.log` path and loaded
/// [`Signatures`] the detection channel needs to open, plus the launched
/// pid. `run_scenario_inner` sets its own `launched_pid` out-param from
/// `pid` immediately after this call returns -- the cleanup contract that
/// out-param exists for (kill it if anything later fails) is unchanged by
/// this split, only where the pid first becomes known is.
pub(super) struct Cs2Session {
    pub(super) console_log_path: PathBuf,
    pub(super) sigs: Signatures,
    pub(super) pid: u32,
}

/// Everything a scenario's CS2 session needs before the detection channel
/// can open: the build-freeze re-check, launch-args reconciliation, Steam
/// readiness, path + keybind resolution, signature load, stale-cs2
/// cleanup, and finally `launch_cs2` + `wait_for_process`. Called once by
/// `run_scenario_inner`, first thing -- its `Result` propagates through
/// that function's own `?`, which `run_scenario`'s caller already runs
/// kill(if needed)+revert on, so a failure at any step here still gets the
/// same cleanup an iteration-loop failure would.
pub(super) async fn prepare_cs2_session(
    sys: &dyn SystemController,
    scenario: &Scenario,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
) -> Result<Cs2Session> {
    // --- Build-freeze re-check (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.5), at every scenario boundary ---
    // A build drift has nothing to do with process control specifically,
    // it's purely "did Steam auto-update CS2 out from under this run since
    // it started" — a question worth asking before this scenario does any
    // more work, baseline included (baseline is still a `run_scenario`
    // call).
    if let Some(run_start) = &ctx.start_build_id {
        let current = sys.app_manifest(730).await?.and_then(|m| m.build_id);
        // `Ok(None)` (CS2 not found — shouldn't happen mid-run, pre-flight
        // already checked this at run start) or an unchanged id both mean
        // "nothing to prompt about" — `filter` collapses both into `None`.
        if let Some(current) = current.filter(|c| c != run_start) {
            events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "CS2 build id changed since run start ({run_start} -> {current})"
                    ),
                })
                .await
                .ok();
            events
                .send(EngineEvent::OperatorPrompt {
                    text: "CS2's installed build has changed since this run started (Steam \
                           likely auto-updated it). Scenarios measured from here on may not be \
                           directly comparable to ones measured earlier in this run. \
                           Acknowledge to continue this run as-is."
                        .into(),
                })
                .await
                .ok();
            // A full abort/re-baseline choice is out of scope for this
            // task (per the plan's own note) — acknowledgement means
            // "continue", matching the launch-args-mismatch prompt below.
            // Whether this errors (control channel closed) or the operator
            // sends Abort, the `?` below now propagates through this
            // function's own `Result`, which `run_scenario`'s caller
            // always runs kill(if needed)+revert on — that's the whole
            // point of this check living here now.
            wait_for_operator_ack(control).await?;
        }
    }

    // --- Reconcile launch_args (§7.2 step 2) ---
    // Deliberately runs BEFORE the general "Ensure Steam is ready" check
    // below: computing
    // `desired_args` is pure (only reads `scenario`), and
    // `sys.read_cs2_launch_options()` is a plain file read that works
    // regardless of whether Steam is running -- so this reconciliation has
    // zero dependency on Steam's readiness. Running it first avoids a
    // redundant launch-kill-relaunch cycle: if Steam wasn't running at
    // scenario start and the readiness check ran first, it would already
    // have launched Steam via `ensure_steam_running` -- and this block's own
    // kill+relaunch would then tear down and restart that just-launched
    // Steam a second time for no reason, risking a kill during Steam's own
    // bootstrap/self-update (the single riskiest moment to terminate it).
    // When args differ, this block's own kill+wait+write+
    // `ensure_steam_running`(-with-fallback) sequence already correctly
    // handles "Steam wasn't running to begin with" on its own (the kill is
    // conditional on `find_process` returning `Some`, and
    // `wait_until_steam_closed` is a no-op when Steam is already not
    // running) -- so it becomes the ONLY thing that launches Steam for this
    // scenario in that case, and the readiness check below (now running
    // after this) sees Steam already running and is a genuine no-op.
    let current_args_raw = sys.read_cs2_launch_options().await?;
    let scenario_launch_args: Option<String> = scenario.modules.iter().find_map(|m| match m {
        Module::LaunchArgs { args } => Some(args.clone()),
        _ => None,
    });
    // A configured `launch_args` module is a deliberate override of
    // whatever's live; no module means the user's real current options pass
    // through unchanged -- VOIDFRAME only ever ADDS its reserved tokens on
    // top, never overwrites what the user already had. "Unchanged" means
    // the run-start snapshot (`ctx.start_launch_args`), not a fresh live
    // read: by the time a later scenario gets here, `current_args_raw` may
    // already be whatever an *earlier scenario in this same run* wrote (its
    // own `launch_args` module's value) -- falling back to that would
    // silently carry a previous scenario's launch options into this one
    // instead of resetting to the user's real baseline.
    let effective_user_args = scenario_launch_args.unwrap_or_else(|| ctx.start_launch_args.clone());
    let desired_args = crate::cs2::keybind_cfg::reconcile(&effective_user_args);

    if current_args_raw != desired_args {
        events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Launch options changed for scenario '{}': before: \"{}\" after: \"{}\"",
                    scenario.name,
                    crate::cs2::keybind_cfg::redact(&current_args_raw),
                    crate::cs2::keybind_cfg::redact(&desired_args)
                ),
            })
            .await
            .ok();
        // docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.2 step 2 "Differ" branch (§2.1a rev 6): VOIDFRAME now
        // performs the whole close -> edit -> relaunch sequence itself.
        // Closing Steam is always safe regardless of its integrity (the
        // risk was always about *restarting* it elevated —
        // spike/findings.md §5's cold-start-leaks-elevation finding);
        // relaunching via `ensure_steam_running` is exactly what avoids
        // that. Falls back to the old manual OperatorPrompt sequence only
        // if the automated relaunch itself fails.
        write_launch_options_with_steam_closed(sys, &desired_args).await?;
        if let Err(e) = ensure_steam_running(sys).await {
            events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "Could not relaunch Steam automatically ({e}); falling back to a manual restart."
                    ),
                })
                .await
                .ok();
            events
                .send(EngineEvent::OperatorPrompt {
                    text: "Launch options updated. Please reopen Steam normally, then acknowledge."
                        .into(),
                })
                .await
                .ok();
            wait_for_operator_ack(control).await?;
            wait_until_steam_ready(sys).await?;
        }
    } else {
        events
            .send(EngineEvent::LogLine {
                text: format!("Launch options unchanged for scenario '{}'", scenario.name),
            })
            .await
            .ok();
    }

    // --- Ensure Steam is ready (§2.1a rev 6, §7.2 step 1) ---
    // Runs AFTER launch_args reconciliation above -- see
    // that block's own comment for why. If args differed, that block
    // already launched/relaunched Steam itself, so this is a genuine no-op
    // check in that case. If args matched, this remains the only thing
    // that ever launches Steam for this scenario -- unchanged behavior for
    // that case.
    const STEAM_ELEVATED_WARNING: &str = "Steam is running elevated -- the CS2 it launches will be elevated too. \
         VOIDFRAME never closes an already-running Steam just to fix this.";
    let steam = sys.steam_status().await?;
    if !steam.running {
        if let Err(e) = ensure_steam_running(sys).await {
            return Err(Error::preflight(format!(
                "Steam is not running and could not be started automatically: {e}"
            )));
        }
        // `ensure_steam_running` just launched Steam
        // successfully -- re-check whether the result came up elevated
        // (it shouldn't, given the de-elevation technique, but nothing
        // else here confirms that) and warn the same way the
        // already-running branch below does.
        if sys.steam_status().await?.elevated {
            events
                .send(EngineEvent::LogLine {
                    text: STEAM_ELEVATED_WARNING.into(),
                })
                .await
                .ok();
        }
    } else if steam.elevated {
        events
            .send(EngineEvent::LogLine {
                text: STEAM_ELEVATED_WARNING.into(),
            })
            .await
            .ok();
    }

    // --- Resolve console.log path + CS2's own cfg\ directory, and ensure
    // VOIDFRAME's keybind file exists there (§7.2 step 1.5) ---
    // `reserved_tokens()` (used by the `reconcile` call above) unconditionally
    // puts `+exec voidframe_keybind.cfg` into every scenario's launch
    // options, so the file itself must exist in CS2's `cfg\` directory
    // *before* CS2 ever launches expecting to `+exec` it — this is
    // resolved and written here, ahead of CS2's own launch further down,
    // rather than never at all. `console_log_path` is resolved
    // once here and reused unchanged by the detection channel further down
    // (§7.2 step 4), instead of being re-resolved a second time right
    // before `LogTail::open`.
    let (console_log_path, cs2_cfg_dir) = if let Some(p) = &ctx.config.console_log_override {
        // Test-only seam (see `RunConfig::console_log_override`'s own doc
        // comment): mirror its directory layout (`<tmp>/cfg/`) rather than
        // touching the real Steam install, so tests stay hermetic.
        let dir = p
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        (p.clone(), dir.join("cfg"))
    } else {
        // Deliberately NOT `steam_install_path` alone: CS2 is very often
        // installed in a different Steam Library Folder than the Steam
        // client itself (a separate drive via Storage Manager) — see
        // `SystemController::app_library_path`'s own doc comment for why
        // that assumption silently broke before this existed.
        let cs2_library = sys.app_library_path(730).await?;
        let game_dir = cs2_library
            .join("steamapps")
            .join("common")
            .join("Counter-Strike Global Offensive")
            .join("game")
            .join("csgo");
        (game_dir.join("console.log"), game_dir.join("cfg"))
    };
    crate::cs2::keybind_cfg::ensure_written(&cs2_cfg_dir)?;

    // --- Load detection signatures (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.4) ---
    // Resolved and loaded here — before the stale-CS2-process check and
    // `launch_cs2` below — so a genuinely broken/missing signatures setup
    // fails fast without ever touching CS2, rather than launching CS2
    // first and only discovering the problem afterward.
    let sigs = load_signatures(&ctx.config.data_root)?;

    // --- Launch CS2 (§7.2 step 3) ---
    // Guard against attaching to a STALE cs2.exe left over from an earlier
    // crash or watchdog failure — every scenario kills its own CS2 before
    // REVERT_MODULES, so finding one now means something didn't clean up
    // last time. Clean it up rather than silently attaching affinity/
    // detection to the wrong process.
    if let Some(stale) = sys.find_process("cs2.exe").await? {
        // Diagnostic, not load-bearing: `started_at` lets a later log line
        // (this launch's own "CS2 process discovered") answer, from real
        // Windows process-creation timestamps rather than a guess, whether
        // that later pid is genuinely a NEW process or this exact one
        // (pid reuse can make two different processes share a pid).
        let started_at = sys.process_started_at(stale.pid).await;
        tracing::warn!(
            pid = stale.pid,
            started_at = ?started_at,
            "found a pre-existing cs2.exe before launch — closing it before proceeding"
        );
        events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Found a pre-existing cs2.exe (pid {}) before launch -- closing it before \
                     proceeding",
                    stale.pid
                ),
            })
            .await
            .ok();
        graceful_kill_cs2(sys).await?;
    }
    events
        .send(EngineEvent::LogLine {
            text: format!("Launching CS2 for scenario '{}'", scenario.name),
        })
        .await
        .ok();
    sys.launch_cs2(&Cs2LaunchSpec { app_id: 730 }).await?;
    let cs2 = wait_for_process(sys, "cs2.exe", Duration::from_secs(60), true).await?;
    let cs2_pid = cs2.pid;
    // See the stale-check's own comment above — comparing this against a
    // later "found a pre-existing cs2.exe" warning's own `started_at` is
    // how to tell whether that was genuinely this same process the whole
    // time, or a different one that happened to reuse the pid.
    let started_at = sys.process_started_at(cs2_pid).await;
    tracing::info!(pid = cs2_pid, started_at = ?started_at, "CS2 process discovered");
    events
        .send(EngineEvent::LogLine {
            text: format!("CS2 ready (pid {cs2_pid})"),
        })
        .await
        .ok();

    Ok(Cs2Session {
        console_log_path,
        sigs,
        pid: cs2_pid,
    })
}

/// Closes Steam (if it's currently running) and writes `desired` into its
/// `LaunchOptions`: kill `steam.exe` if found -> `wait_until_steam_closed`
/// -> `write_cs2_launch_options`. Extracted out of `prepare_cs2_session`'s
/// own "Differ" branch so `rollback`'s run-start launch-options restore
/// (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes task 7) can
/// share the identical close+write sequence. Deliberately does NOT relaunch
/// Steam itself -- each caller calls `ensure_steam_running` on its own right
/// after, and keeps its own fallback for that call failing
/// (`prepare_cs2_session`'s manual `OperatorPrompt` sequence vs.
/// `rollback`'s log-only one), since only `prepare_cs2_session` has a
/// control channel to run an operator prompt against.
pub(super) async fn write_launch_options_with_steam_closed(
    sys: &dyn SystemController,
    desired: &str,
) -> Result<()> {
    if let Some(steam) = sys.find_process("steam.exe").await? {
        kill_process_tree(sys, steam.pid).await?;
    }
    wait_until_steam_closed(sys).await?;
    sys.write_cs2_launch_options(desired).await?;
    Ok(())
}

async fn wait_for_operator_ack(control: &mut mpsc::Receiver<ControlMsg>) -> Result<()> {
    loop {
        match control.recv().await {
            Some(ControlMsg::OperatorAcknowledged) => return Ok(()),
            Some(ControlMsg::Abort) => {
                return Err(Error::aborted("aborted while waiting for operator".into()));
            }
            Some(_) => continue, // Pause/Resume ignored here — an operator prompt IS a pause point
            None => return Err(Error::msg("control channel closed".into())),
        }
    }
}

pub(super) fn is_ready_marker(ev: &DetectionEvent) -> bool {
    // docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.3 step 2 treats "map-loaded / benchmark-started" as one
    // combined wait, not two sequential ones — see this task's report for
    // the full reasoning (spike/findings.md §2 shows `[Server] BeginMatch`
    // following `Loading map` by only a few hundred ms in the one captured
    // run, and repeated `reissue_map` calls against the same already-
    // running CS2 process are not confirmed to always re-emit a fresh
    // `BeginMatch` line the same way they reliably re-emit `Loading map`).
    // Accepting either, whichever the log actually produces first, is the
    // more robust reading.
    matches!(
        ev,
        DetectionEvent::MapLoaded { .. } | DetectionEvent::BenchmarkStarted
    )
}

/// Waits for CS2's own main-menu-load completion signal (see
/// `Signatures::menu_ready`'s doc comment) before the caller sends any
/// simulated input. Confirmed live: the Win32 "window is visible" signal
/// alone (already waited for inside `reissue_map` itself, via
/// `find_visible_window_for_pid`) fires several seconds before the game is
/// actually processing keyboard input — CS2 still shows its own boot/intro
/// sequence after the window appears, and pressing the console-open key
/// during that window is silently swallowed (no error; the console just
/// never opens).
///
/// Best-effort, not a hard gate: if this specific diagnostic line never
/// appears (no network connectivity, Steam in offline mode, or a future CS2
/// build changing what it logs at this point), this does not fail the
/// iteration — it logs a warning and falls back to a fixed delay, since a
/// missing diagnostic line here is a weaker signal that something is
/// actually wrong than a missing map-load marker later in the same
/// iteration would be (that failure mode is already handled by the
/// watchdog). Called once per CS2 launch — right after each `LogTail::open`
/// (the initial one and the watchdog's relaunch one), before the first
/// `reissue_map` call that follows it — never once per iteration, since
/// CS2's own menu-boot sequence only happens once per process lifetime.
///
/// `timeout` is the caller's own scenario `watchdog` duration, reused
/// rather than a separate hardcoded constant: a real run gets a generous,
/// user-configured search budget for free (this signal was observed live
/// to appear well within a typical `watchdog_seconds`), and tests using a
/// short `watchdog_seconds` stay fast automatically rather than every test
/// eating a large fixed wait regardless of its own timing needs.
pub(super) async fn wait_for_menu_ready(
    tail: &mut dyn Cs2LogDetector,
    timeout: Duration,
    events: &mpsc::Sender<EngineEvent>,
) {
    // Capped, not the full `timeout` outright: on the soft-fallback path
    // below this doubles as the delay before proceeding anyway, and a
    // multi-minute `watchdog_seconds` (a real, valid setting) should not
    // turn a missing diagnostic line into a multi-minute stall by itself.
    let fallback_delay = (timeout / 4).min(Duration::from_secs(10));

    match tail
        .wait_for(timeout, &mut |ev| matches!(ev, DetectionEvent::MenuReady))
        .await
    {
        Ok(Some(_)) => {
            // Small buffer past the log line itself, in case the engine's
            // own input handling lags a moment behind writing this
            // diagnostic line to disk.
            tokio::time::sleep(fallback_delay.min(Duration::from_secs(2))).await;
        }
        Ok(None) => {
            // Diagnostics: distinguishes "read nothing at all" (a
            // truncation/identity-detection bug — genuinely nothing ever
            // reached this tail) from "read plenty, just never the right
            // line" (a regex/signature problem) without needing to
            // reproduce a live failure to find out which.
            events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "menu-ready signal not seen within {timeout:?} — read {} line(s) in \
                         that time, last one: {:?} — proceeding after a fixed delay instead \
                         (soft fallback, not a hard failure)",
                        tail.lines_read(),
                        tail.last_line_seen().unwrap_or("<none>")
                    ),
                })
                .await
                .ok();
            tokio::time::sleep(fallback_delay).await;
        }
        Err(e) => {
            // A real I/O error reading console.log here doesn't need to
            // escalate on its own — the caller's subsequent reissue_map/
            // wait_for calls will surface a real error if console.log is
            // genuinely broken.
            tracing::warn!(
                error = ?e,
                "error while waiting for menu-ready signal — proceeding after a fixed delay instead"
            );
            events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "Error while waiting for the menu-ready signal ({e}) -- proceeding after \
                         a fixed delay instead"
                    ),
                })
                .await
                .ok();
            tokio::time::sleep(fallback_delay).await;
        }
    }
}
