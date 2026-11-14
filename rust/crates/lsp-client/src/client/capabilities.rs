//! Server capabilities, dynamic registration and semantic tokens.

use super::*;

/// `capabilities.experimental`: every Lithe protocol a plugin process may
/// speak next to LSP, each negotiated on its own key.
pub(crate) fn experimental_capabilities(
    plugin_owned: bool,
    toolchain_providers: &std::collections::BTreeSet<String>,
    keymap_import_providers: &std::collections::BTreeSet<String>,
) -> Value {
    let mut experimental = crate::build_provider::client_experimental_capabilities();
    if plugin_owned {
        let (logging_key, logging_offer) =
            crate::plugin_logging::client_experimental_capabilities();
        experimental[logging_key] = logging_offer;
        let (functions_key, functions_offer) = lithe_plugin_rpc::client_experimental_capabilities();
        experimental[functions_key] = functions_offer;
        let (vcs_key, vcs_offer) = crate::vcs_provider::client_experimental_capabilities();
        experimental[vcs_key] = vcs_offer;
        if let Some((toolchain_key, toolchain_offer)) =
            crate::toolchain_provider::client_experimental_capabilities(toolchain_providers)
        {
            experimental[toolchain_key] = toolchain_offer;
        }
        if let Some((key, offer)) =
            crate::keymap_provider::client_experimental_capabilities(keymap_import_providers)
        {
            experimental[key] = offer;
        }
    }
    experimental
}

pub(crate) fn text_document_sync_kind(capabilities: &Value) -> LspTextDocumentSyncKind {
    let sync = capabilities.get("textDocumentSync");
    let change = match sync {
        Some(Value::Number(value)) => value.as_i64().unwrap_or(1),
        Some(Value::Object(value)) => value.get("change").and_then(Value::as_i64).unwrap_or(1),
        _ => 1,
    };
    match change {
        0 => LspTextDocumentSyncKind::None,
        2 => LspTextDocumentSyncKind::Incremental,
        _ => LspTextDocumentSyncKind::Full,
    }
}

pub(crate) fn feature_names_from_capabilities(capabilities: &Value) -> Vec<String> {
    let mut values = Vec::new();
    add_capability(
        &mut values,
        capabilities,
        "definitionProvider",
        "definition",
    );
    add_capability(
        &mut values,
        capabilities,
        "declarationProvider",
        "declaration",
    );
    add_capability(
        &mut values,
        capabilities,
        "typeDefinitionProvider",
        "typeDefinition",
    );
    add_capability(
        &mut values,
        capabilities,
        "implementationProvider",
        "implementation",
    );
    add_capability(
        &mut values,
        capabilities,
        "referencesProvider",
        "references",
    );
    add_capability(&mut values, capabilities, "hoverProvider", "hover");
    add_capability(
        &mut values,
        capabilities,
        "completionProvider",
        "completion",
    );
    add_capability(&mut values, capabilities, "renameProvider", "rename");
    add_capability(
        &mut values,
        capabilities,
        "documentFormattingProvider",
        "formatting",
    );
    add_capability(
        &mut values,
        capabilities,
        "documentRangeFormattingProvider",
        "rangeFormatting",
    );
    add_capability(
        &mut values,
        capabilities,
        "documentSymbolProvider",
        "documentSymbols",
    );
    add_capability(&mut values, capabilities, "inlayHintProvider", "inlayHints");
    add_capability(
        &mut values,
        capabilities,
        "foldingRangeProvider",
        "foldingRanges",
    );
    add_capability(
        &mut values,
        capabilities,
        "documentLinkProvider",
        "documentLinks",
    );
    add_capability(&mut values, capabilities, "codeLensProvider", "codeLens");
    if capabilities
        .get("semanticTokensProvider")
        .and_then(semantic_token_legend)
        .is_some()
    {
        insert_unique(&mut values, "semanticTokens");
    }
    add_capability(
        &mut values,
        capabilities,
        "workspaceSymbolProvider",
        "workspaceSymbols",
    );
    add_capability(
        &mut values,
        capabilities,
        "codeActionProvider",
        "codeActions",
    );
    add_capability(
        &mut values,
        capabilities,
        "executeCommandProvider",
        "executeCommand",
    );
    if capabilities
        .get("completionProvider")
        .and_then(|value| value.get("resolveProvider"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        insert_unique(&mut values, "completionResolve");
    }
    if capabilities
        .get("codeActionProvider")
        .and_then(|value| value.get("resolveProvider"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        insert_unique(&mut values, "codeActionResolve");
    }
    if crate::build_provider::negotiated(capabilities) {
        insert_unique(&mut values, crate::build_provider::BUILD_PROVIDER_FEATURE);
    }
    if lithe_plugin_rpc::negotiate(capabilities).is_some() {
        insert_unique(&mut values, lithe_plugin_rpc::FUNCTIONS_FEATURE);
    }
    if crate::vcs_provider::negotiate(capabilities).is_some() {
        insert_unique(&mut values, crate::vcs_provider::VCS_PROVIDER_FEATURE);
    }
    values
}

fn add_capability(values: &mut Vec<String>, capabilities: &Value, key: &str, feature: &str) {
    match capabilities.get(key) {
        Some(Value::Bool(true)) => insert_unique(values, feature),
        Some(Value::Object(_)) => insert_unique(values, feature),
        _ => {}
    }
}

pub(crate) fn apply_dynamic_registration(state: &mut LspClientState, message: &Value) {
    let Some(registrations) = message
        .get("params")
        .and_then(|params| params.get("registrations"))
        .and_then(Value::as_array)
    else {
        return;
    };
    for registration in registrations {
        if registration.get("method").and_then(Value::as_str) == Some("textDocument/semanticTokens")
        {
            if let (Some(id), Some(options)) = (
                registration.get("id").and_then(Value::as_str),
                registration.get("registerOptions"),
            ) {
                if semantic_token_legend(options).is_some() {
                    state
                        .semantic_token_providers
                        .insert(id.to_string(), options.clone());
                    insert_unique(&mut state.server_capabilities, "semanticTokens");
                }
            }
            continue;
        }
        if registration.get("method").and_then(Value::as_str) == Some("textDocument/completion") {
            if let (Some(id), Some(options)) = (
                registration.get("id").and_then(Value::as_str),
                registration
                    .get("registerOptions")
                    .filter(|options| options.is_object()),
            ) {
                state
                    .completion_providers
                    .insert(id.to_string(), options.clone());
            }
        }
        if let Some(feature) = registration
            .get("method")
            .and_then(Value::as_str)
            .and_then(feature_name_for_method)
        {
            insert_unique(&mut state.server_capabilities, feature);
        }
        if registration
            .get("registerOptions")
            .and_then(|options| options.get("resolveProvider"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            if registration.get("method").and_then(Value::as_str) == Some("textDocument/completion")
            {
                insert_unique(&mut state.server_capabilities, "completionResolve");
            }
            if registration.get("method").and_then(Value::as_str) == Some("textDocument/codeAction")
            {
                insert_unique(&mut state.server_capabilities, "codeActionResolve");
            }
        }
    }
}

pub(crate) fn apply_dynamic_unregistration(state: &mut LspClientState, message: &Value) {
    let Some(unregistrations) = message
        .get("params")
        .and_then(|params| {
            params
                .get("unregistrations")
                .or_else(|| params.get("unregisterations"))
        })
        .and_then(Value::as_array)
    else {
        return;
    };
    for unregistration in unregistrations {
        if unregistration.get("method").and_then(Value::as_str)
            == Some("textDocument/semanticTokens")
        {
            if let Some(id) = unregistration.get("id").and_then(Value::as_str) {
                state.semantic_token_providers.remove(id);
            }
            if state.semantic_token_providers.is_empty() {
                state
                    .server_capabilities
                    .retain(|feature| feature != "semanticTokens");
            }
            continue;
        }
        if unregistration.get("method").and_then(Value::as_str) == Some("textDocument/completion") {
            if let Some(id) = unregistration.get("id").and_then(Value::as_str) {
                state.completion_providers.remove(id);
            }
        }
        if let Some(feature) = unregistration
            .get("method")
            .and_then(Value::as_str)
            .and_then(feature_name_for_method)
        {
            state
                .server_capabilities
                .retain(|existing| existing != feature);
        }
    }
}

/// Completion trigger and all-commit characters the session's providers declared.
///
/// Returns `{ "triggerCharacters": [...], "allCommitCharacters": [...] }`, each
/// list the de-duplicated union over the static capability and dynamic
/// registrations in registration order, or `None` while the session offers no
/// completion. Entries that are not non-empty strings are dropped.
pub fn completion_options(state: &LspClientState) -> Option<Value> {
    if !state
        .server_capabilities
        .iter()
        .any(|feature| feature == "completion")
    {
        return None;
    }
    let collect = |key: &str| {
        let mut values: Vec<String> = Vec::new();
        for options in state.completion_providers.values() {
            for value in options
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                insert_unique(&mut values, value);
            }
        }
        values
    };
    Some(json!({
        "triggerCharacters": collect("triggerCharacters"),
        "allCommitCharacters": collect("allCommitCharacters"),
    }))
}

pub(crate) fn feature_name_for_method(method: &str) -> Option<&'static str> {
    match method {
        "textDocument/definition" => Some("definition"),
        "textDocument/declaration" => Some("declaration"),
        "textDocument/typeDefinition" => Some("typeDefinition"),
        "textDocument/implementation" => Some("implementation"),
        "textDocument/references" => Some("references"),
        "textDocument/hover" => Some("hover"),
        "textDocument/completion" => Some("completion"),
        "textDocument/rename" => Some("rename"),
        "textDocument/formatting" => Some("formatting"),
        "textDocument/rangeFormatting" => Some("rangeFormatting"),
        "textDocument/documentSymbol" => Some("documentSymbols"),
        "textDocument/inlayHint" => Some("inlayHints"),
        "textDocument/foldingRange" => Some("foldingRanges"),
        "textDocument/documentLink" => Some("documentLinks"),
        "textDocument/codeLens" => Some("codeLens"),
        "textDocument/codeAction" => Some("codeActions"),
        "workspace/symbol" => Some("workspaceSymbols"),
        "workspace/executeCommand" => Some("executeCommand"),
        _ => None,
    }
}

fn insert_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

/// A range-only provider cannot serve the full-document operation offered here.
pub(crate) fn semantic_token_legend(options: &Value) -> Option<&Value> {
    let full = options.get("full")?;
    if full != &Value::Bool(true) && !full.is_object() {
        return None;
    }
    let legend = options.get("legend")?;
    if !legend
        .get("tokenTypes")?
        .as_array()?
        .iter()
        .all(Value::is_string)
        || !legend
            .get("tokenModifiers")?
            .as_array()?
            .iter()
            .all(Value::is_string)
    {
        return None;
    }
    Some(legend)
}

/// Decode relative UTF-16 positions once in Core. Both editors consume the
/// same normalized tokens and the exact legend used by this server session.
pub(crate) fn semantic_token_result(
    legend: Option<&Value>,
    result: Option<&Value>,
) -> Option<Value> {
    let legend = legend?;
    let empty = Vec::new();
    let data = match result {
        None | Some(Value::Null) => &empty,
        Some(value) => value.get("data")?.as_array()?,
    };
    if data.len() % 5 != 0 {
        return None;
    }
    let mut tokens = Vec::with_capacity(data.len() / 5);
    let (mut line, mut column) = (0_u64, 0_u64);
    for chunk in data.chunks_exact(5) {
        let numbers: Vec<u64> = chunk.iter().map(Value::as_u64).collect::<Option<_>>()?;
        if numbers.iter().any(|value| *value > u32::MAX as u64) {
            return None;
        }
        line = line.checked_add(numbers[0])?;
        column = if numbers[0] == 0 {
            column.checked_add(numbers[1])?
        } else {
            numbers[1]
        };
        if line > u32::MAX as u64
            || column > u32::MAX as u64
            || numbers[2] == 0
            || numbers[3] >= legend["tokenTypes"].as_array()?.len() as u64
        {
            return None;
        }
        tokens.push(json!({"line": line, "startChar": column, "length": numbers[2], "tokenType": numbers[3], "tokenModifiers": numbers[4]}));
    }
    Some(
        json!({"tokenTypes": legend["tokenTypes"], "tokenModifiers": legend["tokenModifiers"], "tokens": tokens}),
    )
}
