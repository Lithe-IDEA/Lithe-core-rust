//! Bounded HTML5 fragment parsing and DOM-to-IR lowering with protected slots.
//!
//! Uses `html5ever` directly with a custom [`BoundedDomSink`] so node, depth,
//! attribute, and text budgets are enforced while building the tree rather than
//! after an unbounded allocation. Protected leaves (such as `{@code ...}` or
//! Markdown code spans) are inserted via unique per-call placeholder elements
//! and verified for exact round-trip fidelity before lowering completes.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::mem;
use std::rc::{Rc, Weak};

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_fragment, Attribute, ExpandedName, ParseOpts, QualName};
use lithe_protocol::{cancellation, CoreError};
use markup5ever::{local_name, ns};

use super::{
    DocAnchor, DocBlock, DocLink, DocSpan, DocTableCell, MAX_ATTRS_PER_ELEM, MAX_DOM_DEPTH,
    MAX_DOM_NODES, MAX_OUTPUT_BLOCKS, MAX_OUTPUT_SPANS, MAX_TEXT_BYTES,
};

/// Opaque leaf content protected from HTML parsing and re-inserted when its
/// placeholder element is lowered to IR.
#[derive(Debug, Clone)]
pub(crate) enum ProtectedSlot {
    /// Inline code, literal, or symbol link span(s) that must never be parsed as HTML.
    Inline(DocSpan),
    /// Fenced or indented Markdown code block preserved verbatim.
    CodeBlock {
        /// Optional language info string for syntax highlighting.
        language: String,
        /// Exact code text without trailing fence lines.
        text: String,
    },
}

/// Result of lowering an HTML5 fragment with protected slots into IR blocks.
pub(crate) struct LoweredFragment {
    /// Top-level IR blocks produced by the fragment.
    pub(crate) blocks: Vec<DocBlock>,
    /// Intra-document anchors discovered on elements (`id` / `name`).
    pub(crate) anchors: Vec<DocAnchor>,
}

/// Failure classification from bounded HTML5 parsing or slot fidelity checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LowerError {
    /// A DOM node, depth, text, or output IR budget was exceeded.
    BudgetExceeded,
    /// A protected placeholder was lost, duplicated, or leaked as literal text.
    SlotFidelityFailed,
}

/// Produces a unique custom-element tag name that does not appear anywhere in
/// `input` (case-insensitively), preventing collisions with user text.
pub(crate) fn unique_slot_tag(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    for nonce in 0_u64.. {
        let candidate = format!("x-lithe-slot-{nonce}");
        if !lower.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 nonce space cannot be exhausted")
}

/// Formats an empty placeholder element referencing slot `index`.
pub(crate) fn slot_placeholder(tag: &str, index: usize) -> String {
    format!("<{tag} data-i=\"{index}\"></{tag}>")
}

/// Escapes plain text before writing it into an HTML5 fragment stream so text
/// such as `<E>` or `&lt;em&gt;` stays literal character data.
pub(crate) fn escape_html_text(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// Escapes an HTML attribute value placed inside double quotes.
pub(crate) fn escape_html_attr(value: &str, out: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Classifies a link href into a safe typed [`DocLink`]. Only `http://`,
/// `https://`, `mailto:`, and intra-document `#anchor` targets are actionable.
pub(crate) fn classify_href(raw: &str) -> Option<DocLink> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return None;
    }
    if trimmed.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return None;
    }
    if trimmed.starts_with('#') && trimmed.len() > 1 {
        return Some(DocLink {
            kind: "anchor".to_owned(),
            target: trimmed.to_owned(),
        });
    }
    let lower = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        return Some(DocLink {
            kind: "external".to_owned(),
            target: trimmed.to_owned(),
        });
    }
    None
}

/// Parses an HTML5 fragment containing `slot_tag` placeholders into bounded IR
/// blocks while verifying slot fidelity and zero sentinel leakage.
pub(crate) fn parse_html_fragment(
    html_input: &str,
    slot_tag: &str,
    slots: &[ProtectedSlot],
    default_code_lang: &str,
) -> Result<Result<LoweredFragment, LowerError>, CoreError> {
    cancellation::check()?;
    let sink = BoundedDomSink::new();
    let context = QualName::new(None, ns!(html), local_name!("div"));
    let dom =
        parse_fragment(sink, ParseOpts::default(), context, Vec::new(), false).one(html_input);

    if let Some(err) = dom.cancelled_error.borrow_mut().take() {
        return Err(err);
    }
    if dom.budget_exceeded.get() {
        return Ok(Err(LowerError::BudgetExceeded));
    }

    let mut lowerer = DomLowerer::new(slot_tag, slots, default_code_lang);
    let root = find_fragment_root(&dom.document);
    lowerer.walk_children(&root, 0, InlineStyle::default(), 0, 0, false)?;
    lowerer.flush_paragraph(0, 0);

    if lowerer.budget_exceeded {
        return Ok(Err(LowerError::BudgetExceeded));
    }
    if lowerer.fidelity_failed || !lowerer.verify_slots() {
        return Ok(Err(LowerError::SlotFidelityFailed));
    }

    let lowered = LoweredFragment {
        blocks: lowerer.blocks,
        anchors: lowerer.anchors,
    };
    if contains_sentinel(&lowered.blocks, slot_tag) {
        return Ok(Err(LowerError::SlotFidelityFailed));
    }
    Ok(Ok(lowered))
}

fn find_fragment_root(document: &Handle) -> Handle {
    let children = document.children.borrow();
    if children.len() == 1 {
        if let DomNodeData::Element { ref name, .. } = children[0].data {
            if name.local.as_ref().eq_ignore_ascii_case("html") {
                return children[0].clone();
            }
        }
    }
    document.clone()
}

fn contains_sentinel(blocks: &[DocBlock], slot_tag: &str) -> bool {
    let check_spans = |spans: &[DocSpan]| {
        spans.iter().any(|s| {
            s.text.contains(slot_tag)
                || s.link.as_ref().is_some_and(|l| l.target.contains(slot_tag))
        })
    };
    for block in blocks {
        match block {
            DocBlock::Paragraph { spans, .. }
            | DocBlock::Heading { spans, .. }
            | DocBlock::Item { spans, .. } => {
                if check_spans(spans) {
                    return true;
                }
            }
            DocBlock::Code { language, text, .. } => {
                if language.contains(slot_tag) || text.contains(slot_tag) {
                    return true;
                }
            }
            DocBlock::Rule { .. } => {}
            DocBlock::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        if check_spans(&cell.spans) {
                            return true;
                        }
                    }
                }
            }
            DocBlock::Labeled {
                label_text,
                label_spans,
                spans,
                ..
            } => {
                if label_text.contains(slot_tag) || check_spans(label_spans) || check_spans(spans) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Bounded html5ever TreeSink
// ---------------------------------------------------------------------------

enum DomNodeData {
    Document,
    Text {
        contents: RefCell<StrTendril>,
    },
    Element {
        name: QualName,
        attrs: RefCell<Vec<Attribute>>,
        template_contents: RefCell<Option<Handle>>,
        mathml_annotation_xml_integration_point: bool,
    },
    Ignored,
}

struct DomNode {
    parent: Cell<Option<Weak<DomNode>>>,
    children: RefCell<Vec<Handle>>,
    data: DomNodeData,
}

type Handle = Rc<DomNode>;

impl DomNode {
    fn new(data: DomNodeData) -> Handle {
        Rc::new(Self {
            parent: Cell::new(None),
            children: RefCell::new(Vec::new()),
            data,
        })
    }
}

impl Drop for DomNode {
    fn drop(&mut self) {
        let mut stack = mem::take(&mut *self.children.borrow_mut());
        while let Some(node) = stack.pop() {
            let children = mem::take(&mut *node.children.borrow_mut());
            stack.extend(children);
            if let DomNodeData::Element {
                ref template_contents,
                ..
            } = node.data
            {
                if let Some(tpl) = template_contents.borrow_mut().take() {
                    stack.push(tpl);
                }
            }
        }
    }
}

struct BoundedDomSink {
    document: Handle,
    node_count: Cell<usize>,
    text_bytes: Cell<usize>,
    budget_exceeded: Cell<bool>,
    cancelled_error: RefCell<Option<CoreError>>,
}

impl BoundedDomSink {
    fn new() -> Self {
        Self {
            document: DomNode::new(DomNodeData::Document),
            node_count: Cell::new(1),
            text_bytes: Cell::new(0),
            budget_exceeded: Cell::new(false),
            cancelled_error: RefCell::new(None),
        }
    }

    fn check_limits(&self) -> bool {
        if self.cancelled_error.borrow().is_some() || self.budget_exceeded.get() {
            return false;
        }
        if self.node_count.get() & 31 == 0 {
            if let Err(err) = cancellation::check() {
                *self.cancelled_error.borrow_mut() = Some(err);
                return false;
            }
        }
        true
    }

    fn charge_node(&self) -> bool {
        if !self.check_limits() {
            return false;
        }
        let next = self.node_count.get().saturating_add(1);
        if next > MAX_DOM_NODES {
            self.budget_exceeded.set(true);
            return false;
        }
        self.node_count.set(next);
        true
    }

    fn charge_text(&self, len: usize) -> bool {
        if !self.check_limits() {
            return false;
        }
        let next = self.text_bytes.get().saturating_add(len);
        if next > MAX_TEXT_BYTES {
            self.budget_exceeded.set(true);
            return false;
        }
        self.text_bytes.set(next);
        true
    }
}

fn attach_child(parent: &Handle, child: Handle) {
    remove_node_from_parent(&child);
    child.parent.set(Some(Rc::downgrade(parent)));
    parent.children.borrow_mut().push(child);
}

fn parent_and_index(target: &Handle) -> Option<(Handle, usize)> {
    let weak = target.parent.take()?;
    let parent = weak.upgrade();
    target.parent.set(Some(weak));
    let parent = parent?;
    let idx = parent
        .children
        .borrow()
        .iter()
        .position(|c| Rc::ptr_eq(c, target))?;
    Some((parent, idx))
}

fn remove_node_from_parent(target: &Handle) {
    if let Some((parent, idx)) = parent_and_index(target) {
        parent.children.borrow_mut().remove(idx);
        target.parent.set(None);
    }
}

fn append_to_text_node(node: &Handle, text: &str) -> bool {
    if let DomNodeData::Text { ref contents } = node.data {
        contents.borrow_mut().push_slice(text);
        true
    } else {
        false
    }
}

impl TreeSink for BoundedDomSink {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self::Output {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.document.clone()
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        if let DomNodeData::Element {
            ref template_contents,
            ..
        } = target.data
        {
            if let Some(tpl) = template_contents.borrow().as_ref() {
                return tpl.clone();
            }
        }
        DomNode::new(DomNodeData::Document)
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }

    fn elem_name<'a>(&self, target: &'a Handle) -> ExpandedName<'a> {
        match target.data {
            DomNodeData::Element { ref name, .. } => name.expanded(),
            _ => panic!("html5ever requested elem_name for a non-element handle"),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        mut attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Handle {
        if !self.charge_node() {
            attrs.clear();
        } else if attrs.len() > MAX_ATTRS_PER_ELEM {
            attrs.truncate(MAX_ATTRS_PER_ELEM);
        }
        let tpl = if flags.template && self.check_limits() {
            Some(DomNode::new(DomNodeData::Document))
        } else {
            None
        };
        DomNode::new(DomNodeData::Element {
            name,
            attrs: RefCell::new(attrs),
            template_contents: RefCell::new(tpl),
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, _text: StrTendril) -> Handle {
        DomNode::new(DomNodeData::Ignored)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Handle {
        DomNode::new(DomNodeData::Ignored)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        if !self.check_limits() {
            return;
        }
        match child {
            NodeOrText::AppendText(text) => {
                if !self.charge_text(text.len()) {
                    return;
                }
                if let Some(last) = parent.children.borrow().last() {
                    if append_to_text_node(last, &text) {
                        return;
                    }
                }
                if !self.charge_node() {
                    return;
                }
                attach_child(
                    parent,
                    DomNode::new(DomNodeData::Text {
                        contents: RefCell::new(text),
                    }),
                );
            }
            NodeOrText::AppendNode(node) => {
                if matches!(node.data, DomNodeData::Ignored) {
                    return;
                }
                attach_child(parent, node);
            }
        }
    }

    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        if !self.check_limits() {
            return;
        }
        let Some((parent, idx)) = parent_and_index(sibling) else {
            return;
        };
        let node = match child {
            NodeOrText::AppendText(text) => {
                if !self.charge_text(text.len()) {
                    return;
                }
                if idx > 0 {
                    let children = parent.children.borrow();
                    if append_to_text_node(&children[idx - 1], &text) {
                        return;
                    }
                }
                if !self.charge_node() {
                    return;
                }
                DomNode::new(DomNodeData::Text {
                    contents: RefCell::new(text),
                })
            }
            NodeOrText::AppendNode(node) => {
                if matches!(node.data, DomNodeData::Ignored) {
                    return;
                }
                remove_node_from_parent(&node);
                node
            }
        };
        node.parent.set(Some(Rc::downgrade(&parent)));
        parent.children.borrow_mut().insert(idx, node);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if !self.check_limits() {
            return;
        }
        let has_parent = {
            let p = element.parent.take();
            let exists = p.is_some();
            element.parent.set(p);
            exists
        };
        if has_parent {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if !self.check_limits() {
            return;
        }
        if let DomNodeData::Element {
            attrs: existing, ..
        } = &target.data
        {
            let mut existing = existing.borrow_mut();
            let names: HashSet<_> = existing.iter().map(|a| a.name.clone()).collect();
            for attr in attrs {
                if existing.len() >= MAX_ATTRS_PER_ELEM {
                    break;
                }
                if !names.contains(&attr.name) {
                    existing.push(attr);
                }
            }
        }
    }

    fn remove_from_parent(&self, target: &Handle) {
        remove_node_from_parent(target);
    }

    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        if !self.check_limits() {
            return;
        }
        let children = mem::take(&mut *node.children.borrow_mut());
        let mut dst = new_parent.children.borrow_mut();
        for child in children {
            child.parent.set(Some(Rc::downgrade(new_parent)));
            dst.push(child);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(
            target.data,
            DomNodeData::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            }
        )
    }
}

// ---------------------------------------------------------------------------
// DOM-to-IR Lowering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<DocLink>,
}

struct DomLowerer<'a> {
    slot_tag: &'a str,
    slots: &'a [ProtectedSlot],
    slot_hits: Vec<u32>,
    slot_dropped: Vec<bool>,
    default_code_lang: &'a str,
    blocks: Vec<DocBlock>,
    anchors: Vec<DocAnchor>,
    pending_anchors: Vec<String>,
    inline_buf: Vec<DocSpan>,
    total_spans: usize,
    total_text_bytes: usize,
    budget_exceeded: bool,
    fidelity_failed: bool,
}

impl<'a> DomLowerer<'a> {
    fn new(slot_tag: &'a str, slots: &'a [ProtectedSlot], default_code_lang: &'a str) -> Self {
        Self {
            slot_tag,
            slots,
            slot_hits: vec![0; slots.len()],
            slot_dropped: vec![false; slots.len()],
            default_code_lang,
            blocks: Vec::new(),
            anchors: Vec::new(),
            pending_anchors: Vec::new(),
            inline_buf: Vec::new(),
            total_spans: 0,
            total_text_bytes: 0,
            budget_exceeded: false,
            fidelity_failed: false,
        }
    }

    fn verify_slots(&self) -> bool {
        for i in 0..self.slots.len() {
            let hits = self.slot_hits[i];
            let dropped = self.slot_dropped[i];
            if !(hits == 1 && !dropped || hits == 0 && dropped) {
                return false;
            }
        }
        true
    }

    fn record_anchor(&mut self, attrs: &[Attribute]) {
        for attr in attrs {
            let key = attr.name.local.as_ref();
            if key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("name") {
                let id = attr.value.trim();
                if !id.is_empty()
                    && id.len() <= 256
                    && !self.pending_anchors.iter().any(|a| a == id)
                {
                    self.pending_anchors.push(id.to_owned());
                }
            }
        }
    }

    fn push_block(&mut self, block: DocBlock) {
        if self.blocks.len() >= MAX_OUTPUT_BLOCKS {
            self.budget_exceeded = true;
            return;
        }
        let block_index = self.blocks.len();
        self.blocks.push(block);
        for id in self.pending_anchors.drain(..) {
            if !self.anchors.iter().any(|a| a.id == id) {
                self.anchors.push(DocAnchor { id, block_index });
            }
        }
    }

    fn push_span(&mut self, mut span: DocSpan) {
        if self.budget_exceeded || span.text.is_empty() {
            return;
        }
        self.total_text_bytes = self.total_text_bytes.saturating_add(span.text.len());
        if self.total_text_bytes > MAX_TEXT_BYTES {
            self.budget_exceeded = true;
            return;
        }
        if let Some(prev) = self.inline_buf.last_mut() {
            if prev.bold == span.bold
                && prev.italic == span.italic
                && prev.strike == span.strike
                && prev.code == span.code
                && prev.link == span.link
                && prev.text != "\n"
                && span.text != "\n"
            {
                prev.text.push_str(&span.text);
                return;
            }
        }
        if self.total_spans >= MAX_OUTPUT_SPANS {
            self.budget_exceeded = true;
            return;
        }
        self.total_spans += 1;
        span.text.shrink_to_fit();
        self.inline_buf.push(span);
    }

    fn push_collapsed_text(&mut self, raw: &str, style: &InlineStyle) {
        let mut out = String::with_capacity(raw.len());
        let mut prev_space = self
            .inline_buf
            .last()
            .is_none_or(|s| s.text == "\n" || s.text.ends_with(' '));
        for ch in raw.chars() {
            if ch.is_ascii_whitespace() {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            } else {
                out.push(ch);
                prev_space = false;
            }
        }
        if !out.is_empty() {
            self.push_span(DocSpan {
                text: out,
                bold: style.bold,
                italic: style.italic,
                strike: style.strike,
                code: style.code,
                link: style.link.clone(),
            });
        }
    }

    fn take_trimmed_spans(&mut self) -> Vec<DocSpan> {
        let mut spans = mem::take(&mut self.inline_buf);
        trim_edge_whitespace(&mut spans);
        spans
    }

    fn flush_paragraph(&mut self, quote: usize, level: usize) {
        let spans = self.take_trimmed_spans();
        if !spans.is_empty() {
            self.push_block(DocBlock::Paragraph {
                quote,
                level,
                spans,
            });
        }
    }

    fn walk_children(
        &mut self,
        node: &Handle,
        depth: usize,
        style: InlineStyle,
        quote: usize,
        list_level: usize,
        in_pre: bool,
    ) -> Result<(), CoreError> {
        cancellation::check()?;
        if self.budget_exceeded || self.fidelity_failed {
            return Ok(());
        }
        if depth > MAX_DOM_DEPTH {
            self.budget_exceeded = true;
            return Ok(());
        }
        let children = node.children.borrow().clone();
        for child in children {
            self.walk_node(&child, depth + 1, &style, quote, list_level, in_pre)?;
            if self.budget_exceeded || self.fidelity_failed {
                return Ok(());
            }
        }
        Ok(())
    }

    fn walk_node(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &InlineStyle,
        quote: usize,
        list_level: usize,
        in_pre: bool,
    ) -> Result<(), CoreError> {
        match &node.data {
            DomNodeData::Document => {
                self.walk_children(node, depth, style.clone(), quote, list_level, in_pre)?;
            }
            DomNodeData::Ignored => {}
            DomNodeData::Text { contents } => {
                let text = contents.borrow();
                if text.contains(self.slot_tag) {
                    self.fidelity_failed = true;
                    return Ok(());
                }
                if in_pre {
                    self.push_span(DocSpan {
                        text: text.to_string(),
                        bold: style.bold,
                        italic: style.italic,
                        strike: style.strike,
                        code: style.code,
                        link: style.link.clone(),
                    });
                } else {
                    self.push_collapsed_text(&text, style);
                }
            }
            DomNodeData::Element { name, attrs, .. } => {
                let tag = name.local.as_ref().to_ascii_lowercase();
                let attrs_vec = attrs.borrow().clone();

                // Protected placeholder element.
                if tag == self.slot_tag {
                    self.emit_slot(&attrs_vec, style, quote, list_level);
                    return Ok(());
                }

                // Dropped executable / non-rendered HTML elements.
                if is_dropped_dangerous_tag(&tag) {
                    self.mark_dropped_slots_in_subtree(node);
                    return Ok(());
                }

                // Non-rendered RAWTEXT/RCDATA elements whose text could hide a raw slot string.
                if matches!(tag.as_str(), "textarea" | "title" | "xmp") {
                    if self.subtree_contains_sentinel(node) {
                        self.fidelity_failed = true;
                    }
                    return Ok(());
                }

                self.record_anchor(&attrs_vec);

                match tag.as_str() {
                    "p" | "div" | "section" | "article" | "header" | "footer" | "main" => {
                        self.flush_paragraph(quote, list_level);
                        self.walk_children(node, depth, style.clone(), quote, list_level, false)?;
                        self.flush_paragraph(quote, list_level);
                    }
                    "br" => {
                        if !self.inline_buf.is_empty() {
                            self.push_span(DocSpan {
                                text: "\n".to_owned(),
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link: None,
                            });
                        }
                    }
                    "hr" => {
                        self.flush_paragraph(quote, list_level);
                        self.push_block(DocBlock::Rule { quote });
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        self.flush_paragraph(quote, list_level);
                        let level = tag.as_bytes()[1] - b'0';
                        self.walk_children(node, depth, style.clone(), quote, list_level, false)?;
                        let spans = self.take_trimmed_spans();
                        if !spans.is_empty() {
                            self.push_block(DocBlock::Heading {
                                level,
                                quote,
                                spans,
                            });
                        }
                    }
                    "blockquote" => {
                        self.flush_paragraph(quote, list_level);
                        let next_quote = (quote + 1).min(8);
                        self.walk_children(
                            node,
                            depth,
                            style.clone(),
                            next_quote,
                            list_level,
                            false,
                        )?;
                        self.flush_paragraph(next_quote, list_level);
                    }
                    "pre" => {
                        self.flush_paragraph(quote, list_level);
                        let mut code_text = String::new();
                        let mut language = self.extract_code_language(&attrs_vec);
                        self.collect_pre_text(node, depth, &mut code_text, &mut language)?;
                        let cleaned = normalize_pre_text(&code_text);
                        if !cleaned.is_empty() {
                            if language.is_empty() {
                                language = self.default_code_lang.to_owned();
                            }
                            self.push_block(DocBlock::Code {
                                quote,
                                language,
                                text: cleaned,
                            });
                        }
                    }
                    "ul" | "ol" => {
                        self.flush_paragraph(quote, list_level);
                        let ordered = tag == "ol";
                        let mut counter = if ordered {
                            attrs_vec
                                .iter()
                                .find(|a| a.name.local.as_ref().eq_ignore_ascii_case("start"))
                                .and_then(|a| a.value.trim().parse::<usize>().ok())
                                .filter(|&n| n > 0)
                                .unwrap_or(1)
                        } else {
                            0
                        };
                        let children = node.children.borrow().clone();
                        for child in children {
                            let is_li = matches!(
                                &child.data,
                                DomNodeData::Element { name, .. }
                                    if name.local.as_ref().eq_ignore_ascii_case("li")
                            );
                            if is_li {
                                let num = if ordered {
                                    let n = counter;
                                    counter = counter.saturating_add(1);
                                    n
                                } else {
                                    0
                                };
                                self.lower_list_item(
                                    &child,
                                    depth + 1,
                                    style,
                                    quote,
                                    list_level,
                                    num,
                                )?;
                            } else {
                                self.walk_node(&child, depth + 1, style, quote, list_level, false)?;
                            }
                            if self.budget_exceeded || self.fidelity_failed {
                                return Ok(());
                            }
                        }
                        self.flush_paragraph(quote, list_level);
                    }
                    "li" => {
                        self.lower_list_item(node, depth, style, quote, list_level, 0)?;
                    }
                    "dl" => {
                        self.flush_paragraph(quote, list_level);
                        self.lower_definition_list(node, depth, style, quote, list_level)?;
                    }
                    "table" => {
                        self.flush_paragraph(quote, list_level);
                        self.lower_table(node, depth, style, quote)?;
                    }
                    "b" | "strong" => {
                        let mut next = style.clone();
                        next.bold = true;
                        self.walk_children(node, depth, next, quote, list_level, in_pre)?;
                    }
                    "i" | "em" | "var" | "cite" | "dfn" => {
                        let mut next = style.clone();
                        next.italic = true;
                        self.walk_children(node, depth, next, quote, list_level, in_pre)?;
                    }
                    "s" | "del" | "strike" => {
                        let mut next = style.clone();
                        next.strike = true;
                        self.walk_children(node, depth, next, quote, list_level, in_pre)?;
                    }
                    "code" | "tt" | "kbd" | "samp" => {
                        let mut next = style.clone();
                        next.code = true;
                        self.walk_children(node, depth, next, quote, list_level, in_pre)?;
                    }
                    "a" => {
                        let mut next = style.clone();
                        if next.link.is_none() {
                            if let Some(href) = attrs_vec
                                .iter()
                                .find(|a| a.name.local.as_ref().eq_ignore_ascii_case("href"))
                            {
                                next.link = classify_href(&href.value);
                            }
                        }
                        self.walk_children(node, depth, next, quote, list_level, in_pre)?;
                    }
                    "img" => {
                        if let Some(alt) = attrs_vec
                            .iter()
                            .find(|a| a.name.local.as_ref().eq_ignore_ascii_case("alt"))
                        {
                            self.push_collapsed_text(&alt.value, style);
                        }
                    }
                    _ => {
                        // Unknown or benign phrasing/container tags: unwrap safely while keeping readable text.
                        self.walk_children(node, depth, style.clone(), quote, list_level, in_pre)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn emit_slot(
        &mut self,
        attrs: &[Attribute],
        style: &InlineStyle,
        quote: usize,
        list_level: usize,
    ) {
        let Some(index) = attrs
            .iter()
            .find(|a| a.name.local.as_ref() == "data-i")
            .and_then(|a| a.value.parse::<usize>().ok())
        else {
            self.fidelity_failed = true;
            return;
        };
        let Some(slot) = self.slots.get(index) else {
            self.fidelity_failed = true;
            return;
        };
        self.slot_hits[index] = self.slot_hits[index].saturating_add(1);
        match slot {
            ProtectedSlot::Inline(span) => {
                self.push_span(DocSpan {
                    text: span.text.clone(),
                    bold: style.bold || span.bold,
                    italic: style.italic || span.italic,
                    strike: style.strike || span.strike,
                    code: style.code || span.code,
                    link: span.link.clone().or_else(|| style.link.clone()),
                });
            }
            ProtectedSlot::CodeBlock { language, text } => {
                self.flush_paragraph(quote, list_level);
                let lang = if language.is_empty() {
                    self.default_code_lang.to_owned()
                } else {
                    language.clone()
                };
                self.push_block(DocBlock::Code {
                    quote,
                    language: lang,
                    text: text.clone(),
                });
            }
        }
    }

    fn extract_code_language(&self, attrs: &[Attribute]) -> String {
        for attr in attrs {
            if attr.name.local.as_ref().eq_ignore_ascii_case("class") {
                for token in attr.value.split_whitespace() {
                    if let Some(lang) = token
                        .strip_prefix("language-")
                        .or_else(|| token.strip_prefix("lang-"))
                    {
                        if !lang.is_empty() {
                            return lang.to_ascii_lowercase();
                        }
                    }
                }
            }
        }
        String::new()
    }

    fn collect_pre_text(
        &mut self,
        node: &Handle,
        depth: usize,
        out: &mut String,
        language: &mut String,
    ) -> Result<(), CoreError> {
        cancellation::check()?;
        if depth > MAX_DOM_DEPTH || out.len() > MAX_TEXT_BYTES {
            self.budget_exceeded = true;
            return Ok(());
        }
        let children = node.children.borrow().clone();
        for child in children {
            match &child.data {
                DomNodeData::Text { contents } => {
                    let t = contents.borrow();
                    if t.contains(self.slot_tag) {
                        self.fidelity_failed = true;
                        return Ok(());
                    }
                    out.push_str(&t);
                }
                DomNodeData::Element { name, attrs, .. } => {
                    let tag = name.local.as_ref().to_ascii_lowercase();
                    let attrs_vec = attrs.borrow().clone();
                    if tag == self.slot_tag {
                        let Some(index) = attrs_vec
                            .iter()
                            .find(|a| a.name.local.as_ref() == "data-i")
                            .and_then(|a| a.value.parse::<usize>().ok())
                        else {
                            self.fidelity_failed = true;
                            return Ok(());
                        };
                        let Some(slot) = self.slots.get(index) else {
                            self.fidelity_failed = true;
                            return Ok(());
                        };
                        self.slot_hits[index] = self.slot_hits[index].saturating_add(1);
                        match slot {
                            ProtectedSlot::Inline(span) => out.push_str(&span.text),
                            ProtectedSlot::CodeBlock { text, .. } => out.push_str(text),
                        }
                    } else if is_dropped_dangerous_tag(&tag) {
                        self.mark_dropped_slots_in_subtree(&child);
                    } else if tag == "br" {
                        out.push('\n');
                    } else {
                        if tag == "code" && language.is_empty() {
                            *language = self.extract_code_language(&attrs_vec);
                        }
                        self.collect_pre_text(&child, depth + 1, out, language)?;
                    }
                }
                _ => {}
            }
            if self.budget_exceeded || self.fidelity_failed {
                return Ok(());
            }
        }
        Ok(())
    }

    fn lower_list_item(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &InlineStyle,
        quote: usize,
        list_level: usize,
        number: usize,
    ) -> Result<(), CoreError> {
        self.flush_paragraph(quote, list_level);
        if let DomNodeData::Element { attrs, .. } = &node.data {
            let attrs_vec = attrs.borrow().clone();
            self.record_anchor(&attrs_vec);
        }
        let children = node.children.borrow().clone();
        let mut emitted_item = false;
        let next_level = (list_level + 1).min(8);

        for child in children {
            let is_block_elem = matches!(
                &child.data,
                DomNodeData::Element { name, .. }
                    if matches!(
                        name.local.as_ref().to_ascii_lowercase().as_str(),
                        "ul" | "ol" | "pre" | "blockquote" | "table" | "hr" | "p" | "div" | "dl"
                    )
            );
            if is_block_elem {
                if !emitted_item {
                    let spans = self.take_trimmed_spans();
                    if !spans.is_empty() {
                        self.push_block(DocBlock::Item {
                            quote,
                            level: list_level,
                            number,
                            spans,
                        });
                        emitted_item = true;
                    }
                } else {
                    self.flush_paragraph(quote, next_level);
                }
                let is_p = matches!(
                    &child.data,
                    DomNodeData::Element { name, .. }
                        if name.local.as_ref().eq_ignore_ascii_case("p")
                );
                if is_p && !emitted_item {
                    self.walk_children(&child, depth + 1, style.clone(), quote, next_level, false)?;
                    let spans = self.take_trimmed_spans();
                    if !spans.is_empty() {
                        self.push_block(DocBlock::Item {
                            quote,
                            level: list_level,
                            number,
                            spans,
                        });
                        emitted_item = true;
                    }
                } else {
                    self.walk_node(&child, depth + 1, style, quote, next_level, false)?;
                }
            } else {
                self.walk_node(&child, depth + 1, style, quote, next_level, false)?;
            }
            if self.budget_exceeded || self.fidelity_failed {
                return Ok(());
            }
        }
        let spans = self.take_trimmed_spans();
        if !spans.is_empty() {
            if !emitted_item {
                self.push_block(DocBlock::Item {
                    quote,
                    level: list_level,
                    number,
                    spans,
                });
            } else {
                self.push_block(DocBlock::Paragraph {
                    quote,
                    level: next_level,
                    spans,
                });
            }
        }
        Ok(())
    }

    fn lower_definition_list(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &InlineStyle,
        quote: usize,
        list_level: usize,
    ) -> Result<(), CoreError> {
        let children = node.children.borrow().clone();
        let mut current_term: Vec<DocSpan> = Vec::new();
        for child in children {
            if let DomNodeData::Element { name, .. } = &child.data {
                let tag = name.local.as_ref().to_ascii_lowercase();
                if tag == "dt" {
                    if !current_term.is_empty() {
                        let term = mem::take(&mut current_term);
                        self.push_block(DocBlock::Labeled {
                            tag_kind: "custom".to_owned(),
                            label_text: spans_plain_text(&term),
                            label_spans: term,
                            spans: Vec::new(),
                        });
                    }
                    current_term = self.lower_subtree_as_inline_spans(
                        &child,
                        depth + 1,
                        style,
                        quote,
                        list_level,
                    )?;
                    continue;
                }
                if tag == "dd" {
                    let value = self.lower_subtree_as_inline_spans(
                        &child,
                        depth + 1,
                        style,
                        quote,
                        list_level,
                    )?;
                    let term = mem::take(&mut current_term);
                    if !term.is_empty() || !value.is_empty() {
                        self.push_block(DocBlock::Labeled {
                            tag_kind: "custom".to_owned(),
                            label_text: spans_plain_text(&term),
                            label_spans: term,
                            spans: value,
                        });
                    }
                    continue;
                }
            }
            self.walk_node(&child, depth + 1, style, quote, list_level, false)?;
        }
        if !current_term.is_empty() {
            let term = mem::take(&mut current_term);
            self.push_block(DocBlock::Labeled {
                tag_kind: "custom".to_owned(),
                label_text: spans_plain_text(&term),
                label_spans: term,
                spans: Vec::new(),
            });
        }
        Ok(())
    }

    /// Lowers a container subtree (such as `<dt>`, `<dd>`, or `<td>`) to inline
    /// spans, folding any nested block elements (e.g. `<p>` inside `<dd>`) back
    /// into inline runs instead of leaking them ahead of the enclosing block.
    fn lower_subtree_as_inline_spans(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &InlineStyle,
        quote: usize,
        list_level: usize,
    ) -> Result<Vec<DocSpan>, CoreError> {
        let saved_buf = mem::take(&mut self.inline_buf);
        let saved_blocks_len = self.blocks.len();
        self.walk_children(node, depth, style.clone(), quote, list_level, false)?;
        let mut tail_spans = self.take_trimmed_spans();
        if self.blocks.len() > saved_blocks_len {
            let extra: Vec<DocBlock> = self.blocks.drain(saved_blocks_len..).collect();
            let mut merged = Vec::new();
            for blk in extra {
                append_block_as_inline_spans(blk, &mut merged);
            }
            if !tail_spans.is_empty() {
                if !merged.is_empty() {
                    merged.push(DocSpan {
                        text: "\n".to_owned(),
                        bold: false,
                        italic: false,
                        strike: false,
                        code: false,
                        link: None,
                    });
                }
                merged.extend(tail_spans);
            }
            tail_spans = merged;
        }
        self.inline_buf = saved_buf;
        trim_edge_whitespace(&mut tail_spans);
        Ok(tail_spans)
    }

    fn lower_table(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &InlineStyle,
        quote: usize,
    ) -> Result<(), CoreError> {
        let mut raw_rows: Vec<Handle> = Vec::new();
        collect_table_rows(node, &mut raw_rows, 0);
        let mut rows: Vec<Vec<DocTableCell>> = Vec::new();
        let mut max_cols = 0_usize;

        for tr in raw_rows {
            cancellation::check()?;
            let mut cells: Vec<DocTableCell> = Vec::new();
            let children = tr.children.borrow().clone();
            for child in children {
                let is_cell = matches!(
                    &child.data,
                    DomNodeData::Element { name, .. }
                        if matches!(name.local.as_ref().to_ascii_lowercase().as_str(), "td" | "th")
                );
                if !is_cell {
                    continue;
                }
                if cells.len() >= 24 {
                    break;
                }
                let align = if let DomNodeData::Element { attrs, .. } = &child.data {
                    attrs
                        .borrow()
                        .iter()
                        .find(|a| a.name.local.as_ref().eq_ignore_ascii_case("align"))
                        .map(|a| match a.value.trim().to_ascii_lowercase().as_str() {
                            "center" => "center".to_owned(),
                            "right" => "right".to_owned(),
                            _ => "left".to_owned(),
                        })
                        .unwrap_or_else(|| "left".to_owned())
                } else {
                    "left".to_owned()
                };
                let saved_buf = mem::take(&mut self.inline_buf);
                let saved_blocks_len = self.blocks.len();
                self.walk_children(&child, depth + 1, style.clone(), 0, 0, false)?;
                let mut cell_spans = self.take_trimmed_spans();
                // If nested block elements inside a cell emitted blocks, fold their text spans back into the cell.
                if self.blocks.len() > saved_blocks_len {
                    let extra: Vec<DocBlock> = self.blocks.drain(saved_blocks_len..).collect();
                    let mut merged = Vec::new();
                    for blk in extra {
                        append_block_as_inline_spans(blk, &mut merged);
                    }
                    if !cell_spans.is_empty() {
                        if !merged.is_empty() {
                            merged.push(DocSpan {
                                text: "\n".to_owned(),
                                bold: false,
                                italic: false,
                                strike: false,
                                code: false,
                                link: None,
                            });
                        }
                        merged.extend(cell_spans);
                    }
                    cell_spans = merged;
                }
                self.inline_buf = saved_buf;
                cells.push(DocTableCell {
                    align,
                    spans: cell_spans,
                });
                if self.budget_exceeded || self.fidelity_failed {
                    return Ok(());
                }
            }
            if !cells.is_empty() {
                max_cols = max_cols.max(cells.len());
                rows.push(cells);
            }
        }
        if rows.is_empty() || max_cols == 0 {
            return Ok(());
        }
        for row in &mut rows {
            while row.len() < max_cols {
                row.push(DocTableCell {
                    align: "left".to_owned(),
                    spans: Vec::new(),
                });
            }
        }
        self.push_block(DocBlock::Table {
            quote,
            columns: max_cols,
            rows,
        });
        Ok(())
    }

    fn mark_dropped_slots_in_subtree(&mut self, node: &Handle) {
        let mut stack = vec![node.clone()];
        while let Some(curr) = stack.pop() {
            match &curr.data {
                DomNodeData::Element { name, attrs, .. } => {
                    let tag = name.local.as_ref().to_ascii_lowercase();
                    if tag == self.slot_tag {
                        if let Some(idx) = attrs
                            .borrow()
                            .iter()
                            .find(|a| a.name.local.as_ref() == "data-i")
                            .and_then(|a| a.value.parse::<usize>().ok())
                        {
                            if let Some(flag) = self.slot_dropped.get_mut(idx) {
                                *flag = true;
                            }
                        }
                    }
                }
                DomNodeData::Text { contents } => {
                    let text = contents.borrow();
                    let prefix = format!("<{} data-i=\"", self.slot_tag);
                    let mut rest = text.as_ref();
                    while let Some(pos) = rest.find(&prefix) {
                        rest = &rest[pos + prefix.len()..];
                        if let Some(end) = rest.find('"') {
                            if let Ok(idx) = rest[..end].parse::<usize>() {
                                if let Some(flag) = self.slot_dropped.get_mut(idx) {
                                    *flag = true;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            stack.extend(curr.children.borrow().iter().cloned());
        }
    }

    fn subtree_contains_sentinel(&self, node: &Handle) -> bool {
        let mut stack = vec![node.clone()];
        while let Some(curr) = stack.pop() {
            match &curr.data {
                DomNodeData::Text { contents } if contents.borrow().contains(self.slot_tag) => {
                    return true;
                }
                DomNodeData::Element { name, .. }
                    if name.local.as_ref().eq_ignore_ascii_case(self.slot_tag) =>
                {
                    return true;
                }
                _ => {}
            }
            stack.extend(curr.children.borrow().iter().cloned());
        }
        false
    }
}

fn is_dropped_dangerous_tag(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "iframe"
            | "object"
            | "embed"
            | "noscript"
            | "noframes"
            | "noembed"
            | "template"
    )
}

fn collect_table_rows(node: &Handle, rows: &mut Vec<Handle>, depth: usize) {
    if depth > 4 {
        return;
    }
    for child in node.children.borrow().iter() {
        if let DomNodeData::Element { name, .. } = &child.data {
            let tag = name.local.as_ref().to_ascii_lowercase();
            match tag.as_str() {
                "tr" => rows.push(child.clone()),
                "thead" | "tbody" | "tfoot" => collect_table_rows(child, rows, depth + 1),
                _ => {}
            }
        }
    }
}

fn append_block_as_inline_spans(block: DocBlock, dst: &mut Vec<DocSpan>) {
    let needs_break = !dst.is_empty();
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
                    push_break(dst);
                }
                dst.extend(spans);
            }
        }
        DocBlock::Code { text, .. } => {
            if !text.is_empty() {
                if needs_break {
                    push_break(dst);
                }
                dst.push(DocSpan {
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
                push_break(dst);
            }
            if !label_text.is_empty() {
                dst.push(DocSpan {
                    text: format!("{label_text}: "),
                    bold: true,
                    italic: false,
                    strike: false,
                    code: false,
                    link: None,
                });
            }
            dst.extend(spans);
        }
        DocBlock::Rule { .. } | DocBlock::Table { .. } => {}
    }
}

fn spans_plain_text(spans: &[DocSpan]) -> String {
    let mut out = String::new();
    for s in spans {
        out.push_str(&s.text);
    }
    out
}

pub(crate) fn trim_edge_whitespace(spans: &mut Vec<DocSpan>) {
    while let Some(first) = spans.first_mut() {
        if first.code {
            break;
        }
        if first.text == "\n" {
            spans.remove(0);
            continue;
        }
        let trimmed = first.text.trim_start_matches(' ');
        if trimmed.is_empty() {
            spans.remove(0);
            continue;
        }
        if trimmed.len() != first.text.len() {
            first.text = trimmed.to_owned();
        }
        break;
    }
    while let Some(last) = spans.last_mut() {
        if last.code {
            break;
        }
        if last.text == "\n" {
            spans.pop();
            continue;
        }
        let trimmed = last.text.trim_end_matches(' ');
        if trimmed.is_empty() {
            spans.pop();
            continue;
        }
        if trimmed.len() != last.text.len() {
            last.text = trimmed.to_owned();
        }
        break;
    }
}

fn normalize_pre_text(raw: &str) -> String {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let stripped = normalized.strip_prefix('\n').unwrap_or(&normalized);
    let trimmed = stripped.trim_end_matches(['\n', ' ', '\t']);
    // Trim only leading completely empty lines, keeping meaningful indentation of the first code line.
    let mut lines: Vec<&str> = trimmed.split('\n').collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    lines.join("\n")
}
