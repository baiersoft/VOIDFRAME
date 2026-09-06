//! Real registry access via raw Win32 FFI (`windows` crate). Every call is
//! synchronous, so each public function wraps its Win32 work in
//! [`tokio::task::spawn_blocking`].
//!
//! Verified against the real Win32 API surface before this file was written
//! (see `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`'s
//! registry task for the exact scratch-crate round trip) — notably:
//! `RegCreateKeyExW` needs the `Win32_Security` crate feature to resolve at
//! all, `RegSetValueExW`'s `reserved` parameter is `Option<u32>` (pass
//! `Some(0)`, not a bare `0`), and a missing value queries back as
//! `ERROR_FILE_NOT_FOUND`, not an empty success.

use crate::error::{Error, Result};
use crate::model::module::Hive;
use crate::system::{RegKey, RegValue};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY,
    REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_QWORD, REG_SAM_FLAGS,
    REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hkey_root(hive: Hive) -> HKEY {
    match hive {
        Hive::Hklm => HKEY_LOCAL_MACHINE,
        Hive::Hkcu => HKEY_CURRENT_USER,
    }
}

/// Opens `k.subkey` under `k.hive`'s root for an existing-key-only operation
/// (read or delete) with the given access right, without creating it.
/// Returns `Ok(None)` if the key itself doesn't exist — callers map that to
/// [`RegValue::Absent`] (read) or a no-op (delete) rather than treating it as
/// an error, and critically without ever creating the key as a side effect
/// of merely looking at it.
fn open_existing_subkey(hive: Hive, subkey: &str, access: REG_SAM_FLAGS) -> Result<Option<HKEY>> {
    let subkey_w = wide(subkey);
    let mut hkey = HKEY::default();
    // SAFETY: `hkey_root(hive)` is always one of the well-known pseudo-handles
    // (HKEY_LOCAL_MACHINE/HKEY_CURRENT_USER), which are valid for the process
    // lifetime and need no closing. `subkey_w` is a local `Vec<u16>` that
    // outlives this call, NUL-terminated by `wide`, so the `PCWSTR` built
    // from its pointer is valid for the duration of the call. `hkey` is a
    // valid `&mut HKEY` for the call to write its output handle into.
    let r = unsafe {
        RegOpenKeyExW(
            hkey_root(hive),
            PCWSTR(subkey_w.as_ptr()),
            Some(0),
            access,
            &mut hkey,
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if r != ERROR_SUCCESS {
        return Err(Error::msg(format!(
            "RegOpenKeyExW({}\\{subkey}) failed: {r:?}",
            hive.as_str()
        )));
    }
    Ok(Some(hkey))
}

/// Opens (creating if absent) `k.subkey` under `k.hive`'s root, with only the
/// access a value write needs (`KEY_SET_VALUE`). The registry key itself
/// always exists after this — only the *value* under it may be
/// [`RegValue::Absent`]. Writing is the one path that legitimately needs
/// create-if-missing semantics.
fn open_or_create_subkey_for_write(hive: Hive, subkey: &str) -> Result<HKEY> {
    let subkey_w = wide(subkey);
    let mut hkey = HKEY::default();
    // SAFETY: `hkey_root(hive)` is a well-known pseudo-handle, valid for the
    // process lifetime. `subkey_w` is a local `Vec<u16>` that outlives this
    // call and is NUL-terminated by `wide`, so the `PCWSTR` built from its
    // pointer stays valid for the call's duration. `hkey` is a valid `&mut
    // HKEY` output slot. `security_attributes: None` and
    // `disposition: None` are both documented-optional out-parameters.
    let r = unsafe {
        RegCreateKeyExW(
            hkey_root(hive),
            PCWSTR(subkey_w.as_ptr()),
            Some(0),
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    if r != ERROR_SUCCESS {
        return Err(Error::msg(format!(
            "RegCreateKeyExW({}\\{subkey}) failed: {r:?}",
            hive.as_str()
        )));
    }
    Ok(hkey)
}

/// Truncates a `REG_BINARY` read buffer to the data call's own reported byte
/// count, not the earlier sizing call's length. The value's actual size can
/// shrink between the sizing call and the data call if a concurrent writer
/// races this read (TOCTOU) — the data call itself never overruns `buf` (it
/// only ever writes at most `buf.len()` bytes), but if it wrote fewer than
/// that, the extra trailing bytes in `buf` are stale and must not be treated
/// as part of the value.
fn truncate_to_actual_len(mut buf: Vec<u8>, actual_len: u32) -> Vec<u8> {
    let actual_len = (actual_len as usize).min(buf.len());
    buf.truncate(actual_len);
    buf
}

/// Decodes a NUL-terminated UTF-16LE byte buffer to a `String`, trimming the
/// terminator. Shared by `REG_SZ` and `REG_EXPAND_SZ` — both have the exact
/// same wire format; only the registry type tag differs (see
/// `references/registry-raw-api.md` in the `windows-native` skill).
fn decode_sz_bytes(buf: &[u8]) -> String {
    // Built byte-by-byte rather than reinterpreting `buf`'s memory as `&[u16]`
    // directly -- a `Vec<u8>` only guarantees 1-byte alignment, which is
    // technically insufficient for a `u16` reinterpretation even though it
    // works on every real allocator.
    let u16s: Vec<u16> = buf
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&u16s)
        .trim_end_matches('\0')
        .to_string()
}

/// Encodes a `String` to `REG_SZ`/`REG_EXPAND_SZ`'s NUL-terminated UTF-16LE
/// wire format.
fn encode_sz_bytes(s: &str) -> Vec<u8> {
    wide(s).iter().flat_map(|u| u.to_le_bytes()).collect()
}

/// Decodes a `REG_MULTI_SZ` byte buffer into its list of strings.
///
/// Wire format: each string is individually NUL-terminated, and the whole
/// list is terminated by one further NUL (Microsoft's own example,
/// `String1\0String2\0String3\0\0`, has exactly one *extra* NUL after the
/// last string's own terminator). Splitting the UTF-16 buffer on every NUL
/// unit (keeping empty subslices, exactly like `str::split`) therefore always
/// yields `N + 2` pieces for `N` real strings: the first `N` are the real
/// strings (including any that are themselves the empty string) and the
/// final two are always artifacts of termination, empty by construction --
/// verified against [`encode_multi_sz`] below and the round-trip/decode tests
/// in this module (an empty `Vec` round-trips as a single terminator NUL, an
/// empty-string element round-trips distinctly from the end-of-list marker).
fn decode_multi_sz(buf: &[u8]) -> Vec<String> {
    let u16s: Vec<u16> = buf
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let mut pieces: Vec<&[u16]> = u16s.split(|&c| c == 0).collect();
    for _ in 0..2 {
        if pieces.last().is_some_and(|p| p.is_empty()) {
            pieces.pop();
        }
    }
    pieces.into_iter().map(String::from_utf16_lossy).collect()
}

/// Encodes a list of strings to `REG_MULTI_SZ`'s wire format: each string
/// followed by its own NUL, then one further NUL terminating the whole list.
/// Symmetric with [`decode_multi_sz`] above.
fn encode_multi_sz(strings: &[String]) -> Vec<u8> {
    let mut u16s: Vec<u16> = Vec::new();
    for s in strings {
        u16s.extend(s.encode_utf16());
        u16s.push(0);
    }
    u16s.push(0);
    u16s.iter().flat_map(|u| u.to_le_bytes()).collect()
}

fn close(hkey: HKEY) {
    // SAFETY: `hkey` was returned by a prior successful RegOpenKeyExW or
    // RegCreateKeyExW call at this function's only call sites, and each of
    // those handles is closed exactly once, here.
    // Best-effort; a failure here isn't actionable and every call site has
    // already gotten what it needed.
    let _ = unsafe { RegCloseKey(hkey) };
}

fn read_sync(k: &RegKey) -> Result<RegValue> {
    let Some(hkey) = open_existing_subkey(k.hive, &k.subkey, KEY_QUERY_VALUE)? else {
        return Ok(RegValue::Absent);
    };
    let name_w = wide(&k.value_name);

    // Sizing call: find the type and required buffer length.
    let mut val_type = REG_VALUE_TYPE::default();
    let mut len: u32 = 0;
    // SAFETY: `hkey` is a valid, open key handle from `open_existing_subkey`
    // above. `name_w` is a local `Vec<u16>` (NUL-terminated by `wide`) that
    // outlives this call, so the `PCWSTR` built from its pointer is valid
    // for the call's duration. The `lpType`/`lpcbData` out-params point at
    // live locals on this stack frame; `lpData: None` means no data buffer
    // is written in this sizing call, matching `lpcbData` being an in/out
    // capacity of 0.
    let r = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            Some(&mut val_type),
            None,
            Some(&mut len),
        )
    };
    if r == ERROR_FILE_NOT_FOUND {
        close(hkey);
        return Ok(RegValue::Absent);
    }
    if r != ERROR_SUCCESS {
        close(hkey);
        return Err(Error::msg(format!(
            "RegQueryValueExW sizing call for {}\\{}\\{} failed: {r:?}",
            k.hive.as_str(),
            k.subkey,
            k.value_name
        )));
    }

    let mut buf = vec![0u8; len as usize];
    let mut len2 = len;
    // SAFETY: `hkey` and `name_w` are valid as above. `buf` is sized to
    // exactly `len` bytes from the prior sizing call and `len2` is
    // initialized to that same capacity, so `RegQueryValueExW` will not
    // write past `buf`'s allocation — the API contract is that it writes at
    // most `*lpcbData` bytes and updates `*lpcbData` to the actual count.
    let r = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            Some(&mut val_type),
            Some(buf.as_mut_ptr()),
            Some(&mut len2),
        )
    };
    close(hkey);
    if r != ERROR_SUCCESS {
        return Err(Error::msg(format!(
            "RegQueryValueExW data call for {}\\{}\\{} failed: {r:?}",
            k.hive.as_str(),
            k.subkey,
            k.value_name
        )));
    }

    Ok(match val_type {
        REG_DWORD if buf.len() >= 4 => {
            RegValue::Dword(u32::from_le_bytes(buf[..4].try_into().unwrap()))
        }
        REG_QWORD if buf.len() >= 8 => {
            RegValue::Qword(u64::from_le_bytes(buf[..8].try_into().unwrap()))
        }
        REG_SZ => RegValue::Sz(decode_sz_bytes(&buf)),
        REG_EXPAND_SZ => RegValue::ExpandSz(decode_sz_bytes(&buf)),
        REG_MULTI_SZ => RegValue::MultiSz(decode_multi_sz(&truncate_to_actual_len(buf, len2))),
        REG_BINARY => RegValue::Binary(truncate_to_actual_len(buf, len2)),
        other => {
            return Err(Error::msg(format!(
                "unsupported registry value type {other:?} for {}\\{}\\{}",
                k.hive.as_str(),
                k.subkey,
                k.value_name
            )));
        }
    })
}

fn write_sync(k: &RegKey, v: &RegValue) -> Result<()> {
    let hkey = open_or_create_subkey_for_write(k.hive, &k.subkey)?;
    let name_w = wide(&k.value_name);

    let (reg_type, bytes): (REG_VALUE_TYPE, Vec<u8>) = match v {
        RegValue::Dword(n) => (REG_DWORD, n.to_le_bytes().to_vec()),
        RegValue::Qword(n) => (REG_QWORD, n.to_le_bytes().to_vec()),
        RegValue::Sz(s) => (REG_SZ, encode_sz_bytes(s)),
        RegValue::Binary(b) => (REG_BINARY, b.clone()),
        RegValue::ExpandSz(s) => (REG_EXPAND_SZ, encode_sz_bytes(s)),
        RegValue::MultiSz(v) => (REG_MULTI_SZ, encode_multi_sz(v)),
        RegValue::Absent => {
            close(hkey);
            return Err(Error::msg(format!(
                "cannot write RegValue::Absent to {}\\{}\\{} — use delete_registry_value instead",
                k.hive.as_str(),
                k.subkey,
                k.value_name
            )));
        }
    };

    // SAFETY: `hkey` is a valid, open key handle from
    // `open_or_create_subkey_for_write` above, opened with `KEY_SET_VALUE`
    // which is exactly the right this call needs. `name_w` is a local
    // NUL-terminated `Vec<u16>` outliving the call, so the `PCWSTR` built
    // from it is valid. `bytes` outlives the call and its length matches
    // what `Some(&bytes)` reports to the API, so `RegSetValueExW` reads
    // exactly the bytes that exist.
    let r = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            Some(0),
            reg_type,
            Some(&bytes),
        )
    };
    close(hkey);
    if r != ERROR_SUCCESS {
        return Err(Error::msg(format!(
            "RegSetValueExW for {}\\{}\\{} failed: {r:?}",
            k.hive.as_str(),
            k.subkey,
            k.value_name
        )));
    }
    Ok(())
}

fn delete_value_sync(k: &RegKey) -> Result<()> {
    // Deleting a value under a key that doesn't exist at all is itself a
    // no-op — nothing to delete — not a key-creation event. `RegDeleteValueW`
    // only needs `KEY_SET_VALUE`, the same right the write path uses.
    let Some(hkey) = open_existing_subkey(k.hive, &k.subkey, KEY_SET_VALUE)? else {
        return Ok(());
    };
    let name_w = wide(&k.value_name);
    // SAFETY: `hkey` is a valid, open key handle from `open_existing_subkey`
    // above, opened with `KEY_SET_VALUE` which is what `RegDeleteValueW`
    // requires. `name_w` is a local NUL-terminated `Vec<u16>` outliving the
    // call, so the `PCWSTR` built from it is valid for the call's duration.
    let r = unsafe { RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr())) };
    close(hkey);
    // Deleting a value that's already absent is a no-op success, matching
    // MockController's delete_registry_value
    // (`docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`,
    // system/mock.rs) —
    // idempotent revert.
    if r != ERROR_SUCCESS && r != ERROR_FILE_NOT_FOUND {
        return Err(Error::msg(format!(
            "RegDeleteValueW for {}\\{}\\{} failed: {r:?}",
            k.hive.as_str(),
            k.subkey,
            k.value_name
        )));
    }
    Ok(())
}

pub async fn read(k: &RegKey) -> Result<RegValue> {
    let k = k.clone();
    tokio::task::spawn_blocking(move || read_sync(&k))
        .await
        .map_err(|e| Error::msg(format!("registry read task panicked: {e}")))?
}

pub async fn write(k: &RegKey, v: &RegValue) -> Result<()> {
    let k = k.clone();
    let v = v.clone();
    tokio::task::spawn_blocking(move || write_sync(&k, &v))
        .await
        .map_err(|e| Error::msg(format!("registry write task panicked: {e}")))?
}

pub async fn delete_value(k: &RegKey) -> Result<()> {
    let k = k.clone();
    tokio::task::spawn_blocking(move || delete_value_sync(&k))
        .await
        .map_err(|e| Error::msg(format!("registry delete task panicked: {e}")))?
}

// RegDeleteKeyW is intentionally not imported in M1 — no mutation type
// deletes an entire subkey, only individual values (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.3: "Snapshot
// records 'absent' so rollback deletes rather than writing a guessed
// default" refers to values, not keys). Add it back if a later milestone
// adds key-level deletion and needs it.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::module::Hive;

    // Every test below uses this fixed scratch subkey under HKCU — safe
    // (no admin needed, isolated from anything real), and each test cleans
    // up its own values so a panic mid-test doesn't leak state into the
    // next test run. Tests run serially within a process by default in
    // `cargo test`, but different test *binaries* could still race on this
    // key if run in parallel — acceptable here since this crate has one
    // test binary and `cargo test` runs a crate's own tests single-process.
    fn scratch_key(value_name: &str) -> RegKey {
        RegKey {
            hive: Hive::Hkcu,
            subkey: "Software\\VOIDFRAME_TEST_SCRATCH".into(),
            value_name: value_name.into(),
        }
    }

    // Regression test for the REG_BINARY TOCTOU finding (2026-09-02 audit,
    // "Medium" section, confirmed live by an independent follow-up review):
    // the data call's own returned length (here `len2`/`actual_len`) must
    // win over the earlier sizing call's length, or a value that shrinks
    // between the two calls (a concurrent registry writer) leaves stale
    // trailing bytes in the buffer that get treated as part of the value.
    #[test]
    fn truncate_to_actual_len_trims_to_second_calls_own_length() {
        let sized_for_first_call = vec![1u8, 2, 3, 4, 5];
        let actual_len_from_second_call = 2u32;

        let result = truncate_to_actual_len(sized_for_first_call, actual_len_from_second_call);

        assert_eq!(result, vec![1u8, 2]);
    }

    #[test]
    fn truncate_to_actual_len_is_a_no_op_when_lengths_match() {
        let buf = vec![1u8, 2, 3];
        let result = truncate_to_actual_len(buf.clone(), buf.len() as u32);
        assert_eq!(result, buf);
    }

    #[tokio::test]
    async fn absent_value_reads_as_absent() {
        let k = scratch_key("DoesNotExist_absent_test");
        assert_eq!(read(&k).await.unwrap(), RegValue::Absent);
    }

    #[tokio::test]
    async fn dword_round_trips() {
        let k = scratch_key("Dword_test");
        write(&k, &RegValue::Dword(42)).await.unwrap();
        assert_eq!(read(&k).await.unwrap(), RegValue::Dword(42));
        delete_value(&k).await.unwrap();
        assert_eq!(read(&k).await.unwrap(), RegValue::Absent);
    }

    #[tokio::test]
    async fn sz_round_trips_including_unicode() {
        let k = scratch_key("Sz_test");
        write(&k, &RegValue::Sz("hello voidframe — ünïcödé".into()))
            .await
            .unwrap();
        assert_eq!(
            read(&k).await.unwrap(),
            RegValue::Sz("hello voidframe — ünïcödé".into())
        );
        delete_value(&k).await.unwrap();
    }

    // Regression coverage for the M16 finding (docs/refactor/2026-09-02's
    // Medium section, "REG_EXPAND_SZ/REG_MULTI_SZ abort the snapshot"):
    // `read_sync` used to hard-error on both types via the `other` match arm.
    #[tokio::test]
    async fn expand_sz_round_trips_verbatim_unexpanded() {
        let k = scratch_key("ExpandSz_test");
        // Deliberately contains an environment-variable reference that must
        // survive the round trip unexpanded -- VOIDFRAME never expands it.
        write(&k, &RegValue::ExpandSz(r"%SystemRoot%\System32".into()))
            .await
            .unwrap();
        assert_eq!(
            read(&k).await.unwrap(),
            RegValue::ExpandSz(r"%SystemRoot%\System32".into())
        );
        delete_value(&k).await.unwrap();
    }

    #[tokio::test]
    async fn multi_sz_round_trips_including_empty_vec_and_empty_element() {
        let k = scratch_key("MultiSz_test");
        write(
            &k,
            &RegValue::MultiSz(vec!["alpha".into(), "beta — ünïcödé".into()]),
        )
        .await
        .unwrap();
        assert_eq!(
            read(&k).await.unwrap(),
            RegValue::MultiSz(vec!["alpha".into(), "beta — ünïcödé".into()])
        );

        // Edge case: an empty `Vec<String>` (zero real strings).
        write(&k, &RegValue::MultiSz(vec![])).await.unwrap();
        assert_eq!(read(&k).await.unwrap(), RegValue::MultiSz(vec![]));

        // Edge case: a `Vec` with an empty string element, which must
        // round-trip as a real (empty) element distinct from list
        // termination -- see `decode_multi_sz`'s doc comment.
        write(
            &k,
            &RegValue::MultiSz(vec!["a".into(), "".into(), "c".into()]),
        )
        .await
        .unwrap();
        assert_eq!(
            read(&k).await.unwrap(),
            RegValue::MultiSz(vec!["a".into(), "".into(), "c".into()])
        );

        delete_value(&k).await.unwrap();
    }

    // Pure-decode unit test, independent of a real registry round trip:
    // proves the double-NUL-terminator edge case directly against a
    // synthetic byte buffer, per the M16 spec's explicit requirement.
    #[test]
    fn decode_multi_sz_splits_on_nul_and_drops_only_the_terminator() {
        fn wide_bytes(units: &[u16]) -> Vec<u8> {
            units.iter().flat_map(|u| u.to_le_bytes()).collect()
        }

        // "a" \0 "b" \0 \0 -- two real strings, standard double-NUL end.
        let buf = wide_bytes(&[b'a' as u16, 0, b'b' as u16, 0, 0]);
        assert_eq!(
            decode_multi_sz(&buf),
            vec!["a".to_string(), "b".to_string()]
        );

        // \0 alone -- zero real strings (an empty `Vec`'s own encoding).
        let buf = wide_bytes(&[0]);
        assert_eq!(decode_multi_sz(&buf), Vec::<String>::new());

        // "a" \0 \0 \0 -- "a" followed by a real empty-string element, then
        // the list terminator. Must decode to two elements, not one.
        let buf = wide_bytes(&[b'a' as u16, 0, 0, 0]);
        assert_eq!(decode_multi_sz(&buf), vec!["a".to_string(), String::new()]);
    }

    #[tokio::test]
    async fn qword_and_binary_round_trip() {
        let kq = scratch_key("Qword_test");
        write(&kq, &RegValue::Qword(u64::MAX - 1)).await.unwrap();
        assert_eq!(read(&kq).await.unwrap(), RegValue::Qword(u64::MAX - 1));
        delete_value(&kq).await.unwrap();

        let kb = scratch_key("Binary_test");
        write(&kb, &RegValue::Binary(vec![1, 2, 3, 255, 0]))
            .await
            .unwrap();
        assert_eq!(
            read(&kb).await.unwrap(),
            RegValue::Binary(vec![1, 2, 3, 255, 0])
        );
        delete_value(&kb).await.unwrap();
    }

    #[tokio::test]
    async fn delete_of_absent_value_is_ok() {
        let k = scratch_key("NeverWritten_delete_test");
        delete_value(&k).await.unwrap();
    }

    #[tokio::test]
    async fn write_overwrites_existing_value() {
        let k = scratch_key("Overwrite_test");
        write(&k, &RegValue::Dword(1)).await.unwrap();
        write(&k, &RegValue::Dword(2)).await.unwrap();
        assert_eq!(read(&k).await.unwrap(), RegValue::Dword(2));
        delete_value(&k).await.unwrap();
    }

    // Regression test for H4 (2026-09-02 audit): `open_or_create_subkey`
    // used `RegCreateKeyExW` for reads too, so reading a value under a key
    // that didn't exist yet actually created the key. Uses a subkey name
    // unique to this test run (a UUID, not the shared `scratch_key` subkey
    // the other tests write under) so the assertion can't be confused by
    // leftover state — this file's tests don't clean up the shared scratch
    // *key* between runs (only the values under it) — and so this test
    // doesn't need to run serially with (or after) the others to be valid.
    #[tokio::test]
    async fn read_of_nonexistent_key_does_not_create_it() {
        let subkey = format!(
            "Software\\VOIDFRAME_TEST_SCRATCH_PROBE_{}",
            uuid::Uuid::new_v4()
        );
        let k = RegKey {
            hive: Hive::Hkcu,
            subkey: subkey.clone(),
            value_name: "AnyValue".into(),
        };

        // The regression itself: reading a value under an absent key must
        // not create the key as a side effect.
        assert_eq!(read(&k).await.unwrap(), RegValue::Absent);

        // Independently confirm the key itself was never created, using the
        // same open-for-read path the fix relies on (not just re-reading
        // the value, which would pass even if the key now exists).
        match open_existing_subkey(Hive::Hkcu, &subkey, KEY_QUERY_VALUE).unwrap() {
            None => {}
            Some(hkey) => {
                close(hkey);
                panic!("reading a nonexistent key's value created the key: {subkey}");
            }
        }
    }
}
