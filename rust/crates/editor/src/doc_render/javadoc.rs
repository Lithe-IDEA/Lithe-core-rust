//! Javadoc-HTML documentation comment parser.
//!
//! Splits block tags (`@param`, `@return`, `@throws`, `@since`, `@see`,
//! `@author`, `@deprecated`, `@version`) and extracts balanced inline tags
//! (`{@code}`, `{@literal}`, `{@link}`, `{@linkplain}`) into protected slots
//! before feeding each section body into a single stateful HTML5 fragment pass.
//! This keeps HTML scopes such as `<em>before {@code <E>} after</em>` intact
//! while preventing `<E>` or generic expressions inside `{@code}` / `{@literal}`
//! from being misinterpreted as HTML tags.

use lithe_protocol::CoreError;

use super::html::{
    classify_href, parse_html_fragment, slot_placeholder, trim_edge_whitespace, unique_slot_tag,
    LowerError, ProtectedSlot,
};
use super::{DocAnchor, DocBlock, DocLink, DocRenderResponse, DocSpan, MAX_OUTPUT_BLOCKS};

/// Parses a Javadoc-HTML comment body (already stripped of `/**`, `*/`, and
/// leading `* ` comment markers) into structured IR.
pub(crate) fn parse_javadoc_html(
    body: &str,
    default_code_lang: &str,
) -> Result<DocRenderResponse, CoreError> {
    let slot_tag = unique_slot_tag(body);
    let (main_body, block_tags) = split_block_tags(body);

    let mut blocks: Vec<DocBlock> = Vec::new();
    let mut anchors: Vec<DocAnchor> = Vec::new();

    if !main_body.trim().is_empty() {
        match lower_javadoc_section(&main_body, &slot_tag, default_code_lang)? {
            Ok((sec_blocks, sec_anchors)) => {
                let offset = blocks.len();
                blocks.extend(sec_blocks);
                for mut anchor in sec_anchors {
                    anchor.block_index += offset;
                    if !anchors.iter().any(|a| a.id == anchor.id) {
                        anchors.push(anchor);
                    }
                }
            }
            Err(err) => return Ok(fallback_response(err)),
        }
    }

    // Group consecutive `@see` and `@author` entries like IntelliJ IDEA's rendered view.
    let mut i = 0;
    while i < block_tags.len() {
        if blocks.len() >= MAX_OUTPUT_BLOCKS {
            return Ok(fallback_response(LowerError::BudgetExceeded));
        }
        let (ref name, ref content) = block_tags[i];
        match name.as_str() {
            "see" => {
                let mut grouped_spans: Vec<DocSpan> = Vec::new();
                while i < block_tags.len() && block_tags[i].0 == "see" {
                    let entry_spans =
                        match parse_see_entry(&block_tags[i].1, &slot_tag, default_code_lang)? {
                            Ok(spans) => spans,
                            Err(err) => return Ok(fallback_response(err)),
                        };
                    if !entry_spans.is_empty() {
                        if !grouped_spans.is_empty() {
                            grouped_spans.push(DocSpan {
                                text: ", ".to_owned(),
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link: None,
                            });
                            grouped_spans.push(DocSpan {
                                text: "\n".to_owned(),
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link: None,
                            });
                        }
                        grouped_spans.extend(entry_spans);
                    }
                    i += 1;
                }
                if !grouped_spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: "see".to_owned(),
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans: grouped_spans,
                    });
                }
            }
            "author" => {
                let mut grouped_spans: Vec<DocSpan> = Vec::new();
                while i < block_tags.len() && block_tags[i].0 == "author" {
                    let entry_spans =
                        match lower_inline_section(&block_tags[i].1, &slot_tag, default_code_lang)?
                        {
                            Ok(spans) => spans,
                            Err(err) => return Ok(fallback_response(err)),
                        };
                    if !entry_spans.is_empty() {
                        if !grouped_spans.is_empty() {
                            grouped_spans.push(DocSpan {
                                text: ", ".to_owned(),
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link: None,
                            });
                        }
                        grouped_spans.extend(entry_spans);
                    }
                    i += 1;
                }
                if !grouped_spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: "author".to_owned(),
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans: grouped_spans,
                    });
                }
            }
            "param" => {
                let (tag_kind, spans) =
                    match parse_named_tag_entry(content, true, &slot_tag, default_code_lang)? {
                        Ok(res) => res,
                        Err(err) => return Ok(fallback_response(err)),
                    };
                if !spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind,
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans,
                    });
                }
                i += 1;
            }
            "throws" | "exception" => {
                let (_, spans) =
                    match parse_named_tag_entry(content, false, &slot_tag, default_code_lang)? {
                        Ok(res) => res,
                        Err(err) => return Ok(fallback_response(err)),
                    };
                if !spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: "throws".to_owned(),
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans,
                    });
                }
                i += 1;
            }
            "return" | "returns" => {
                let spans = match lower_inline_section(content, &slot_tag, default_code_lang)? {
                    Ok(s) => s,
                    Err(err) => return Ok(fallback_response(err)),
                };
                if !spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: "returns".to_owned(),
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans,
                    });
                }
                i += 1;
            }
            "since" | "deprecated" | "version" => {
                let spans = match lower_inline_section(content, &slot_tag, default_code_lang)? {
                    Ok(s) => s,
                    Err(err) => return Ok(fallback_response(err)),
                };
                if !spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: name.clone(),
                        label_text: String::new(),
                        label_spans: Vec::new(),
                        spans,
                    });
                }
                i += 1;
            }
            other => {
                let spans = match lower_inline_section(content, &slot_tag, default_code_lang)? {
                    Ok(s) => s,
                    Err(err) => return Ok(fallback_response(err)),
                };
                if !spans.is_empty() {
                    blocks.push(DocBlock::Labeled {
                        tag_kind: "custom".to_owned(),
                        label_text: format!("@{other}"),
                        label_spans: Vec::new(),
                        spans,
                    });
                }
                i += 1;
            }
        }
    }

    Ok(DocRenderResponse {
        fallback_to_source: false,
        fallback_reason: None,
        blocks,
        anchors,
    })
}

fn fallback_response(err: LowerError) -> DocRenderResponse {
    let reason = match err {
        LowerError::BudgetExceeded => "budget_exceeded",
        LowerError::SlotFidelityFailed => "slot_fidelity_failed",
    };
    DocRenderResponse {
        fallback_to_source: true,
        fallback_reason: Some(reason.to_owned()),
        blocks: Vec::new(),
        anchors: Vec::new(),
    }
}

/// Splits the main description from top-level Javadoc block tags (`@tag ...`)
/// while ignoring `@` markers inside `<pre>` or inline `{@...}` tags.
fn split_block_tags(body: &str) -> (String, Vec<(String, String)>) {
    let mut main_lines: Vec<&str> = Vec::new();
    let mut tags: Vec<(String, Vec<&str>)> = Vec::new();
    let mut in_pre = false;

    for line in body.split('\n') {
        let trimmed = line.trim_start();
        if !in_pre {
            if let Some((tag_name, rest)) = parse_block_tag_line(trimmed) {
                tags.push((tag_name.to_ascii_lowercase(), vec![rest]));
                in_pre = update_pre_state(in_pre, rest);
                continue;
            }
        }
        in_pre = update_pre_state(in_pre, line);
        if let Some((_, ref mut lines)) = tags.last_mut() {
            lines.push(line);
        } else {
            main_lines.push(line);
        }
    }

    let main_body = main_lines.join("\n");
    let joined_tags = tags
        .into_iter()
        .map(|(name, lines)| (name, lines.join("\n")))
        .collect();
    (main_body, joined_tags)
}

fn parse_block_tag_line(trimmed: &str) -> Option<(&str, &str)> {
    let after_at = trimmed.strip_prefix('@')?;
    let name_len = after_at
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'-' | b'.'))
        .count();
    if name_len == 0 {
        return None;
    }
    let name = &after_at[..name_len];
    let rest = &after_at[name_len..];
    if !rest.is_empty() && !rest.as_bytes()[0].is_ascii_whitespace() {
        return None;
    }
    Some((name, rest.trim_start()))
}

fn update_pre_state(mut in_pre: bool, line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while !rest.is_empty() {
        if !in_pre {
            if let Some(pos) = rest.find("<pre") {
                let after = &rest[pos + 4..];
                if after.starts_with('>') || after.starts_with(|c: char| c.is_ascii_whitespace()) {
                    in_pre = true;
                }
                rest = after;
                continue;
            }
            break;
        } else if let Some(pos) = rest.find("</pre>") {
            in_pre = false;
            rest = &rest[pos + 6..];
        } else {
            break;
        }
    }
    in_pre
}

type SectionResult = Result<(Vec<DocBlock>, Vec<DocAnchor>), LowerError>;

fn lower_javadoc_section(
    section: &str,
    slot_tag: &str,
    default_code_lang: &str,
) -> Result<SectionResult, CoreError> {
    let (html_with_slots, slots) = extract_inline_javadoc_tags(section, slot_tag);
    match parse_html_fragment(&html_with_slots, slot_tag, &slots, default_code_lang)? {
        Ok(lowered) => Ok(Ok((lowered.blocks, lowered.anchors))),
        Err(err) => Ok(Err(err)),
    }
}

fn lower_inline_section(
    section: &str,
    slot_tag: &str,
    default_code_lang: &str,
) -> Result<Result<Vec<DocSpan>, LowerError>, CoreError> {
    let (blocks, _) = match lower_javadoc_section(section, slot_tag, default_code_lang)? {
        Ok(res) => res,
        Err(err) => return Ok(Err(err)),
    };
    Ok(Ok(flatten_blocks_to_inline_spans(blocks)))
}

fn parse_named_tag_entry(
    content: &str,
    is_param: bool,
    slot_tag: &str,
    default_code_lang: &str,
) -> Result<Result<(String, Vec<DocSpan>), LowerError>, CoreError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Ok((
            if is_param { "param" } else { "throws" }.to_owned(),
            Vec::new(),
        )));
    }
    let (token, rest) = split_first_token(trimmed);
    let is_type_param =
        is_param && token.starts_with('<') && token.ends_with('>') && token.len() >= 3;
    let tag_kind = if is_type_param {
        "typeParam"
    } else if is_param {
        "param"
    } else {
        "throws"
    };

    let mut spans = vec![DocSpan {
        text: token.to_owned(),
        bold: false,
        italic: false,
        strike: false,
        code: true,
        link: if !is_param {
            Some(DocLink {
                kind: "symbolRef".to_owned(),
                target: token.to_owned(),
            })
        } else {
            None
        },
    }];

    let desc = rest.trim();
    if !desc.is_empty() {
        let desc_spans = match lower_inline_section(desc, slot_tag, default_code_lang)? {
            Ok(s) => s,
            Err(err) => return Ok(Err(err)),
        };
        if !desc_spans.is_empty() {
            spans.push(DocSpan {
                text: " – ".to_owned(),
                bold: false,
                italic: false,
                strike: false,
                code: false,
                link: None,
            });
            spans.extend(desc_spans);
        }
    }
    Ok(Ok((tag_kind.to_owned(), spans)))
}

fn parse_see_entry(
    content: &str,
    slot_tag: &str,
    default_code_lang: &str,
) -> Result<Result<Vec<DocSpan>, LowerError>, CoreError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Ok(Vec::new()));
    }
    if trimmed.starts_with('<') || trimmed.starts_with('"') {
        return lower_inline_section(trimmed, slot_tag, default_code_lang);
    }
    let (target, label) = parse_link_reference(trimmed);
    let display = if !label.is_empty() {
        label.to_owned()
    } else {
        display_name_for_symbol(target)
    };
    Ok(Ok(vec![DocSpan {
        text: display,
        bold: false,
        italic: false,
        strike: false,
        code: true,
        link: if !target.is_empty() {
            Some(DocLink {
                kind: "symbolRef".to_owned(),
                target: target.to_owned(),
            })
        } else {
            None
        },
    }]))
}

/// Scans `section` once, extracting balanced inline `{@code ...}`, `{@literal ...}`,
/// `{@link ...}`, and `{@linkplain ...}` tags into `slots` and replacing each
/// occurrence with an empty placeholder element using `slot_tag`.
fn extract_inline_javadoc_tags(section: &str, slot_tag: &str) -> (String, Vec<ProtectedSlot>) {
    let mut out = String::with_capacity(section.len());
    let mut slots: Vec<ProtectedSlot> = Vec::new();
    let bytes = section.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'@' {
            if let Some((kind, content, end_idx)) = parse_balanced_inline_tag(section, i) {
                if let Some(slot) = build_inline_tag_slot(kind, content) {
                    let idx = slots.len();
                    slots.push(slot);
                    out.push_str(&slot_placeholder(slot_tag, idx));
                    i = end_idx;
                    continue;
                }
            }
        }
        let ch = section[i..].chars().next().expect("valid UTF-8 boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, slots)
}

fn parse_balanced_inline_tag(src: &str, start: usize) -> Option<(&str, &str, usize)> {
    let rest = &src[start + 2..];
    let name_len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'-'))
        .count();
    if name_len == 0 {
        return None;
    }
    let kind = &rest[..name_len];
    if !matches!(kind, "code" | "literal" | "link" | "linkplain" | "value") {
        return None;
    }
    let after_name = start + 2 + name_len;
    if after_name >= src.len() {
        return None;
    }
    let next_byte = src.as_bytes()[after_name];
    if next_byte != b'}' && !next_byte.is_ascii_whitespace() {
        return None;
    }

    let mut depth = 1_usize;
    let mut pos = after_name;
    let bytes = src.as_bytes();
    while pos < bytes.len() {
        match bytes[pos] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let raw_content = &src[after_name..pos];
                    let content = strip_single_leading_space(raw_content);
                    return Some((kind, content, pos + 1));
                }
            }
            _ => {}
        }
        pos += 1;
    }
    None
}

fn strip_single_leading_space(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix(' ') {
        rest
    } else if let Some(rest) = s.strip_prefix('\t') {
        rest
    } else {
        s.trim_start_matches([' ', '\t'])
    }
}

fn build_inline_tag_slot(kind: &str, content: &str) -> Option<ProtectedSlot> {
    match kind {
        "code" => {
            let text = content.trim_end_matches([' ', '\t']);
            Some(ProtectedSlot::Inline(DocSpan {
                text: text.to_owned(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                link: None,
            }))
        }
        "literal" => Some(ProtectedSlot::Inline(DocSpan {
            text: content.to_owned(),
            bold: false,
            italic: false,
            strike: false,
            code: false,
            link: None,
        })),
        "value" => {
            let text = content.trim();
            Some(ProtectedSlot::Inline(DocSpan {
                text: text.to_owned(),
                bold: false,
                italic: false,
                strike: false,
                code: true,
                link: None,
            }))
        }
        "link" | "linkplain" => {
            let is_code = kind == "link";
            let (target, label) = parse_link_reference(content.trim());
            let display = if !label.is_empty() {
                collapse_inline_whitespace(label)
            } else {
                display_name_for_symbol(target)
            };
            let link = if target.is_empty() {
                None
            } else if target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
            {
                classify_href(target)
            } else {
                Some(DocLink {
                    kind: "symbolRef".to_owned(),
                    target: target.to_owned(),
                })
            };
            Some(ProtectedSlot::Inline(DocSpan {
                text: display,
                bold: false,
                italic: false,
                strike: false,
                code: is_code,
                link,
            }))
        }
        _ => None,
    }
}

/// Splits a Javadoc link body such as `ListIterator#add(Object, int) add` into
/// `(target, optional_label)`, keeping parenthesized parameter lists intact.
fn parse_link_reference(s: &str) -> (&str, &str) {
    let mut paren_depth = 0_usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            c if c.is_ascii_whitespace() && paren_depth == 0 => {
                return (s[..idx].trim(), s[idx..].trim());
            }
            _ => {}
        }
    }
    (s.trim(), "")
}

/// Derives a readable label when `{@link ...}` omits an explicit label,
/// matching IntelliJ IDEA conventions (`#iterator()` -> `iterator()`,
/// `Collections#synchronizedList` -> `Collections.synchronizedList`).
fn display_name_for_symbol(target: &str) -> String {
    let trimmed = target.trim();
    if let Some(member) = trimmed.strip_prefix('#') {
        return member.to_owned();
    }
    trimmed.replace('#', ".")
}

fn split_first_token(s: &str) -> (&str, &str) {
    for (idx, ch) in s.char_indices() {
        if ch.is_ascii_whitespace() {
            return (&s[..idx], &s[idx..]);
        }
    }
    (s, "")
}

fn collapse_inline_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim_end().to_owned()
}

fn flatten_blocks_to_inline_spans(blocks: Vec<DocBlock>) -> Vec<DocSpan> {
    let mut out: Vec<DocSpan> = Vec::new();
    for block in blocks {
        let needs_break = !out.is_empty();
        let push_break = |dst: &mut Vec<DocSpan>| {
            dst.push(DocSpan {
                text: "\n".to_owned(),
                bold: false,
                italic: false,
                strike: false,
                code: false,
                link: None,
            });
        };
        match block {
            DocBlock::Paragraph { spans, .. }
            | DocBlock::Heading { spans, .. }
            | DocBlock::Item { spans, .. } => {
                if !spans.is_empty() {
                    if needs_break {
                        push_break(&mut out);
                    }
                    out.extend(spans);
                }
            }
            DocBlock::Code { text, .. } => {
                if !text.is_empty() {
                    if needs_break {
                        push_break(&mut out);
                    }
                    out.push(DocSpan {
                        text,
                        bold: false,
                        italic: false,
                        strike: false,
                        code: true,
                        link: None,
                    });
                }
            }
            DocBlock::Labeled {
                label_text, spans, ..
            } => {
                if needs_break {
                    push_break(&mut out);
                }
                if !label_text.is_empty() {
                    out.push(DocSpan {
                        text: format!("{label_text}: "),
                        bold: true,
                        italic: false,
                        strike: false,
                        code: false,
                        link: None,
                    });
                }
                out.extend(spans);
            }
            DocBlock::Rule { .. } | DocBlock::Table { .. } => {}
        }
    }
    trim_edge_whitespace(&mut out);
    out
}
