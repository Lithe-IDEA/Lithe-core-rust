//! Independent static editor resources, without language-server or Run providers.

use crate::plugins::declarative::{decode_grammar_command, validate_command};
use serde_json::{json, Value};

macro_rules! contract_file {
    ($path:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../shared/contracts/plugins/v1/",
            $path
        ))
    };
}

const GRAMMAR_PLUGIN: &str = contract_file!("fixtures/languages/language-grammar.plugin.json");
const HTTP_GRAMMAR: &str = contract_file!("fixtures/languages/grammars/http.tmLanguage.json");

fn manifest(source: &str) -> Value {
    serde_json::from_str(source).expect("fixture manifest is JSON")
}

fn decode_grammar(grammar: &str) -> Value {
    decode_grammar_command(json!({
        "manifest": manifest(GRAMMAR_PLUGIN),
        "languageID": "http",
        "grammar": grammar,
    }))
    .expect("payload is well formed")
}

/// A grammar of scope `source.test` whose top-level patterns are `patterns`.
fn grammar_with(patterns: Value) -> Value {
    json!({ "scopeName": "source.test", "patterns": patterns })
}

fn grammar_rejection(grammar: &str) -> (String, String) {
    let outcome = decode_grammar(grammar);
    assert_eq!(outcome["status"], "rejected", "{outcome}");
    (
        outcome["error"]["code"].as_str().unwrap().to_string(),
        outcome["error"]["field"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The editor gets the grammar file unchanged with what it needs to
/// register the language: file matching, engine kind, scope and configuration.
#[test]
fn a_grammar_file_decodes_to_the_language_the_editor_registers() {
    let outcome = decode_grammar(HTTP_GRAMMAR);
    assert_eq!(outcome["status"], "ok", "{outcome}");
    let language = &outcome["language"];
    assert_eq!(language["id"], "http");
    assert_eq!(language["fileExtensions"], json!(["http", "rest"]));
    assert_eq!(language["grammar"]["kind"], "textmate");
    assert_eq!(language["grammar"]["scopeName"], "source.http");
    assert_eq!(language["grammar"]["source"], HTTP_GRAMMAR);
    assert_eq!(language["configuration"]["lineComment"], "#");
    assert_eq!(
        language["configuration"]["brackets"],
        json!([["{", "}"], ["[", "]"]])
    );
}

/// Core hands the editor the text it validated, not a re-encoding of it.
#[test]
fn grammar_source_survives_json_roundtrips_byte_for_byte() {
    let source = "{ \"patterns\": [ { \"match\": \"\\\\bGET\\\\b\", \"name\": \"keyword\" } ],\n  \"scopeName\": \"source.test\" }";
    let outcome = decode_grammar(source);
    assert_eq!(outcome["status"], "ok", "{outcome}");
    let roundtrip: Value = serde_json::from_str(&outcome.to_string()).unwrap();
    assert_eq!(roundtrip["language"]["grammar"]["source"], source);
}

#[test]
fn a_language_without_a_grammar_has_nothing_to_decode() {
    let outcome = decode_grammar_command(json!({
        "manifest": manifest(GRAMMAR_PLUGIN),
        "languageID": "rest",
        "grammar": HTTP_GRAMMAR,
    }))
    .unwrap();
    assert_eq!(
        outcome["error"]["code"], "plugin.grammar.unavailable",
        "{outcome}"
    );
}

/// Anything the editor's engine could not load, or that would make it
/// allocate without bound, is refused before it reaches the editor, with the
/// field that failed.
#[test]
fn grammar_files_are_refused_when_the_editor_could_not_load_them() {
    let rejected = |grammar: Value, field: &str| {
        let (code, actual) = grammar_rejection(&grammar.to_string());
        assert_eq!(code, "plugin.grammar.invalid", "{grammar}");
        assert_eq!(actual, field, "{grammar}");
    };
    rejected(json!([]), "grammar");
    rejected(json!({ "patterns": [] }), "grammar.scopeName");
    rejected(
        json!({ "scopeName": "source test", "patterns": [] }),
        "grammar.scopeName",
    );
    rejected(
        json!({ "scopeName": "s".repeat(129), "patterns": [] }),
        "grammar.scopeName",
    );
    rejected(json!({ "scopeName": "source.test" }), "grammar.patterns");
    rejected(
        json!({ "scopeName": "source.test", "name": 1, "patterns": [] }),
        "grammar.name",
    );
    rejected(
        json!({ "scopeName": "source.test", "patterns": [], "repository": [] }),
        "grammar.repository",
    );
    rejected(grammar_with(json!(["\\w+"])), "grammar.patterns[0]");
    rejected(
        grammar_with(json!([{ "match": 1 }])),
        "grammar.patterns[0].match",
    );
    rejected(
        grammar_with(json!([{ "include": "" }])),
        "grammar.patterns[0].include",
    );
    rejected(
        grammar_with(json!([{ "match": "a", "name": ["keyword"] }])),
        "grammar.patterns[0].name",
    );
    rejected(
        grammar_with(json!([{ "begin": "a", "end": "b", "patterns": {} }])),
        "grammar.patterns[0].patterns",
    );
    rejected(
        grammar_with(json!([{ "match": "a", "applyEndPatternLast": {} }])),
        "grammar.patterns[0].applyEndPatternLast",
    );
    rejected(
        grammar_with(json!([{ "match": "a", "applyEndPatternLast": "1" }])),
        "grammar.patterns[0].applyEndPatternLast",
    );
    rejected(
        grammar_with(json!([{ "match": "a", "applyEndPatternLast": 2 }])),
        "grammar.patterns[0].applyEndPatternLast",
    );
    rejected(
        json!({
            "scopeName": "source.test",
            "patterns": [],
            "repository": { "word": { "patterns": [{ "match": null }] } }
        }),
        "grammar.repository.word.patterns[0].match",
    );
    rejected(
        grammar_with(json!([{ "match": "(a)", "captures": "keyword" }])),
        "grammar.patterns[0].captures",
    );
    rejected(
        grammar_with(json!([{ "match": "(a)", "captures": { "1": "keyword" } }])),
        "grammar.patterns[0].captures.1",
    );
    rejected(
        grammar_with(json!([{ "match": "a".repeat(32 * 1024 + 1) }])),
        "grammar.patterns[0].match",
    );
}

/// The editor keeps one slot per capture group up to the highest a rule
/// styles, so a group number is what bounds that allocation.
#[test]
fn capture_groups_are_bounded_where_the_editor_allocates_them() {
    let rejected = |captures: Value, field: &str| {
        let grammar = grammar_with(json!([{ "match": "(a)", "beginCaptures": captures }]));
        let (code, actual) = grammar_rejection(&grammar.to_string());
        assert_eq!(code, "plugin.grammar.invalid", "{grammar}");
        assert_eq!(actual, field, "{grammar}");
    };
    rejected(
        json!({ "4097": { "name": "a" } }),
        "grammar.patterns[0].beginCaptures.4097",
    );
    rejected(
        json!({ "+99999999999999999999999": { "name": "a" } }),
        "grammar.patterns[0].beginCaptures.+99999999999999999999999",
    );
    rejected(
        Value::Array(vec![Value::Null; 4098]),
        "grammar.patterns[0].beginCaptures",
    );
    let accepted = grammar_with(json!([{
        "match": "(a)",
        "captures": { "4096": { "name": "a" }, "-0": { "name": "b" } }
    }]));
    assert_eq!(decode_grammar(&accepted.to_string())["status"], "ok");
}

#[test]
fn grammar_size_rule_count_and_nesting_are_bounded() {
    let oversized = format!(
        "{{\"scopeName\":\"source.test\",\"patterns\":[],\"padding\":\"{}\"}}",
        "x".repeat(2 * 1024 * 1024)
    );
    assert_eq!(
        grammar_rejection(&oversized),
        ("plugin.grammar.invalid".into(), "grammar".into())
    );

    let many = grammar_with(Value::Array(vec![json!({ "match": "a" }); 16385]));
    assert_eq!(
        grammar_rejection(&many.to_string()),
        ("plugin.grammar.invalid".into(), "grammar".into())
    );
    let most = grammar_with(Value::Array(vec![json!({ "match": "a" }); 16384]));
    assert_eq!(decode_grammar(&most.to_string())["status"], "ok");

    let mut nested = json!({ "match": "a" });
    for _ in 0..32 {
        nested = json!({ "patterns": [nested] });
    }
    let (code, field) = grammar_rejection(&grammar_with(json!([nested])).to_string());
    assert_eq!(code, "plugin.grammar.invalid");
    assert!(field.ends_with(".patterns[0]"), "{field}");
}

/// What VS Code grammars carry that the editor does not run is accepted
/// unchecked: notes, injections, includes of grammars the editor may not have,
/// and repository entries nothing includes.
#[test]
fn what_the_engine_does_not_read_is_accepted_as_in_vs_code() {
    let grammar = json!({
        "information_for_contributors": ["generated"],
        "version": "https://example.com/commit",
        "name": "Test",
        "scopeName": "source.test+jsx-1.2_x",
        "fileTypes": 5,
        "uuid": {},
        "injectionSelector": "L:source.other",
        "injections": { "R:source.test": { "patterns": [{ "match": 1 }] } },
        "patterns": [
            { "include": "$self" },
            { "include": "$base" },
            { "include": "#missing" },
            { "include": "source.js#expression" },
            { "include": "text.html.basic" },
            {
                "comment": "notes are ignored",
                "//": ["so are", "these"],
                "begin": "(\")",
                "beginCaptures": { "0": { "name": "punctuation" }, "comment": 5, "-1": 6 },
                "end": "\"",
                "endCaptures": [null, { "name": "punctuation" }],
                "applyEndPatternLast": 1,
                "contentName": "string.quoted meta.embedded",
                "patterns": [{ "match": "\\\\.", "name": "constant.character.escape" }]
            },
            { "begin": "^>", "while": "^>", "whileCaptures": {}, "applyEndPatternLast": true }
        ],
        "repository": { "unused": { "match": "[" } }
    });
    let outcome = decode_grammar(&grammar.to_string());
    assert_eq!(outcome["status"], "ok", "{outcome}");
    assert_eq!(
        outcome["language"]["grammar"]["scopeName"],
        "source.test+jsx-1.2_x"
    );
}

/// A grammar on its own is a complete plugin: it needs no language server,
/// no backend, and no permission.
#[test]
fn a_grammar_only_plugin_validates_without_a_language_server() {
    let outcome = validate_command(json!({ "manifest": manifest(GRAMMAR_PLUGIN) })).unwrap();
    assert_eq!(outcome["status"], "ok", "{outcome}");
    let language = &outcome["plugin"]["languages"][0];
    assert_eq!(language["grammar"]["kind"], "textmate");
    assert_eq!(language["grammar"]["path"], "grammars/http.tmLanguage.json");
}

/// `languages` 2 named Monaco's Monarch tokenizer, which no editor runs any
/// more; a manifest written against it is unsupported rather than misread.
#[test]
fn the_withdrawn_monarch_version_is_unsupported() {
    let mut plugin = manifest(GRAMMAR_PLUGIN);
    plugin["requires"]["languages"] = json!(2);
    plugin["languages"][0]["grammar"]["kind"] = json!("monarch");
    let outcome = validate_command(json!({ "manifest": plugin })).unwrap();
    assert_eq!(
        outcome["error"]["code"], "plugin.api.unsupported",
        "{outcome}"
    );

    let mut plugin = manifest(GRAMMAR_PLUGIN);
    plugin["languages"][0]["grammar"]["kind"] = json!("monarch");
    let outcome = validate_command(json!({ "manifest": plugin })).unwrap();
    assert_eq!(
        outcome["error"]["code"], "plugin.manifest.invalid",
        "{outcome}"
    );
}

#[test]
fn configuration_only_contribution_decodes_without_grammar() {
    let mut plugin = manifest(GRAMMAR_PLUGIN);
    plugin["languages"][0]
        .as_object_mut()
        .unwrap()
        .remove("grammar");
    // A backend is a complete tool plugin, with no language-server ownership.
    let tool = manifest(contract_file!(
        "fixtures/backend/backend-workspace.plugin.json"
    ));
    plugin["backend"] = tool["backend"].clone();
    plugin["requires"]["backend"] = json!(2);
    plugin["requires"]["serverProcess"] = json!(1);
    plugin["permissions"] = json!(["backend.launch"]);
    plugin["activationEvents"] = json!(["onWorkspaceOpen"]);
    let result =
        decode_grammar_command(json!({"manifest":plugin,"languageID":"http","grammar":""}))
            .unwrap();
    assert_eq!(result["status"], "ok", "{result}");
    assert!(result["language"]["grammar"].is_null());
    assert_eq!(result["language"]["configuration"]["lineComment"], "#");
}
