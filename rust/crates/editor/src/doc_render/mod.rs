//! Deterministic native document IR for rendered documentation comments and
//! language-server hover popups.
//!
//! Hosts confirm documentation comment ranges through their syntax tokenizer or
//! language provider, then pass a single comment slice (or hover payload) here
//! via the borrowed-document C ABI (`editor.renderDocComment` and
//! `editor.renderHoverDoc`). Core owns shell stripping, dialect-aware
//! indentation handling, HTML5 fragment parsing, protected inline tags,
//! budgets, and deterministic fallback; hosts own layout, localization of
//! semantic section keys, and native drawing.

mod html;
mod javadoc;
mod markdown;

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum UTF-8 bytes accepted for one doc comment or hover rendering request.
pub(crate) const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Maximum DOM nodes allocated by the bounded `html5ever` tree sink.
pub(crate) const MAX_DOM_NODES: usize = 2048;
/// Maximum element nesting depth lowered to IR.
pub(crate) const MAX_DOM_DEPTH: usize = 16;
/// Maximum attributes retained per HTML element.
pub(crate) const MAX_ATTRS_PER_ELEM: usize = 16;
/// Maximum accumulated text bytes retained across the DOM and output IR.
pub(crate) const MAX_TEXT_BYTES: usize = 128 * 1024;
/// Maximum top-level IR blocks returned in one response.
pub(crate) const MAX_OUTPUT_BLOCKS: usize = 256;
/// Maximum inline spans returned across all blocks in one response.
pub(crate) const MAX_OUTPUT_SPANS: usize = 2048;

fn is_zero(v: &usize) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_left_align(v: &str) -> bool {
    v.is_empty() || v == "left"
}

/// Request payload for `editor.renderDocComment`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderDocCommentRequest {
    /// Comment shell format confirmed by the host tokenizer or provider:
    /// `"block-star"` (`/** ... */`), `"line-slash"` (`///` / `//!`), or `"raw"`.
    #[serde(default = "default_shell_kind")]
    pub shell_kind: String,
    /// Documentation content dialect declared by the syntax resource or
    /// language provider: `"javadoc-html"`, `"markdown-html"`, `"html"`, or
    /// `"markdown"`.
    pub dialect: String,
    /// Optional fallback language identifier applied to untagged code blocks.
    #[serde(default)]
    pub language: String,
    /// Inline source slice used when the caller does not pass borrowed bytes
    /// through `lithe_core_execute_document_json`.
    #[serde(default)]
    pub source: Option<String>,
}

fn default_shell_kind() -> String {
    "block-star".to_owned()
}

/// Request payload for `editor.renderHoverDoc`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderHoverDocRequest {
    /// Whether the hover payload is Markdown (`true`) or plain text (`false`).
    /// Plain text is never interpreted as HTML or Markdown.
    #[serde(default)]
    pub is_markdown: bool,
    /// Optional fallback language identifier applied to untagged fenced blocks.
    #[serde(default)]
    pub language: String,
    /// Inline hover contents used when borrowed bytes are not provided.
    #[serde(default)]
    pub contents: Option<String>,
}

/// Structured native document IR returned by `editor.renderDocComment` and
/// `editor.renderHoverDoc`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocRenderResponse {
    /// When true, the editor must keep rendering the comment as raw source.
    pub fallback_to_source: bool,
    /// Stable diagnostic reason token when fallback or downgrade occurred.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// Ordered top-level presentation blocks.
    pub blocks: Vec<DocBlock>,
    /// Named intra-document anchors (`id` / `name` attributes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<DocAnchor>,
}

/// One intra-document anchor attached to a top-level IR block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocAnchor {
    /// Normalized anchor identifier without a leading `#`.
    pub id: String,
    /// Zero-based index into [`DocRenderResponse::blocks`].
    pub block_index: usize,
}

/// One top-level block of structured documentation content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DocBlock {
    /// Running prose paragraph.
    Paragraph {
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
        /// Enclosing list indentation depth.
        #[serde(default, skip_serializing_if = "is_zero")]
        level: usize,
        /// Inline styled runs in reading order.
        spans: Vec<DocSpan>,
    },
    /// Section heading (`level` in `1..=6`).
    Heading {
        /// Heading level (`1..=6`).
        level: u8,
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
        /// Inline styled runs in reading order.
        spans: Vec<DocSpan>,
    },
    /// One bulleted (`number == 0`) or numbered (`number >= 1`) list item.
    Item {
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
        /// Zero-based list nesting depth.
        #[serde(default, skip_serializing_if = "is_zero")]
        level: usize,
        /// Zero for unordered bullets; `>= 1` for ordered list items.
        #[serde(default, skip_serializing_if = "is_zero")]
        number: usize,
        /// Inline styled runs in reading order.
        spans: Vec<DocSpan>,
    },
    /// Preformatted or fenced code block.
    Code {
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
        /// Optional language hint for syntax highlighting.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        language: String,
        /// Verbatim code text with normalized `\n` line breaks.
        text: String,
    },
    /// Horizontal divider rule.
    Rule {
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
    },
    /// Tabular data with a fixed column count.
    Table {
        /// Enclosing block-quote nesting depth (`0..=8`).
        #[serde(default, skip_serializing_if = "is_zero")]
        quote: usize,
        /// Number of columns in each row.
        columns: usize,
        /// Table rows in display order; the first row is styled as the header.
        rows: Vec<Vec<DocTableCell>>,
    },
    /// Borderless two-column label–content row (semantic doc tag or HTML `<dl>`).
    Labeled {
        /// Stable semantic tag key (`"param"`, `"typeParam"`, `"returns"`,
        /// `"throws"`, `"since"`, `"see"`, `"author"`, `"deprecated"`,
        /// `"version"`, `"custom"`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tag_kind: String,
        /// Fallback literal label when `tag_kind` is `"custom"` or empty.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label_text: String,
        /// Optional rich inline spans for the label column (e.g. HTML `<dt>`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        label_spans: Vec<DocSpan>,
        /// Rich inline content of the value column.
        spans: Vec<DocSpan>,
    },
}

/// One cell of a [`DocBlock::Table`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocTableCell {
    /// Horizontal alignment: `"left"`, `"center"`, or `"right"`.
    #[serde(default, skip_serializing_if = "is_left_align")]
    pub align: String,
    /// Inline styled runs inside the cell.
    pub spans: Vec<DocSpan>,
}

/// One inline styled run inside a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocSpan {
    /// Text content of this run. `"\n"` represents an explicit inline line break.
    pub text: String,
    /// Bold emphasis (`<strong>` / `<b>` / `**`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    /// Italic emphasis (`<em>` / `<i>` / `*`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Strikethrough (`<del>` / `<s>` / `~~`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
    /// Monospace inline code (`<code>` / `{@code}` / `` ` ``).
    #[serde(default, skip_serializing_if = "is_false")]
    pub code: bool,
    /// Optional typed link target attached to this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<DocLink>,
}

/// Typed link target carried by a [`DocSpan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocLink {
    /// Target category: `"external"`, `"anchor"`, `"symbolRef"`, or `"none"`.
    pub kind: String,
    /// Raw destination or opaque symbol reference string.
    pub target: String,
}

fn invalid_request(reason: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Documentation render request was rejected",
    )
    .with_details(reason)
}

/// Parses one confirmed documentation comment slice into structured native IR
/// (`editor.renderDocComment`).
pub fn render_doc_comment_command(
    payload: Value,
    borrowed_source: Option<&str>,
) -> Result<Value, CoreError> {
    let request: RenderDocCommentRequest =
        serde_json::from_value(payload).map_err(|_| invalid_request("invalid_request"))?;
    let source = borrowed_source
        .or(request.source.as_deref())
        .ok_or_else(|| invalid_request("missing_source"))?;
    let response = render_doc_comment(
        source,
        &request.shell_kind,
        &request.dialect,
        &request.language,
    )?;
    serde_json::to_value(response).map_err(|_| invalid_request("encode_failed"))
}

/// Parses one hover payload into structured native IR (`editor.renderHoverDoc`).
pub fn render_hover_doc_command(
    payload: Value,
    borrowed_source: Option<&str>,
) -> Result<Value, CoreError> {
    let request: RenderHoverDocRequest =
        serde_json::from_value(payload).map_err(|_| invalid_request("invalid_request"))?;
    let contents = borrowed_source
        .or(request.contents.as_deref())
        .ok_or_else(|| invalid_request("missing_contents"))?;
    let response = render_hover_doc(contents, request.is_markdown, &request.language)?;
    serde_json::to_value(response).map_err(|_| invalid_request("encode_failed"))
}

/// Core implementation of `editor.renderDocComment`.
pub fn render_doc_comment(
    source: &str,
    shell_kind: &str,
    dialect: &str,
    default_code_lang: &str,
) -> Result<DocRenderResponse, CoreError> {
    if source.len() > MAX_INPUT_BYTES {
        return Ok(DocRenderResponse {
            fallback_to_source: true,
            fallback_reason: Some("input_too_large".to_owned()),
            blocks: Vec::new(),
            anchors: Vec::new(),
        });
    }

    let stripped = match shell_kind {
        "block-star" => strip_block_star_shell(source, dialect),
        "line-slash" => strip_line_slash_shell(source, dialect),
        "raw" => normalize_newlines(source),
        _ => {
            return Ok(DocRenderResponse {
                fallback_to_source: true,
                fallback_reason: Some("unsupported_shell".to_owned()),
                blocks: Vec::new(),
                anchors: Vec::new(),
            });
        }
    };

    match dialect {
        "javadoc-html" => javadoc::parse_javadoc_html(&stripped, default_code_lang),
        "html" => {
            let slot_tag = html::unique_slot_tag(&stripped);
            match html::parse_html_fragment(&stripped, &slot_tag, &[], default_code_lang)? {
                Ok(lowered) => Ok(DocRenderResponse {
                    fallback_to_source: false,
                    fallback_reason: None,
                    blocks: lowered.blocks,
                    anchors: lowered.anchors,
                }),
                Err(err) => Ok(DocRenderResponse {
                    fallback_to_source: true,
                    fallback_reason: Some(
                        match err {
                            html::LowerError::BudgetExceeded => "budget_exceeded",
                            html::LowerError::SlotFidelityFailed => "slot_fidelity_failed",
                        }
                        .to_owned(),
                    ),
                    blocks: Vec::new(),
                    anchors: Vec::new(),
                }),
            }
        }
        "markdown-html" | "markdown" => {
            markdown::parse_markdown_with_html(&stripped, default_code_lang, false)
        }
        _ => Ok(DocRenderResponse {
            fallback_to_source: true,
            fallback_reason: Some("unsupported_dialect".to_owned()),
            blocks: Vec::new(),
            anchors: Vec::new(),
        }),
    }
}

/// Core implementation of `editor.renderHoverDoc`.
pub fn render_hover_doc(
    contents: &str,
    is_markdown: bool,
    default_code_lang: &str,
) -> Result<DocRenderResponse, CoreError> {
    let normalized = normalize_newlines(contents);
    if !is_markdown {
        let trimmed = normalized.trim_end_matches('\n');
        let blocks = if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![DocBlock::Code {
                quote: 0,
                language: String::new(),
                text: trimmed.to_owned(),
            }]
        };
        return Ok(DocRenderResponse {
            fallback_to_source: false,
            fallback_reason: None,
            blocks,
            anchors: Vec::new(),
        });
    }

    if normalized.len() > MAX_INPUT_BYTES {
        let mut cut = MAX_INPUT_BYTES;
        while cut > 0 && !normalized.is_char_boundary(cut) {
            cut -= 1;
        }
        return Ok(DocRenderResponse {
            fallback_to_source: false,
            fallback_reason: Some("input_too_large".to_owned()),
            blocks: vec![DocBlock::Paragraph {
                quote: 0,
                level: 0,
                spans: vec![DocSpan {
                    text: normalized[..cut].to_owned(),
                    bold: false,
                    italic: false,
                    strike: false,
                    code: false,
                    link: None,
                }],
            }],
            anchors: Vec::new(),
        });
    }

    markdown::parse_markdown_with_html(&normalized, default_code_lang, true)
}

fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_owned();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Strips `/** ... */` or `/*! ... */` comment delimiters and per-line `*`
/// decoration prefixes. Indentation stripping is dialect-aware: Javadoc-HTML
/// removes only one space after `*` so `<pre>` indentation stays intact, while
/// Markdown-HTML also strips uniform common leading indentation so indented
/// code blocks stay aligned.
fn strip_block_star_shell(source: &str, dialect: &str) -> String {
    let normalized = normalize_newlines(source);
    let trimmed = normalized.trim();
    let inner = trimmed
        .strip_prefix("/**")
        .or_else(|| trimmed.strip_prefix("/*!"))
        .or_else(|| trimmed.strip_prefix("/*"))
        .unwrap_or(trimmed);
    let inner = inner.strip_suffix("*/").unwrap_or(inner);

    let mut lines: Vec<&str> = inner.split('\n').collect();
    for (idx, line) in lines.iter_mut().enumerate() {
        if idx == 0 {
            *line = line.strip_prefix(' ').unwrap_or(line);
            continue;
        }
        let ws_trimmed = line.trim_start_matches([' ', '\t']);
        if let Some(after_star) = ws_trimmed.strip_prefix('*') {
            *line = after_star.strip_prefix(' ').unwrap_or(after_star);
        }
    }
    // Trim leading/trailing empty lines left by opening `/**` and closing `*/` lines.
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if matches!(dialect, "markdown-html" | "markdown") {
        strip_common_indent(&mut lines);
    }
    lines.join("\n")
}

/// Strips `///` or `//!` line-doc comment markers on each line.
fn strip_line_slash_shell(source: &str, dialect: &str) -> String {
    let normalized = normalize_newlines(source);
    let mut lines: Vec<&str> = normalized
        .split('\n')
        .map(|line| {
            let trimmed = line.trim_start_matches([' ', '\t']);
            let rest = trimmed
                .strip_prefix("///")
                .or_else(|| trimmed.strip_prefix("//!"))
                .unwrap_or(trimmed);
            rest.strip_prefix(' ').unwrap_or(rest)
        })
        .collect();
    if matches!(dialect, "markdown-html" | "markdown") {
        strip_common_indent(&mut lines);
    }
    lines.join("\n")
}

fn strip_common_indent(lines: &mut [&str]) {
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.bytes().take_while(|&b| b == b' ').count())
        .min()
        .unwrap_or(0);
    if common == 0 {
        return;
    }
    for line in lines.iter_mut() {
        if line.len() >= common && line.as_bytes()[..common].iter().all(|&b| b == b' ') {
            *line = &line[common..];
        } else if line.trim().is_empty() {
            *line = "";
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arraylist_javadoc_parses_unclosed_paragraphs_links_pre_and_tags() {
        let src = r#"/**
 * Resizable-array implementation of the {@code List} interface.
 *
 * <p><strong>Note that this implementation is not synchronized.</strong>
 * Wrap using {@link Collections#synchronizedList Collections.synchronizedList}:<pre>
 *   List list = Collections.synchronizedList(new ArrayList(...));</pre>
 *
 * <p id="fail-fast">
 * The iterators returned by this class's {@link #iterator() iterator} and
 * {@link #listIterator(int) listIterator} methods are <em>fail-fast</em>:
 * throw {@link ConcurrentModificationException}.
 *
 * <p>Note that the fail-fast behavior cannot be guaranteed.
 *
 * @param <E> the type of elements in this list
 * @author Josh Bloch
 * @author Neal Gafter
 * @see Collection
 * @see List
 * @since 1.2
 */"#;

        let res = render_doc_comment(src, "block-star", "javadoc-html", "java").unwrap();
        assert!(!res.fallback_to_source);
        assert!(res.anchors.iter().any(|a| a.id == "fail-fast"));

        // Verify code block preserved its indentation & line content.
        assert!(res.blocks.iter().any(|b| matches!(
            b,
            DocBlock::Code { text, .. }
                if text.contains("List list = Collections.synchronizedList(new ArrayList(...));")
        )));

        // Verify fail-fast paragraph has italic "fail-fast" and code+link "iterator".
        let fail_fast_idx = res
            .anchors
            .iter()
            .find(|a| a.id == "fail-fast")
            .unwrap()
            .block_index;
        match &res.blocks[fail_fast_idx] {
            DocBlock::Paragraph { spans, .. } => {
                assert!(spans.iter().any(|s| s.text == "iterator"
                    && s.code
                    && s.link
                        .as_ref()
                        .is_some_and(|l| l.kind == "symbolRef" && l.target == "#iterator()")));
                assert!(spans.iter().any(|s| s.text == "listIterator"
                    && s.code
                    && s.link
                        .as_ref()
                        .is_some_and(|l| l.target == "#listIterator(int)")));
                assert!(spans.iter().any(|s| s.text == "fail-fast" && s.italic));
            }
            other => panic!("expected fail-fast paragraph, got {other:?}"),
        }

        // Verify grouped tags & typeParam extraction of <E>.
        assert!(res.blocks.iter().any(|b| matches!(
            b,
            DocBlock::Labeled { tag_kind, spans, .. }
                if tag_kind == "typeParam" && spans.first().is_some_and(|s| s.text == "<E>" && s.code)
        )));
        assert!(res.blocks.iter().any(|b| matches!(
            b,
            DocBlock::Labeled { tag_kind, spans, .. }
                if tag_kind == "author"
                    && spans.iter().map(|s| s.text.as_str()).collect::<String>() == "Josh Bloch, Neal Gafter"
        )));
        assert!(res.blocks.iter().any(|b| matches!(
            b,
            DocBlock::Labeled { tag_kind, spans, .. }
                if tag_kind == "see"
                    && spans.iter().any(|s| s.text == "Collection" && s.code)
                    && spans.iter().any(|s| s.text == "List" && s.code)
        )));
        assert!(res.blocks.iter().any(|b| matches!(
            b,
            DocBlock::Labeled { tag_kind, spans, .. }
                if tag_kind == "since" && spans.first().is_some_and(|s| s.text == "1.2")
        )));
    }

    #[test]
    fn javadoc_preserves_html_scope_across_protected_inline_code() {
        let src = "/** <em>before {@code <E>} after</em> {@literal A<B>C} */";
        let res = render_doc_comment(src, "block-star", "javadoc-html", "java").unwrap();
        assert!(!res.fallback_to_source);
        let DocBlock::Paragraph { spans, .. } = &res.blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(spans
            .iter()
            .any(|s| s.text == "before " && s.italic && !s.code));
        assert!(spans.iter().any(|s| s.text == "<E>" && s.italic && s.code));
        assert!(spans
            .iter()
            .any(|s| s.text == " after" && s.italic && !s.code));
        assert!(spans.iter().any(|s| s.text.contains("A<B>C") && !s.code));
    }

    #[test]
    fn hover_preserves_cross_node_markdown_and_html_and_protects_code_and_escaped_text() {
        let md = "<em>text</em> and <a href=\"https://example.com\">**label**</a> and *before <em>inside* after</em>\n\nEscaped: &lt;em&gt;literal&lt;/em&gt;\n\nCode: `<p>raw</p>`\n\n```java\nList<String> x;\n```";
        let res = render_hover_doc(md, true, "java").unwrap();
        assert!(!res.fallback_to_source);

        let DocBlock::Paragraph { spans, .. } = &res.blocks[0] else {
            panic!("expected first paragraph");
        };
        assert!(spans.iter().any(|s| s.text == "text" && s.italic));
        assert!(spans.iter().any(|s| {
            s.text == "label"
                && s.bold
                && s.link
                    .as_ref()
                    .is_some_and(|l| l.kind == "external" && l.target == "https://example.com")
        }));
        assert!(spans.iter().any(|s| s.text.contains("before") && s.italic));
        assert!(spans.iter().any(|s| s.text.contains("inside") && s.italic));
        assert!(spans.iter().any(|s| s.text.contains("after") && s.italic));

        // Escaped entity &lt;em&gt; must stay literal text, not become italic.
        let DocBlock::Paragraph {
            spans: esc_spans, ..
        } = &res.blocks[1]
        else {
            panic!("expected second paragraph");
        };
        let joined: String = esc_spans.iter().map(|s| s.text.as_str()).collect();
        assert!(joined.contains("<em>literal</em>"), "got {joined:?}");
        assert!(esc_spans.iter().all(|s| !s.italic));

        // Inline code `<p>raw</p>` stays verbatim code.
        let DocBlock::Paragraph {
            spans: code_spans, ..
        } = &res.blocks[2]
        else {
            panic!("expected third paragraph");
        };
        assert!(code_spans.iter().any(|s| s.text == "<p>raw</p>" && s.code));

        // Fenced code block stays verbatim.
        assert!(matches!(
            &res.blocks[3],
            DocBlock::Code { language, text, .. } if language == "java" && text == "List<String> x;"
        ));
    }

    #[test]
    fn hover_plaintext_never_parses_html_or_markdown() {
        let raw = "<p><em>**not markdown**</em></p>";
        let res = render_hover_doc(raw, false, "java").unwrap();
        assert!(!res.fallback_to_source);
        assert_eq!(
            res.blocks,
            vec![DocBlock::Code {
                quote: 0,
                language: String::new(),
                text: raw.to_owned(),
            }]
        );
    }

    #[test]
    fn dangerous_tags_and_malformed_slots_fail_safely_without_leaking_sentinels() {
        let dangerous = "/** Hello <script>alert(1); {@code x}</script> world */";
        let res = render_doc_comment(dangerous, "block-star", "javadoc-html", "java").unwrap();
        assert!(!res.fallback_to_source);
        let serialized = serde_json::to_string(&res).unwrap();
        assert!(!serialized.contains("alert(1)"));
        assert!(!serialized.contains("x-lithe-slot"));

        // Unclosed attribute swallowing a slot must trigger fidelity fallback to source.
        let swallowed = "/** <a href=\"{@code hidden}\">broken */";
        let res2 = render_doc_comment(swallowed, "block-star", "javadoc-html", "java").unwrap();
        assert!(res2.fallback_to_source);
        assert_eq!(
            res2.fallback_reason.as_deref(),
            Some("slot_fidelity_failed")
        );

        // Hover RCDATA tag swallowing a code-span slot must trigger fidelity fallback
        // from unmodified Comrak AST without losing text or leaking the sentinel.
        let hover_swallowed = "before <textarea>`hidden`</textarea> after";
        let res3 = render_hover_doc(hover_swallowed, true, "java").unwrap();
        assert!(!res3.fallback_to_source);
        assert_eq!(
            res3.fallback_reason.as_deref(),
            Some("slot_fidelity_failed")
        );
        let s3 = serde_json::to_string(&res3).unwrap();
        assert!(s3.contains("before"));
        assert!(s3.contains("hidden"));
        assert!(s3.contains("after"));
        assert!(!s3.contains("x-lithe-slot"));
    }

    #[test]
    fn oversized_dom_triggers_bounded_fallback() {
        let mut huge = String::from("/** ");
        for _ in 0..2500 {
            huge.push_str("<span>a</span>");
        }
        huge.push_str(" */");
        let res = render_doc_comment(&huge, "block-star", "javadoc-html", "java").unwrap();
        assert!(res.fallback_to_source);
        assert_eq!(res.fallback_reason.as_deref(), Some("budget_exceeded"));
    }

    #[test]
    fn command_wrappers_accept_borrowed_source_and_reject_missing_inputs() {
        let ok = render_doc_comment_command(
            json!({"shellKind": "block-star", "dialect": "javadoc-html", "language": "java"}),
            Some("/** <p>Hello <b>world</b> */"),
        )
        .unwrap();
        assert_eq!(ok["fallbackToSource"], false);
        assert!(render_doc_comment_command(
            json!({"shellKind": "block-star", "dialect": "javadoc-html"}),
            None,
        )
        .is_err());
    }

    #[test]
    fn shared_doc_render_fixture_vectors_match_contract() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/editor/doc-render-v1.json"
        ))
        .expect("doc-render-v1.json must parse");

        for case in fixture["docComments"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let res = render_doc_comment(
                case["source"].as_str().unwrap(),
                case["shellKind"].as_str().unwrap(),
                case["dialect"].as_str().unwrap(),
                case["language"].as_str().unwrap_or(""),
            )
            .unwrap_or_else(|e| panic!("docComments[{name}] failed: {e:?}"));
            assert_eq!(
                res.fallback_to_source,
                case["expectFallback"].as_bool().unwrap(),
                "docComments[{name}] fallback mismatch"
            );
            if let Some(reason) = case["expectFallbackReason"].as_str() {
                assert_eq!(
                    res.fallback_reason.as_deref(),
                    Some(reason),
                    "docComments[{name}]"
                );
            }
            if let Some(anchors) = case["expectAnchors"].as_array() {
                let got: Vec<&str> = res.anchors.iter().map(|a| a.id.as_str()).collect();
                let want: Vec<&str> = anchors.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(got, want, "docComments[{name}] anchors mismatch");
            }
            if let Some(kinds) = case["expectBlockKinds"].as_array() {
                let got: Vec<&str> = res.blocks.iter().map(block_kind_name).collect();
                let want: Vec<&str> = kinds.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(got, want, "docComments[{name}] block kinds mismatch");
            }
            if let Some(forbid) = case["forbidSubstrings"].as_array() {
                let encoded = serde_json::to_string(&res).unwrap();
                for bad in forbid {
                    let needle = bad.as_str().unwrap();
                    assert!(
                        !encoded.contains(needle),
                        "docComments[{name}] leaked {needle:?}"
                    );
                }
            }
        }

        for case in fixture["hoverDocs"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let res = render_hover_doc(
                case["contents"].as_str().unwrap(),
                case["isMarkdown"].as_bool().unwrap(),
                case["language"].as_str().unwrap_or(""),
            )
            .unwrap_or_else(|e| panic!("hoverDocs[{name}] failed: {e:?}"));
            assert_eq!(
                res.fallback_to_source,
                case["expectFallback"].as_bool().unwrap(),
                "hoverDocs[{name}] fallback mismatch"
            );
            if let Some(reason) = case["expectFallbackReason"].as_str() {
                assert_eq!(
                    res.fallback_reason.as_deref(),
                    Some(reason),
                    "hoverDocs[{name}]"
                );
            }
            if let Some(kinds) = case["expectBlockKinds"].as_array() {
                let got: Vec<&str> = res.blocks.iter().map(block_kind_name).collect();
                let want: Vec<&str> = kinds.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(got, want, "hoverDocs[{name}] block kinds mismatch");
            }
            if let Some(want_code) = case["expectFirstCodeText"].as_str() {
                match res.blocks.first() {
                    Some(DocBlock::Code { text, .. }) => {
                        assert_eq!(text, want_code, "hoverDocs[{name}] code text mismatch");
                    }
                    other => panic!("hoverDocs[{name}] expected code block, got {other:?}"),
                }
            }
            let encoded = serde_json::to_string(&res).unwrap();
            if let Some(expect) = case["expectSubstrings"].as_array() {
                for s in expect {
                    let needle = s.as_str().unwrap();
                    assert!(
                        encoded.contains(needle),
                        "hoverDocs[{name}] missing {needle:?}"
                    );
                }
            }
            if let Some(forbid) = case["forbidSubstrings"].as_array() {
                for bad in forbid {
                    let needle = bad.as_str().unwrap();
                    assert!(
                        !encoded.contains(needle),
                        "hoverDocs[{name}] leaked {needle:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn review_edge_cases_multiline_code_annotation_dl_paragraphs_uppercase_url_and_nested_fallback()
    {
        // 1. Multi-line {@code ...} containing a line starting with @Override must not split as a block tag.
        let src = "/**\n * Example:\n * {@code\n * @Override\n * public String toString() { return \"\"; }\n * }\n * See {@link HTTPS://example.com/docs Docs}.\n * @since 2.0\n */";
        let res = render_doc_comment(src, "block-star", "javadoc-html", "java").unwrap();
        assert!(!res.fallback_to_source);
        let encoded = serde_json::to_string(&res).unwrap();
        assert!(encoded.contains("@Override"));
        assert!(!encoded.contains("\"@override\""));
        assert!(encoded.contains("\"kind\":\"external\""));
        assert!(encoded.contains("HTTPS://example.com/docs"));
        assert!(res
            .blocks
            .iter()
            .any(|b| matches!(b, DocBlock::Labeled { tag_kind, .. } if tag_kind == "since")));

        // 2. HTML <dl> with <dd><p>...</p></dd> keeps paragraphs inside the Labeled value column.
        let dl_src =
            "/**\n * <dl><dt>Term</dt><dd><p>First line</p><p>Second line</p></dd></dl>\n */";
        let dl_res = render_doc_comment(dl_src, "block-star", "javadoc-html", "java").unwrap();
        assert!(!dl_res.fallback_to_source);
        assert_eq!(dl_res.blocks.len(), 1);
        match &dl_res.blocks[0] {
            DocBlock::Labeled {
                label_text, spans, ..
            } => {
                assert_eq!(label_text, "Term");
                let combined: String = spans.iter().map(|s| s.text.as_str()).collect();
                assert!(combined.contains("First line"));
                assert!(combined.contains("Second line"));
            }
            other => panic!("expected Labeled block for <dl>, got {other:?}"),
        }

        // 3. Hover AST fallback preserves nested sub-list items and fenced code blocks inside a list item.
        let hover_src = "- outer\n  - nested item\n\n  ```rust\n  let v = 1;\n  ```\n\nprefix <textarea>`kept`</textarea> suffix";
        let hover_res = render_hover_doc(hover_src, true, "rust").unwrap();
        assert_eq!(
            hover_res.fallback_reason.as_deref(),
            Some("slot_fidelity_failed")
        );
        let hover_json = serde_json::to_string(&hover_res).unwrap();
        assert!(hover_json.contains("nested item"));
        assert!(hover_json.contains("let v = 1;"));
    }

    fn block_kind_name(block: &DocBlock) -> &'static str {
        match block {
            DocBlock::Paragraph { .. } => "paragraph",
            DocBlock::Heading { .. } => "heading",
            DocBlock::Item { .. } => "item",
            DocBlock::Code { .. } => "code",
            DocBlock::Rule { .. } => "rule",
            DocBlock::Table { .. } => "table",
            DocBlock::Labeled { .. } => "labeled",
        }
    }
}
