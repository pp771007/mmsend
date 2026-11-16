//! Console plumbing for an exe that is both a console tool and a GUI app.

use windows_sys::Win32::Globalization::{CP_ACP, CP_OEMCP, CP_UTF8, MultiByteToWideChar};
use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleOutputCP, GetConsoleProcessList};

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const UTF16LE_BOM: &[u8] = &[0xFF, 0xFE];

/// The exe is built for the console subsystem so that cmd waits for it and sees its
/// exit code. When it was double-clicked, Windows created a console just for us;
/// detaching closes that window. A console shared with cmd is left alone.
pub fn release_own_console() {
    let mut process_ids = [0u32; 2];
    // SAFETY: the buffer length passed matches the array.
    unsafe {
        if GetConsoleProcessList(process_ids.as_mut_ptr(), process_ids.len() as u32) == 1 {
            FreeConsole();
        }
    }
}

/// Text piped in from cmd (`echo 中文 | mmsend ...`) arrives in the console code page
/// (cp950 on Traditional Chinese Windows), not UTF-8.
pub fn decode_piped_text(bytes: &[u8]) -> String {
    // SAFETY: plain query with no arguments.
    decode(bytes, piped_fallback_codepage(unsafe { GetConsoleOutputCP() }))
}

/// The code page to try once the bytes turned out not to be UTF-8. A UTF-8 console
/// (`chcp 65001`, or a UTF-8 Windows Terminal profile) cannot be the answer then:
/// it means a legacy tool such as ipconfig wrote in the OEM code page regardless.
fn piped_fallback_codepage(console_codepage: u32) -> u32 {
    match console_codepage {
        0 | CP_UTF8 => CP_OEMCP,
        codepage => codepage,
    }
}

/// Message files: UTF-8 or UTF-16 (PowerShell 5.1's `>` writes UTF-16LE), falling back
/// to the ANSI code page for legacy Big5 files.
pub fn decode_file_text(bytes: &[u8]) -> String {
    decode(bytes, CP_ACP)
}

fn decode(bytes: &[u8], fallback_codepage: u32) -> String {
    if let Some(rest) = bytes.strip_prefix(UTF16LE_BOM) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => decode_codepage(bytes, fallback_codepage),
    }
}

fn decode_codepage(bytes: &[u8], codepage: u32) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let len = i32::try_from(bytes.len()).expect("message larger than 2 GB");
    // SAFETY: first call sizes the buffer, second call fills exactly that many units.
    unsafe {
        let units = MultiByteToWideChar(codepage, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
        let mut buffer = vec![0u16; units.max(0) as usize];
        MultiByteToWideChar(codepage, 0, bytes.as_ptr(), len, buffer.as_mut_ptr(), units);
        String::from_utf16_lossy(&buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG5: u32 = 950;
    /// 「中文」 encoded in Big5 / cp950.
    const CHINESE_BIG5: &[u8] = &[0xA4, 0xA4, 0xA4, 0xE5];

    #[test]
    fn cp950_pipe_input_is_decoded() {
        assert_eq!(decode(CHINESE_BIG5, BIG5), "中文");
    }

    #[test]
    fn oem_output_piped_into_a_utf8_console_is_decoded() {
        assert_eq!(piped_fallback_codepage(CP_UTF8), CP_OEMCP);
        assert_eq!(piped_fallback_codepage(BIG5), BIG5);
    }

    #[test]
    fn utf8_input_wins_over_the_fallback_codepage() {
        assert_eq!(decode("中文".as_bytes(), BIG5), "中文");
        assert_eq!(decode(&[UTF8_BOM, "中文".as_bytes()].concat(), BIG5), "中文");
    }

    #[test]
    fn powershell_utf16_file_is_decoded() {
        let mut bytes = UTF16LE_BOM.to_vec();
        bytes.extend("中文".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode(&bytes, BIG5), "中文");
    }
}
