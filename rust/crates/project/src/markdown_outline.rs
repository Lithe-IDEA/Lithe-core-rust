//! Heading outline of a document in the Markdown dialect shared by every
//! frontend.
//!
//! Hosts show Markdown headings where they show a language server's document
//! symbols, so the outline takes the normalized document-symbol shape of
//! `lsp.request` for `textDocument/documentSymbol`: zero-based lines and UTF-16
//! columns, with each heading's section as its range.

use crate::markdown::dialect;
use comrak::nodes::{AstNode, NodeHeading, NodeValue, Sourcepos};
use comrak::{parse_document, Arena};
use lithe_text::{LspPositionResponse, LspRangeResponse};
use serde::{Deserialize, Serialize};

/// LSP `SymbolKind.String`, the kind editors give Markdown headings.
pub const MARKDOWN_HEADING_SYMBOL_KIND: i64 = 15;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Markdown source to outline with the shared dialect.
pub struct MarkdownOutlineRequest {
    /// Markdown text to outline.
    pub source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// The headings of a Markdown document as a tree of document symbols.
pub struct MarkdownOutlineResponse {
    /// Top-level headings in document order, each holding the headings under it.
    pub symbols: Vec<MarkdownOutlineSymbol>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// One heading in the normalized document-symbol shape.
pub struct MarkdownOutlineSymbol {
    /// Heading text as the preview shows it, with runs of whitespace collapsed.
    pub name: String,
    /// Always `None`; present so the symbol has every normalized field.
    pub detail: Option<String>,
    /// Always [`MARKDOWN_HEADING_SYMBOL_KIND`].
    pub kind: i64,
    /// Always empty; headings carry no symbol tags.
    pub tags: Vec<i64>,
    /// Always `false`.
    pub deprecated: bool,
    /// The heading's section: from the start of its first line to the start of
    /// the line of the next heading of the same or a higher level, or to the end
    /// of the document.
    pub range: LspRangeResponse,
    /// The heading itself, from its first character to just after its last,
    /// underline included for a setext heading.
    pub selection_range: LspRangeResponse,
    /// Always `None`; the tree carries the hierarchy.
    pub container_name: Option<String>,
    /// The headings directly under this one, in document order.
    pub children: Vec<MarkdownOutlineSymbol>,
}

/// Outlines the headings of `request.source`.
///
/// Every heading Comrak parses with the shared dialect is listed in document
/// order, including headings inside block quotes, lists and alerts; front matter
/// and raw HTML headings are not headings. A heading whose text is empty after
/// collapsing whitespace is left out, and its section belongs to the heading
/// before it. A heading is a child of the nearest heading before it with a lower
/// level, so a skipped level nests one step.
pub fn outline(request: MarkdownOutlineRequest) -> MarkdownOutlineResponse {
    let source = request.source.as_str();
    let arena = Arena::new();
    let root = parse_document(&arena, source, &dialect());
    let lines = SourceLines::new(source);
    let document_end = lines.end();

    let mut symbols = Vec::new();
    let mut open: Vec<(u8, MarkdownOutlineSymbol)> = Vec::new();
    for node in root.descendants() {
        let (level, sourcepos) = {
            let data = node.data();
            let NodeValue::Heading(NodeHeading { level, .. }) = data.value else {
                continue;
            };
            (level, data.sourcepos)
        };
        let name = heading_name(node);
        if name.is_empty() {
            continue;
        }
        let selection_range = lines.range(sourcepos);
        let section_start = LspPositionResponse {
            line: selection_range.start.line,
            utf16_column: 0,
        };
        while open
            .last()
            .is_some_and(|(open_level, _)| *open_level >= level)
        {
            let (_, mut closed) = open.pop().expect("open heading");
            closed.range.end = section_start;
            attach(&mut open, &mut symbols, closed);
        }
        open.push((
            level,
            MarkdownOutlineSymbol {
                name,
                detail: None,
                kind: MARKDOWN_HEADING_SYMBOL_KIND,
                tags: Vec::new(),
                deprecated: false,
                range: LspRangeResponse {
                    start: section_start,
                    end: document_end,
                },
                selection_range,
                container_name: None,
                children: Vec::new(),
            },
        ));
    }
    while let Some((_, closed)) = open.pop() {
        attach(&mut open, &mut symbols, closed);
    }
    MarkdownOutlineResponse { symbols }
}

/// Adds a closed heading to the heading still open above it, or to the top level.
fn attach(
    open: &mut [(u8, MarkdownOutlineSymbol)],
    symbols: &mut Vec<MarkdownOutlineSymbol>,
    symbol: MarkdownOutlineSymbol,
) {
    match open.last_mut() {
        Some((_, parent)) => parent.children.push(symbol),
        None => symbols.push(symbol),
    }
}

/// The text of a heading as the preview shows it: text, code, math and emoji,
/// without images, footnote references or HTML tags, with every run of
/// whitespace collapsed to one space and the ends trimmed.
fn heading_name<'a>(heading: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    append_text(heading, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_text<'a>(node: &'a AstNode<'a>, text: &mut String) {
    for child in node.children() {
        match &child.data().value {
            NodeValue::Text(literal) => text.push_str(literal),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::Math(math) => text.push_str(&math.literal),
            NodeValue::ShortCode(code) => text.push_str(&code.emoji),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            NodeValue::Image(_)
            | NodeValue::FootnoteReference(_)
            | NodeValue::HtmlInline(_)
            | NodeValue::Raw(_) => {}
            _ => append_text(child, text),
        }
    }
}

/// The lines of a source as Comrak counts them, to turn its one-based line and
/// UTF-8 byte columns into zero-based lines and UTF-16 columns.
struct SourceLines<'s> {
    /// Each line's text without its line ending. `\n`, `\r\n` and a lone `\r`
    /// end a line; a source that ends with one has an empty last line.
    lines: Vec<&'s str>,
}

impl<'s> SourceLines<'s> {
    fn new(source: &'s str) -> Self {
        let bytes = source.as_bytes();
        let mut lines = Vec::new();
        let mut start = 0;
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\n' => {
                    lines.push(&source[start..index]);
                    start = index + 1;
                }
                b'\r' => {
                    lines.push(&source[start..index]);
                    if bytes.get(index + 1) == Some(&b'\n') {
                        index += 1;
                    }
                    start = index + 1;
                }
                _ => {}
            }
            index += 1;
        }
        lines.push(&source[start..]);
        Self { lines }
    }

    /// The position just after the last character of the source.
    fn end(&self) -> LspPositionResponse {
        let last = self.lines.len() - 1;
        LspPositionResponse {
            line: last as i64,
            utf16_column: utf16_len(self.lines[last]),
        }
    }

    /// The half-open range of a node whose source position Comrak reports from
    /// its first character to its last, both inclusive.
    fn range(&self, sourcepos: Sourcepos) -> LspRangeResponse {
        LspRangeResponse {
            start: self.position(sourcepos.start.line, sourcepos.start.column - 1, false),
            end: self.position(sourcepos.end.line, sourcepos.end.column, true),
        }
    }

    /// The position of the byte `column` (from zero) of the one-based `line`,
    /// clamped to the source and moved to a character boundary: forward for the
    /// end of a range, so it takes in a character that `column` falls inside.
    fn position(&self, line: usize, column: usize, end: bool) -> LspPositionResponse {
        let index = line.saturating_sub(1).min(self.lines.len() - 1);
        let text = self.lines[index];
        let mut byte = column.min(text.len());
        while !text.is_char_boundary(byte) {
            if end {
                byte += 1;
            } else {
                byte -= 1;
            }
        }
        LspPositionResponse {
            line: index as i64,
            utf16_column: utf16_len(&text[..byte]),
        }
    }
}

fn utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

#[cfg(test)]
mod tests {
    use super::{outline, MarkdownOutlineRequest, MARKDOWN_HEADING_SYMBOL_KIND};
    use serde_json::{json, Value};

    fn symbols(source: &str) -> Value {
        serde_json::to_value(
            outline(MarkdownOutlineRequest {
                source: source.to_owned(),
            })
            .symbols,
        )
        .expect("outline should encode")
    }

    /// Names of the tree as `name(children…)`, to compare its shape at a glance.
    fn shape(symbols: &Value) -> String {
        symbols
            .as_array()
            .expect("symbols")
            .iter()
            .map(|symbol| {
                let name = symbol["name"].as_str().expect("name");
                let children = shape(&symbol["children"]);
                if children.is_empty() {
                    name.to_owned()
                } else {
                    format!("{name}({children})")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn satisfies_shared_outline_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/editor/markdown-outline-v1.json"
        ))
        .expect("Markdown outline fixture should decode");
        for case in fixture["cases"].as_array().expect("fixture cases") {
            let name = case["name"].as_str().expect("case name");
            let source = case["source"].as_str().expect("case source");
            assert_eq!(symbols(source), case["expected"]["symbols"], "{name}");
        }
    }

    #[test]
    fn skipped_levels_nest_once_and_sections_end_at_the_next_peer() {
        let output = symbols("# A\n### B\n## C\ntext\n# D\n");

        assert_eq!(shape(&output), "A(B C) D");
        assert_eq!(output[0]["kind"], MARKDOWN_HEADING_SYMBOL_KIND);
        assert_eq!(
            output[0]["range"],
            json!({"start": {"line": 0, "utf16Column": 0}, "end": {"line": 4, "utf16Column": 0}})
        );
        assert_eq!(
            output[0]["children"][0]["range"]["end"],
            json!({"line": 2, "utf16Column": 0})
        );
        assert_eq!(
            output[1]["range"]["end"],
            json!({"line": 5, "utf16Column": 0})
        );
    }

    #[test]
    fn headings_inside_containers_are_listed_but_code_and_html_are_not() {
        let output = symbols(
            "# Top\n\n> ## Quoted\n\n- ### In list\n\n```\n# not a heading\n```\n\n<h2>Raw</h2>\n\n    # indented code\n",
        );

        assert_eq!(shape(&output), "Top(Quoted(In list))");
        assert_eq!(
            output[0]["children"][0]["selectionRange"]["start"],
            json!({"line": 2, "utf16Column": 2})
        );
    }

    #[test]
    fn empty_headings_are_left_out_and_their_section_joins_the_one_before() {
        let output = symbols("# A\n##\n### B\n#   \n## C\n");

        assert_eq!(shape(&output), "A(B C)");
    }

    #[test]
    fn names_keep_what_the_preview_shows_and_drop_images_and_markup() {
        let output = symbols(
            "# ![logo](logo.png) **Bold**  `code` <small>small</small> :smile: $x^2$ note[^1]\n\n[^1]: Footnote\n",
        );

        assert_eq!(output[0]["name"], "Bold code small 😄 x^2 note");
    }

    #[test]
    fn empty_source_has_no_symbols() {
        assert_eq!(symbols(""), json!([]));
        assert_eq!(symbols("plain text\n"), json!([]));
    }

    #[test]
    fn lone_carriage_returns_end_lines_as_comrak_counts_them() {
        let output = symbols("# A\r## B\rtext");

        assert_eq!(shape(&output), "A(B)");
        assert_eq!(
            output[0]["children"][0]["selectionRange"]["start"],
            json!({"line": 1, "utf16Column": 0})
        );
        assert_eq!(
            output[0]["range"]["end"],
            json!({"line": 2, "utf16Column": 4})
        );
    }
}
