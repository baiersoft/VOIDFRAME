#![deny(clippy::undocumented_unsafe_blocks)]
use tauri::{Emitter, Manager, WebviewWindow, WindowEvent};
use tokio::sync::Mutex as AsyncMutex;

mod commands;
mod singleton_mutex;
mod state;
mod webview_orphan_cleanup;
mod webview_pid;

#[tauri::command]
fn minimize_window(window: WebviewWindow) {
    let _ = window.minimize();
}

#[tauri::command]
fn toggle_maximize_window(window: WebviewWindow) -> bool {
    if let Ok(is_max) = window.is_maximized() {
        if is_max {
            let _ = window.unmaximize();
            false
        } else {
            let _ = window.maximize();
            true
        }
    } else {
        let _ = window.maximize();
        true
    }
}

#[tauri::command]
fn close_window(window: WebviewWindow) {
    let _ = window.close();
}

#[tauri::command]
fn is_window_maximized(window: WebviewWindow) -> bool {
    window.is_maximized().unwrap_or(false)
}

/// Builds the `tauri-specta` command/type registry shared by both the
/// (debug-only) TS export and the real IPC dispatch — kept as its own
/// function so both call sites are guaranteed to register the exact same
/// command list.
///
/// `EngineEvent` is registered via [`tauri_specta::Builder::typ`], not
/// [`tauri_specta::collect_events!`]: the latter requires the event type to
/// implement `tauri_specta::Event` (via its `#[derive(Event)]` macro),
/// which lives in the `tauri-specta` crate — a Tauri-specific, IPC-oriented
/// dependency `voidframe-engine` intentionally does not take (it's shared
/// with `voidframe-cli`, which has no Tauri dependency at all). `EngineEvent`
/// is already emitted over a plain, hand-named `"vf:event"` channel
/// (`commands::run::start_run`'s forwarder), not through
/// `tauri_specta::Event`'s own named-emit mechanism, so `collect_events!`'s
/// auto-derived event name would go unused anyway. `.typ::<EngineEvent>()`
/// gets the identical practical result — `EngineEvent`'s full tagged-union
/// shape exported to `bindings.ts` — without that dependency.
///
/// `pub` so `tests/bindings_sync.rs` can export the same registry the app
/// dispatches from.
pub fn build_specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            commands::projects::list_projects,
            commands::projects::get_project,
            commands::projects::save_project,
            commands::projects::delete_project,
            commands::projects::validate_scenario,
            commands::preflight::preflight,
            commands::hardware::cpu_model,
            commands::hardware::gpu_model,
            commands::power_plan::list_power_plans,
            commands::launch_args::read_cs2_launch_options,
            commands::config::get_config,
            commands::config::save_config,
            commands::catalog::list_catalog_tweaks,
            commands::run::start_run,
            commands::run::send_control,
            commands::results::get_results,
            commands::results::list_results,
            commands::results::get_run_snapshot,
            commands::rollback::rollback_now,
            commands::rollback::emergency_rollback,
            commands::shell::open_data_dir,
            commands::shell::reveal_restore_bat,
        ])
        .typ::<voidframe_engine::run::EngineEvent>()
}

/// Whether a `CloseRequested` window event should be blocked because a run
/// is currently active. Split out from the `.on_window_event` closure
/// installed in [`run`] so it's unit-testable against a plain
/// `AsyncMutex<Option<ActiveRun>>`, without a live `tauri::Window`/`Event` --
/// the same extraction `commands::run::send_control_impl` and
/// `commands::rollback::reject_if_run_active` already use for identical
/// reasons.
///
/// Quitting mid-run can leave tweaks applied and CS2/PresentMon still
/// running, with only next-launch crash recovery left to catch it -- so an
/// active run blocks the close outright rather than just warning.
async fn should_block_close(active_run: &AsyncMutex<Option<state::ActiveRun>>) -> bool {
    active_run.lock().await.is_some()
}

/// Formats a panic's location and payload for the log sink. Split out from
/// the `std::panic::set_hook` closure installed in [`run`] so it is
/// unit-testable on plain strings/`Option`s — a real `PanicHookInfo` has no
/// public constructor outside an actual panic hook invocation, and mutating
/// the process-global panic hook from a test would race every other test in
/// this binary that panics concurrently.
fn format_panic_message(location: Option<String>, payload: Option<String>) -> String {
    let location = location.unwrap_or_else(|| "<unknown location>".into());
    let payload = payload.unwrap_or_else(|| "<non-string panic payload>".into());
    format!("panic at {location}: {payload}")
}

/// Removes the engine-level `instance.lock` file directly, mirroring the
/// exact `VOIDFRAME_DATA_ROOT`-aware resolution `AppState::new()` itself
/// uses (`DataRoot::with_base` when set, `DataRoot::resolve(false)`
/// otherwise) so this targets the identical path a subsequent
/// `AppState::new()` call will check. Only ever called after this process
/// has already proven, via the real singleton mutex, that it is the sole
/// legitimate VOIDFRAME instance -- see the call site's own comment.
fn clear_stale_instance_lock_file() {
    let data_root = match std::env::var_os("VOIDFRAME_DATA_ROOT") {
        Some(path) => voidframe_engine::paths::DataRoot::with_base(std::path::PathBuf::from(path)),
        None => voidframe_engine::paths::DataRoot::resolve(false),
    };
    if let Ok(data_root) = data_root {
        let _ = std::fs::remove_file(data_root.state_dir().join("instance.lock"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // This is a `windows_subsystem = "windows"` release build (see
    // `main.rs`) with no attached console, so an unhandled panic would
    // otherwise vanish with zero diagnostic trail. Installed first, before
    // anything below that could itself panic (e.g. the `.expect()` calls a
    // few lines down), so as much of `run()` as possible is covered.
    // `log::error!` is a no-op until `tauri-plugin-log` attaches the global
    // logger during `Builder::run()`'s own setup phase (see the `.plugin(...)`
    // call below) — a panic before that point still won't reach the log file,
    // but every other logging call site in this app has the exact same
    // limitation.
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(ToString::to_string);
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned());
        let msg = format_panic_message(location, payload);
        log::error!("{msg}");
    }));

    // The real single-instance guard `voidframe_engine::paths`'s own doc
    // comment already anticipated ("layers a Global\VOIDFRAME_SINGLETON
    // named mutex on top in a later phase") but was never actually built --
    // confirmed needed live 2026-09-05: the engine-level file lock's
    // pid-liveness heuristic false-positived on a reused pid under heavy
    // process churn, permanently blocking every later launch. See
    // `singleton_mutex`'s own module doc for the full mechanism.
    let _singleton_mutex_handle = match singleton_mutex::acquire() {
        Ok(Some(handle)) => {
            // Holding this mutex proves no other real instance exists,
            // regardless of what the file lock's own pid-liveness check
            // would separately conclude -- see `clear_stale_instance_lock_file`'s
            // own doc comment.
            clear_stale_instance_lock_file();
            Some(handle)
        }
        Ok(None) => {
            // Another live VOIDFRAME instance already holds the mutex.
            return;
        }
        Err(_) => {
            // Non-fatal: fall back to the file lock's own recovery.
            None
        }
    };

    // MUST stay wrapped in `tauri::async_runtime::block_on`. `run()` is a
    // plain, non-async fn called straight from `main`, before
    // `tauri::Builder` (and therefore any tokio runtime) exists on this
    // thread -- but `AppState::new()` reaches
    // `voidframe_engine::store::spawn_store`, whose body is a bare
    // `tokio::spawn`. Calling it outside a runtime panics immediately with
    // "there is no reactor running, must be called from the context of a
    // Tokio 1.x runtime", which meant the app could not start in ANY build
    // profile. `tauri::async_runtime::block_on` enters Tauri's own global
    // tokio runtime -- the same one the app then runs on -- so the store
    // actor's task attaches to it correctly and outlives this call.
    let app_state = tauri::async_runtime::block_on(async { state::AppState::new() })
        .expect("failed to initialize AppState");

    // Must run here: strictly after the single-instance lock `AppState::new()`
    // just acquired (confirming no other legitimate VOIDFRAME instance is
    // running right now, so any match below can only be a stale leftover),
    // and strictly before `tauri::Builder`'s own webview creation a few
    // lines down — by the time `.setup()` runs, a new WebView2 environment
    // has already been created, which is exactly the point at which a
    // leftover suspended browser process from a killed previous instance
    // would already have caused the hang this cleans up. See
    // `webview_orphan_cleanup`'s module doc for the full mechanism.
    // Best-effort: `log::error!` is a no-op this early (see the panic hook
    // comment above), so a failure here is deliberately swallowed rather
    // than surfaced — this sweep finding nothing to clean up, or even
    // failing outright, must never block startup.
    // NOTE: `catch_unwind` would NOT protect this call even if added -- this
    // profile builds with `panic = "abort"` (workspace `Cargo.toml`), so any
    // panic here aborts the process immediately regardless; `catch_unwind`
    // only works under the (unwind) panic strategy. If the "returned" marker
    // below never appears, that alone proves a panic (or worse) happened
    // inside this call, without needing to catch it.
    // Run on a dedicated, throwaway thread rather than this one -- this
    // thread is the one `tauri::Builder::run()` a few lines down uses to set
    // up its OWN COM apartment for WebView2. Even a properly balanced
    // CoInitializeEx/CoUninitialize cycle right here, on the same thread,
    // left residual state that caused Tauri's own webview creation to hang
    // afterward (confirmed live 2026-09-05: the cleanup itself completed
    // and reported success, but `builder.run()` right after it never
    // produced another sign of life). A separate thread gets its own,
    // completely independent COM apartment -- no possible interference with
    // whatever Tauri sets up on this one, regardless of the exact mechanism.
    let _ = std::thread::spawn(webview_orphan_cleanup::cleanup_orphaned_webview_processes).join();

    // Read out before `.manage(app_state)` moves it below; the log plugin
    // needs `DataRoot`'s already-resolved `logs/` directory (see
    // `docs/02-architecture.md` §3) rather than Tauri's own
    // `app_handle.path().app_log_dir()`, which resolves against the bundle
    // identifier and would not land under `%LOCALAPPDATA%\baiersoft\VOIDFRAME`.
    let log_dir = app_state.data_root.logs_dir();

    let specta_builder = build_specta_builder();

    // Regenerated on every debug build (never in release) so `src/lib/bindings.ts`
    // stays in sync with the real command/type surface automatically as
    // commands change through ongoing frontend-integration work — committed like a
    // lockfile, not gitignored.
    //
    // Verified working
    // (`docs/superpowers/plans/2026-09-02-phase-4b-1-frontend-foundation.md`'s
    // first task): the STATUS_ENTRYPOINT_NOT_FOUND
    // crash this call used to hit at OS load time was a missing Windows
    // Common-Controls v6 manifest dependency (tauri's dialog APIs statically
    // import TaskDialogIndirect from comctl32.dll, only present in v6) --
    // fixed in build.rs's custom app_manifest, which now carries both the
    // Common-Controls dependency AND the requireAdministrator block. A real
    // debug build now produces this file successfully.
    //
    // Getting a clean export past that also required narrowing every
    // specta-exported `usize`/`u64` field to `u32` (specta-typescript 0.0.12
    // hard-forbids BigInt-style Rust types -- `usize`, `u64`, `i64`, `u128`,
    // `i128` -- by default): `RunState::revision` (voidframe_engine::store),
    // and `EngineEvent::RollbackProgress::reverted` + `RevertReport::reverted`
    // (voidframe_engine::run / voidframe_engine::journal::replay, whose
    // ripple effect also required narrowing voidframe-cli's own
    // `ScenarioOutcome::reverted`, a plain passthrough of the same value) --
    // all plain monotonic counters that never approach u32's range. A config
    // escape hatch *does* exist for this (`Typescript::enable_lossless_bigints()`
    // would map these to TS `bigint`), deliberately not used here: Tauri's
    // JSON-based IPC can't losslessly carry a real BigInt across the wire, so
    // enabling it would produce TS types that lie about what actually arrives
    // at runtime -- narrowing the Rust side to `u32` instead keeps the
    // generated types honest. Two more fields are deliberately-untyped
    // `serde_json::Value` (`CatalogEntry::module_template` and
    // `RegistryPayload::value`), which specta's `serde_json` feature
    // recursively derives down to `serde_json::Number`'s internal i64/u64
    // representation -- a type this crate doesn't own, so instead of
    // narrowing, both carry a field-level
    // `#[specta(type = specta_typescript::Unknown)]` override (specta-typescript's
    // own documented escape hatch for opaque JSON) to export as TypeScript
    // `unknown`, forcing type-narrowing at every read site rather than
    // silently disabling type checking (`any`) on them.
    //
    // Finally, the destination path below is intentionally anchored on
    // `CARGO_MANIFEST_DIR` (a compile-time env var, always `src-tauri/`'s own
    // absolute path) rather than a runtime-relative literal: a plain
    // `"../src/lib/bindings.ts"` only resolves correctly when the process's
    // *runtime* CWD happens to be `src-tauri/` (true for `npm run tauri:dev`,
    // which the Tauri CLI sets up that way) but silently writes one directory
    // above the actual repo root when the built exe is launched directly from
    // anywhere else (e.g. the repo root) -- exactly what happened during this
    // task's own verification.
    #[cfg(debug_assertions)]
    specta_builder
        .export(
            specta_typescript::Typescript::default(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts"),
        )
        .expect("failed to export TS bindings");

    // The 4 window-chrome commands predate this plan's own command surface
    // and are deliberately NOT specta-annotated/exported (task-13-brief.md's
    // own `collect_commands!` list omits them) -- so they're dispatched
    // through a second, plain `tauri::generate_handler!`, and the two
    // handlers are combined by command name below rather than through
    // `tauri_specta::Builder::commands` (which would require annotating and
    // exporting them too).
    let window_commands_handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool = tauri::generate_handler![
        minimize_window,
        toggle_maximize_window,
        close_window,
        is_window_maximized,
    ];
    let specta_invoke_handler = specta_builder.invoke_handler();

    let builder = tauri::Builder::default()
        .plugin(
            // Sole global `log::Log` for the process (`log::set_boxed_logger`,
            // done internally by this plugin's own `setup` hook) — every
            // `log::*` call in this crate, plus every `tracing::*` call in
            // `voidframe-engine` (bridged via `tracing`'s own `"log"` feature
            // flag, turned on workspace-wide in the root `Cargo.toml`; that
            // feature emits a `log::Record` for each tracing span/event only
            // when no `tracing::Subscriber` is active, which is always true
            // here since this app never installs one), lands in the same
            // rotating file. `voidframe-cli` sets a real `tracing_subscriber`
            // in its own `main.rs`, so the same feature flag is a no-op there
            // and its own tracing-based output is unaffected.
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Folder {
                        path: log_dir,
                        file_name: Some("voidframe".into()),
                    },
                ))
                // KeepOne (the plugin default) would delete the previous
                // session's log outright on every rotation; KeepSome keeps a
                // rolling window of dated archives instead, matching
                // `docs/02-architecture.md`'s documented
                // `logs\voidframe-YYYY-MM-DD.log` history.
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(14))
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .manage(app_state)
        // Guards against quitting mid-run (see `should_block_close`'s own
        // doc comment): a plain `close_window`/OS close button previously
        // tore the window down unconditionally, with no
        // `CloseRequested`/`ExitRequested` handler anywhere in this chain to
        // catch it. `block_on` here is safe/non-blocking in practice --
        // `active_run`'s lock is only ever held across the tiny synchronous
        // sections `start_run`/`send_control_impl`/rollback already use it
        // for, never across an `.await` -- and this closure itself runs on
        // Tauri's event-loop thread, not inside a tokio worker task, so
        // there is no "block_on inside block_on" hazard (the same reasoning
        // `AppState::new()`'s own `block_on` call above already relies on).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let Some(state) = window.try_state::<state::AppState>() else {
                    return;
                };
                if tauri::async_runtime::block_on(should_block_close(&state.active_run)) {
                    api.prevent_close();
                    let _ = window.emit(
                        "vf:event",
                        &voidframe_engine::run::EngineEvent::LogLine {
                            text: "Cannot close VOIDFRAME while a benchmark run is active -- \
                                   pause or abort it first."
                                .into(),
                        },
                    );
                }
            }
        })
        .setup(move |app| {
            // Required for `EngineEvent`'s registered type info to resolve;
            // harmless even without any `tauri_specta::Event`-derived event
            // actually mounted (see `build_specta_builder`'s doc comment).
            specta_builder.mount_events(app);

            // Best-effort: a tester having something runnable on first
            // launch matters, but must never block startup if it fails
            // (disk full, permissions) -- same "degrade, don't crash"
            // posture as the thermal-cooldown fallback.
            let state = app.state::<state::AppState>();
            if let Err(e) =
                voidframe_engine::model::seed_alpha_test_project(&state.data_root.projects_dir())
            {
                log::warn!("failed to seed the Alpha Test starter project: {e}");
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(move |invoke| match invoke.message.command() {
            "minimize_window"
            | "toggle_maximize_window"
            | "close_window"
            | "is_window_maximized" => window_commands_handler(invoke),
            _ => specta_invoke_handler(invoke),
        });

    // Only compiled in at all behind the `webdriver-testing` Cargo feature
    // (never on for a plain `cargo build`/`tauri build`/`tauri:build`
    // release -- see Cargo.toml's own comment on why `cfg(debug_assertions)`
    // can't do this), AND only activated when `WDIO_EMBEDDED_SERVER` is set
    // -- an elevated, system-mutating app must never expose an automation
    // HTTP server just because it happens to be that build.
    // `WDIO_EMBEDDED_SERVER` (and `TAURI_WEBDRIVER_PORT`, which the plugin
    // itself reads) is the convention `@wdio/tauri-service`'s embedded
    // driver provider always sets automatically when it spawns the app
    // (see its own source, `startEmbeddedDriver`) -- not a name invented
    // for this app, so e2e/wdio.conf.ts needs no extra env-passing config.
    // This server sidesteps WebView2's own `--remote-debugging-port`/CDP
    // attach path entirely -- the mechanism plain `tauri-driver`/
    // msedgedriver relies on, which hangs against this app's
    // `requireAdministrator` elevation ("session not created:
    // DevToolsActivePort file doesn't exist", the app itself stays
    // perfectly healthy) -- a currently-open upstream Tauri/WebView2 issue
    // for elevated apps, not something fixable here.
    // `tauri_plugin_wdio` is the companion plugin that actually injects the
    // frontend-side bridge (`window.__wdio_original_core__`) which
    // `browser.tauri.execute()` and the service's per-command window-focus
    // check both depend on -- `tauri-plugin-wdio-webdriver` alone only runs
    // the embedded HTTP server, so without this every WebdriverIO command
    // paid an unconditional 5s timeout waiting for a bridge that never
    // showed up (see Cargo.toml's comment for the full symptom).
    #[cfg(feature = "webdriver-testing")]
    let builder = if std::env::var_os("WDIO_EMBEDDED_SERVER").is_some() {
        builder
            .plugin(tauri_plugin_wdio::init())
            .plugin(tauri_plugin_wdio_webdriver::init())
    } else {
        builder
    };

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod panic_message_tests {
    use super::format_panic_message;

    #[test]
    fn includes_location_and_payload_when_both_are_known() {
        let msg = format_panic_message(Some("src/lib.rs:1:1".into()), Some("boom".into()));
        assert_eq!(msg, "panic at src/lib.rs:1:1: boom");
    }

    #[test]
    fn falls_back_when_location_and_payload_are_unknown() {
        let msg = format_panic_message(None, None);
        assert_eq!(
            msg,
            "panic at <unknown location>: <non-string panic payload>"
        );
    }
}

#[cfg(test)]
mod close_guard_tests {
    use super::{AsyncMutex, should_block_close};
    use crate::state::ActiveRun;

    #[tokio::test]
    async fn blocks_close_while_a_run_is_active() {
        let (control_tx, _control_rx) = tokio::sync::mpsc::channel(1);
        let active_run = AsyncMutex::new(Some(ActiveRun {
            run_id: "r1".into(),
            control_tx,
        }));
        assert!(should_block_close(&active_run).await);
    }

    #[tokio::test]
    async fn allows_close_with_no_active_run() {
        let active_run: AsyncMutex<Option<ActiveRun>> = AsyncMutex::new(None);
        assert!(!should_block_close(&active_run).await);
    }
}
