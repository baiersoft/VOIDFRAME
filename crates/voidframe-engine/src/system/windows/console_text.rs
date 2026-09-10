//! Decodes the redirected stdout/stderr of console tools (`powercfg.exe`)
//! spawned under a hidden console.
//!
//! Such a child writes in its console output codepage, which for a fresh
//! console defaults to the system OEM codepage: 850 on a German Windows,
//! 437 on US English, 65001 only when "Use Unicode UTF-8 for worldwide
//! language support" is enabled (confirmed live on this rig, where
//! `powercfg /list` emitted a `Höchstleistung` plan name as UTF-8 `C3 B6`).
//! Decoding as UTF-8 therefore looked correct here and mangled every
//! non-ASCII plan name for the first alpha's German tester.

use windows::Win32::Globalization::{CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};

/// Decodes `bytes` from the system OEM codepage. Falls back to lossy UTF-8
/// if the conversion itself fails, so a decode problem can never turn into
/// a failed powercfg call.
pub(super) fn decode_console_output(bytes: &[u8]) -> String {
    decode_with_codepage(CP_OEMCP, bytes)
}

fn decode_with_codepage(codepage: u32, bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let no_flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: `bytes` is a live slice for the duration of the call; the
    // `windows` binding passes its pointer and length together. `None` for
    // the output buffer is the documented "return the required length only"
    // form, so nothing is written.
    let needed = unsafe { MultiByteToWideChar(codepage, no_flags, bytes, None) };
    let Ok(needed) = usize::try_from(needed) else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    if needed == 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed];
    // SAFETY: `wide` has exactly the length the sizing call reported for
    // this same input and codepage, and the binding passes that length
    // alongside the pointer, so the call cannot write past it.
    let written = unsafe { MultiByteToWideChar(codepage, no_flags, bytes, Some(&mut wide)) };
    match usize::try_from(written) {
        Ok(written) if written > 0 && written <= wide.len() => {
            String::from_utf16_lossy(&wide[..written])
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Globalization::CP_UTF8;

    const CP_OEM_850: u32 = 850;
    const CP_ANSI_1252: u32 = 1252;

    #[test]
    fn ascii_round_trips_through_the_system_oem_codepage() {
        // Every OEM codepage shares the ASCII range, so this holds whether
        // the machine is on 437, 850, or 65001.
        let line = "Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced) *\r\n";
        assert_eq!(decode_console_output(line.as_bytes()), line);
    }

    #[test]
    fn decodes_a_german_umlaut_from_oem_850() {
        assert_eq!(
            decode_with_codepage(CP_OEM_850, b"H\x94chstleistung"),
            "Höchstleistung"
        );
    }

    #[test]
    fn decodes_a_german_umlaut_from_ansi_1252() {
        assert_eq!(
            decode_with_codepage(CP_ANSI_1252, b"H\xF6chstleistung"),
            "Höchstleistung"
        );
    }

    #[test]
    fn decodes_utf8_when_the_codepage_is_65001() {
        assert_eq!(
            decode_with_codepage(CP_UTF8, "Höchstleistung".as_bytes()),
            "Höchstleistung"
        );
    }

    #[test]
    fn empty_input_decodes_to_an_empty_string() {
        assert_eq!(decode_console_output(b""), "");
    }
}
