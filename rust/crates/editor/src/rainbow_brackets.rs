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
    fn kind(self) -> usize {
        match self {
            Self::OpenParen | Self::CloseParen => 0,
            Self::OpenSquare | Self::CloseSquare => 1,
            Self::OpenBrace | Self::CloseBrace => 2,
        }
    }

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
/// A closer pairs with the innermost opener of its kind, discarding intervening
/// unclosed openers from the active stack. Discarded openers retain their color
/// without a match; a closer with no opener of its kind is omitted.
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
    let mut open = Vec::new();
    let mut open_by_kind = [0_usize; 3];
    let mut kinds = Vec::with_capacity(request.brackets.len());
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

        let kind = token.bracket.kind();
        let (depth, match_offset) = if token.bracket.closing().is_some() {
            open.push(colors.len());
            open_by_kind[kind] += 1;
            (open.len() - 1, None)
        } else if open_by_kind[kind] > 0 {
            // Counts avoid searching an unmatched stack. Each opener is popped
            // at most once, so malformed edits retain the linear token budget.
            let opener = loop {
                let innermost = open.pop().expect("an opener of this kind remains");
                open_by_kind[kinds[innermost]] -= 1;
                if kinds[innermost] == kind {
                    break innermost;
                }
            };
            colors[opener].match_offset = Some(token.offset);
            (open.len(), Some(colors[opener].offset))
        } else {
            continue;
        };
        kinds.push(kind);
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
    fn mismatched_inner_openers_do_not_leak_depth_into_later_pairs() {
        let result = color_command(json!({"brackets":[
            {"offset":0,"bracket":"{"}, {"offset":5,"bracket":"("},
            {"offset":7,"bracket":"}"}, {"offset":9,"bracket":"("},
            {"offset":10,"bracket":")"}
        ]}))
        .unwrap();
        assert_eq!(
            result["colors"],
            json!([
                {"offset":0,"depth":0,"matchOffset":7}, {"offset":5,"depth":1},
                {"offset":7,"depth":0,"matchOffset":0}, {"offset":9,"depth":0,"matchOffset":10},
                {"offset":10,"depth":0,"matchOffset":9}
            ])
        );
    }

    #[test]
    fn repeated_missing_closers_leave_deep_openers_for_the_matching_kind() {
        let mut brackets: Vec<Value> = (0..5000)
            .map(|offset| json!({"offset":offset,"bracket":"("}))
            .collect();
        brackets.extend((5000..9999).map(|offset| json!({"offset":offset,"bracket":"]"})));
        brackets.push(json!({"offset":9999,"bracket":")"}));
        let result = color_command(json!({"brackets":brackets})).unwrap();
        let colors = result["colors"].as_array().unwrap();
        assert_eq!(colors.len(), 5001);
        assert_eq!(colors[4999]["matchOffset"], 9999);
        assert_eq!(colors[5000]["depth"], 4999);
    }

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
