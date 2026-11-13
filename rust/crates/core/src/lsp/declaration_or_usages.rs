//! Whether a go-to-declaration gesture shows the declaration or its usages
//! (`lsp.declarationOrUsages`).
//!
//! Editors follow IntelliJ IDEA's Go to Declaration or Usages: from a use the
//! gesture opens the declaration, and from the declaration itself, where the
//! definition answer is the place the caret already sits on, it shows the
//! usages instead. Hosts send the definition answer they received and ask the
//! language server for references without the declaration when Core says
//! `usages`; Core owns the decision so every host and gesture agree.

use crate::protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

/// The most definition locations one decision accepts; a definition answer
/// is a handful of places, so more is a malformed request.
const MAX_DEFINITIONS: usize = 100;

/// Zero-based line and UTF-16 column, as the host's semantic requests use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Position {
    line: u32,
    utf16_column: u32,
}

/// A half-open range; `start` never follows `end`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Range {
    start: Position,
    end: Position,
}

/// One definition location as the host received it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Place {
    uri: String,
    range: Range,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    /// The document the gesture was made in, as the semantic request named it.
    uri: String,
    /// Where the gesture was made: the caret or the clicked character.
    position: Position,
    /// The definition answer for that position, in the server's order.
    definitions: Vec<Place>,
}

/// Returns `{ action }`: `usages` when every definition location is the
/// gesture's own place, otherwise `definitions` (including an empty answer,
/// which the host reports as no definition).
///
/// A location is the gesture's own place when it is in the same document and
/// its range contains the position, end included: an editor resolves the
/// identifier left of the caret, so a caret right after a declaration's name
/// is still on that declaration.
pub(crate) fn decide_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|error| {
        invalid("Invalid declaration-or-usages request").with_details(error.to_string())
    })?;
    if request.uri.is_empty()
        || request.definitions.len() > MAX_DEFINITIONS
        || request
            .definitions
            .iter()
            .any(|place| place.uri.is_empty() || place.range.end < place.range.start)
    {
        return Err(invalid("Invalid declaration-or-usages request"));
    }
    let own_place = |place: &Place| {
        place.uri == request.uri
            && place.range.start <= request.position
            && request.position <= place.range.end
    };
    let usages = !request.definitions.is_empty() && request.definitions.iter().all(own_place);
    Ok(json!({ "action": if usages { "usages" } else { "definitions" } }))
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    /// Every shared-fixture case yields its expected data or error code
    /// through the public JSON command.
    #[test]
    fn matches_shared_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/lsp/declaration-or-usages-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let response: Value = serde_json::from_str(&crate::execute_json(
                &json!({"command": "lsp.declarationOrUsages", "payload": case["request"]})
                    .to_string(),
            ))
            .unwrap();
            if let Some(code) = case.get("expectedError") {
                assert_eq!(response["ok"], false, "{}: {response}", case["name"]);
                assert_eq!(&response["error"]["code"], code, "{}", case["name"]);
            } else {
                assert_eq!(response["ok"], true, "{}: {response}", case["name"]);
                assert_eq!(response["data"], case["expected"], "{}", case["name"]);
            }
        }
    }
}
