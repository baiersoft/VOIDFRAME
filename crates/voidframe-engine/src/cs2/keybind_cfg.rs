//! VOIDFRAME's own console keybind — a small `.cfg` file the engine writes
//! to CS2's `cfg\` directory and always `+exec`s, so `reissue_map`
//! (`system/windows/input.rs`) can rely on a known-good F9 binding
//! regardless of the user's own keybind customization. Non-destructive:
//! Source engine multi-binds (a new bind on an already-bound key doesn't
//! remove the existing one) — confirmed on the dev rig, `spike/findings.md`
//! §6b.

use crate::error::{Error, Result};
use std::path::Path;

pub const CFG_FILENAME: &str = "voidframe_keybind.cfg";

// `toggleconsole`, confirmed live to actually work. An earlier version of
// this cfg bound `showconsole` instead, on the theory that an idempotent
// open (never closes an already-open console) would remove the desync
// risk below on its own — confirmed live NOT to work at all in CS2: bound
// correctly (visible in the in-game keybind list exactly as written here),
// but pressing it does nothing. Reverted to the one bind actually proven
// to open the console.
//
// The desync this cfg used to cause: `reissue_map`'s own closing step used
// to be a second F12 press against this same toggleconsole bind, sent
// after a fixed delay — but a real cold map load can outlast that delay,
// leaving the console open. The *next* reissue_map's opening F12 press
// then toggled that already-open console CLOSED instead of open, and
// every input after that (the typed map command, Enter) landed in the
// live game instead of the console — confirmed live as the cause of a
// benchmark run where the console started re-opening mid-pass. Fixed not
// by finding a better open bind, but by moving closing off this bind
// entirely: `input::hide_console` sends `Escape` instead (Source engine's
// built-in dismiss key, no custom bind needed, confirmed live to work).
// Open and close now live on two unrelated keys, so `toggleconsole`'s own
// state never has anything left to desync against.
//
// F9, not F12: F12 is Steam's own DEFAULT global screenshot hotkey (a
// user-level, non-elevated hotkey — nothing to do with the anti-cheat
// theory floated earlier for the close-side bug above, a separate,
// simpler problem). A dev rig with that rebound away from F12 never hits
// it, but a normal user with Steam's defaults would trigger a screenshot
// on every console-open. F12 itself was still confirmed live to actually
// open CS2's console (this bug is about the SCREENSHOT side effect, not
// console-open failing) — F9 has no known Steam or CS2 default binding,
// but that claim is NOT independently live-verified the way F12/Escape
// both were; confirm it live before trusting it the same way.
const CFG_CONTENTS: &str = "con_enable \"true\"\nbind \"F9\" \"toggleconsole\"\n";

/// Writes (or overwrites — idempotent) the keybind file into CS2's own
/// `cfg\` directory. `cs2_cfg_dir` is `<cs2_library>\steamapps\common\
/// Counter-Strike Global Offensive\game\csgo\cfg\`, where `<cs2_library>`
/// is whichever Steam Library Folder actually has CS2 installed — not
/// necessarily the Steam client's own install path (see
/// `SystemController::app_library_path`'s doc comment) — the caller
/// resolves this path.
pub fn ensure_written(cs2_cfg_dir: &Path) -> Result<()> {
    // A real CS2 install always ships its `cfg\` directory already, so this
    // is normally a no-op — but creating it defensively (rather than
    // assuming it exists) costs nothing and matches this function's own
    // "idempotent, safe to call every time" contract.
    std::fs::create_dir_all(cs2_cfg_dir)
        .map_err(|e| Error::msg(format!("creating {}: {e}", cs2_cfg_dir.display())))?;
    let path = cs2_cfg_dir.join(CFG_FILENAME);
    std::fs::write(&path, CFG_CONTENTS)
        .map_err(|e| Error::msg(format!("writing {}: {e}", path.display())))
}

pub fn reserved_tokens() -> Vec<String> {
    vec![
        "-condebug".to_string(),
        "-conclearlog".to_string(),
        format!("+exec {CFG_FILENAME}"),
    ]
}

/// Prepends `reserved_tokens()` to `user_args` (VOIDFRAME's own required
/// tokens always come first, matching the previous merge order) and
/// whitespace-normalizes the result. No validation of any kind: the user's
/// launch options are their own business — see [`strip_reserved`] for the
/// inverse operation this module relies on elsewhere to avoid re-merging
/// VOIDFRAME's own tokens back into themselves on a later run.
pub fn reconcile(user_args: &str) -> String {
    let mut words = reserved_tokens();
    words.extend(user_args.split_whitespace().map(str::to_string));
    words.join(" ")
}

/// The inverse of folding `reserved_tokens()` into a launch-options string:
/// removes every word that IS a reserved token, or that reserved token's
/// first word (so the two-word `"+exec voidframe_keybind.cfg"` token is
/// removed as a pair, not left with a dangling `voidframe_keybind.cfg`),
/// case-insensitively (CS2's own command-line parsing is), and returns
/// whatever's left, whitespace-normalized. Used both to compute the "live
/// user args" `reconcile` should preserve when a scenario has no
/// `launch_args` module, and by the `read_cs2_launch_options` command so the
/// frontend only ever sees the human part.
pub fn strip_reserved(text: &str) -> String {
    let reserved = reserved_tokens();
    // Every reserved token's OWN word sequence, lowercased, e.g.
    // `["condebug"]` -> not applicable (single word); `["+exec",
    // "voidframe_keybind.cfg"]` for the two-word one. Matched as a whole
    // sequence against `text`'s words below, not just by first word, so a
    // stray `+exec` pointing at some OTHER file is left alone.
    let reserved_word_seqs: Vec<Vec<String>> = reserved
        .iter()
        .map(|t| {
            t.split_whitespace()
                .map(|w| w.to_ascii_lowercase())
                .collect()
        })
        .collect();

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let mut matched_len = 0;
        for seq in &reserved_word_seqs {
            if i + seq.len() <= words.len()
                && words[i..i + seq.len()]
                    .iter()
                    .zip(seq)
                    .all(|(w, r)| w.to_ascii_lowercase() == *r)
            {
                matched_len = seq.len();
                break;
            }
        }
        if matched_len > 0 {
            i += matched_len;
        } else {
            out.push(words[i]);
            i += 1;
        }
    }
    out.join(" ")
}

/// Replaces the token immediately following any of `+password`,
/// `+rcon_password`, `+sv_password` (case-insensitive match on the key
/// token only) with the literal `<redacted>`, preserving every other token
/// and whitespace-normalizing the result (single-space joined), matching
/// [`reconcile`] and [`strip_reserved`]'s own style. A matching key as the
/// very last token (no following value) is left as-is rather than panicking
/// or inventing a value to redact. A matching key immediately followed by
/// ANOTHER matching key is treated the same way — the first key's own
/// "value" slot is left empty rather than consuming (and thereby leaking
/// unredacted) the second key's value.
///
/// Steam launch options commonly carry one of these three keys with a
/// secret value (`+password`, `+rcon_password`, `+sv_password`); this is
/// used at every `LogLine` site that would otherwise put the raw launch
/// string into the Live Monitor, CLI stdout, or `run.log`. The values
/// written to Steam and compared for equality are never touched — only the
/// text passed through this function is.
pub fn redact(args: &str) -> String {
    const SECRET_KEYS: [&str; 3] = ["+password", "+rcon_password", "+sv_password"];
    let is_secret_key = |w: &str| SECRET_KEYS.iter().any(|k| w.eq_ignore_ascii_case(k));

    let words: Vec<&str> = args.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        out.push(words[i]);
        let this_is_secret_key = is_secret_key(words[i]);
        i += 1;
        if this_is_secret_key && i < words.len() && !is_secret_key(words[i]) {
            out.push("<redacted>");
            i += 1;
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_tokens_always_present_with_empty_user_args() {
        let merged = reconcile("");
        assert!(merged.contains("-condebug"));
        assert!(merged.contains("-conclearlog"));
        assert!(merged.contains(&format!("+exec {CFG_FILENAME}")));
    }

    #[test]
    fn user_args_pass_through_completely_unvalidated() {
        // No blocklist, no allowlist, nothing rejected -- including a flag
        // that used to be blocked. This is deliberate: see the launch-args
        // rework design, "all the checks ... are useless".
        let merged = reconcile("-insecure -tools -textmode +sv_cheats 1 -whatever-i-want");
        for word in ["-insecure", "-tools", "-textmode", "-whatever-i-want"] {
            assert!(merged.contains(word), "{merged} missing {word}");
        }
    }

    #[test]
    fn reserved_tokens_come_before_user_args() {
        let merged = reconcile("-novid");
        let condebug_pos = merged.find("-condebug").unwrap();
        let novid_pos = merged.find("-novid").unwrap();
        assert!(condebug_pos < novid_pos);
    }

    #[test]
    fn strip_reserved_removes_all_three_reserved_tokens_case_insensitively() {
        let raw = format!("-CONDEBUG -conclearlog +EXEC {CFG_FILENAME} -novid");
        let stripped = strip_reserved(&raw);
        assert_eq!(stripped, "-novid");
    }

    #[test]
    fn strip_reserved_leaves_a_stray_exec_pointing_at_a_different_file_alone() {
        let stripped = strip_reserved("+exec something_else.cfg -novid");
        assert!(stripped.contains("+exec something_else.cfg"));
        assert!(stripped.contains("-novid"));
    }

    #[test]
    fn strip_reserved_of_empty_text_is_empty() {
        assert_eq!(strip_reserved(""), "");
    }

    #[test]
    fn strip_reserved_of_reconcile_of_empty_user_args_is_empty() {
        assert_eq!(strip_reserved(&reconcile("")), "");
    }

    #[test]
    fn round_trip_strip_reserved_of_reconcile_reproduces_user_args() {
        let user_args = "-high -threads 8 -novid";
        let round_tripped = strip_reserved(&reconcile(user_args));
        assert!(
            round_tripped
                .split_whitespace()
                .eq(user_args.split_whitespace()),
            "{round_tripped:?} != {user_args:?}"
        );
    }

    #[test]
    fn redact_replaces_the_value_following_password_case_insensitively() {
        assert_eq!(
            redact("+password hunter2 -novid"),
            "+password <redacted> -novid"
        );
        assert_eq!(
            redact("+PASSWORD hunter2 -novid"),
            "+PASSWORD <redacted> -novid"
        );
    }

    #[test]
    fn redact_replaces_all_three_secret_keys() {
        assert_eq!(
            redact("+rcon_password hunter2 +sv_password hunter3 -novid"),
            "+rcon_password <redacted> +sv_password <redacted> -novid"
        );
    }

    #[test]
    fn redact_of_a_string_with_no_secret_is_unchanged_modulo_whitespace() {
        // Whitespace normalization (leading/trailing/multiple spaces
        // collapsed to single-space joins) is asserted explicitly here,
        // same as `reconcile`/`strip_reserved`'s own contract.
        assert_eq!(redact("  -novid   -tools  "), "-novid -tools");
    }

    #[test]
    fn redact_of_a_trailing_secret_key_with_no_value_does_not_panic() {
        assert_eq!(redact("-novid +password"), "-novid +password");
    }

    #[test]
    fn redact_does_not_let_a_secret_key_consume_a_following_secret_keys_value() {
        // `+password` must not swallow `+rcon_password` as its own "value" --
        // that would leave `hunter2` (the REAL rcon password) unredacted.
        assert_eq!(
            redact("+password +rcon_password hunter2"),
            "+password +rcon_password <redacted>"
        );
    }

    #[test]
    fn redact_handles_a_repeated_key_with_two_separate_values() {
        assert_eq!(
            redact("+password a +password b"),
            "+password <redacted> +password <redacted>"
        );
    }
}
