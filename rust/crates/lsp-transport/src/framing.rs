//! Bounded encoding and incremental parsing of LSP transport frames.

use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// JSON-RPC message to encode with the LSP content-length framing protocol.
pub struct FrameMessageRequest {
    /// Serialized JSON-RPC message body.
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Complete transport frame ready to write to a language server.
pub struct FrameMessageResponse {
    /// `Content-Length` header, blank line and body.
    pub frame: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Previously buffered bytes and the newest process-output chunk.
pub struct ParseServerMessagesRequest {
    /// Unconsumed bytes returned by the previous parse.
    #[serde(default)]
    pub buffer: Vec<u8>,
    /// Bytes read from the server since the previous parse.
    #[serde(default)]
    pub chunk: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Complete JSON-RPC messages plus the unconsumed partial frame.
pub struct ParseServerMessagesResponse {
    /// Bytes of an incomplete trailing frame, passed back with the next chunk.
    pub buffer: Vec<u8>,
    /// Bodies of the complete frames, in arrival order.
    pub messages: Vec<String>,
}
/// Encodes one JSON-RPC message with its UTF-8 byte length.
pub fn frame_message(request: FrameMessageRequest) -> Result<FrameMessageResponse, CoreError> {
    if request.message.contains('\0') {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "LSP message frame cannot contain NUL bytes.",
        ));
    }
    Ok(FrameMessageResponse {
        frame: format!(
            "Content-Length: {}\r\n\r\n{}",
            request.message.len(),
            request.message
        ),
    })
}

/// Incrementally extracts bounded, complete LSP messages from process output.
pub fn parse_server_messages(
    request: ParseServerMessagesRequest,
) -> Result<ParseServerMessagesResponse, CoreError> {
    let mut buffer = request.buffer;
    buffer.extend(request.chunk);
    let mut messages = Vec::new();

    loop {
        let Some(header_end) = find_header_end(&buffer) else {
            if buffer.len() > MAX_HEADER_BYTES {
                return Err(transport_error("LSP header exceeded the maximum size."));
            }
            break;
        };
        if header_end > MAX_HEADER_BYTES {
            return Err(transport_error("LSP header exceeded the maximum size."));
        }
        let header = String::from_utf8_lossy(&buffer[..header_end]);
        let content_length = content_length_from_header(&header)?;
        if content_length > MAX_MESSAGE_BYTES {
            return Err(transport_error("LSP message exceeded the maximum size."));
        }
        let body_start = header_end + 4;
        let body_end = body_start
            .checked_add(content_length)
            .ok_or_else(|| transport_error("LSP Content-Length overflowed."))?;
        if buffer.len() < body_end {
            break;
        }
        let body = buffer[body_start..body_end].to_vec();
        buffer.drain(..body_end);
        let message = String::from_utf8(body).map_err(|error| {
            transport_error("LSP message body was not valid UTF-8.").with_details(error.to_string())
        })?;
        messages.push(message);
    }

    Ok(ParseServerMessagesResponse { buffer, messages })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length_from_header(header: &str) -> Result<usize, CoreError> {
    let value = header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            Some(value.trim())
        } else {
            None
        }
    });
    let Some(value) = value else {
        return Err(transport_error("LSP frame did not contain Content-Length."));
    };
    value.parse::<usize>().map_err(|error| {
        transport_error("LSP Content-Length was not a valid non-negative integer.")
            .with_details(error.to_string())
    })
}

fn transport_error(message: &str) -> CoreError {
    CoreError::new(ErrorCode::ParseFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_content_length_is_a_transport_error() {
        let missing = parse_server_messages(ParseServerMessagesRequest {
            buffer: Vec::new(),
            chunk: b"Content-Type: application/json\r\n\r\n{}".to_vec(),
        });
        assert!(missing.is_err());

        let invalid = parse_server_messages(ParseServerMessagesRequest {
            buffer: Vec::new(),
            chunk: b"Content-Length: nope\r\n\r\n{}".to_vec(),
        });
        assert!(invalid.is_err());
    }

    #[test]
    fn invalid_utf8_body_is_a_transport_error() {
        let result = parse_server_messages(ParseServerMessagesRequest {
            buffer: Vec::new(),
            chunk: b"Content-Length: 1\r\n\r\n\xff".to_vec(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn frame_message_uses_lsp_content_length_bytes() {
        let message =
            r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"你好"}}"#;
        let framed = frame_message(FrameMessageRequest {
            message: message.to_string(),
        })
        .unwrap();
        assert!(framed
            .frame
            .starts_with(&format!("Content-Length: {}\r\n\r\n", message.len())));
        assert!(framed.frame.ends_with(message));
    }

    #[test]
    fn parse_server_messages_returns_complete_messages_and_remaining_buffer() {
        let first = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let second = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"message":"ok"}}"#;
        let first_frame = frame_message(FrameMessageRequest {
            message: first.to_string(),
        })
        .unwrap()
        .frame;
        let second_frame = frame_message(FrameMessageRequest {
            message: second.to_string(),
        })
        .unwrap()
        .frame;
        let split_at = first_frame.len() - 3;
        let partial = parse_server_messages(ParseServerMessagesRequest {
            buffer: Vec::new(),
            chunk: first_frame.as_bytes()[..split_at].to_vec(),
        })
        .unwrap();
        assert!(partial.messages.is_empty());
        assert_eq!(partial.buffer, first_frame.as_bytes()[..split_at]);

        let mut next_chunk = first_frame.as_bytes()[split_at..].to_vec();
        next_chunk.extend(second_frame.as_bytes());
        let parsed = parse_server_messages(ParseServerMessagesRequest {
            buffer: partial.buffer,
            chunk: next_chunk,
        })
        .unwrap();
        assert_eq!(parsed.messages, vec![first.to_string(), second.to_string()]);
        assert!(parsed.buffer.is_empty());
    }
}
