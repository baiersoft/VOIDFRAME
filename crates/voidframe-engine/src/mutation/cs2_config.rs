//! `cs2_config` module apply / revert -- whole-file snapshot/restore of
//! CS2's video.txt (D7: simpler and safer than per-key inverses).

use crate::error::{Error, Result};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::module::Cs2ConfigPayload;
use crate::system::{MutationCtx, SystemController};
use serde_json::json;

/// Splits `text` into `(content, terminator)` pairs, where `terminator` is
/// `"\r\n"`, `"\n"`, or `""` (only possible for the final line, when the
/// text doesn't end in a newline at all) -- unlike `str::lines()`, which
/// discards this information entirely, so a caller reconstructing the text
/// line-by-line can't otherwise tell a CRLF-terminated line from an
/// LF-terminated one, or a trailing newline from its absence.
fn split_lines_keep_terminators(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(idx) => {
                let has_cr = idx > 0 && rest.as_bytes()[idx - 1] == b'\r';
                let content_end = if has_cr { idx - 1 } else { idx };
                out.push((&rest[..content_end], &rest[content_end..idx + 1]));
                rest = &rest[idx + 1..];
            }
            None => {
                out.push((rest, ""));
                rest = "";
            }
        }
    }
    out
}

/// Parses a single line against the same quoted-KeyValues shape the spike
/// script's own confirmed regex expects (`spike/04-dump-cs2-video.ps1` line
/// 67: `^\s*"([^"]+)"\s*"([^"]*)"\s*$`) -- optional leading whitespace, a
/// quoted non-empty key, whitespace, a quoted (possibly empty) value, then
/// only whitespace to the end of the line's content. Returns the key and the
/// byte range of the *value's contents* (between its quotes, exclusive) so a
/// caller can splice in a replacement value while leaving every other byte
/// of the line -- including the key's own quotes and the separator
/// whitespace between key and value -- untouched.
fn parse_quoted_kv_line(line: &str) -> Option<(&str, usize, usize)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    let key_start = i + 1;
    let key_end = key_start + line[key_start..].find('"')?;
    if key_end == key_start {
        return None; // the spike regex's key group is `[^"]+`, non-empty
    }
    let key = &line[key_start..key_end];

    let mut j = key_end + 1;
    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    if bytes.get(j) != Some(&b'"') {
        return None;
    }
    let value_start = j + 1;
    let value_end = value_start + line[value_start..].find('"')?;

    // Everything after the value's closing quote must be whitespace only,
    // matching the regex's trailing `\s*$`.
    if !line[value_end + 1..].chars().all(char::is_whitespace) {
        return None;
    }
    Some((key, value_start, value_end))
}

/// Rewrites only the given keys in a CS2 `cs2_video.txt`, preserving every
/// other line verbatim (including its exact original formatting -- line
/// ending and all) and every matched line's own quote/whitespace structure
/// (only the value's contents change). A key that wasn't already present is
/// inserted as a new `"key"\t\t"value"` line immediately after the last line
/// that matched the quoted key/value shape -- not at the absolute end of the
/// file -- so the insertion stays inside whatever KeyValues block structure
/// surrounds the existing lines (real `cs2_video.txt` samples are not
/// available in this repo to confirm that structure exactly; see
/// `docs/superpowers/specs/2026-09-08-custom-script-cs2-config-design.md`
/// §4.3). If the file has no such line at all, there is no known-safe
/// insertion point, so the new lines are appended at the end as a last
/// resort.
fn rewrite_video_settings(
    text: &str,
    settings: &std::collections::BTreeMap<String, String>,
) -> String {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut pieces: Vec<String> = Vec::new();
    let mut last_kv_piece_idx: Option<usize> = None;

    for (line, terminator) in split_lines_keep_terminators(text) {
        let mut rendered = String::new();
        if let Some((key, value_start, value_end)) = parse_quoted_kv_line(line) {
            last_kv_piece_idx = Some(pieces.len());
            if let Some(new_value) = settings.get(key) {
                rendered.push_str(&line[..value_start]);
                rendered.push_str(new_value);
                rendered.push_str(&line[value_end..]);
                seen.insert(key);
            } else {
                rendered.push_str(line);
            }
        } else {
            rendered.push_str(line);
        }
        rendered.push_str(terminator);
        pieces.push(rendered);
    }

    let mut new_lines = String::new();
    for (key, value) in settings {
        if !seen.contains(key.as_str()) {
            new_lines.push_str(&format!("\t\"{key}\"\t\t\"{value}\"\n"));
        }
    }

    if !new_lines.is_empty() {
        match last_kv_piece_idx {
            Some(idx) => pieces.insert(idx + 1, new_lines),
            None => pieces.push(new_lines),
        }
    }

    pieces.concat()
}

/// Reads every quoted key/value line in `text` into a map -- the read-side
/// counterpart to [`rewrite_video_settings`]'s write-side line parsing, used
/// to pre-fill the `cs2_config` builder form with the user's real, current
/// settings (`commands::cs2_config::read_cs2_video_config`) rather than
/// starting empty. Reuses [`parse_quoted_kv_line`] so a line this crate
/// considers writable is exactly the same set of lines it considers
/// readable -- no second, drifting definition of "what counts as a setting
/// line".
pub fn parse_video_settings(text: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value_start, value_end)) = parse_quoted_kv_line(line) {
            map.insert(key.to_string(), line[value_start..value_end].to_string());
        }
    }
    map
}

pub async fn apply(
    p: &Cs2ConfigPayload,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
) -> Result<()> {
    let original = sys.read_cs2_video_config().await?;
    let target = json!({ "settings": p.settings });
    let inverse = json!({ "original_text": original });
    let new_text = rewrite_video_settings(&original, &p.settings);
    let seq = journal.record(
        Op::Cs2ConfigApply,
        ctx,
        target,
        json!({ "new_text": new_text }),
        inverse,
    )?;
    sys.write_cs2_video_config(&new_text).await?;
    journal.mark_applied(seq)?;
    Ok(())
}

pub async fn revert(
    rec: &JournalRecord,
    sys: &dyn SystemController,
    _ctx: &MutationCtx,
) -> Result<()> {
    let original = rec.inverse["original_text"].as_str().ok_or_else(|| {
        Error::msg(format!(
            "seq {}: cs2_config revert inverse is missing a string original_text -- refusing to write, would truncate video.txt",
            rec.seq
        ))
    })?;
    sys.write_cs2_video_config(original).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::MockController;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    /// Modeled after the same nested-block KeyValues convention this crate
    /// already parses for `localconfig.vdf` (`system/windows/vdf.rs`) --
    /// there is no real `cs2_video.txt` sample committed to this repo
    /// (privacy-redacted, per `spike/04-dump-cs2-video.ps1`'s own comment),
    /// so this fixture is a plausible approximation, not a confirmed one.
    fn sample_video_config() -> &'static str {
        "\"VideoConfig\"\n{\n\t\"setting.defaultres\"\t\t\"1920\"\n\t\"setting.fullscreen\"\t\t\"1\"\n}\n"
    }

    #[test]
    fn parse_video_settings_reads_every_quoted_kv_line_into_a_map() {
        let map = parse_video_settings(sample_video_config());
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("setting.defaultres").map(String::as_str),
            Some("1920")
        );
        assert_eq!(map.get("setting.fullscreen").map(String::as_str), Some("1"));
    }

    #[test]
    fn parse_video_settings_ignores_non_kv_lines_like_the_block_header_and_braces() {
        let map = parse_video_settings(sample_video_config());
        assert!(!map.contains_key("VideoConfig"));
    }

    #[test]
    fn parse_video_settings_of_empty_text_is_an_empty_map() {
        assert!(parse_video_settings("").is_empty());
    }

    #[test]
    fn rewrite_video_settings_changes_only_the_given_keys() {
        let original = sample_video_config();
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.fullscreen".to_string(), "0".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        assert!(rewritten.contains("\"setting.defaultres\"\t\t\"1920\""));
        assert!(rewritten.contains("\"setting.fullscreen\"\t\t\"0\""));
        assert!(!rewritten.contains("\"setting.fullscreen\"\t\t\"1\""));
    }

    /// (a) Rewriting an existing quoted key preserves the surrounding block
    /// structure and the line's own separator style byte-for-byte -- only
    /// the value's own quoted contents change.
    #[test]
    fn rewrite_video_settings_preserves_block_structure_when_changing_an_existing_key() {
        let original = sample_video_config();
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.fullscreen".to_string(), "0".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        assert_eq!(
            rewritten,
            "\"VideoConfig\"\n{\n\t\"setting.defaultres\"\t\t\"1920\"\n\t\"setting.fullscreen\"\t\t\"0\"\n}\n"
        );
    }

    /// (b) A brand-new key must land *inside* the block (right after the
    /// last matched key/value line), never after the closing `}` -- that is
    /// exactly the corruption Finding 2 is about.
    #[test]
    fn rewrite_video_settings_inserts_a_new_key_inside_the_block_not_after_the_closing_brace() {
        let original = sample_video_config();
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.new_key".to_string(), "42".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        let new_key_pos = rewritten.find("\"setting.new_key\"").expect(&rewritten);
        let last_existing_pos = rewritten.find("\"setting.fullscreen\"").unwrap();
        let close_brace_pos = rewritten.rfind('}').unwrap();
        assert!(
            last_existing_pos < new_key_pos && new_key_pos < close_brace_pos,
            "{rewritten}"
        );
    }

    #[test]
    fn rewrite_video_settings_preserves_line_order_for_untouched_lines() {
        let original = "a 1\nb 2\nc 3\n";
        let changes = std::collections::BTreeMap::new();
        assert_eq!(rewrite_video_settings(original, &changes), original);
    }

    #[test]
    fn rewrite_video_settings_preserves_crlf_line_endings_on_untouched_lines() {
        // Steam actually writes CRLF-terminated cs2_video.txt on Windows --
        // an untouched line's own "\r\n" must survive byte-for-byte, not get
        // silently normalized to a bare "\n".
        let original = "\"setting.defaultres\"\t\t\"1920\"\r\n\"setting.fullscreen\"\t\t\"1\"\r\n";
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.fullscreen".to_string(), "0".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        assert_eq!(
            rewritten,
            "\"setting.defaultres\"\t\t\"1920\"\r\n\"setting.fullscreen\"\t\t\"0\"\r\n"
        );
    }

    #[test]
    fn rewrite_video_settings_does_not_add_a_trailing_newline_that_was_not_there() {
        // The last line has no trailing newline at all and isn't among the
        // changed keys -- rewriting must not gain one it didn't have.
        let original = "\"setting.defaultres\"\t\t\"1920\"\n\"setting.fullscreen\"\t\t\"1\"";
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.defaultres".to_string(), "1280".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        assert_eq!(
            rewritten,
            "\"setting.defaultres\"\t\t\"1280\"\n\"setting.fullscreen\"\t\t\"1\""
        );
    }

    #[test]
    fn rewrite_video_settings_rewrites_every_occurrence_of_a_duplicate_key() {
        let original = "\"setting.fullscreen\"\t\t\"1\"\n\"setting.fullscreen\"\t\t\"1\"\n";
        let mut changes = std::collections::BTreeMap::new();
        changes.insert("setting.fullscreen".to_string(), "0".to_string());
        let rewritten = rewrite_video_settings(original, &changes);
        assert_eq!(
            rewritten,
            "\"setting.fullscreen\"\t\t\"0\"\n\"setting.fullscreen\"\t\t\"0\"\n"
        );
    }

    #[tokio::test]
    async fn apply_snapshots_the_original_and_writes_only_the_given_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new().with_cs2_video_config(sample_video_config());

        let mut settings = std::collections::BTreeMap::new();
        settings.insert("setting.fullscreen".to_string(), "0".to_string());
        let p = Cs2ConfigPayload { settings };

        apply(&p, &mock, &mut j, &ctx()).await.unwrap();
        let current = mock.read_cs2_video_config().await.unwrap();
        assert!(current.contains("\"setting.defaultres\"\t\t\"1920\""));
        assert!(current.contains("\"setting.fullscreen\"\t\t\"0\""));

        let recs = Journal::load_pending(j.path()).unwrap();
        assert!(recs[0].applied);
        assert_eq!(recs[0].inverse["original_text"], sample_video_config());
    }

    #[tokio::test]
    async fn revert_restores_the_exact_original_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let original = "setting.defaultres 1920\nsetting.fullscreen 1\n";
        let mock = MockController::new().with_cs2_video_config(original);

        let mut settings = std::collections::BTreeMap::new();
        settings.insert("setting.fullscreen".to_string(), "0".to_string());
        let p = Cs2ConfigPayload { settings };
        apply(&p, &mock, &mut j, &ctx()).await.unwrap();

        let recs = Journal::load_pending(j.path()).unwrap();
        revert(&recs[0], &mock, &ctx()).await.unwrap();
        assert_eq!(mock.read_cs2_video_config().await.unwrap(), original);
    }

    /// A corrupted or partially-written journal line whose `inverse` has no
    /// `original_text` field must not silently resolve to an empty string
    /// and truncate the user's real `video.txt` -- `revert` must refuse and
    /// the mock must never observe a write at all.
    #[tokio::test]
    async fn revert_refuses_when_original_text_is_missing_from_the_inverse() {
        let mock = MockController::new().with_cs2_video_config("setting.defaultres 1920\n");
        let rec = JournalRecord {
            seq: 1,
            ts_unix_ms: 0,
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
            op: Op::Cs2ConfigApply,
            target: json!({}),
            new: json!({}),
            inverse: json!({}),
            applied: true,
        };

        let err = revert(&rec, &mock, &ctx()).await.unwrap_err();
        assert!(err.to_string().contains("original_text"), "{err}");
        assert_eq!(
            mock.read_cs2_video_config().await.unwrap(),
            "setting.defaultres 1920\n",
            "revert must never have written when the inverse was malformed"
        );
    }

    /// Same guard, but for a non-string `original_text` (e.g. a number or
    /// null slipped in by a corrupted journal write) -- `.as_str()` returns
    /// `None` for these too, so the same refusal path must trigger.
    #[tokio::test]
    async fn revert_refuses_when_original_text_is_not_a_string() {
        let mock = MockController::new().with_cs2_video_config("setting.defaultres 1920\n");
        let rec = JournalRecord {
            seq: 1,
            ts_unix_ms: 0,
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
            op: Op::Cs2ConfigApply,
            target: json!({}),
            new: json!({}),
            inverse: json!({ "original_text": 42 }),
            applied: true,
        };

        let err = revert(&rec, &mock, &ctx()).await.unwrap_err();
        assert!(err.to_string().contains("original_text"), "{err}");
        assert_eq!(
            mock.read_cs2_video_config().await.unwrap(),
            "setting.defaultres 1920\n",
            "revert must never have written when the inverse was malformed"
        );
    }
}
