//! Pure LSP client-state transitions and JSON-RPC message construction.

use super::types::*;
use lithe_document_uri::{file_path_for_uri, file_path_from_uri, hex_value};
use lithe_protocol::{CoreError, ErrorCode};
use lithe_text::apply_content_changes_sequential;
use serde_json::{json, Value};

mod capabilities;
mod diagnostics;
mod json_rpc;
mod lifecycle;
mod requests;
mod results;
mod server_messages;
mod validation;

pub use capabilities::*;
pub(crate) use diagnostics::*;
pub use json_rpc::*;
pub use lifecycle::*;
pub(crate) use requests::*;
pub(crate) use results::*;
pub use server_messages::*;
pub(crate) use validation::*;

#[cfg(test)]
mod semantic_tests {
    use super::*;

    fn options() -> Value {
        json!({"full": true, "legend": {"tokenTypes": ["class", "method"], "tokenModifiers": ["declaration"]}})
    }

    #[test]
    fn semantic_tokens_decode_relative_utf16_positions_and_reject_malformed_data() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/lsp/semantic-tokens-v1.json"
        ))
        .unwrap();
        let options = fixture["serverOptions"].clone();
        let legend = semantic_token_legend(&options);
        let value = semantic_token_result(
            legend,
            Some(&json!({"data": [0, 6, 5, 0, 1, 2, 4, 3, 1, 0, 0, 7, 2, 1, 0]})),
        )
        .unwrap();
        assert_eq!(value, fixture["expectedResult"]);
        assert_eq!(value["tokens"][0]["startChar"], 6);
        assert_eq!(value["tokens"][1]["line"], 2);
        assert_eq!(value["tokens"][2]["startChar"], 11);
        assert_eq!(value["tokenTypes"], json!(["class", "method"]));
        assert_eq!(
            semantic_token_result(legend, Some(&Value::Null)).unwrap()["tokens"],
            json!([])
        );
        for data in [
            json!([0]),
            json!([0, 0, 0, 0, 0]),
            json!([0, 0, 1, 2, 0]),
            json!([-1, 0, 1, 0, 0]),
        ] {
            assert!(semantic_token_result(legend, Some(&json!({"data": data}))).is_none());
        }
        assert!(
            semantic_token_legend(&json!({"range": true, "legend": options["legend"]})).is_none()
        );
    }

    #[test]
    fn semantic_registration_snapshot_survives_unregister_and_refresh_is_acknowledged() {
        let mut state = LspClientState::default();
        apply_dynamic_registration(
            &mut state,
            &json!({"params": {"registrations": [{"id": "semantic", "method": "textDocument/semanticTokens", "registerOptions": options()}]}}),
        );
        assert!(state
            .server_capabilities
            .contains(&"semanticTokens".to_string()));
        let id = allocate_request(&mut state, "textDocument/semanticTokens/full");
        apply_dynamic_unregistration(
            &mut state,
            &json!({"params": {"unregisterations": [{"id": "semantic", "method": "textDocument/semanticTokens"}]}}),
        );
        assert!(!state
            .server_capabilities
            .contains(&"semanticTokens".to_string()));
        let response = client_apply_server_message(ClientApplyServerMessageRequest {
            state,
            message: json!({"jsonrpc": "2.0", "id": id, "result": {"data": [0,6,5,0,1]}})
                .to_string(),
        })
        .unwrap();
        assert_eq!(
            response.events[0].result.as_ref().unwrap()["tokens"][0]["length"],
            5
        );
        assert!(response.state.pending_semantic_legends.is_empty());
        let refresh = client_apply_server_message(ClientApplyServerMessageRequest {
            state: response.state, message: json!({"jsonrpc": "2.0", "id": "refresh", "method": "workspace/semanticTokens/refresh"}).to_string(),
        }).unwrap();
        assert_eq!(refresh.events[0].kind, "semanticTokensRefresh");
        assert_eq!(refresh.messages.len(), 1);
        assert!(refresh.messages[0].contains("result"));
    }
}

#[cfg(test)]
mod formatting_and_completion_tests {
    use super::*;

    fn request(method: &str, options: Option<Value>, with_range: bool) -> ClientFeatureRequest {
        ClientFeatureRequest {
            query: None,
            state: LspClientState::default(),
            uri: "file:///w/a.c".to_string(),
            method: method.to_string(),
            position: None,
            new_name: None,
            range: with_range.then_some(LspRange {
                start: LspPosition {
                    line: 1,
                    utf16_column: 2,
                },
                end: LspPosition {
                    line: 3,
                    utf16_column: 0,
                },
            }),
            diagnostics: Vec::new(),
            completion_context: None,
            completion_item: None,
            code_action: None,
            command: None,
            formatting_options: options,
            include_declaration: None,
        }
    }

    #[test]
    fn formatting_keeps_defaults_without_caller_options() {
        let params =
            feature_request_params(&request("textDocument/formatting", None, false)).unwrap();
        assert_eq!(
            params["options"],
            json!({"tabSize": 4, "insertSpaces": true, "trimTrailingWhitespace": true,
                   "insertFinalNewline": true, "trimFinalNewlines": true})
        );
    }

    #[test]
    fn formatting_and_range_formatting_merge_caller_options() {
        let options =
            json!({"tabSize": 2, "insertSpaces": false, "trimFinalNewlines": false, "custom": "x"});
        let full = feature_request_params(&request(
            "textDocument/formatting",
            Some(options.clone()),
            false,
        ))
        .unwrap();
        assert_eq!(full["options"]["tabSize"], 2);
        assert_eq!(full["options"]["insertSpaces"], false);
        assert_eq!(full["options"]["trimFinalNewlines"], false);
        assert_eq!(full["options"]["custom"], "x");
        assert_eq!(full["options"]["insertFinalNewline"], true);
        let range = feature_request_params(&request(
            "textDocument/rangeFormatting",
            Some(options),
            true,
        ))
        .unwrap();
        assert_eq!(range["range"]["start"], json!({"line": 1, "character": 2}));
        assert_eq!(range["range"]["end"], json!({"line": 3, "character": 0}));
        assert_eq!(range["options"]["tabSize"], 2);
        assert!(
            feature_request_params(&request("textDocument/rangeFormatting", None, false)).is_err()
        );
    }

    #[test]
    fn invalid_formatting_options_are_rejected() {
        for options in [
            json!({"tabSize": 0}),
            json!({"tabSize": "4"}),
            json!({"insertSpaces": 1}),
            json!({"nested": {}}),
            json!([1]),
        ] {
            assert!(feature_request_params(&request(
                "textDocument/formatting",
                Some(options),
                false
            ))
            .is_err());
        }
    }

    #[test]
    fn range_formatting_capability_and_result_are_normalized() {
        let features =
            feature_names_from_capabilities(&json!({"documentRangeFormattingProvider": true}));
        assert!(features.contains(&"rangeFormatting".to_string()));
        let result = lsp_feature_result_for_method(
            Some("textDocument/rangeFormatting"),
            Some(&json!([{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "newText": "x"
            }])),
        )
        .unwrap();
        assert_eq!(result["edits"][0]["newText"], "x");
    }

    #[test]
    fn document_symbol_golden_fixture_matches_normalization() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/lsp/document-symbols-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let result = lsp_feature_result_for_method(
                Some("textDocument/documentSymbol"),
                Some(&case["serverResult"]),
            )
            .unwrap();
            assert_eq!(result, case["expected"], "case {}", case["name"]);
        }
    }

    #[test]
    fn document_symbol_capability_request_and_dynamic_name() {
        let features = feature_names_from_capabilities(&json!({"documentSymbolProvider": true}));
        assert!(features.contains(&"documentSymbols".to_string()));
        assert_eq!(
            feature_name_for_method("textDocument/documentSymbol"),
            Some("documentSymbols")
        );
        let mut request = request("textDocument/documentSymbol", None, false);
        request.uri = "file:///a.rs".to_string();
        assert_eq!(
            feature_request_params(&request).unwrap(),
            json!({"textDocument": {"uri": "file:///a.rs"}})
        );
        assert!(validate_lsp_method("textDocument/documentSymbol").is_ok());
    }

    #[test]
    fn completion_options_union_static_and_dynamic_providers() {
        let mut state = LspClientState::default();
        assert!(completion_options(&state).is_none());
        state.server_capabilities.push("completion".into());
        state.completion_providers.insert(
            String::new(),
            json!({"triggerCharacters": [".", ":", ""], "allCommitCharacters": [";"]}),
        );
        apply_dynamic_registration(
            &mut state,
            &json!({"params": {"registrations": [{"id": "c1", "method": "textDocument/completion",
                "registerOptions": {"triggerCharacters": [":", ">"]}}]}}),
        );
        assert_eq!(
            completion_options(&state).unwrap(),
            json!({"triggerCharacters": [".", ":", ">"], "allCommitCharacters": [";"]})
        );
        apply_dynamic_unregistration(
            &mut state,
            &json!({"params": {"unregisterations": [{"id": "c1", "method": "textDocument/completion"}]}}),
        );
        // Withdrawing the completion registration also withdraws the feature.
        assert!(completion_options(&state).is_none());
        assert!(!state.completion_providers.contains_key("c1"));
    }

    #[test]
    fn completion_result_preserves_is_incomplete() {
        let result = lsp_feature_result_for_method(
            Some("textDocument/completion"),
            Some(&json!({"isIncomplete": true, "items": [{"label": "a"}]})),
        )
        .unwrap();
        assert_eq!(result["isIncomplete"], true);
        assert_eq!(result["items"][0]["label"], "a");
    }
}
