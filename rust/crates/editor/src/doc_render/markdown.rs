//! Markdown + embedded HTML lowering for Hover popups and Markdown doc comments.
//!
//! Parses Markdown via `comrak`, recursively expands Markdown container and
//! inline nodes (`Emph`, `Strong`, `Link`, `Paragraph`, `List`, `Table`, ...)
//! into a single HTML5 fragment stream (escaping every `Text` node and
//! attribute value so decoded text like `&lt;em&gt;` is never re-interpreted as
//! an HTML tag), while protecting only true opaque leaves (`Code`, `CodeBlock`)
//! via placeholder slots. Cross-node HTML scopes such as `*before <em>inside*
//! after</em>` or `<a href="...">**label**</a>` are preserved, and any budget
//! or fidelity failure deterministically falls back directly from the unmodified
//! Comrak AST without losing text or leaking internal placeholders.

use comrak::nodes::{AstNode, ListType, NodeValue, TableAlignment};
use comrak::{parse_document, Arena, Options};
use lithe_protocol::{cancellation, CoreError};

use super::html::{
    classify_href, escape_html_attr, escape_html_text, parse_html_fragment, slot_placeholder,
    trim_edge_whitespace, unique_slot_tag, LowerError, ProtectedSlot,
};
use super::{
    DocBlock, DocRenderResponse, DocSpan, DocTableCell, MAX_OUTPUT_BLOCKS, MAX_OUTPUT_SPANS,
    MAX_TEXT_BYTES,
};

fn comrak_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.description_lists = true;
    options.render.r#unsafe = true;
    options
}

/// Parses Markdown (possibly containing embedded raw HTML tags) into bounded
/// structured IR.
///
/// When `allow_plain_ast_fallback` is true (used by Hover), a budget or slot
/// fidelity failure falls back directly from the unmodified Comrak AST with raw
/// HTML rendered literally rather than discarding the popup. When false (used
/// by in-editor doc comments), failures return `fallback_to_source: true`.
pub(crate) fn parse_markdown_with_html(
    source: &str,
    default_code_lang: &str,
    allow_plain_ast_fallback: bool,
) -> Result<DocRenderResponse, CoreError> {
    cancellation::check()?;
    let arena = Arena::new();
    let root = parse_document(&arena, source, &comrak_options());

    let slot_tag = unique_slot_tag(source);
    let mut html_stream = String::with_capacity(source.len().saturating_add(64));
    let mut slots: Vec<ProtectedSlot> = Vec::new();
    let mut stream_budget_exceeded = false;

    expand_comrak_to_html(
        root,
        &slot_tag,
        &mut slots,
        &mut html_stream,
        &mut stream_budget_exceeded,
        0,
    )?;

    if !stream_budget_exceeded {
        match parse_html_fragment(&html_stream, &slot_tag, &slots, default_code_lang)? {
            Ok(lowered) => {
                return Ok(DocRenderResponse {
                    fallback_to_source: false,
                    fallback_reason: None,
                    blocks: lowered.blocks,
                    anchors: lowered.anchors,
                });
            }
            Err(err) => {
                if !allow_plain_ast_fallback {
                    let reason = match err {
                        LowerError::BudgetExceeded => "budget_exceeded",
                        LowerError::SlotFidelityFailed => "slot_fidelity_failed",
                    };
                    return Ok(DocRenderResponse {
                        fallback_to_source: true,
                        fallback_reason: Some(reason.to_owned()),
                        blocks: Vec::new(),
                        anchors: Vec::new(),
                    });
                }
                let reason = match err {
                    LowerError::BudgetExceeded => "budget_exceeded",
                    LowerError::SlotFidelityFailed => "slot_fidelity_failed",
                };
                return Ok(fallback_from_unmodified_ast(
                    root,
                    default_code_lang,
                    reason,
                ));
            }
        }
    }

    if !allow_plain_ast_fallback {
        return Ok(DocRenderResponse {
            fallback_to_source: true,
            fallback_reason: Some("budget_exceeded".to_owned()),
            blocks: Vec::new(),
            anchors: Vec::new(),
        });
    }
    Ok(fallback_from_unmodified_ast(
        root,
        default_code_lang,
        "budget_exceeded",
    ))
}

fn expand_comrak_to_html<'a>(
    node: &'a AstNode<'a>,
    slot_tag: &str,
    slots: &mut Vec<ProtectedSlot>,
    out: &mut String,
    budget_exceeded: &mut bool,
    depth: usize,
) -> Result<(), CoreError> {
    cancellation::check()?;
    if *budget_exceeded || depth > 32 || out.len() > MAX_TEXT_BYTES * 2 {
        *budget_exceeded = true;
        return Ok(());
    }

    match &node.data.borrow().value {
        NodeValue::Document | NodeValue::FrontMatter(_) => {
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
        }
        NodeValue::Paragraph => {
            out.push_str("<p>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</p>");
        }
        NodeValue::Heading(heading) => {
            let lvl = heading.level.clamp(1, 6);
            out.push_str(&format!("<h{lvl}>"));
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str(&format!("</h{lvl}>"));
        }
        NodeValue::BlockQuote => {
            out.push_str("<blockquote>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</blockquote>");
        }
        NodeValue::List(list) => {
            match list.list_type {
                ListType::Bullet => out.push_str("<ul>"),
                ListType::Ordered => {
                    if list.start > 1 {
                        out.push_str(&format!("<ol start=\"{}\">", list.start));
                    } else {
                        out.push_str("<ol>");
                    }
                }
            }
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            match list.list_type {
                ListType::Bullet => out.push_str("</ul>"),
                ListType::Ordered => out.push_str("</ol>"),
            }
        }
        NodeValue::Item(_) => {
            out.push_str("<li>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</li>");
        }
        NodeValue::TaskItem(task) => {
            out.push_str("<li>");
            if task.symbol.is_some() {
                out.push_str("[x] ");
            } else {
                out.push_str("[ ] ");
            }
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</li>");
        }
        NodeValue::DescriptionList => {
            out.push_str("<dl>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</dl>");
        }
        NodeValue::DescriptionItem(_) => {
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
        }
        NodeValue::DescriptionTerm => {
            out.push_str("<dt>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</dt>");
        }
        NodeValue::DescriptionDetails => {
            out.push_str("<dd>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</dd>");
        }
        NodeValue::ThematicBreak => {
            out.push_str("<hr>");
        }
        NodeValue::Table(table) => {
            out.push_str("<table>");
            for row in node.children() {
                let is_header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
                out.push_str("<tr>");
                for (col_idx, cell) in row.children().enumerate() {
                    let align = table
                        .alignments
                        .get(col_idx)
                        .copied()
                        .unwrap_or(TableAlignment::None);
                    let align_attr = match align {
                        TableAlignment::Center => " align=\"center\"",
                        TableAlignment::Right => " align=\"right\"",
                        TableAlignment::Left | TableAlignment::None => "",
                    };
                    let tag = if is_header { "th" } else { "td" };
                    out.push_str(&format!("<{tag}{align_attr}>"));
                    for child in cell.children() {
                        expand_comrak_to_html(
                            child,
                            slot_tag,
                            slots,
                            out,
                            budget_exceeded,
                            depth + 1,
                        )?;
                    }
                    out.push_str(&format!("</{tag}>"));
                }
                out.push_str("</tr>");
            }
            out.push_str("</table>");
        }
        NodeValue::TableRow(_) | NodeValue::TableCell => {
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
        }
        NodeValue::CodeBlock(code) => {
            let lang = code
                .info
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let text = code.literal.trim_end_matches('\n').to_owned();
            let idx = slots.len();
            slots.push(ProtectedSlot::CodeBlock {
                language: lang,
                text,
            });
            out.push_str(&slot_placeholder(slot_tag, idx));
        }
        NodeValue::Code(code) => {
            let idx = slots.len();
            slots.push(ProtectedSlot::Inline(DocSpan {
                text: code.literal.clone(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                link: None,
            }));
            out.push_str(&slot_placeholder(slot_tag, idx));
        }
        NodeValue::HtmlBlock(html) => {
            out.push_str(&html.literal);
        }
        NodeValue::HtmlInline(html) => {
            out.push_str(html);
        }
        NodeValue::Text(text) => {
            // Escape plain Markdown text so previously decoded entities such as
            // `&lt;em&gt;` are never re-interpreted as HTML tags.
            escape_html_text(text, out);
        }
        NodeValue::SoftBreak => {
            out.push(' ');
        }
        NodeValue::LineBreak => {
            out.push_str("<br>");
        }
        NodeValue::Emph => {
            out.push_str("<em>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</em>");
        }
        NodeValue::Strong => {
            out.push_str("<strong>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</strong>");
        }
        NodeValue::Strikethrough => {
            out.push_str("<del>");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</del>");
        }
        NodeValue::Link(link) => {
            out.push_str("<a href=\"");
            escape_html_attr(&link.url, out);
            out.push_str("\">");
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
            out.push_str("</a>");
        }
        NodeValue::Image(_) => {
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
        }
        NodeValue::Math(math) => {
            let idx = slots.len();
            slots.push(ProtectedSlot::Inline(DocSpan {
                text: math.literal.clone(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                link: None,
            }));
            out.push_str(&slot_placeholder(slot_tag, idx));
        }
        _ => {
            for child in node.children() {
                expand_comrak_to_html(child, slot_tag, slots, out, budget_exceeded, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Deterministic fallback directly from the unmodified Comrak AST. Raw HTML
/// nodes are preserved as literal text (never re-parsed), guaranteeing zero
/// text loss and zero internal placeholder leakage.
fn fallback_from_unmodified_ast<'a>(
    root: &'a AstNode<'a>,
    default_code_lang: &str,
    reason: &str,
) -> DocRenderResponse {
    let mut blocks = Vec::new();
    let mut span_budget = MAX_OUTPUT_SPANS;
    lower_ast_fallback(root, default_code_lang, 0, 0, &mut blocks, &mut span_budget);
    DocRenderResponse {
        fallback_to_source: false,
        fallback_reason: Some(reason.to_owned()),
        blocks,
        anchors: Vec::new(),
    }
}

fn lower_ast_fallback<'a>(
    node: &'a AstNode<'a>,
    default_code_lang: &str,
    quote: usize,
    level: usize,
    blocks: &mut Vec<DocBlock>,
    span_budget: &mut usize,
) {
    if blocks.len() >= MAX_OUTPUT_BLOCKS {
        return;
    }
    match &node.data.borrow().value {
        NodeValue::Document => {
            for child in node.children() {
                lower_ast_fallback(child, default_code_lang, quote, level, blocks, span_budget);
            }
        }
        NodeValue::BlockQuote => {
            for child in node.children() {
                lower_ast_fallback(
                    child,
                    default_code_lang,
                    (quote + 1).min(8),
                    level,
                    blocks,
                    span_budget,
                );
            }
        }
        NodeValue::Paragraph => {
            let mut spans = Vec::new();
            collect_ast_inline(node, false, false, false, None, &mut spans, span_budget);
            trim_edge_whitespace(&mut spans);
            if !spans.is_empty() {
                blocks.push(DocBlock::Paragraph {
                    quote,
                    level,
                    spans,
                });
            }
        }
        NodeValue::Heading(h) => {
            let mut spans = Vec::new();
            collect_ast_inline(node, false, false, false, None, &mut spans, span_budget);
            trim_edge_whitespace(&mut spans);
            if !spans.is_empty() {
                blocks.push(DocBlock::Heading {
                    level: h.level.clamp(1, 6),
                    quote,
                    spans,
                });
            }
        }
        NodeValue::List(list) => {
            let mut counter = if list.list_type == ListType::Ordered {
                list.start.max(1)
            } else {
                0
            };
            for item in node.children() {
                let num = if list.list_type == ListType::Ordered {
                    let n = counter;
                    counter += 1;
                    n
                } else {
                    0
                };
                let mut spans = Vec::new();
                let mut nested_blocks = Vec::new();
                for child in item.children() {
                    if matches!(child.data.borrow().value, NodeValue::Paragraph) {
                        collect_ast_inline(
                            child,
                            false,
                            false,
                            false,
                            None,
                            &mut spans,
                            span_budget,
                        );
                    } else {
                        nested_blocks.push(child);
                    }
                }
                trim_edge_whitespace(&mut spans);
                if !spans.is_empty() {
                    blocks.push(DocBlock::Item {
                        quote,
                        level,
                        number: num,
                        spans,
                    });
                }
                let next_level = (level + 1).min(8);
                for nested in nested_blocks {
                    lower_ast_fallback(
                        nested,
                        default_code_lang,
                        quote,
                        next_level,
                        blocks,
                        span_budget,
                    );
                }
            }
        }
        NodeValue::CodeBlock(code) => {
            let lang = code
                .info
                .split_whitespace()
                .next()
                .map(str::to_ascii_lowercase)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_code_lang.to_owned());
            blocks.push(DocBlock::Code {
                quote,
                language: lang,
                text: code.literal.trim_end_matches('\n').to_owned(),
            });
        }
        NodeValue::HtmlBlock(html) => {
            let trimmed = html.literal.trim();
            if !trimmed.is_empty() {
                blocks.push(DocBlock::Paragraph {
                    quote,
                    level,
                    spans: vec![DocSpan {
                        text: trimmed.to_owned(),
                        bold: false,
                        italic: false,
                        strike: false,
                        code: false,
                        link: None,
                    }],
                });
            }
        }
        NodeValue::ThematicBreak => {
            blocks.push(DocBlock::Rule { quote });
        }
        NodeValue::Table(t) => {
            let mut rows = Vec::new();
            let mut max_cols = 0;
            for row in node.children() {
                let mut cells = Vec::new();
                for (ci, cell) in row.children().enumerate() {
                    let mut spans = Vec::new();
                    collect_ast_inline(cell, false, false, false, None, &mut spans, span_budget);
                    trim_edge_whitespace(&mut spans);
                    let align = match t
                        .alignments
                        .get(ci)
                        .copied()
                        .unwrap_or(TableAlignment::None)
                    {
                        TableAlignment::Center => "center",
                        TableAlignment::Right => "right",
                        _ => "left",
                    };
                    cells.push(DocTableCell {
                        align: align.to_owned(),
                        spans,
                    });
                }
                if !cells.is_empty() {
                    max_cols = max_cols.max(cells.len());
                    rows.push(cells);
                }
            }
            if !rows.is_empty() && max_cols > 0 {
                blocks.push(DocBlock::Table {
                    quote,
                    columns: max_cols,
                    rows,
                });
            }
        }
        _ => {}
    }
}

fn collect_ast_inline<'a>(
    node: &'a AstNode<'a>,
    bold: bool,
    italic: bool,
    strike: bool,
    link: Option<&super::DocLink>,
    out: &mut Vec<DocSpan>,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    for child in node.children() {
        if *budget == 0 {
            return;
        }
        match &child.data.borrow().value {
            NodeValue::Text(t) => {
                *budget -= 1;
                out.push(DocSpan {
                    text: t.to_string(),
                    bold,
                    italic,
                    strike,
                    code: false,
                    link: link.cloned(),
                });
            }
            NodeValue::HtmlInline(raw) => {
                *budget -= 1;
                out.push(DocSpan {
                    text: raw.clone(),
                    bold,
                    italic,
                    strike,
                    code: false,
                    link: link.cloned(),
                });
            }
            NodeValue::Code(c) => {
                *budget -= 1;
                out.push(DocSpan {
                    text: c.literal.clone(),
                    bold,
                    italic,
                    strike,
                    code: true,
                    link: link.cloned(),
                });
            }
            NodeValue::SoftBreak => {
                *budget -= 1;
                out.push(DocSpan {
                    text: " ".to_owned(),
                    bold,
                    italic,
                    strike,
                    code: false,
                    link: link.cloned(),
                });
            }
            NodeValue::LineBreak => {
                *budget -= 1;
                out.push(DocSpan {
                    text: "\n".to_owned(),
                    bold: false,
                    italic: false,
                    strike: false,
                    code: false,
                    link: None,
                });
            }
            NodeValue::Emph => {
                collect_ast_inline(child, bold, true, strike, link, out, budget);
            }
            NodeValue::Strong => {
                collect_ast_inline(child, true, italic, strike, link, out, budget);
            }
            NodeValue::Strikethrough => {
                collect_ast_inline(child, bold, italic, true, link, out, budget);
            }
            NodeValue::Link(l) => {
                let next_link = classify_href(&l.url);
                collect_ast_inline(
                    child,
                    bold,
                    italic,
                    strike,
                    next_link.as_ref().or(link),
                    out,
                    budget,
                );
            }
            _ => {
                collect_ast_inline(child, bold, italic, strike, link, out, budget);
            }
        }
    }
}
