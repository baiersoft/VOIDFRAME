fn main() {
    #[cfg(windows)]
    {
        // Placeholder-binary guard: `src-tauri/binaries/*.exe` are checked-in
        // text placeholders (see `src-tauri/binaries/README.md`) until the
        // real PresentMon/HWiNFO64 binaries are dropped in before a release
        // build. `validate_presentmon_path` (src-tauri/src/commands/config.rs)
        // happily accepts a placeholder -- it exists, is a regular file, and
        // has an `.exe` extension -- so nothing downstream catches a release
        // build that still ships them. Fail the build outright in the
        // `release` profile if either bundled binary isn't a real PE
        // executable (missing the `MZ` magic bytes real placeholders, being
        // plain text, never have). Gated on `PROFILE == "release"` only --
        // dev/test builds must keep working with placeholders present.
        if std::env::var("PROFILE").as_deref() == Ok("release") {
            let manifest_dir =
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
            for exe_name in [
                "presentmon-x86_64-pc-windows-msvc.exe",
                "hwinfo64-x86_64-pc-windows-msvc.exe",
            ] {
                let path = std::path::Path::new(&manifest_dir)
                    .join("binaries")
                    .join(exe_name);
                let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    panic!("failed to read bundled binary {}: {e}", path.display())
                });
                if bytes.get(0..2) != Some(b"MZ") {
                    panic!(
                        "{} is not a real PE executable (missing the 'MZ' magic bytes) -- this \
                         is still the checked-in placeholder from src-tauri/binaries/README.md. \
                         Replace it with the real binary before building a release installer.",
                        path.display()
                    );
                }
            }
        }

        // NOTE: `app_manifest` REPLACES `tauri-build`'s own default manifest
        // outright — it does not merge with it. That default manifest's
        // entire content is the `Microsoft.Windows.Common-Controls 6.0.0.0`
        // dependency block reproduced below, which Tauri's own dialog APIs
        // (native file/message pickers — Phase 4b's own work depends on
        // them) require; `tauri-build`'s documentation warns about exactly
        // this. So this custom manifest must carry BOTH blocks: the
        // `trustInfo`/`requireAdministrator` block this app needs (design
        // decision D4 — VOIDFRAME's registry/powercfg/affinity mutations all
        // require elevation) AND the Common-Controls dependency it would
        // otherwise silently drop.
        let windows = tauri_build::WindowsAttributes::new().app_manifest(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
        );
        let attrs = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attrs).expect("failed to embed the requireAdministrator manifest");

        // `tests/bindings_sync.rs` links `tauri_specta::Builder::export`, which
        // statically imports `TaskDialogIndirect` from comctl32 v6. Without a
        // manifest declaring that dependency, the OS loader fails the whole
        // test process at start with `STATUS_ENTRYPOINT_NOT_FOUND` before any
        // test body runs -- the same failure mode the app manifest above
        // already fixes for the real `[[bin]] voidframe` executable.
        // Deliberately Common-Controls-v6 ONLY here, with no
        // `trustInfo`/`requireAdministrator` block: `cargo test` must keep
        // running unelevated. `cargo:rustc-link-arg-tests` (rather than the
        // unqualified `cargo:rustc-link-arg`) confines this to `[[test]]`
        // targets so the `[[bin]] voidframe` executable's own manifest above
        // is untouched.
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        let test_manifest_path = std::path::Path::new(&out_dir).join("test.manifest");
        std::fs::write(
            &test_manifest_path,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>"#,
        )
        .expect("failed to write test.manifest");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}",
            test_manifest_path.display()
        );
    }
    #[cfg(not(windows))]
    tauri_build::build();
}
