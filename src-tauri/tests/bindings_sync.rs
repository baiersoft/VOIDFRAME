use voidframe_lib::build_specta_builder;

/// The committed bindings file, anchored on this crate's manifest dir
/// (the same anchoring `run()`'s debug-build export uses).
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

/// `src/lib/bindings.ts` must equal what the current command/type
/// surface exports. Set `VOIDFRAME_WRITE_BINDINGS=1` to rewrite the
/// committed file instead of comparing -- the agent-friendly
/// replacement for launching `tauri dev` (elevated, GUI) just to
/// regenerate bindings.
#[test]
fn bindings_ts_matches_the_command_and_type_surface() {
    let dir = tempfile::tempdir().unwrap();
    let exported_path = dir.path().join("bindings.ts");
    build_specta_builder()
        .export(specta_typescript::Typescript::default(), &exported_path)
        .expect("specta export failed");
    let fresh = std::fs::read_to_string(&exported_path).unwrap();

    if std::env::var_os("VOIDFRAME_WRITE_BINDINGS").is_some() {
        std::fs::write(BINDINGS_PATH, &fresh).unwrap();
        return;
    }

    let committed = std::fs::read_to_string(BINDINGS_PATH).unwrap();
    assert_eq!(
        fresh.replace("\r\n", "\n"),
        committed.replace("\r\n", "\n"),
        "src/lib/bindings.ts is stale. Regenerate with:\n  \
         $env:VOIDFRAME_WRITE_BINDINGS=1; cargo test -p voidframe --test bindings_sync"
    );
}
