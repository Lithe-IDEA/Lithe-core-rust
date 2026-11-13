//! Native paths and percent-decoded text named by LSP file URIs.

/// Decodes a file URI using native drive/UNC syntax; opaque inputs retain
/// their percent-decoded spelling for existing non-file consumers.
pub fn file_path_from_uri(uri: &str) -> String {
    file_path_for_uri(uri).unwrap_or_else(|| percent_decode(uri))
}

/// Native path of a `file:` URI; `None` for any other scheme, which callers
/// treat as a read-only location.
pub fn file_path_for_uri(uri: &str) -> Option<String> {
    let (scheme, _) = uri.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    // Workspace edits already decode percent escapes, UNC authorities and the
    // leading slash of file:///C:/... without depending on the build host OS.
    native_path(uri)
}

/// Decodes `%XX` escapes in a URI component, keeping the input when the
/// decoded bytes are not UTF-8.
pub fn percent_decode(value: &str) -> String {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

/// Native path named by an LSP document or workspace-edit key; `None` for a
/// non-file URI. Shared with navigation results and diagnostic placement.
/// Core's URI projection keeps the leading slash of `file:///C:/...`; strip it
/// so drive paths parse as Windows paths.
pub fn native_path(key: &str) -> Option<String> {
    let path = match key.split_once(':') {
        // A one-letter "scheme" is a drive letter, not a URI scheme.
        Some((scheme, rest))
            if scheme.len() > 1
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            if !scheme.eq_ignore_ascii_case("file") {
                return None;
            }
            let rest = rest.strip_prefix("//").unwrap_or(rest);
            // `file:///p` leaves `/p`; `file://host/share` leaves `host/share`.
            if rest.starts_with('/') {
                percent_decode(rest)
            } else {
                format!("//{}", percent_decode(rest))
            }
        }
        _ => key.to_string(),
    };
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return Some(path[1..].to_string());
    }
    Some(path)
}

/// Value of one ASCII hexadecimal digit, either case.
pub fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::file_path_from_uri;

    #[test]
    fn file_uri_paths_decode_spaces_and_utf8_characters() {
        assert_eq!(
            file_path_from_uri("file:///C:/work/Main.java"),
            "C:/work/Main.java"
        );
        assert_eq!(
            file_path_from_uri("file:///C%3A/work/A%20B.java"),
            "C:/work/A B.java"
        );
        assert_eq!(
            file_path_from_uri("file:///tmp/go%20project/%E4%B8%AD%E6%96%87/main.go"),
            "/tmp/go project/中文/main.go"
        );
        assert_eq!(
            file_path_from_uri("file://server/share/Main.java"),
            "//server/share/Main.java"
        );
    }
}
