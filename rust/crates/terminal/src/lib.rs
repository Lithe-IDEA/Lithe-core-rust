//! Terminal output interpretation shared by the native hosts.

mod encoding;
mod link;
mod style;

pub use encoding::{
    resolve_output_encoding, OutputEncoding, OutputEncodingPolicy, ResolveOutputEncodingResponse,
    OUTPUT_ENCODINGS,
};
pub use link::{parse_link_targets, ParseLinkTargetsRequest};
pub use style::{
    decode_output_style, DecodeOutputStyleRequest, DecodeOutputStyleResponse, OutputColor,
    OutputLine, OutputSpan, OutputStyle, OutputStyleState, MAX_LINE_BYTES, MAX_SPANS_PER_LINE,
};
