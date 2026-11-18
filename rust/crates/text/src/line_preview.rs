//! How a result row shows a source line with highlighted fragments: the Find
//! in Files and Search Everywhere content matches and the References tool
//! window's usages all apply this one rule.
//!
//! The row drops the line's indentation and line break. When the first
//! highlight lies far to the right, the front of the line is replaced by an
//! ellipsis so the highlight is not lost past the row's end. Highlights are
//! UTF-16 code units, the unit the platforms measure text in.

use serde::{Deserialize, Serialize};

/// UTF-16 units of a line kept in front of its first highlight; a highlight
/// further right makes the preview cut the line, keeping half of this lead.
pub const LINE_PREVIEW_LEAD: usize = 32;

/// The ellipsis that replaces the cut front of a line. It is one UTF-16 unit.
const ELLIPSIS: &str = "…";

/// One half-open highlighted fragment of a line, in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Utf16Span {
    /// First highlighted UTF-16 code unit.
    pub start: usize,
    /// Highlighted length in UTF-16 code units.
    pub length: usize,
}

/// A line as a result row shows it, with its highlights moved into the shown
/// text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinePreview {
    /// The shown text.
    pub text: String,
    /// The highlights inside `text`, ordered by start, each non-empty.
    pub ranges: Vec<Utf16Span>,
}

/// Returns the preview of `line` with `highlights` given in UTF-16 units of
/// `line`.
///
/// Leading spaces and tabs and the trailing line break are dropped, with the
/// highlights in the indentation. When the first highlight starts more than
/// [`LINE_PREVIEW_LEAD`] units into what is left, the text in front of it is
/// cut at a character boundary and replaced by an ellipsis, keeping half the
/// lead before the highlight; highlights in the cut part are dropped.
/// `max_bytes` then bounds the text at a character boundary. Highlights that
/// run past the shown text are shortened to it, and empty ones are dropped.
pub fn line_preview(line: &str, highlights: &[Utf16Span], max_bytes: Option<usize>) -> LinePreview {
    let indented = line.trim_start_matches([' ', '\t']);
    // Spaces and tabs are one UTF-16 unit each.
    let removed = line.len() - indented.len();
    let body = indented.trim_end_matches(['\r', '\n']);
    let mut ranges: Vec<Utf16Span> = highlights
        .iter()
        .filter(|span| span.length > 0 && span.start >= removed)
        .map(|span| Utf16Span {
            start: span.start - removed,
            length: span.length,
        })
        .collect();
    ranges.sort_by_key(|span| (span.start, span.length));

    let (prefix, offset) = match ranges.first() {
        Some(first) if first.start > LINE_PREVIEW_LEAD => {
            let cut = first.start - LINE_PREVIEW_LEAD / 2;
            // Walk to the first character boundary at or past unit `cut`, so a
            // surrogate pair is never split.
            let (mut units, mut offset) = (0, body.len());
            for (index, character) in body.char_indices() {
                if units >= cut {
                    offset = index;
                    break;
                }
                units += character.len_utf16();
            }
            ranges.retain(|span| span.start >= units);
            for span in &mut ranges {
                span.start = span.start - units + ELLIPSIS.encode_utf16().count();
            }
            (ELLIPSIS, offset)
        }
        _ => ("", 0),
    };

    let rest = &body[offset..];
    let end = match max_bytes {
        Some(max) if prefix.len() + rest.len() > max => {
            floor_char_boundary(rest, max.saturating_sub(prefix.len()))
        }
        _ => rest.len(),
    };
    let mut text = String::with_capacity(prefix.len() + end);
    text.push_str(prefix);
    text.push_str(&rest[..end]);

    let shown = text.encode_utf16().count();
    ranges.retain(|span| span.start < shown);
    for span in &mut ranges {
        span.length = span.length.min(shown - span.start);
    }
    LinePreview { text, ranges }
}

/// The largest character boundary of `text` at or before byte `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, length: usize) -> Utf16Span {
        Utf16Span { start, length }
    }

    #[test]
    fn drops_the_indentation_and_the_line_break_with_the_highlights_in_them() {
        let preview = line_preview("\t  let x = 1;\r\n", &[span(0, 1), span(7, 1)], None);
        assert_eq!(preview.text, "let x = 1;");
        assert_eq!(preview.ranges, vec![span(4, 1)]);
    }

    #[test]
    fn keeps_a_near_highlight_and_cuts_in_front_of_a_far_one() {
        let near = format!("{}hit", "a".repeat(LINE_PREVIEW_LEAD));
        assert_eq!(
            line_preview(&near, &[span(LINE_PREVIEW_LEAD, 3)], None),
            LinePreview {
                text: near.clone(),
                ranges: vec![span(LINE_PREVIEW_LEAD, 3)]
            }
        );

        let far = format!("{}hit tail", "a".repeat(40));
        let preview = line_preview(&far, &[span(44, 4), span(40, 3)], None);
        // Sixteen units stay in front of the first highlight, after the ellipsis.
        assert_eq!(preview.text, format!("…{}hit tail", "a".repeat(16)));
        assert_eq!(preview.ranges, vec![span(17, 3), span(21, 4)]);
    }

    #[test]
    fn never_splits_a_surrogate_pair_when_cutting() {
        // 23 single units, then one pair, so the cut at unit 24 falls inside the pair.
        let line = format!("{}😀{}hit", "a".repeat(23), "b".repeat(15));
        let preview = line_preview(&line, &[span(40, 3)], None);
        assert_eq!(preview.text, format!("…{}hit", "b".repeat(15)));
        assert_eq!(preview.ranges, vec![span(16, 3)]);
    }

    #[test]
    fn bounds_the_text_at_a_character_boundary_and_shortens_the_highlights() {
        let preview = line_preview("中文中文中文", &[span(1, 5)], Some(7));
        // Seven bytes end inside the third character, which is dropped.
        assert_eq!(preview.text, "中文");
        assert_eq!(preview.ranges, vec![span(1, 1)]);

        let far = format!("{}hit", "a".repeat(40));
        let preview = line_preview(&far, &[span(40, 3)], Some(10));
        assert_eq!(preview.text, format!("…{}", "a".repeat(7)));
        assert!(preview.ranges.is_empty());
    }

    #[test]
    fn drops_empty_highlights_and_those_past_the_text() {
        let preview = line_preview("abc\r", &[span(1, 0), span(2, 9), span(5, 1)], None);
        assert_eq!(preview.text, "abc");
        assert_eq!(preview.ranges, vec![span(2, 1)]);
    }
}
