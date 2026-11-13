//! Shared document text-encoding rules for `file.read` and `file.write`.
//!
//! Both native clients keep editor buffers as Unicode and only convert bytes at
//! the file boundary. The catalog, BOM handling, automatic detection and
//! strict/lossy decoding live here so macOS and Windows agree byte for byte.
//! Codec tables come from `encoding_rs` (WHATWG Encoding Standard).

use encoding_rs::{
    Encoding, BIG5, EUC_JP, EUC_KR, GB18030, GBK, KOI8_R, SHIFT_JIS, UTF_16BE, UTF_16LE, UTF_8,
    WINDOWS_1250, WINDOWS_1251, WINDOWS_1252,
};
use sha2::{Digest, Sha256};

use crate::protocol::{CoreError, ErrorCode};

/// UTF-8 byte order mark.
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// UTF-16 little-endian byte order mark.
const UTF16_LE_BOM: [u8; 2] = [0xFF, 0xFE];
/// UTF-16 big-endian byte order mark.
const UTF16_BE_BOM: [u8; 2] = [0xFE, 0xFF];
/// UTF-32 LE mark; it starts with the UTF-16 LE one and must not be mistaken for it.
const UTF32_LE_BOM: [u8; 4] = [0xFF, 0xFE, 0x00, 0x00];

/// A document encoding selectable by clients, identified by a stable string id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DocumentEncoding {
    Utf8,
    Utf8Bom,
    Utf16LeBom,
    Utf16BeBom,
    Utf16Le,
    Utf16Be,
    Gbk,
    Gb18030,
    Big5,
    ShiftJis,
    EucJp,
    EucKr,
    Windows1250,
    Windows1251,
    Windows1252,
    Koi8R,
}

/// Every supported encoding in catalog order.
pub(crate) const ALL_ENCODINGS: [DocumentEncoding; 16] = [
    DocumentEncoding::Utf8,
    DocumentEncoding::Utf8Bom,
    DocumentEncoding::Utf16LeBom,
    DocumentEncoding::Utf16BeBom,
    DocumentEncoding::Utf16Le,
    DocumentEncoding::Utf16Be,
    DocumentEncoding::Gbk,
    DocumentEncoding::Gb18030,
    DocumentEncoding::Big5,
    DocumentEncoding::ShiftJis,
    DocumentEncoding::EucJp,
    DocumentEncoding::EucKr,
    DocumentEncoding::Windows1250,
    DocumentEncoding::Windows1251,
    DocumentEncoding::Windows1252,
    DocumentEncoding::Koi8R,
];

/// Result of decoding a file's bytes.
pub(crate) struct DecodedDocument {
    pub(crate) text: String,
    pub(crate) encoding: DocumentEncoding,
    /// A leading BOM was present and removed from `text`.
    pub(crate) had_bom: bool,
    /// Invalid bytes were replaced with U+FFFD (only for an explicit encoding).
    pub(crate) lossy: bool,
}

impl DocumentEncoding {
    /// Stable identifier exchanged with clients.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf8Bom => "utf-8-bom",
            Self::Utf16LeBom => "utf-16le-bom",
            Self::Utf16BeBom => "utf-16be-bom",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
            Self::Gbk => "gbk",
            Self::Gb18030 => "gb18030",
            Self::Big5 => "big5",
            Self::ShiftJis => "shift-jis",
            Self::EucJp => "euc-jp",
            Self::EucKr => "euc-kr",
            Self::Windows1250 => "windows-1250",
            Self::Windows1251 => "windows-1251",
            Self::Windows1252 => "windows-1252",
            Self::Koi8R => "koi8-r",
        }
    }

    /// Parses a stable id or one of the accepted aliases, case-insensitively.
    pub(crate) fn parse(value: &str) -> Result<Self, CoreError> {
        let normalized = value.trim().to_ascii_lowercase();
        let encoding = match normalized.as_str() {
            "utf-8" | "utf8" => Self::Utf8,
            "utf-8-bom" | "utf8-bom" | "utf-8 with bom" => Self::Utf8Bom,
            "utf-16le-bom" | "utf16le-bom" | "utf-16 le with bom" => Self::Utf16LeBom,
            "utf-16be-bom" | "utf16be-bom" | "utf-16 be with bom" => Self::Utf16BeBom,
            "utf-16le" | "utf16le" | "utf-16 le" => Self::Utf16Le,
            "utf-16be" | "utf16be" | "utf-16 be" => Self::Utf16Be,
            "gbk" | "cp936" => Self::Gbk,
            "gb18030" => Self::Gb18030,
            "big5" | "cp950" => Self::Big5,
            "shift-jis" | "shift_jis" | "shift jis" | "sjis" => Self::ShiftJis,
            "euc-jp" | "eucjp" => Self::EucJp,
            "euc-kr" | "euckr" | "cp949" => Self::EucKr,
            "windows-1250" | "cp1250" => Self::Windows1250,
            "windows-1251" | "cp1251" => Self::Windows1251,
            // ISO-8859-1 resolves to Windows-1252, as in the WHATWG standard.
            "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" => Self::Windows1252,
            "koi8-r" | "koi8r" => Self::Koi8R,
            _ => {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unsupported document encoding",
                )
                .with_details(format!(
                    "'{value}' is not one of: {}",
                    ALL_ENCODINGS.map(Self::id).join(", ")
                )))
            }
        };
        Ok(encoding)
    }

    fn codec(self) -> &'static Encoding {
        match self {
            Self::Utf8 | Self::Utf8Bom => UTF_8,
            Self::Utf16LeBom | Self::Utf16Le => UTF_16LE,
            Self::Utf16BeBom | Self::Utf16Be => UTF_16BE,
            Self::Gbk => GBK,
            Self::Gb18030 => GB18030,
            Self::Big5 => BIG5,
            Self::ShiftJis => SHIFT_JIS,
            Self::EucJp => EUC_JP,
            Self::EucKr => EUC_KR,
            Self::Windows1250 => WINDOWS_1250,
            Self::Windows1251 => WINDOWS_1251,
            Self::Windows1252 => WINDOWS_1252,
            Self::Koi8R => KOI8_R,
        }
    }

    /// The BOM this encoding writes and strips. Plain UTF-8 writes none but
    /// still tolerates one when reading, so reopening a BOM file as UTF-8 does
    /// not put U+FEFF in the buffer.
    fn bom(self) -> &'static [u8] {
        match self {
            Self::Utf8 | Self::Utf8Bom => &UTF8_BOM,
            Self::Utf16LeBom => &UTF16_LE_BOM,
            Self::Utf16BeBom => &UTF16_BE_BOM,
            _ => &[],
        }
    }

    /// Whether a BOM can accompany this encoding at all.
    fn supports_bom(self) -> bool {
        matches!(
            self,
            Self::Utf8
                | Self::Utf8Bom
                | Self::Utf16LeBom
                | Self::Utf16BeBom
                | Self::Utf16Le
                | Self::Utf16Be
        )
    }

    /// The BOM bytes to write for an explicit `bom` override.
    fn bom_for_write(self) -> &'static [u8] {
        match self {
            Self::Utf8 | Self::Utf8Bom => &UTF8_BOM,
            Self::Utf16LeBom | Self::Utf16Le => &UTF16_LE_BOM,
            Self::Utf16BeBom | Self::Utf16Be => &UTF16_BE_BOM,
            _ => &[],
        }
    }

    fn is_utf16(self) -> bool {
        matches!(
            self,
            Self::Utf16LeBom | Self::Utf16BeBom | Self::Utf16Le | Self::Utf16Be
        )
    }
}

/// SHA-256 of the exact bytes, as lowercase hex; used for external-change guards.
pub(crate) fn bytes_identity(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Picks an encoding for bytes read without an explicit choice.
///
/// Order: UTF-8 BOM, UTF-16 LE BOM (unless it is really a UTF-32 LE mark),
/// UTF-16 BE BOM, valid UTF-8, then GB18030 narrowed to GBK when the text
/// re-encodes to the same bytes. Anything else is not guessed.
fn detect(bytes: &[u8]) -> Result<DocumentEncoding, CoreError> {
    if bytes.starts_with(&UTF8_BOM) {
        return Ok(DocumentEncoding::Utf8Bom);
    }
    if bytes.starts_with(&UTF16_LE_BOM) && !bytes.starts_with(&UTF32_LE_BOM) {
        return Ok(DocumentEncoding::Utf16LeBom);
    }
    if bytes.starts_with(&UTF16_BE_BOM) {
        return Ok(DocumentEncoding::Utf16BeBom);
    }
    if std::str::from_utf8(bytes).is_ok() {
        return Ok(DocumentEncoding::Utf8);
    }
    let (text, had_errors) = {
        let (text, errors) = GB18030.decode_without_bom_handling(bytes);
        (text, errors)
    };
    if had_errors {
        return Err(
            CoreError::new(ErrorCode::ParseFailed, "File is not valid UTF-8")
                .with_details("No BOM was present and the bytes are not valid UTF-8 or GB18030"),
        );
    }
    // GB18030 is a superset of GBK; keep GB18030 when GBK cannot reproduce the bytes.
    Ok(match encode_bytes(DocumentEncoding::Gbk, &text) {
        Ok(reencoded) if reencoded == bytes => DocumentEncoding::Gbk,
        _ => DocumentEncoding::Gb18030,
    })
}

/// Decodes `bytes`, detecting the encoding when none is requested.
///
/// Automatic detection is strict: invalid bytes fail with `parse_failed`. An
/// explicit encoding is a request to inspect the bytes through that codec, so
/// invalid sequences become U+FFFD and `lossy` is set.
pub(crate) fn decode(
    bytes: &[u8],
    requested: Option<DocumentEncoding>,
) -> Result<DecodedDocument, CoreError> {
    let selected = match requested {
        Some(encoding) => encoding,
        None => detect(bytes)?,
    };
    let bom = selected.bom();
    let had_bom = !bom.is_empty() && bytes.starts_with(bom);
    let payload = if had_bom { &bytes[bom.len()..] } else { bytes };
    let (text, had_errors) = selected.codec().decode_without_bom_handling(payload);
    if had_errors && requested.is_none() {
        return Err(
            CoreError::new(ErrorCode::ParseFailed, "File is not valid UTF-8").with_details(
                format!(
                    "The bytes are not valid {}",
                    selected.id().to_ascii_uppercase()
                ),
            ),
        );
    }
    Ok(DecodedDocument {
        text: text.into_owned(),
        encoding: selected,
        had_bom,
        lossy: had_errors,
    })
}

/// Encodes `text` for writing.
///
/// `bom` overrides the encoding's own BOM choice for the Unicode encodings;
/// requesting a BOM for a legacy encoding is `invalid_request`. Characters the
/// target encoding cannot represent fail with `encoding_unmappable` and are
/// never substituted.
pub(crate) fn encode(
    text: &str,
    encoding: DocumentEncoding,
    bom: Option<bool>,
) -> Result<Vec<u8>, CoreError> {
    let write_bom = match bom {
        Some(true) if !encoding.supports_bom() => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "This encoding cannot carry a byte order mark",
            )
            .with_details(encoding.id()));
        }
        Some(flag) => flag,
        None => !encoding.bom().is_empty() && encoding != DocumentEncoding::Utf8,
    };
    let mut bytes = if write_bom {
        encoding.bom_for_write().to_vec()
    } else {
        Vec::new()
    };
    bytes.extend(encode_bytes(encoding, text)?);
    Ok(bytes)
}

fn encode_bytes(encoding: DocumentEncoding, text: &str) -> Result<Vec<u8>, CoreError> {
    // encoding_rs only decodes UTF-16; its encoder falls back to UTF-8, so the
    // code units are serialized here.
    if encoding.is_utf16() {
        let little = matches!(
            encoding,
            DocumentEncoding::Utf16Le | DocumentEncoding::Utf16LeBom
        );
        return Ok(text
            .encode_utf16()
            .flat_map(|unit| {
                if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect());
    }
    let (encoded, _, had_errors) = encoding.codec().encode(text);
    if had_errors {
        return Err(unmappable_error(encoding, text));
    }
    Ok(encoded.into_owned())
}

/// Locates the first character `encoding` cannot represent for the error details.
fn unmappable_error(encoding: DocumentEncoding, text: &str) -> CoreError {
    let mut buffer = [0_u8; 4];
    let offending = text.chars().enumerate().find(|(_, character)| {
        let (_, _, failed) = encoding.codec().encode(character.encode_utf8(&mut buffer));
        failed
    });
    let details = match offending {
        Some((offset, character)) => format!(
            "encoding={}; scalarOffset={offset}; codePoint=U+{:04X}",
            encoding.id(),
            character as u32
        ),
        None => format!("encoding={}", encoding.id()),
    };
    CoreError::new(
        ErrorCode::EncodingUnmappable,
        "Text contains characters unavailable in the selected encoding",
    )
    .with_details(details)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_auto(bytes: &[u8]) -> DecodedDocument {
        decode(bytes, None).expect("bytes should decode")
    }

    #[test]
    fn aliases_resolve_and_unknown_ids_are_rejected() {
        for (alias, id) in [
            ("UTF8", "utf-8"),
            ("cp936", "gbk"),
            ("Shift_JIS", "shift-jis"),
            ("latin1", "windows-1252"),
            ("ISO-8859-1", "windows-1252"),
            ("CP949", "euc-kr"),
        ] {
            assert_eq!(DocumentEncoding::parse(alias).unwrap().id(), id);
        }
        assert!(matches!(
            DocumentEncoding::parse("ebcdic").unwrap_err().code,
            ErrorCode::InvalidRequest
        ));
        for encoding in ALL_ENCODINGS {
            assert_eq!(DocumentEncoding::parse(encoding.id()).unwrap(), encoding);
        }
    }

    #[test]
    fn detection_follows_boms_utf8_then_gbk_and_gb18030() {
        let utf8_bom = decode_auto(&[0xEF, 0xBB, 0xBF, b'a']);
        assert_eq!(
            (utf8_bom.encoding, utf8_bom.had_bom, utf8_bom.text.as_str()),
            (DocumentEncoding::Utf8Bom, true, "a")
        );
        let le = decode_auto(&[0xFF, 0xFE, 0x41, 0x00]);
        assert_eq!(
            (le.encoding, le.text.as_str()),
            (DocumentEncoding::Utf16LeBom, "A")
        );
        let be = decode_auto(&[0xFE, 0xFF, 0x00, 0x41]);
        assert_eq!(
            (be.encoding, be.text.as_str()),
            (DocumentEncoding::Utf16BeBom, "A")
        );
        assert_eq!(
            decode_auto("中文".as_bytes()).encoding,
            DocumentEncoding::Utf8
        );
        // The two characters for "Chinese text" in GBK.
        let gbk = decode_auto(&[0xD6, 0xD0, 0xCE, 0xC4]);
        assert_eq!(
            (gbk.encoding, gbk.text.as_str()),
            (DocumentEncoding::Gbk, "中文")
        );
        // A four-byte GB18030 sequence (U+20000) keeps GB18030.
        let gb18030 = decode_auto(&[0x95, 0x32, 0x82, 0x36]);
        assert_eq!(gb18030.encoding, DocumentEncoding::Gb18030);
        assert_eq!(gb18030.text, "\u{20000}");
    }

    #[test]
    fn utf32_le_bom_is_not_decoded_as_utf16() {
        let bytes = [0xFF, 0xFE, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00];
        assert_ne!(detect(&bytes).ok(), Some(DocumentEncoding::Utf16LeBom));
    }

    #[test]
    fn automatic_decoding_is_strict_and_explicit_decoding_is_lossy() {
        assert!(
            decode(&[0xFF, 0xFE, 0x41], None).is_err(),
            "odd UTF-16 length"
        );
        assert!(
            decode(&[0xFF, 0xFE, 0x00, 0xD8], None).is_err(),
            "lone surrogate"
        );
        let repaired = decode(&[0x41, 0x00, 0x00, 0xD8], Some(DocumentEncoding::Utf16Le)).unwrap();
        assert!(repaired.lossy);
        assert_eq!(repaired.text, "A\u{FFFD}");
        // Reopening a BOM file as plain UTF-8 drops the mark.
        let reopened = decode(&[0xEF, 0xBB, 0xBF, b'x'], Some(DocumentEncoding::Utf8)).unwrap();
        assert_eq!((reopened.text.as_str(), reopened.had_bom), ("x", true));
    }

    #[test]
    fn encoding_round_trips_every_catalog_entry() {
        let samples = [
            (DocumentEncoding::Utf8, "A中😀"),
            (DocumentEncoding::Utf8Bom, "A中😀"),
            (DocumentEncoding::Utf16LeBom, "A中😀"),
            (DocumentEncoding::Utf16BeBom, "A中😀"),
            (DocumentEncoding::Utf16Le, "A中😀"),
            (DocumentEncoding::Utf16Be, "A中😀"),
            (DocumentEncoding::Gbk, "A中文"),
            (DocumentEncoding::Gb18030, "A中\u{20000}"),
            (DocumentEncoding::Big5, "A中文"),
            (DocumentEncoding::ShiftJis, "Aこんにちは"),
            (DocumentEncoding::EucJp, "Aこんにちは"),
            (DocumentEncoding::EucKr, "A한국어"),
            (DocumentEncoding::Windows1250, "Aąęł"),
            (DocumentEncoding::Windows1251, "Aпривет"),
            (DocumentEncoding::Windows1252, "Aéü"),
            (DocumentEncoding::Koi8R, "Aпривет"),
        ];
        assert_eq!(samples.len(), ALL_ENCODINGS.len());
        for (encoding, text) in samples {
            let bytes = encode(text, encoding, None).unwrap();
            let decoded = decode(&bytes, Some(encoding)).unwrap();
            assert_eq!(decoded.text, text, "{}", encoding.id());
            assert!(!decoded.lossy, "{}", encoding.id());
        }
    }

    #[test]
    fn known_byte_sequences_match_the_native_clients() {
        assert_eq!(
            encode("中文", DocumentEncoding::Big5, None).unwrap(),
            [0xA4, 0xA4, 0xA4, 0xE5]
        );
        assert_eq!(
            encode("A中", DocumentEncoding::Utf16Le, None).unwrap(),
            [0x41, 0x00, 0x2D, 0x4E]
        );
        assert_eq!(
            encode("A中", DocumentEncoding::Utf16Be, None).unwrap(),
            [0x00, 0x41, 0x4E, 0x2D]
        );
        assert_eq!(
            encode("A", DocumentEncoding::Utf16LeBom, None).unwrap(),
            [0xFF, 0xFE, 0x41, 0x00]
        );
        assert_eq!(
            encode("a", DocumentEncoding::Utf8Bom, None).unwrap(),
            [0xEF, 0xBB, 0xBF, b'a']
        );
    }

    #[test]
    fn bom_override_applies_only_to_unicode_encodings() {
        assert_eq!(
            encode("a", DocumentEncoding::Utf8, Some(true)).unwrap(),
            [0xEF, 0xBB, 0xBF, b'a']
        );
        assert_eq!(
            encode("a", DocumentEncoding::Utf8Bom, Some(false)).unwrap(),
            [b'a']
        );
        assert_eq!(
            encode("a", DocumentEncoding::Utf16Be, Some(true)).unwrap(),
            [0xFE, 0xFF, 0x00, 0x61]
        );
        assert!(matches!(
            encode("a", DocumentEncoding::Gbk, Some(true))
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        ));
    }

    #[test]
    fn unmappable_characters_report_a_structured_error() {
        for encoding in [
            DocumentEncoding::Gbk,
            DocumentEncoding::Big5,
            DocumentEncoding::Windows1252,
            DocumentEncoding::ShiftJis,
        ] {
            let error = encode("ab😀", encoding, None).unwrap_err();
            let json = serde_json::to_value(&error).unwrap();
            assert_eq!(json["code"], "encoding_unmappable", "{}", encoding.id());
            assert!(json["details"]
                .as_str()
                .unwrap()
                .contains("scalarOffset=2; codePoint=U+1F600"));
        }
        // GB18030 and the Unicode encodings represent every scalar.
        assert!(encode("😀", DocumentEncoding::Gb18030, None).is_ok());
    }
}
