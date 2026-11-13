//! Formats exact argument vectors for the run editor's reversible text grammar.

use serde::Deserialize;

/// Argument vectors to format in order, with no shell expansion or execution.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArgumentTextRequest {
    /// Each vector becomes one editable text value in the response.
    pub argument_lists: Vec<Vec<String>>,
}

/// Formats each vector using the same quoting and escaping accepted by the editor parser.
pub fn format_argument_text(request: ArgumentTextRequest) -> Vec<String> {
    request
        .argument_lists
        .iter()
        .map(|arguments| {
            arguments
                .iter()
                .map(|argument| {
                    if !argument.is_empty()
                        && !argument.chars().any(|character| {
                            character.is_whitespace() || matches!(character, '\'' | '"' | '\\')
                        })
                    {
                        return argument.clone();
                    }
                    format!(
                        "\"{}\"",
                        argument.replace('\\', "\\\\").replace('"', "\\\"")
                    )
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn argument_text_matches_the_shared_roundtrip_examples() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/run-configuration/argument-text.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let arguments: Vec<String> = serde_json::from_value(case["values"].clone()).unwrap();
            assert_eq!(
                format_argument_text(ArgumentTextRequest {
                    argument_lists: vec![arguments]
                })[0],
                case["text"]
            );
        }
    }
}
