//! `localconfig.vdf` `LaunchOptions` read/write. VDF is a brace-nested
//! key-value format (Valve's own, not JSON) — walk it by brace depth to
//! find the `apps` -> `<appid>` block, rather than a regex over the whole
//! file (a naive regex risks matching a `LaunchOptions` line belonging to a
//! *different* app's block).
//!
//! **Caller's responsibility, not this module's:** `write` must only be
//! called while Steam has no live process — confirmed empirically
//! (`spike/findings.md` §6a) that editing while Steam runs is silently
//! ignored (cached in memory) even though the write itself succeeds and
//! survives on disk.

use crate::error::{Error, Result};
use crate::model::module::Hive;
use crate::system::windows::registry;
use crate::system::{AppManifest, RegKey, RegValue};
use std::path::PathBuf;

/// Advances past a quoted string starting at `text[open_quote]` (which must
/// be `"`), honoring backslash-escaped characters inside it (so an escaped
/// `\"` doesn't terminate the string early). Returns the index just past
/// the closing (unescaped) `"`.
fn skip_quoted(text: &str, open_quote: usize, end: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[open_quote], b'"');
    let mut j = open_quote + 1;
    while j < end {
        match bytes[j] {
            b'\\' => j += 2, // skip the escaped character too (write_launch_options only ever emits \\ and \" — both single-byte escapes)
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    Err(Error::msg("unterminated quoted string in VDF text".into()))
}

/// Given the position of an opening `{` (within `[..end)`), finds its
/// matching `}` by tracking brace depth from that point, skipping over
/// quoted-string contents so a stray `{`/`}` inside a value is never
/// mistaken for structure.
fn find_matching_brace(text: &str, open_brace: usize, end: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[open_brace], b'{');
    let mut i = open_brace;
    let mut depth = 0i32;
    while i < end {
        match bytes[i] {
            b'"' => i = skip_quoted(text, i, end)?,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    Err(Error::msg(
        "unterminated block in VDF text (no matching '}')".into(),
    ))
}

/// Scans `[start, end)` for a quoted key matching `key` (case-insensitively)
/// that is a *direct child* of this range (not nested inside some other
/// block within the range — such blocks are skipped over whole, by jumping
/// straight to their matching `}`, without descending into them) and is
/// itself followed by `{` (i.e. it opens a nested block, not a leaf value).
/// Returns the byte range of that block's *contents*: just inside its
/// opening `{` up to (but not including) its matching `}`.
fn find_direct_block(text: &str, start: usize, end: usize, key: &str) -> Result<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'"' => {
                let tok_start = i + 1;
                let after_tok = skip_quoted(text, i, end)?;
                let tok = &text[tok_start..after_tok - 1];

                let mut k = after_tok;
                while k < end && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }

                if k < end && bytes[k] == b'{' {
                    // `tok` names a block. Either it's the one we want, or
                    // we skip past it whole (without descending) and keep
                    // looking at this same depth.
                    let close = find_matching_brace(text, k, end)?;
                    if tok.eq_ignore_ascii_case(key) {
                        return Ok((k + 1, close));
                    }
                    i = close + 1;
                    continue;
                }

                // Not followed by `{` — this token is either a leaf key or
                // a leaf value. Either way it's not a block; move past just
                // this one token and keep scanning.
                i = after_tok;
            }
            _ => i += 1,
        }
    }
    Err(Error::msg(format!(
        "VDF block \"{key}\" not found in the searched range"
    )))
}

/// Walks `text` by brace depth, looking for a quoted key matching (case-
/// insensitively) each element of `path` in turn, nested in order. Returns
/// the byte range `[start, end)` of that final block's *contents* (the
/// span between its own opening and matching closing brace), so the caller
/// can both read from it and splice a replacement into it.
pub(crate) fn find_block_span(text: &str, path: &[&str]) -> Result<(usize, usize)> {
    let mut start = 0usize;
    let mut end = text.len();
    for key in path {
        let (s, e) = find_direct_block(text, start, end, key).map_err(|_| {
            Error::msg(format!(
                "VDF path element \"{key}\" not found (full path: {path:?})"
            ))
        })?;
        start = s;
        end = e;
    }
    Ok((start, end))
}

/// Reverses the escaping `write_launch_options` applies (`\\` -> `\`,
/// `\"` -> `"`): a backslash always introduces a literal copy of the next
/// character.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Reads the value of a quoted `"KeyName"    "value"` line that is a
/// *direct* child of the block spanning `[start, end)` (not nested deeper —
/// `LaunchOptions` is a leaf value, never itself a block).
pub(crate) fn find_leaf_value(
    text: &str,
    start: usize,
    end: usize,
    key: &str,
) -> Option<(usize, usize, String)> {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'"' => {
                let tok_start = i + 1;
                let after_tok = skip_quoted(text, i, end).ok()?;
                let tok = &text[tok_start..after_tok - 1];

                let mut k = after_tok;
                while k < end && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }

                if k < end && bytes[k] == b'{' {
                    // A nested block, not a leaf — skip it whole without
                    // descending, same-depth scan continues after it.
                    let close = find_matching_brace(text, k, end).ok()?;
                    i = close + 1;
                    continue;
                }

                if tok.eq_ignore_ascii_case(key) && k < end && bytes[k] == b'"' {
                    let val_start = k + 1;
                    let val_after = skip_quoted(text, k, end).ok()?;
                    let val_end = val_after - 1;
                    let raw = &text[val_start..val_end];
                    return Some((val_start, val_end, unescape(raw)));
                }

                i = after_tok;
            }
            _ => i += 1,
        }
    }
    None
}

pub fn read_launch_options(vdf_text: &str, app_id: u32) -> Result<String> {
    let app_id_str = app_id.to_string();
    let (start, end) = find_block_span(
        vdf_text,
        &[
            "UserLocalConfigStore",
            "Software",
            "Valve",
            "Steam",
            "apps",
            &app_id_str,
        ],
    )?;
    match find_leaf_value(vdf_text, start, end, "LaunchOptions") {
        Some((_, _, value)) => Ok(value),
        None => Ok(String::new()), // no LaunchOptions set yet for this app — empty, not an error
    }
}

pub fn write_launch_options(vdf_text: &str, app_id: u32, new_value: &str) -> Result<String> {
    let app_id_str = app_id.to_string();
    let (start, end) = find_block_span(
        vdf_text,
        &[
            "UserLocalConfigStore",
            "Software",
            "Valve",
            "Steam",
            "apps",
            &app_id_str,
        ],
    )?;
    let escaped = new_value.replace('\\', "\\\\").replace('"', "\\\"");
    match find_leaf_value(vdf_text, start, end, "LaunchOptions") {
        Some((val_start, val_end, _)) => {
            let mut out = String::with_capacity(vdf_text.len());
            out.push_str(&vdf_text[..val_start]);
            out.push_str(&escaped);
            out.push_str(&vdf_text[val_end..]);
            Ok(out)
        }
        None => {
            // Insert a new leaf line right after the block's opening brace,
            // matching Steam's own indentation style loosely (exact
            // whitespace doesn't matter to Steam's own VDF parser).
            let insertion_point = start; // just inside the opening brace
            let mut out = String::with_capacity(vdf_text.len() + 64);
            out.push_str(&vdf_text[..insertion_point]);
            out.push_str(&format!("\n\t\t\t\t\t\"LaunchOptions\"\t\t\"{escaped}\"\n"));
            out.push_str(&vdf_text[insertion_point..]);
            Ok(out)
        }
    }
}

/// Locates the local Steam installation via
/// `HKCU\Software\Valve\Steam\SteamPath` (a `REG_SZ`, forward-slash-
/// separated per Steam's own convention — normalized to backslashes here),
/// falling back to the common install-path guesses if that registry value
/// is absent.
pub(super) async fn find_steam_path() -> Result<PathBuf> {
    let key = RegKey {
        hive: Hive::Hkcu,
        subkey: "Software\\Valve\\Steam".into(),
        value_name: "SteamPath".into(),
    };
    if let RegValue::Sz(s) = registry::read(&key).await?
        && !s.is_empty()
    {
        return Ok(PathBuf::from(s.replace('/', "\\")));
    }

    for guess in [
        std::env::var("ProgramFiles(x86)")
            .ok()
            .map(|p| PathBuf::from(p).join("Steam")),
        std::env::var("ProgramFiles")
            .ok()
            .map(|p| PathBuf::from(p).join("Steam")),
    ]
    .into_iter()
    .flatten()
    {
        if guess.exists() {
            return Ok(guess);
        }
    }

    Err(Error::msg(
        "could not locate Steam installation via registry (HKCU\\Software\\Valve\\Steam\\SteamPath) or common install paths".into(),
    ))
}

/// Every direct child block of `[start, end)`, as `(key, block_start,
/// block_end)` triples in document order. Unlike `find_direct_block`
/// (which looks up one named key), this doesn't know the child keys ahead
/// of time — `libraryfolders.vdf`'s per-library blocks are keyed by an
/// arbitrary numeric index (`"0"`, `"1"`, `"2"`, ...), not a fixed name.
fn find_all_direct_blocks(
    text: &str,
    start: usize,
    end: usize,
) -> Result<Vec<(String, usize, usize)>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'"' => {
                let tok_start = i + 1;
                let after_tok = skip_quoted(text, i, end)?;
                let tok = text[tok_start..after_tok - 1].to_string();

                let mut k = after_tok;
                while k < end && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }

                if k < end && bytes[k] == b'{' {
                    let close = find_matching_brace(text, k, end)?;
                    out.push((tok, k + 1, close));
                    i = close + 1;
                    continue;
                }

                i = after_tok;
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

/// Which registered Steam Library Folder (per `libraryfolders.vdf`'s own
/// `"apps"` block for each library) actually has `app_id` in it, or
/// `None` if the file's missing, unparseable, or genuinely doesn't list
/// `app_id` anywhere. Best-effort by design: any failure here reads the
/// same as "didn't find it" to the caller, which falls back to the
/// pre-multi-library assumption — this is a refinement over that
/// fallback, not a hard requirement.
fn find_app_library_sync(text: &str, app_id_str: &str) -> Option<PathBuf> {
    let (root_start, root_end) = find_block_span(text, &["libraryfolders"]).ok()?;
    for (_, lib_start, lib_end) in find_all_direct_blocks(text, root_start, root_end).ok()? {
        let Some((_, _, path_str)) = find_leaf_value(text, lib_start, lib_end, "path") else {
            continue;
        };
        // The "apps" sub-block lists every appid installed in THIS
        // library, keyed by appid with its download size as the (unused
        // here) value — presence of the key is all that matters.
        if let Ok((apps_start, apps_end)) = find_direct_block(text, lib_start, lib_end, "apps")
            && find_leaf_value(text, apps_start, apps_end, app_id_str).is_some()
        {
            return Some(PathBuf::from(path_str.replace('/', "\\")));
        }
    }
    None
}

/// The Steam Library Folder root that actually has `app_id` installed —
/// **not** necessarily the Steam client's own install directory
/// (`find_steam_path`). Steam's multi-library-folder feature (Storage
/// Manager) means a game is very often installed on a different drive
/// than the Steam client itself (a common real-world setup: Steam on the
/// OS drive, games on a separate SSD/HDD) — code that assumes a game
/// lives under `find_steam_path()`'s own `steamapps\common\...` silently
/// breaks for any such user. Confirmed as a real gap live, not
/// theoretical: this project's own `run::execute` and
/// `WindowsController::app_manifest` (see `vdf::parse_app_manifest`) both
/// made exactly that assumption before this function existed.
///
/// Resolves via `<steam_path>\steamapps\libraryfolders.vdf` (the Steam
/// client's own registry of every library, always present there
/// regardless of where any individual game lives) — falls back to
/// `find_steam_path()`'s own result if that file is missing, unparseable,
/// or doesn't list `app_id` in any library (an unusually old Steam
/// install, or a genuine "not installed anywhere" case a caller's own
/// file-existence check will catch more specifically afterward).
pub(super) async fn find_app_library_path(app_id: u32) -> Result<PathBuf> {
    let steam_path = find_steam_path().await?;
    let vdf_path = steam_path.join("steamapps").join("libraryfolders.vdf");
    let app_id_str = app_id.to_string();

    let Ok(text) = tokio::fs::read_to_string(&vdf_path).await else {
        return Ok(steam_path);
    };

    let found = tokio::task::spawn_blocking(move || find_app_library_sync(&text, &app_id_str))
        .await
        .map_err(|e| Error::msg(format!("libraryfolders.vdf parse task panicked: {e}")))?;

    Ok(found.unwrap_or(steam_path))
}

/// Parses `appmanifest_<appid>.acf` (one `AppState` block) into the two
/// fields VOIDFRAME reads. Pure; the file read lives in
/// `WindowsController::app_manifest`.
pub(crate) fn parse_app_manifest(text: &str) -> Result<AppManifest> {
    let (start, end) = find_block_span(text, &["AppState"])?;
    let leaf = |field: &str| find_leaf_value(text, start, end, field).map(|(_, _, v)| v);
    Ok(AppManifest {
        build_id: leaf("buildid"),
        state_flags: leaf("StateFlags").and_then(|s| s.parse().ok()),
    })
}

/// Locates the currently-used account's `localconfig.vdf` under
/// `<steam_path>\userdata\<accountID>\config\localconfig.vdf`. Enumerates
/// every numeric `userdata\*` subdirectory that has a `config\localconfig.vdf`.
///
/// If exactly one such account exists, uses it. If more than one does (a
/// shared machine with multiple cached Steam accounts), uses the most-
/// recently-modified `localconfig.vdf` — the actively-used account's file
/// is the one Steam itself last wrote — and logs a warning. This is a
/// heuristic, not a true "which account is logged in right now" check;
/// M1 accepts the limitation since pre-flight already requires Steam
/// running and logged in for any of this to matter, and a proper answer
/// would mean parsing the sibling `config.vdf` for the active-account
/// marker, which is more machinery than a hobby tool's M1 needs.
pub async fn find_localconfig_path() -> Result<PathBuf> {
    let steam_path = find_steam_path().await?;
    let userdata_root = steam_path.join("userdata");

    let mut entries = tokio::fs::read_dir(&userdata_root)
        .await
        .map_err(|e| Error::msg(format!("reading {}: {e}", userdata_root.display())))?;

    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| Error::msg(format!("reading {}: {e}", userdata_root.display())))?
    {
        let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let vdf_path = entry.path().join("config").join("localconfig.vdf");
        if let Ok(meta) = tokio::fs::metadata(&vdf_path).await {
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            candidates.push((vdf_path, modified));
        }
    }

    match candidates.len() {
        0 => Err(Error::msg(format!(
            "no localconfig.vdf found under any account in {}",
            userdata_root.display()
        ))),
        1 => Ok(candidates.into_iter().next().unwrap().0),
        _ => {
            tracing::warn!(
                "multiple Steam accounts found under {} — using the most-recently-modified localconfig.vdf",
                userdata_root.display()
            );
            candidates.sort_by_key(|(_, modified)| *modified);
            Ok(candidates.pop().unwrap().0)
        }
    }
}

pub async fn read(app_id: u32) -> Result<String> {
    let path = find_localconfig_path().await?;
    tokio::task::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
        read_launch_options(&text, app_id)
    })
    .await
    .map_err(|e| Error::msg(format!("vdf read task panicked: {e}")))?
}

pub async fn write(app_id: u32, new_value: &str) -> Result<()> {
    let path = find_localconfig_path().await?;
    let new_value = new_value.to_string();
    tokio::task::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::msg(format!("reading {}: {e}", path.display())))?;
        let updated = write_launch_options(&text, app_id, &new_value)?;
        // Atomic (temp file + rename), not a direct in-place write: an
        // interruption mid-write over the original path would otherwise
        // leave the user's entire per-user `localconfig.vdf` corrupted,
        // not just the `LaunchOptions` section. Reuses
        // `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`'s
        // own helper rather than hand-rolling a second temp-file+rename
        // implementation.
        crate::paths::atomic_write(&path, updated.as_bytes())
    })
    .await
    .map_err(|e| Error::msg(format!("vdf write task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_vdf() -> &'static str {
        r#"
"UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"570"
					{
						"LaunchOptions"		"-console"
					}
					"730"
					{
						"LaunchOptions"		"-netconport 2121 -condebug"
						"AutoUpdateBehavior"		"0"
					}
				}
			}
		}
	}
}
"#
    }

    #[test]
    fn reads_the_right_apps_launch_options_not_the_first_one_in_the_file() {
        assert_eq!(
            read_launch_options(sample_vdf(), 730).unwrap(),
            "-netconport 2121 -condebug"
        );
        assert_eq!(read_launch_options(sample_vdf(), 570).unwrap(), "-console");
    }

    #[test]
    fn missing_app_block_is_an_error() {
        assert!(read_launch_options(sample_vdf(), 999999).is_err());
    }

    #[test]
    fn reading_an_app_block_with_no_launch_options_line_returns_empty() {
        let vdf = sample_vdf().replace(r#""LaunchOptions"		"-netconport 2121 -condebug""#, "");
        assert_eq!(read_launch_options(&vdf, 730).unwrap(), "");
    }

    #[test]
    fn writes_replace_the_right_apps_value_and_leave_the_other_app_untouched() {
        let updated = write_launch_options(sample_vdf(), 730, "-novid").unwrap();
        assert_eq!(read_launch_options(&updated, 730).unwrap(), "-novid");
        assert_eq!(read_launch_options(&updated, 570).unwrap(), "-console"); // untouched
    }

    #[test]
    fn write_inserts_a_new_launchoptions_line_when_absent() {
        let vdf = sample_vdf().replace(r#""LaunchOptions"		"-netconport 2121 -condebug""#, "");
        assert_eq!(read_launch_options(&vdf, 730).unwrap(), "");
        let updated = write_launch_options(&vdf, 730, "-novid").unwrap();
        assert_eq!(read_launch_options(&updated, 730).unwrap(), "-novid");
    }

    #[test]
    fn write_escapes_embedded_quotes_and_backslashes() {
        let updated = write_launch_options(sample_vdf(), 730, r#"+exec "my cfg.cfg""#).unwrap();
        // The read-back must round-trip to the ORIGINAL unescaped value —
        // read_launch_options un-escapes on the way out, matching how a
        // real VDF value containing a literal quote is stored escaped on
        // disk but used unescaped by the game.
        assert_eq!(
            read_launch_options(&updated, 730).unwrap(),
            r#"+exec "my cfg.cfg""#
        );
    }

    // ---- find_app_library_sync (libraryfolders.vdf) -----------------------

    fn sample_libraryfolders_vdf() -> &'static str {
        // Shaped like a real, modern Steam `libraryfolders.vdf`: one
        // library per Steam Library Folder, each with its own "apps"
        // block keyed by appid (value is the app's download size — unused
        // here, presence of the key is all that matters). CS2 (730) lives
        // in library "1", a different drive from the primary library "0" —
        // the exact real-world shape this function exists to handle.
        r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"570"		"30037150356"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
			"730"		"85000000000"
			"440"		"15000000000"
		}
	}
}
"#
    }

    #[test]
    fn finds_the_library_that_actually_has_the_app() {
        assert_eq!(
            find_app_library_sync(sample_libraryfolders_vdf(), "730"),
            Some(PathBuf::from(r"D:\SteamLibrary"))
        );
        assert_eq!(
            find_app_library_sync(sample_libraryfolders_vdf(), "570"),
            Some(PathBuf::from(r"C:\Program Files (x86)\Steam"))
        );
    }

    #[test]
    fn returns_none_when_no_library_has_the_app() {
        assert_eq!(
            find_app_library_sync(sample_libraryfolders_vdf(), "999999"),
            None
        );
    }

    #[test]
    fn returns_none_for_malformed_libraryfolders_text_rather_than_panicking() {
        assert_eq!(
            find_app_library_sync("not valid vdf at all {{{", "730"),
            None
        );
        assert_eq!(find_app_library_sync("", "730"), None);
    }

    #[test]
    fn parse_app_manifest_reads_buildid_and_state_flags() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"buildid\"\t\t\"25000182\"\n}\n";
        let m = parse_app_manifest(acf).unwrap();
        assert_eq!(m.build_id.as_deref(), Some("25000182"));
        assert_eq!(m.state_flags, Some(4));
    }

    #[test]
    fn parse_app_manifest_tolerates_missing_fields() {
        let m = parse_app_manifest("\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n}\n").unwrap();
        assert_eq!(
            m,
            AppManifest {
                build_id: None,
                state_flags: None
            }
        );
    }

    #[test]
    fn a_single_library_setup_still_resolves_correctly() {
        // The common case: everything (Steam client + every game) on one
        // drive, one library folder — must still work, not just the
        // multi-library case this function exists for.
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"730"		"85000000000"
		}
	}
}
"#;
        assert_eq!(
            find_app_library_sync(vdf, "730"),
            Some(PathBuf::from(r"C:\Program Files (x86)\Steam"))
        );
    }
}
