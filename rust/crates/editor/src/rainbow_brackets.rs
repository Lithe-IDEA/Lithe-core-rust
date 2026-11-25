//! Matching pairs and nesting depths for structural brackets identified by the host's tokenizer.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Maximum number of bracket tokens and colors in one request.
pub const MAX_BRACKETS: usize = 10_000;

#[derive(Clone, Copy, Deserialize, PartialEq)]
enum Bracket {
    #[serde(rename = "(")]
    OpenParen,
    #[serde(rename = ")")]
    CloseParen,
    #[serde(rename = "[")]
    OpenSquare,
    #[serde(rename = "]")]
    CloseSquare,
    #[serde(rename = "{")]
    OpenBrace,
    #[serde(rename = "}")]
    CloseBrace,
}

impl Bracket {
    fn closing(self) -> Option<Self> {
        match self {
            Self::OpenParen => Some(Self::CloseParen),
            Self::OpenSquare => Some(Self::CloseSquare),
            Self::OpenBrace => Some(Self::CloseBrace),
            Self::CloseParen | Self::CloseSquare | Self::CloseBrace => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    /// Zero-based UTF-8 byte offset into the host-owned document snapshot.
    offset: u32,
    bracket: Bracket,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    /// Structural tokens in strictly increasing byte offset order.
    brackets: Vec<Token>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Color {
    offset: u32,
    /// Zero-based nesting depth, before wrapping to the active palette size.
    depth: u32,
    /// Byte offset of the other end, absent until the bracket has a partner.
    #[serde(skip_serializing_if = "Option::is_none")]
    match_offset: Option<u32>,
}

/// Returns both endpoints of each matched pair at the same nesting depth.
/// A mismatched closer is omitted and leaves the stack available for a later
/// match. Openers retain their current depth while the document is incomplete.
pub fn color_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "Invalid rainbow bracket tokens"))?;
    if request.brackets.len() > MAX_BRACKETS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Rainbow bracket tokens exceed 10000 entries",
        ));
    }

    let mut previous_offset = None;
    let mut expected_closers = Vec::new();
    let mut colors: Vec<Color> = Vec::with_capacity(request.brackets.len());
    for token in request.brackets {
        // The host uses a one-byte range, so its exclusive end must fit too.
        if token.offset == u32::MAX || previous_offset.is_some_and(|offset| token.offset <= offset)
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Rainbow bracket offsets must be strictly increasing with a representable end",
            ));
        }
        previous_offset = Some(token.offset);

        let (depth, match_offset) = if let Some(closing) = token.bracket.closing() {
            let depth = expected_closers.len();
            expected_closers.push((closing, colors.len()));
            (depth, None)
        } else if let Some(&(_, opener)) = expected_closers
            .last()
            .filter(|(closing, _)| *closing == token.bracket)
        {
            expected_closers.pop();
            colors[opener].match_offset = Some(token.offset);
            (expected_closers.len(), Some(colors[opener].offset))
        } else {
            continue;
        };
        colors.push(Color {
            offset: token.offset,
            depth: depth as u32,
            match_offset,
        });
    }
    Ok(json!({ "colors": colors }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_token_limit_and_rejects_the_next_token() {
        let mut brackets: Vec<Value> = (0..MAX_BRACKETS)
            .map(|offset| json!({"offset": offset, "bracket": "("}))
            .collect();
        let result = color_command(json!({"brackets": &brackets})).unwrap();
        let colors = result["colors"].as_array().unwrap();
        assert_eq!(colors.len(), MAX_BRACKETS);
        assert_eq!(colors[MAX_BRACKETS - 1]["depth"], MAX_BRACKETS - 1);

        brackets.push(json!({"offset": MAX_BRACKETS, "bracket": "("}));
        let error = color_command(json!({"brackets": brackets})).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }
}
