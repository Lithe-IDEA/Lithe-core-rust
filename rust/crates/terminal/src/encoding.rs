//! Character encodings of console output, and which one decodes each line.
//!
//! A process writes bytes. Most write UTF-8, but a Windows program that writes to a pipe often
//! uses the system ANSI code page instead (a JVM on a Chinese system writes GBK there), and one
//! build can print both: a tool's own lines and the lines of a process it starts may differ.
//! Core therefore decides per line. A line that is valid UTF-8 is UTF-8, because text in a legacy
//! encoding is almost never valid UTF-8 by accident; otherwise the next candidate in which the
//! line is valid decodes it. The candidates come from an [`OutputEncodingPolicy`]: the user's
//! fixed choice alone, or, automatically, the encoding the starter of the process declared,
//! UTF-8, and the encoding of the system code page the host reports.
//!
//! Every encoding here is ASCII-compatible, and in none of them does a byte of a multibyte
//! character equal `\n`, `\r` or ESC, so lines and escape sequences can be found in the bytes
//! before the characters are decoded. That rules out UTF-16, which file encodings support.
//! Codec tables come from `encoding_rs` (WHATWG Encoding Standard).

use encoding_rs::{
    CoderResult, DecoderResult, Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, KOI8_R, SHIFT_JIS,
    UTF_8, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252, WINDOWS_1253, WINDOWS_1254, WINDOWS_1255,
    WINDOWS_1256, WINDOWS_1257, WINDOWS_1258, WINDOWS_874,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

/// An encoding console output can be decoded with, identified by a stable string id. The ids
/// are the same as the file encoding ids for the encodings both support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputEncoding {
    /// UTF-8 (`utf-8`); code page 65001.
    Utf8,
    /// Simplified Chinese GBK (`gbk`); code page 936.
    Gbk,
    /// Simplified Chinese GB 18030 (`gb18030`); code page 54936.
    Gb18030,
    /// Traditional Chinese Big5 (`big5`); code page 950.
    Big5,
    /// Japanese Shift_JIS (`shift-jis`); code page 932.
    ShiftJis,
    /// Japanese EUC-JP (`euc-jp`); code pages 20932 and 51932.
    EucJp,
    /// Korean EUC-KR (`euc-kr`); code pages 949 and 51949.
    EucKr,
    /// Thai (`windows-874`); code page 874.
    Windows874,
    /// Central European (`windows-1250`); code page 1250.
    Windows1250,
    /// Cyrillic (`windows-1251`); code page 1251.
    Windows1251,
    /// Western European (`windows-1252`); code pages 1252 and 28591 (ISO-8859-1).
    Windows1252,
    /// Greek (`windows-1253`); code page 1253.
    Windows1253,
    /// Turkish (`windows-1254`); code page 1254.
    Windows1254,
    /// Hebrew (`windows-1255`); code page 1255.
    Windows1255,
    /// Arabic (`windows-1256`); code page 1256.
    Windows1256,
    /// Baltic (`windows-1257`); code page 1257.
    Windows1257,
    /// Vietnamese (`windows-1258`); code page 1258.
    Windows1258,
    /// Russian KOI8-R (`koi8-r`); code page 20866.
    Koi8R,
}

/// Every output encoding, in the order a menu lists them.
pub const OUTPUT_ENCODINGS: [OutputEncoding; 18] = [
    OutputEncoding::Utf8,
    OutputEncoding::Gbk,
    OutputEncoding::Gb18030,
    OutputEncoding::Big5,
    OutputEncoding::ShiftJis,
    OutputEncoding::EucJp,
    OutputEncoding::EucKr,
    OutputEncoding::Windows874,
    OutputEncoding::Windows1250,
    OutputEncoding::Windows1251,
    OutputEncoding::Windows1252,
    OutputEncoding::Windows1253,
    OutputEncoding::Windows1254,
    OutputEncoding::Windows1255,
    OutputEncoding::Windows1256,
    OutputEncoding::Windows1257,
    OutputEncoding::Windows1258,
    OutputEncoding::Koi8R,
];

impl OutputEncoding {
    /// Stable identifier exchanged with hosts and plugins.
    pub fn id(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Gbk => "gbk",
            Self::Gb18030 => "gb18030",
            Self::Big5 => "big5",
            Self::ShiftJis => "shift-jis",
            Self::EucJp => "euc-jp",
            Self::EucKr => "euc-kr",
            Self::Windows874 => "windows-874",
            Self::Windows1250 => "windows-1250",
            Self::Windows1251 => "windows-1251",
            Self::Windows1252 => "windows-1252",
            Self::Windows1253 => "windows-1253",
            Self::Windows1254 => "windows-1254",
            Self::Windows1255 => "windows-1255",
            Self::Windows1256 => "windows-1256",
            Self::Windows1257 => "windows-1257",
            Self::Windows1258 => "windows-1258",
            Self::Koi8R => "koi8-r",
        }
    }

    /// Parses a stable id or an accepted alias, case-insensitively.
    ///
    /// # Errors
    ///
    /// `InvalidRequest` naming the supported ids when `value` is none of them.
    pub fn parse(value: &str) -> Result<Self, CoreError> {
        let encoding = match value.trim().to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => Self::Utf8,
            "gbk" | "cp936" => Self::Gbk,
            "gb18030" => Self::Gb18030,
            "big5" | "cp950" => Self::Big5,
            "shift-jis" | "shift_jis" | "sjis" | "cp932" => Self::ShiftJis,
            "euc-jp" | "eucjp" => Self::EucJp,
            "euc-kr" | "euckr" | "cp949" => Self::EucKr,
            "windows-874" | "cp874" => Self::Windows874,
            "windows-1250" | "cp1250" => Self::Windows1250,
            "windows-1251" | "cp1251" => Self::Windows1251,
            // ISO-8859-1 resolves to Windows-1252, as in the WHATWG standard.
            "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" => Self::Windows1252,
            "windows-1253" | "cp1253" => Self::Windows1253,
            "windows-1254" | "cp1254" => Self::Windows1254,
            "windows-1255" | "cp1255" => Self::Windows1255,
            "windows-1256" | "cp1256" => Self::Windows1256,
            "windows-1257" | "cp1257" => Self::Windows1257,
            "windows-1258" | "cp1258" => Self::Windows1258,
            "koi8-r" | "koi8r" => Self::Koi8R,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unsupported output encoding",
                )
                .with_details(format!(
                    "'{value}' is not one of: {}",
                    OUTPUT_ENCODINGS.map(Self::id).join(", ")
                )))
            }
        };
        Ok(encoding)
    }

    /// The encoding of a Windows code page (what `GetACP` returns), or `None` for a code page
    /// none of these encodes.
    pub fn from_code_page(code_page: u32) -> Option<Self> {
        Some(match code_page {
            65001 => Self::Utf8,
            936 => Self::Gbk,
            54936 => Self::Gb18030,
            950 => Self::Big5,
            932 => Self::ShiftJis,
            20932 | 51932 => Self::EucJp,
            949 | 51949 => Self::EucKr,
            874 => Self::Windows874,
            1250 => Self::Windows1250,
            1251 => Self::Windows1251,
            1252 | 28591 => Self::Windows1252,
            1253 => Self::Windows1253,
            1254 => Self::Windows1254,
            1255 => Self::Windows1255,
            1256 => Self::Windows1256,
            1257 => Self::Windows1257,
            1258 => Self::Windows1258,
            20866 => Self::Koi8R,
            _ => return None,
        })
    }

    fn codec(self) -> &'static Encoding {
        match self {
            Self::Utf8 => UTF_8,
            Self::Gbk => GBK,
            Self::Gb18030 => GB18030,
            Self::Big5 => BIG5,
            Self::ShiftJis => SHIFT_JIS,
            Self::EucJp => EUC_JP,
            Self::EucKr => EUC_KR,
            Self::Windows874 => WINDOWS_874,
            Self::Windows1250 => WINDOWS_1250,
            Self::Windows1251 => WINDOWS_1251,
            Self::Windows1252 => WINDOWS_1252,
            Self::Windows1253 => WINDOWS_1253,
            Self::Windows1254 => WINDOWS_1254,
            Self::Windows1255 => WINDOWS_1255,
            Self::Windows1256 => WINDOWS_1256,
            Self::Windows1257 => WINDOWS_1257,
            Self::Windows1258 => WINDOWS_1258,
            Self::Koi8R => KOI8_R,
        }
    }
}

/// How one console's bytes become text: the `encoding` of `outputStyle.decode`, and the request
/// of `outputEncoding.resolve`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputEncodingPolicy {
    /// `auto`, or the id of the one encoding every line is decoded with. Absent is `auto`.
    #[serde(default)]
    pub setting: Option<String>,
    /// The encoding the starter of the process declared it writes, tried first in `auto`.
    #[serde(default)]
    pub declared: Option<String>,
    /// The system code page of the host (Windows `GetACP`), tried after UTF-8 in `auto`. Hosts
    /// whose programs write UTF-8 to pipes leave it out.
    #[serde(default)]
    pub code_page: Option<u32>,
}

impl OutputEncodingPolicy {
    /// The encodings a line is tried in, in order, without repeats; never empty.
    ///
    /// # Errors
    ///
    /// `InvalidRequest` when `setting` or `declared` is not a supported id or alias.
    pub fn candidates(&self) -> Result<Vec<OutputEncoding>, CoreError> {
        if let Some(setting) = self.setting.as_deref() {
            if !setting.trim().eq_ignore_ascii_case("auto") {
                return Ok(vec![OutputEncoding::parse(setting)?]);
            }
        }
        let mut candidates = Vec::with_capacity(3);
        if let Some(declared) = self.declared.as_deref() {
            candidates.push(OutputEncoding::parse(declared)?);
        }
        candidates.push(OutputEncoding::Utf8);
        candidates.extend(self.code_page.and_then(OutputEncoding::from_code_page));
        let mut unique = Vec::with_capacity(candidates.len());
        for encoding in candidates {
            if !unique.contains(&encoding) {
                unique.push(encoding);
            }
        }
        Ok(unique)
    }
}

/// `outputEncoding.resolve` answer: what a policy decodes with, for a settings page.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolveOutputEncodingResponse {
    /// The ids a line is tried in, in order.
    pub candidates: Vec<&'static str>,
    /// Every id a setting may choose, in menu order.
    pub encodings: Vec<&'static str>,
}

/// `outputEncoding.resolve`: the encodings a policy tries, and every encoding it may choose.
///
/// # Errors
///
/// `InvalidRequest` when the policy names an unsupported encoding.
pub fn resolve_output_encoding(
    policy: OutputEncodingPolicy,
) -> Result<ResolveOutputEncodingResponse, CoreError> {
    Ok(ResolveOutputEncodingResponse {
        candidates: policy
            .candidates()?
            .into_iter()
            .map(OutputEncoding::id)
            .collect(),
        encodings: OUTPUT_ENCODINGS.map(OutputEncoding::id).to_vec(),
    })
}

/// Decodes one line's bytes, without its line ending, with the first candidate they are valid
/// in; when they are valid in none, with the candidate that replaces the fewest characters, the
/// earlier on a tie. Unless `last`, a character the bytes end inside is left out instead of
/// being replaced: the line is still being written.
pub(crate) fn decode_line(bytes: &[u8], candidates: &[OutputEncoding], last: bool) -> String {
    if bytes.is_ascii() {
        // Every candidate is ASCII-compatible.
        return bytes.iter().map(|&byte| char::from(byte)).collect();
    }
    for &encoding in candidates {
        if let Some(text) = decode_strict(encoding, bytes, last) {
            return text;
        }
    }
    let mut best: Option<(String, usize)> = None;
    for &encoding in candidates {
        let (text, replaced) = decode_lossy(encoding, bytes, last);
        if best.as_ref().is_none_or(|(_, fewest)| replaced < *fewest) {
            best = Some((text, replaced));
        }
    }
    best.map(|(text, _)| text).unwrap_or_default()
}

/// How much of the start of `bytes` ends on a character boundary in some candidate: all of
/// them, or up to the three bytes a character can still lack. When no candidate decodes even
/// that, all of them, since the line is replaced in any case.
pub(crate) fn complete_prefix(bytes: &[u8], candidates: &[OutputEncoding]) -> usize {
    (0..=3usize.min(bytes.len()))
        .map(|missing| bytes.len() - missing)
        .find(|&end| {
            candidates
                .iter()
                .any(|&encoding| decode_strict(encoding, &bytes[..end], true).is_some())
        })
        .unwrap_or(bytes.len())
}

/// `bytes` decoded with `encoding`, or `None` when they are not valid in it. Unless `last`, a
/// character they end inside is valid and left out.
fn decode_strict(encoding: OutputEncoding, bytes: &[u8], last: bool) -> Option<String> {
    if encoding == OutputEncoding::Utf8 {
        return match std::str::from_utf8(bytes) {
            Ok(text) => Some(text.to_owned()),
            Err(error) if !last && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()])
                    .ok()
                    .map(str::to_owned)
            }
            Err(_) => None,
        };
    }
    let mut decoder = encoding.codec().new_decoder_without_bom_handling();
    let mut text =
        String::with_capacity(decoder.max_utf8_buffer_length_without_replacement(bytes.len())?);
    match decoder.decode_to_string_without_replacement(bytes, &mut text, last) {
        (DecoderResult::InputEmpty, _) => Some(text),
        _ => None,
    }
}

/// `bytes` decoded with `encoding`, invalid sequences replaced by U+FFFD, and how many were
/// replaced. Unless `last`, a character they end inside is left out.
fn decode_lossy(encoding: OutputEncoding, bytes: &[u8], last: bool) -> (String, usize) {
    let mut decoder = encoding.codec().new_decoder_without_bom_handling();
    let mut text = String::new();
    let mut rest = bytes;
    let mut replaced = false;
    loop {
        text.reserve(
            decoder
                .max_utf8_buffer_length(rest.len())
                .unwrap_or(rest.len()),
        );
        let (result, read, replaced_now) = decoder.decode_to_string(rest, &mut text, last);
        replaced |= replaced_now;
        rest = &rest[read..];
        if result == CoderResult::InputEmpty {
            break;
        }
    }
    let count = if replaced {
        text.matches('\u{fffd}').count()
    } else {
        0
    };
    (text, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gbk(text: &str) -> Vec<u8> {
        GBK.encode(text).0.into_owned()
    }

    fn auto(code_page: Option<u32>) -> Vec<OutputEncoding> {
        OutputEncodingPolicy {
            code_page,
            ..OutputEncodingPolicy::default()
        }
        .candidates()
        .unwrap()
    }

    #[test]
    fn utf8_lines_stay_utf8_and_others_fall_back_to_the_code_page() {
        let candidates = auto(Some(936));
        assert_eq!(candidates, [OutputEncoding::Utf8, OutputEncoding::Gbk]);
        assert_eq!(
            decode_line("构建成功".as_bytes(), &candidates, true),
            "构建成功"
        );
        assert_eq!(decode_line(&gbk("构建成功"), &candidates, true), "构建成功");
    }

    #[test]
    fn without_a_code_page_invalid_utf8_is_replaced() {
        assert_eq!(
            decode_line(&gbk("中"), &auto(None), true),
            "\u{fffd}\u{fffd}"
        );
    }

    #[test]
    fn a_declared_encoding_comes_first_and_a_setting_is_the_only_one() {
        let declared = OutputEncodingPolicy {
            declared: Some("Shift_JIS".to_owned()),
            code_page: Some(65001),
            ..OutputEncodingPolicy::default()
        };
        assert_eq!(
            declared.candidates().unwrap(),
            [OutputEncoding::ShiftJis, OutputEncoding::Utf8]
        );
        let fixed = OutputEncodingPolicy {
            setting: Some("big5".to_owned()),
            ..declared
        };
        assert_eq!(fixed.candidates().unwrap(), [OutputEncoding::Big5]);
        let unknown = OutputEncodingPolicy {
            setting: Some("utf-16le".to_owned()),
            ..OutputEncodingPolicy::default()
        };
        assert!(matches!(
            unknown.candidates().unwrap_err().code,
            ErrorCode::InvalidRequest
        ));
    }

    #[test]
    fn an_unfinished_character_is_left_out_until_the_line_ends() {
        let bytes = gbk("中文");
        let candidates = auto(Some(936));
        assert_eq!(decode_line(&bytes[..3], &candidates, false), "中");
        assert_eq!(complete_prefix(&bytes[..3], &candidates), 2);
        assert_eq!(complete_prefix(&bytes, &candidates), 4);
    }

    #[test]
    fn a_line_valid_in_no_candidate_takes_the_one_that_replaces_least() {
        // GBK text with one stray byte: UTF-8 would replace every character.
        let mut bytes = gbk("编译错误");
        bytes.push(0xff);
        assert_eq!(
            decode_line(&bytes, &auto(Some(936)), true),
            "编译错误\u{fffd}"
        );
    }

    #[test]
    fn every_id_parses_back_and_known_code_pages_map() {
        for encoding in OUTPUT_ENCODINGS {
            assert_eq!(OutputEncoding::parse(encoding.id()).unwrap(), encoding);
        }
        assert_eq!(
            OutputEncoding::from_code_page(54936),
            Some(OutputEncoding::Gb18030)
        );
        assert_eq!(OutputEncoding::from_code_page(437), None);
    }
}
