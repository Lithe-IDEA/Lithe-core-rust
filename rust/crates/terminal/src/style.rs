//! Decodes console output that carries ANSI escape sequences into plain lines with styled ranges.
//!
//! Build tools and run processes colour their output with SGR sequences when asked to (for
//! example Maven with `-Dstyle.color=always`). Hosts keep and show plain text, so copying,
//! searching, output links and the IDE API never see an escape, and draw the styled ranges with
//! their theme. Core owns what the characters mean: which sequences style text, which are dropped,
//! how a carriage return rewrites a line and how long a line may grow. The host owns the palette
//! and how a style is drawn.
//!
//! Decoding keeps no state between calls. Every answer returns the state the next chunk of the
//! same console continues from: an escape sequence the chunk ended inside, the style in effect,
//! a pending carriage return and the line the output has not ended yet. Splitting the same output
//! into different chunks therefore never changes the lines.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

/// Largest chunk one request may carry, in UTF-8 bytes.
const MAX_TEXT_BYTES: usize = 1 << 20;
/// Longest line Core keeps, in UTF-8 bytes; longer output continues on the next line.
pub const MAX_LINE_BYTES: usize = 16 << 10;
/// Most styled ranges one line keeps; text styled after the last of them is plain.
pub const MAX_SPANS_PER_LINE: usize = 512;
/// Longest unfinished escape sequence carried to the next chunk, in UTF-8 bytes.
const MAX_PENDING_BYTES: usize = 4096;

const ESC: char = '\u{1b}';

/// A colour an SGR sequence selected.
///
/// The host maps palette entries 0–15 to its theme's ANSI colours and 16–255 to the xterm
/// 6×6×6 cube and grey ramp. On the wire it is `{"index": n}` or `{"rgb": "#rrggbb"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ColorWire", into = "ColorWire")]
pub enum OutputColor {
    /// A palette entry: 0–7 normal, 8–15 bright, 16–255 the extended palette.
    Index(u8),
    /// A 24-bit colour: red, green and blue.
    Rgb([u8; 3]),
}

/// Wire form of [`OutputColor`]; exactly one member is present.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColorWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index: Option<u8>,
    /// `#rrggbb` in lowercase hexadecimal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rgb: Option<String>,
}

impl From<OutputColor> for ColorWire {
    fn from(color: OutputColor) -> Self {
        match color {
            OutputColor::Index(index) => Self {
                index: Some(index),
                rgb: None,
            },
            OutputColor::Rgb([red, green, blue]) => Self {
                index: None,
                rgb: Some(format!("#{red:02x}{green:02x}{blue:02x}")),
            },
        }
    }
}

impl TryFrom<ColorWire> for OutputColor {
    type Error = String;

    fn try_from(wire: ColorWire) -> Result<Self, Self::Error> {
        match (wire.index, wire.rgb) {
            (Some(index), None) => Ok(Self::Index(index)),
            (None, Some(rgb)) => {
                let digits = rgb
                    .strip_prefix('#')
                    .filter(|digits| digits.len() == 6 && digits.is_ascii())
                    .ok_or_else(|| format!("invalid colour {rgb:?}"))?;
                let channel = |at: usize| {
                    u8::from_str_radix(&digits[at..at + 2], 16)
                        .map_err(|_| format!("invalid colour {rgb:?}"))
                };
                Ok(Self::Rgb([channel(0)?, channel(2)?, channel(4)?]))
            }
            _ => Err("a colour has exactly one of index and rgb".to_owned()),
        }
    }
}

/// The rendition SGR set for a range of text. Absent members are the console's defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputStyle {
    /// Text colour; absent is the console's text colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<OutputColor>,
    /// Background colour; absent is the console's background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<OutputColor>,
    /// Bold (SGR 1).
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    /// Faint (SGR 2): the host draws the text dimmer.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    /// Italic (SGR 3).
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Single or double underline (SGR 4 and 21); the host draws one line.
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Reverse video (SGR 7): the host swaps the text and background colours.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inverse: bool,
    /// Concealed (SGR 8): the host draws the text in its background colour.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Crossed out (SGR 9).
    #[serde(default, skip_serializing_if = "is_false")]
    pub strikethrough: bool,
}

impl OutputStyle {
    fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One styled range of a line. `start` and `length` count UTF-16 code units, as output links do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSpan {
    /// First UTF-16 code unit of the range.
    pub start: u32,
    /// UTF-16 code units in the range; never zero.
    pub length: u32,
    /// How the range is drawn; never the plain style.
    pub style: OutputStyle,
}

/// One line of decoded output: its plain text without the line ending, and its styled ranges
/// in ascending order without overlaps. Text outside every range is plain; adjacent text with
/// the same style shares one range.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputLine {
    /// The line's text without escapes or the line ending.
    pub text: String,
    /// The styled ranges, omitted when the whole line is plain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<OutputSpan>,
}

/// Where one console's output stands after a chunk.
///
/// The host passes it back unchanged with the next chunk of the same console and shows `line`
/// after the finished lines; it reads nothing else in it. Dropping the state ends the console:
/// the open line stays as it is and an unfinished escape sequence is discarded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputStyleState {
    /// The line the output has not ended yet; empty text when the last chunk ended a line.
    #[serde(default)]
    pub line: OutputLine,
    /// The style the next character is drawn with.
    #[serde(default, skip_serializing_if = "OutputStyle::is_plain")]
    pub style: OutputStyle,
    /// The start of an escape sequence the previous chunk ended inside.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pending: String,
    /// A carriage return moved to the start of `line`: the next printed character replaces it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub carriage: bool,
}

/// `outputStyle.decode` request: the next chunk of one console and the state the previous
/// chunk left. A console's first chunk carries no state.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecodeOutputStyleRequest {
    /// The chunk, at most 1 MiB of UTF-8.
    pub text: String,
    /// The state the previous answer for this console returned, unchanged.
    #[serde(default)]
    pub state: Option<OutputStyleState>,
}

/// `outputStyle.decode` answer.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DecodeOutputStyleResponse {
    /// The lines this chunk ended, oldest first; the first continues the request's open line.
    pub lines: Vec<OutputLine>,
    /// The state for the next chunk; its `line` is the line still open.
    pub state: OutputStyleState,
}

/// `outputStyle.decode`: decodes one chunk of console output.
///
/// SGR sequences (`ESC [ … m`) set the style of the text after them, across chunk and line
/// boundaries. `\n` ends a line, `\r` makes the next printed text replace the line (so
/// `\r\n` is a plain line ending and progress rewrites keep only their last state), erase in
/// line (`ESC [ 2 K`, and `ESC [ K` right after a carriage return) clears it, and moving to the
/// first column (`ESC [ G`) acts as `\r`. Every other control sequence, string sequence (OSC,
/// DCS, APC, PM, SOS) and escape, and every control character except tab, is dropped; the text
/// between OSC 8 hyperlink markers is kept. Lines longer than 16 KiB continue on the next line.
///
/// # Errors
///
/// `InvalidRequest` when the chunk is larger than 1 MiB or the state is not one this command
/// returned: an open line over the limits, ranges outside the line or out of order, or a
/// pending sequence that is not an escape.
pub fn decode_output_style(
    request: DecodeOutputStyleRequest,
) -> Result<DecodeOutputStyleResponse, CoreError> {
    if request.text.len() > MAX_TEXT_BYTES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "The output chunk is too large.",
        ));
    }
    let state = request.state.unwrap_or_default();
    let units = validated_units(&state)?;
    let mut input = state.pending;
    input.push_str(&request.text);
    let mut decoder = Decoder {
        line: LineBuilder {
            text: state.line.text,
            units,
            spans: state.line.spans,
        },
        style: state.style,
        carriage: state.carriage,
        finished: Vec::new(),
    };
    let pending = decoder.run(&input);
    Ok(DecodeOutputStyleResponse {
        lines: decoder.finished,
        state: OutputStyleState {
            line: decoder.line.take(),
            style: decoder.style,
            pending,
            carriage: decoder.carriage,
        },
    })
}

/// Checks a state the host passed back and returns the UTF-16 length of its open line.
fn validated_units(state: &OutputStyleState) -> Result<u32, CoreError> {
    let invalid = || {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "The output style state is invalid.",
        )
    };
    let line = &state.line;
    if line.text.len() > MAX_LINE_BYTES
        || line.spans.len() > MAX_SPANS_PER_LINE
        || state.pending.len() > MAX_PENDING_BYTES
        || !(state.pending.is_empty() || state.pending.starts_with(ESC))
    {
        return Err(invalid());
    }
    let units = line.text.encode_utf16().count() as u32;
    let mut end = 0u32;
    for span in &line.spans {
        if span.length == 0
            || span.start < end
            || span.style.is_plain()
            || u64::from(span.start) + u64::from(span.length) > u64::from(units)
        {
            return Err(invalid());
        }
        end = span.start + span.length;
    }
    Ok(units)
}

/// The open line while a chunk is decoded. Its UTF-16 length is kept so a range never needs
/// a rescan of the text.
struct LineBuilder {
    text: String,
    units: u32,
    spans: Vec<OutputSpan>,
}

impl LineBuilder {
    fn clear(&mut self) {
        self.text.clear();
        self.units = 0;
        self.spans.clear();
    }

    fn take(&mut self) -> OutputLine {
        self.units = 0;
        OutputLine {
            text: std::mem::take(&mut self.text),
            spans: std::mem::take(&mut self.spans),
        }
    }

    /// Appends one character drawn with `style`, extending the last range when it ends here
    /// with the same style.
    fn push(&mut self, ch: char, style: &OutputStyle) {
        let width = ch.len_utf16() as u32;
        self.text.push(ch);
        if !style.is_plain() {
            let count = self.spans.len();
            match self.spans.last_mut() {
                Some(last) if last.start + last.length == self.units && last.style == *style => {
                    last.length += width;
                }
                _ if count < MAX_SPANS_PER_LINE => self.spans.push(OutputSpan {
                    start: self.units,
                    length: width,
                    style: *style,
                }),
                _ => {}
            }
        }
        self.units += width;
    }
}

/// Decoding state for one call.
struct Decoder {
    line: LineBuilder,
    style: OutputStyle,
    carriage: bool,
    finished: Vec<OutputLine>,
}

/// What an escape character at some position starts.
enum Scan<'a> {
    /// The input ended inside the sequence. `string` is true for a string sequence (OSC, DCS,
    /// APC, PM, SOS), whose contents up to the terminator are dropped.
    Incomplete { string: bool },
    /// A complete control sequence (CSI): parameter bytes, intermediate bytes and final byte.
    Control {
        parameters: &'a str,
        intermediates: &'a str,
        final_byte: u8,
        end: usize,
    },
    /// Any other sequence, or one a byte that cannot belong to it ended; it is dropped and
    /// decoding resumes at `end`.
    Skip { end: usize },
}

impl Decoder {
    /// Decodes `input` and returns the unfinished escape sequence it ends inside, if any.
    fn run(&mut self, input: &str) -> String {
        let mut index = 0;
        while let Some(ch) = input[index..].chars().next() {
            if ch == ESC {
                match scan_escape(input, index) {
                    Scan::Incomplete { string } => return pending(&input[index..], string),
                    Scan::Control {
                        parameters,
                        intermediates,
                        final_byte,
                        end,
                    } => {
                        self.control(parameters, intermediates, final_byte);
                        index = end;
                    }
                    Scan::Skip { end } => index = end,
                }
                continue;
            }
            match ch {
                '\n' => {
                    self.finished.push(self.line.take());
                    self.carriage = false;
                }
                '\r' => self.carriage = true,
                '\t' => self.print(ch),
                // C0 controls, DEL and C1 controls (BEL, backspace, …) have no drawing.
                _ if ch.is_control() => {}
                _ => self.print(ch),
            }
            index += ch.len_utf8();
        }
        String::new()
    }

    fn print(&mut self, ch: char) {
        if self.carriage {
            self.line.clear();
            self.carriage = false;
        }
        if self.line.text.len() + ch.len_utf8() > MAX_LINE_BYTES {
            self.finished.push(self.line.take());
        }
        self.line.push(ch, &self.style);
    }

    fn control(&mut self, parameters: &str, intermediates: &str, final_byte: u8) {
        // Private sequences (a leading `<`, `=`, `>` or `?`, such as xterm's `CSI > 4 ; 2 m`)
        // and sequences with intermediates are not SGR, EL or CHA whatever their final byte.
        if !intermediates.is_empty() || parameters.starts_with(['<', '=', '>', '?']) {
            return;
        }
        match final_byte {
            b'm' => apply_sgr(&mut self.style, parameters),
            // Erase in line. After a carriage return the cursor is at the first column, so
            // erasing to the end clears the whole line.
            b'K' => match parameters {
                "" | "0" if self.carriage => {
                    self.line.clear();
                    self.carriage = false;
                }
                "2" => self.line.clear(),
                _ => {}
            },
            // Cursor to the first column acts as a carriage return.
            b'G' if matches!(parameters, "" | "0" | "1") => self.carriage = true,
            _ => {}
        }
    }
}

/// Reads the escape sequence that starts at `start` (an ESC character). Every byte it looks at
/// past the ESC is ASCII, so each position it returns is a character boundary.
fn scan_escape(input: &str, start: usize) -> Scan<'_> {
    let bytes = input.as_bytes();
    let Some(&kind) = bytes.get(start + 1) else {
        return Scan::Incomplete { string: false };
    };
    match kind {
        b'[' => {
            let parameters_start = start + 2;
            let mut index = parameters_start;
            while bytes
                .get(index)
                .is_some_and(|byte| (0x30..=0x3f).contains(byte))
            {
                index += 1;
            }
            let parameters_end = index;
            while bytes
                .get(index)
                .is_some_and(|byte| (0x20..=0x2f).contains(byte))
            {
                index += 1;
            }
            match bytes.get(index) {
                None => Scan::Incomplete { string: false },
                Some(&final_byte) if (0x40..=0x7e).contains(&final_byte) => Scan::Control {
                    parameters: &input[parameters_start..parameters_end],
                    intermediates: &input[parameters_end..index],
                    final_byte,
                    end: index + 1,
                },
                // A byte that cannot belong to a control sequence ends it unfinished and is
                // decoded again as output.
                Some(_) => Scan::Skip { end: index },
            }
        }
        b']' | b'P' | b'X' | b'^' | b'_' => {
            let mut index = start + 2;
            while let Some(&byte) = bytes.get(index) {
                match byte {
                    // BEL ends OSC in xterm's form; ST is `ESC \`.
                    0x07 => return Scan::Skip { end: index + 1 },
                    0x1b => {
                        return match bytes.get(index + 1) {
                            Some(b'\\') => Scan::Skip { end: index + 2 },
                            // Another escape cancels the string; it is decoded on its own.
                            Some(_) => Scan::Skip { end: index },
                            None => Scan::Incomplete { string: true },
                        };
                    }
                    _ => index += 1,
                }
            }
            Scan::Incomplete { string: true }
        }
        0x20..=0x2f => {
            let mut index = start + 2;
            while bytes
                .get(index)
                .is_some_and(|byte| (0x20..=0x2f).contains(byte))
            {
                index += 1;
            }
            match bytes.get(index) {
                None => Scan::Incomplete { string: false },
                Some(0x30..=0x7e) => Scan::Skip { end: index + 1 },
                Some(_) => Scan::Skip { end: index },
            }
        }
        0x30..=0x7e => Scan::Skip { end: start + 2 },
        // A lone ESC: the character after it is decoded again as output.
        _ => Scan::Skip { end: start + 1 },
    }
}

/// The unfinished sequence carried to the next chunk. A string sequence over the limit keeps
/// only its introducer (and a trailing ESC that may start its terminator), so the next chunk
/// still drops the rest of it; any other sequence over the limit is dropped.
fn pending(tail: &str, string: bool) -> String {
    if tail.len() <= MAX_PENDING_BYTES {
        return tail.to_owned();
    }
    if !string {
        return String::new();
    }
    let mut kept = tail[..2].to_owned();
    if tail.ends_with(ESC) {
        kept.push(ESC);
    }
    kept
}

/// A decimal SGR parameter; an empty one is 0, as ECMA-48 defines.
fn number(text: &str) -> Option<u32> {
    if text.is_empty() {
        return Some(0);
    }
    if text.len() > 5 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn apply_sgr(style: &mut OutputStyle, parameters: &str) {
    let groups: Vec<&str> = parameters.split(';').collect();
    let mut index = 0;
    while index < groups.len() {
        let mut parts = groups[index].split(':');
        index += 1;
        let Some(code) = parts.next().and_then(number) else {
            continue;
        };
        match code {
            0 => *style = OutputStyle::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            // `4:0` is "no underline"; `4:1`–`4:5` are underline styles.
            4 => style.underline = parts.next().and_then(number) != Some(0),
            7 => style.inverse = true,
            8 => style.hidden = true,
            9 => style.strikethrough = true,
            21 => style.underline = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.inverse = false,
            28 => style.hidden = false,
            29 => style.strikethrough = false,
            30..=37 => style.foreground = Some(OutputColor::Index((code - 30) as u8)),
            39 => style.foreground = None,
            40..=47 => style.background = Some(OutputColor::Index((code - 40) as u8)),
            49 => style.background = None,
            90..=97 => style.foreground = Some(OutputColor::Index((code - 90 + 8) as u8)),
            100..=107 => style.background = Some(OutputColor::Index((code - 100 + 8) as u8)),
            38 | 48 | 58 => {
                let subparameters: Vec<&str> = parts.collect();
                let color = if subparameters.is_empty() {
                    // Semicolon form: the colour's values are the next parameters.
                    let (color, used) = extended_color(&groups[index..], false);
                    index += used;
                    color
                } else {
                    extended_color(&subparameters, true).0
                };
                match (code, color) {
                    (38, Some(color)) => style.foreground = Some(color),
                    (48, Some(color)) => style.background = Some(color),
                    // 58 sets the underline colour, which hosts do not draw; its values are
                    // still consumed so they are not read as other parameters.
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Reads an extended colour (`5;n` or `2;r;g;b`) from the values after 38, 48 or 58, and
/// returns it with the number of values it used. In the colon form the values are the
/// subparameters, all of which belong to the colour.
fn extended_color(values: &[&str], colon: bool) -> (Option<OutputColor>, usize) {
    let channel = |value: Option<&&str>| {
        value
            .and_then(|value| number(value))
            .and_then(|value| u8::try_from(value).ok())
    };
    match values.first().and_then(|mode| number(mode)) {
        Some(5) => {
            let color = channel(values.get(1)).map(OutputColor::Index);
            (
                color,
                if colon {
                    values.len()
                } else {
                    values.len().min(2)
                },
            )
        }
        Some(2) => {
            // ITU T.416 puts a colour space ID before the components in the colon form
            // (`38:2::r:g:b`); xterm also accepts `38:2:r:g:b`.
            let components = if colon && values.len() >= 5 {
                &values[2..]
            } else {
                &values[1..]
            };
            let color = (|| {
                Some(OutputColor::Rgb([
                    channel(components.first())?,
                    channel(components.get(1))?,
                    channel(components.get(2))?,
                ]))
            })();
            (
                color,
                if colon {
                    values.len()
                } else {
                    values.len().min(4)
                },
            )
        }
        _ => (
            None,
            if colon {
                values.len()
            } else {
                values.len().min(1)
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&str]) -> (Vec<OutputLine>, OutputStyleState) {
        let mut lines = Vec::new();
        let mut state = None;
        for chunk in chunks {
            let response = decode_output_style(DecodeOutputStyleRequest {
                text: (*chunk).to_owned(),
                state: state.take(),
            })
            .unwrap();
            lines.extend(response.lines);
            state = Some(response.state);
        }
        (lines, state.unwrap())
    }

    fn style(foreground: u8, bold: bool) -> OutputStyle {
        OutputStyle {
            foreground: Some(OutputColor::Index(foreground)),
            bold,
            ..OutputStyle::default()
        }
    }

    #[test]
    fn every_split_of_a_chunk_decodes_the_same_lines() {
        // Guards the carried state: an escape, a CRLF or a multi-byte character cut at any
        // point must not change the result.
        let text = "[\u{1b}[1;34mINFO\u{1b}[m] 构建 \u{1b}[38;2;1;2;3m✓\u{1b}]8;;x\u{1b}\\\r\nnext 10%\rnext 100%\n";
        let (whole, whole_state) = decode(&[text]);
        for split in (1..text.len()).filter(|at| text.is_char_boundary(*at)) {
            let (lines, state) = decode(&[&text[..split], &text[split..]]);
            assert_eq!(lines, whole, "split at {split}");
            assert_eq!(state, whole_state, "split at {split}");
        }
    }

    #[test]
    fn a_long_line_continues_on_the_next_line_with_its_style() {
        let text = format!("\u{1b}[31m{}\n", "a".repeat(MAX_LINE_BYTES + 3));
        let (lines, _) = decode(&[&text]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text.len(), MAX_LINE_BYTES);
        assert_eq!(lines[1].text, "aaa");
        assert_eq!(lines[1].spans[0].style, style(1, false));
    }

    #[test]
    fn a_line_never_keeps_more_ranges_than_the_limit() {
        let text = "\u{1b}[31ma\u{1b}[32mb".repeat(MAX_SPANS_PER_LINE);
        let (_, state) = decode(&[&text]);
        assert_eq!(state.line.spans.len(), MAX_SPANS_PER_LINE);
        assert_eq!(state.line.text.len(), MAX_SPANS_PER_LINE * 2);
    }

    #[test]
    fn an_overlong_string_sequence_is_still_dropped_up_to_its_terminator() {
        let start = format!("a\u{1b}]8;;{}", "u".repeat(MAX_PENDING_BYTES));
        let (_, state) = decode(&[&start]);
        assert_eq!(state.pending, "\u{1b}]");
        let (lines, _) = decode(&[&start, "rest\u{7}b\n"]);
        assert_eq!(lines[0].text, "ab");
    }

    #[test]
    fn semicolon_colours_consume_their_values() {
        // `38;5;1` must not read its `1` as bold, nor `48;2;…;4` its 4 as underline.
        let (lines, _) = decode(&["\u{1b}[38;5;1;48;2;9;8;4mx\n"]);
        assert_eq!(
            lines[0].spans[0].style,
            OutputStyle {
                foreground: Some(OutputColor::Index(1)),
                background: Some(OutputColor::Rgb([9, 8, 4])),
                ..OutputStyle::default()
            }
        );
    }

    #[test]
    fn a_state_with_ranges_outside_its_line_is_rejected() {
        let error = decode_output_style(DecodeOutputStyleRequest {
            text: String::new(),
            state: Some(OutputStyleState {
                line: OutputLine {
                    text: "ab".to_owned(),
                    spans: vec![OutputSpan {
                        start: 1,
                        length: 2,
                        style: style(1, false),
                    }],
                },
                ..OutputStyleState::default()
            }),
        })
        .unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
    }

    #[test]
    fn colours_cross_the_wire_as_index_or_hex() {
        let color: OutputColor = serde_json::from_str(r##"{"rgb":"#0a0B0c"}"##).unwrap();
        assert_eq!(color, OutputColor::Rgb([10, 11, 12]));
        assert_eq!(
            serde_json::to_string(&color).unwrap(),
            r##"{"rgb":"#0a0b0c"}"##
        );
        assert!(serde_json::from_str::<OutputColor>(r##"{"index":1,"rgb":"#000000"}"##).is_err());
        assert!(serde_json::from_str::<OutputColor>(r##"{"rgb":"#00000g"}"##).is_err());
    }
}
