# AveYo `benchmark.cfg` v2

Source: https://github.com/AveYo/Gaming/tree/main/CS2
Files: `benchmark.cfg`, `benchmark2.cfg`
Version: v2026.05.19 (per the files' own header comment)
License: MIT (see `LICENSE-AveYo.txt` in this directory)

Bundled byte-for-byte, unmodified. VOIDFRAME writes both files verbatim into CS2's own `cfg\`
directory for `BenchmarkKind::AveYoCfgV2` projects
(`crates/voidframe-engine/src/cs2/aveyo_cfg.rs`) and triggers the single-run (`BB`) variant via
`alias set v2;sv_cheats 1;exec_async benchmark`. See
`docs/superpowers/specs/2026-09-09-aveyo-benchmark-design.md` for the full design.
