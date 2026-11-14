use super::*;
use serde_json::Value;

#[test]
fn text_edits_use_lsp_utf16_positions() {
    let response = apply_text_edits(ApplyTextEditsRequest {
        text: "one 😀\ntwo three\n".to_string(),
        edits: vec![
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        utf16_column: 4,
                    },
                    end: LspPosition {
                        line: 0,
                        utf16_column: 6,
                    },
                },
                new_text: "rocket".to_string(),
            },
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 1,
                        utf16_column: 4,
                    },
                    end: LspPosition {
                        line: 1,
                        utf16_column: 9,
                    },
                },
                new_text: "four".to_string(),
            },
        ],
    })
    .unwrap();

    assert_eq!(response.text, "one rocket\ntwo four\n");
}

#[test]
fn text_edits_reject_invalid_and_overlapping_ranges() {
    let invalid = apply_text_edits(ApplyTextEditsRequest {
        text: "one line".to_string(),
        edits: vec![LspTextEdit {
            range: LspRange {
                start: LspPosition {
                    line: 9,
                    utf16_column: 0,
                },
                end: LspPosition {
                    line: 9,
                    utf16_column: 1,
                },
            },
            new_text: "x".to_string(),
        }],
    })
    .unwrap_err();
    assert_eq!(invalid.details.as_deref(), Some("invalidRange"));

    let overlapping = apply_text_edits(ApplyTextEditsRequest {
        text: "one line".to_string(),
        edits: vec![
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        utf16_column: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        utf16_column: 4,
                    },
                },
                new_text: "a".to_string(),
            },
            LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        utf16_column: 2,
                    },
                    end: LspPosition {
                        line: 0,
                        utf16_column: 6,
                    },
                },
                new_text: "b".to_string(),
            },
        ],
    })
    .unwrap_err();
    assert_eq!(overlapping.details.as_deref(), Some("overlappingEdits"));
}

#[test]
fn snippet_plain_text_removes_tab_stops_and_keeps_defaults() {
    assert_eq!(snippet_plain_text("print(${1:value})$0"), "print(value)");
    assert_eq!(snippet_plain_text("${1:let} ${2:name} = $3"), "let name = ");
}

#[test]
fn builtin_completion_returns_current_file_identifiers_for_prefix() {
    let response = builtin_completions(BuiltinRequest {
        file_path: "/tmp/main.swift".to_string(),
        text: "struct RocketShip {}\nlet rocketSpeed = Roc\n".to_string(),
        position: LspPosition {
            line: 1,
            utf16_column: 19,
        },
    })
    .unwrap();

    assert!(response.items.iter().any(|item| item.label == "RocketShip"));
    let item = response
        .items
        .iter()
        .find(|item| item.label == "RocketShip")
        .unwrap();
    assert_eq!(item.text_edit.range.start.utf16_column, 18);
    assert_eq!(item.text_edit.new_text, "RocketShip");
}

#[test]
fn builtin_hover_returns_current_identifier_range() {
    let response = builtin_hover(BuiltinRequest {
        file_path: "/tmp/main.rs".to_string(),
        text: "fn launch() {}\n".to_string(),
        position: LspPosition {
            line: 0,
            utf16_column: 4,
        },
    })
    .unwrap();

    let hover = response.hover.unwrap();
    assert_eq!(hover.contents, "`launch`");
    assert_eq!(hover.range.start.utf16_column, 3);
    assert_eq!(hover.range.end.utf16_column, 9);
}

#[test]
fn builtin_navigation_prefers_declarations_and_finds_references() {
    let text = "let service = 1\nprint(service)\n";
    let definitions = builtin_navigation(BuiltinNavigationRequest {
        file_path: "/tmp/main.swift".to_string(),
        text: text.to_string(),
        position: LspPosition {
            line: 1,
            utf16_column: 8,
        },
        method: "textDocument/definition".to_string(),
    })
    .unwrap();
    assert_eq!(definitions.locations.len(), 1);
    assert_eq!(definitions.locations[0].range.start.line, 0);
    assert_eq!(definitions.locations[0].range.start.utf16_column, 4);

    let references = builtin_navigation(BuiltinNavigationRequest {
        file_path: "/tmp/main.swift".to_string(),
        text: text.to_string(),
        position: LspPosition {
            line: 1,
            utf16_column: 8,
        },
        method: "textDocument/references".to_string(),
    })
    .unwrap();
    assert_eq!(references.locations.len(), 2);
}

#[test]
fn text_edits_follow_shared_windows_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/lsp/text-edits-v1.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let edits: Vec<Value> = case["edits"].as_array().unwrap().iter().map(|edit| serde_json::json!({
            "range": { "start": { "line": edit["range"]["start"]["line"], "utf16Column": edit["range"]["start"]["character"] },
                       "end": { "line": edit["range"]["end"]["line"], "utf16Column": edit["range"]["end"]["character"] } },
            "newText": edit["newText"]
        })).collect();
        let result = apply_text_edits(
            serde_json::from_value(serde_json::json!({ "text": case["text"], "edits": edits }))
                .unwrap(),
        );
        if let Some(expected) = case.get("expectedError") {
            assert_eq!(
                result.unwrap_err().details.as_deref(),
                expected.as_str(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                result.unwrap().text,
                case["expectedText"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}
