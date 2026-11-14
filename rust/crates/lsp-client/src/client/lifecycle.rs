//! Initializing a client, syncing documents, shutting down and sending feature requests.

use super::*;

/// Creates a host-owned initialize request for stateless client tests.
#[cfg(test)]
pub fn client_initialize(request: ClientInitializeRequest) -> Result<LspClientResponse, CoreError> {
    client_initialize_for_plugin(request, false)
}

/// Offers plugin functions only when the runtime owns an identified plugin session.
pub fn client_initialize_for_plugin(
    request: ClientInitializeRequest,
    plugin_owned: bool,
) -> Result<LspClientResponse, CoreError> {
    validate_uri(&request.root_uri)?;
    let workspace_name = workspace_name_from_uri(&request.root_uri);
    let mut state = request.state;
    let id = allocate_request(&mut state, "initialize");
    let message = json_rpc_request(
        &id,
        "initialize",
        json!({
            "processId": request.process_id,
            "clientInfo": {
                "name": "Lithe",
                "version": env!("CARGO_PKG_VERSION")
            },
            "rootUri": &request.root_uri,
            "workspaceFolders": [{
                "uri": &request.root_uri,
                "name": workspace_name
            }],
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "completion": {
                        "dynamicRegistration": true,
                        "contextSupport": true,
                        "completionItem": {
                            "snippetSupport": true,
                            "documentationFormat": ["markdown", "plaintext"],
                            // Some servers (Intelephense, for one) omit typed class and
                            // namespace labels unless the client advertises labelDetailsSupport.
                            "labelDetailsSupport": true,
                            "resolveSupport": {
                                "properties": [
                                    "detail",
                                    "documentation",
                                    "textEdit",
                                    "additionalTextEdits",
                                    "labelDetails"
                                ]
                            }
                        }
                    },
                    "hover": {
                        "dynamicRegistration": true,
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    "definition": { "dynamicRegistration": true },
                    "declaration": { "dynamicRegistration": true },
                    "typeDefinition": { "dynamicRegistration": true },
                    "implementation": { "dynamicRegistration": true },
                    "references": { "dynamicRegistration": true },
                    "rename": { "dynamicRegistration": true },
                    "formatting": { "dynamicRegistration": true },
                    "rangeFormatting": { "dynamicRegistration": true },
                    "documentSymbol": {
                        "dynamicRegistration": true,
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "inlayHint": { "dynamicRegistration": true },
                    "semanticTokens": {
                        "dynamicRegistration": true,
                        "requests": { "full": true, "range": false },
                        "tokenTypes": ["namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator", "decorator"],
                        "tokenModifiers": ["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary"],
                        "formats": ["relative"],
                        "overlappingTokenSupport": false,
                        "multilineTokenSupport": false
                    },
                    "foldingRange": {
                        "dynamicRegistration": true,
                        "lineFoldingOnly": false,
                        "foldingRangeKind": {
                            "valueSet": ["comment", "imports", "region"]
                        },
                        "foldingRange": {
                            "collapsedText": true
                        }
                    },
                    "codeLens": { "dynamicRegistration": true },
                    "codeAction": {
                        "dynamicRegistration": true,
                        "resolveSupport": {
                            "properties": ["edit", "command"]
                        },
                        "codeActionLiteralSupport": {
                            "codeActionKind": {
                                "valueSet": ["quickfix", "refactor", "source"]
                            }
                        }
                    },
                    "publishDiagnostics": {
                        "versionSupport": true,
                        "relatedInformation": true,
                        "tagSupport": {
                            "valueSet": [1, 2]
                        }
                    }
                },
                "workspace": { "semanticTokens": { "refreshSupport": true },
                    "applyEdit": true,
                    "configuration": true,
                    "workspaceFolders": true,
                    "workspaceEdit": {
                        "documentChanges": true
                    },
                    "executeCommand": { "dynamicRegistration": true },
                    "symbol": {
                        "dynamicRegistration": true,
                        "symbolKind": {
                            "valueSet": [
                                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13,
                                14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26
                            ]
                        }
                    }
                },
                "window": {
                    "workDoneProgress": true
                },
                "experimental": experimental_capabilities(plugin_owned, &state.offered_toolchain_providers, &state.offered_keymap_import_providers)
            },
            "initializationOptions": request.initialization_options
        }),
    )?;
    Ok(client_response(state, vec![message], Vec::new()))
}

/// Opens a document and emits the corresponding LSP notification.
pub fn client_open_document(
    request: ClientOpenDocumentRequest,
) -> Result<LspClientResponse, CoreError> {
    validate_uri(&request.uri)?;
    let mut state = request.state;
    let document = LspClientDocument {
        uri: request.uri.clone(),
        language_id: request.language_id,
        version: 1,
        text: request.text.into(),
    };
    let message = json_rpc_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": document.uri,
                "languageId": document.language_id,
                "version": document.version,
                "text": document.text
            }
        }),
    )?;
    state.diagnostics.remove(&request.uri);
    state.diagnostic_versions.remove(&request.uri);
    state.open_documents.insert(request.uri, document);
    Ok(client_response(state, vec![message], Vec::new()))
}

/// Updates a document and emits a monotonically versioned change notification.
///
/// `content_changes` follow LSP `didChange` semantics: they apply in array order
/// and each range is relative to the result of the previous change. `text` is
/// optional alongside them; when present it is checked against the replay.
pub fn client_change_document(
    request: ClientChangeDocumentRequest,
) -> Result<LspClientResponse, CoreError> {
    validate_uri(&request.uri)?;
    let mut state = request.state;
    let Some(document) = state.open_documents.get(&request.uri).cloned() else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Cannot change a document that is not open in the LSP client.",
        ));
    };
    let replayed = if request.content_changes.is_empty() {
        None
    } else {
        match apply_content_changes_sequential(&document.text, &request.content_changes) {
            Ok(text) => Some(text),
            Err(_) if !request.text.is_empty() => None,
            Err(error) => return Err(error),
        }
    };
    // A host snapshot that disagrees with the replay means one side missed an
    // edit, so the snapshot replaces the document instead of extending the drift.
    let (next_text, replayed) = match replayed {
        Some(text) if request.text.is_empty() || text == request.text => (text, true),
        _ => (request.text, false),
    };
    let wire_incremental = (replayed
        && state.text_document_sync == LspTextDocumentSyncKind::Incremental
        && request
            .content_changes
            .iter()
            .all(|change| change.range.is_some()))
    .then_some(request.content_changes);
    let content_changes = if let Some(changes) = wire_incremental.as_ref() {
        json!(changes
            .iter()
            .map(|change| {
                let range = change.range.expect("incremental changes require a range");
                json!({
                    "range": {
                        "start": {
                            "line": range.start.line,
                            "character": range.start.utf16_column
                        },
                        "end": {
                            "line": range.end.line,
                            "character": range.end.utf16_column
                        }
                    },
                    "text": change.text
                })
            })
            .collect::<Vec<_>>())
    } else {
        json!([{ "text": next_text }])
    };
    let mut document = document;
    document.version += 1;
    document.text = next_text.into();
    let message = json_rpc_notification(
        "textDocument/didChange",
        json!({
            "textDocument": {
                "uri": document.uri,
                "version": document.version
            },
            "contentChanges": content_changes
        }),
    )?;
    state.open_documents.insert(request.uri, document);
    Ok(client_response(state, vec![message], Vec::new()))
}

/// Closes a document and clears diagnostics owned by its URI.
pub fn client_close_document(
    request: ClientCloseDocumentRequest,
) -> Result<LspClientResponse, CoreError> {
    validate_uri(&request.uri)?;
    let mut state = request.state;
    let Some(document) = state.open_documents.remove(&request.uri) else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Cannot close a document that is not open in the LSP client.",
        ));
    };
    state.diagnostics.remove(&request.uri);
    state.diagnostic_versions.remove(&request.uri);
    let message = json_rpc_notification(
        "textDocument/didClose",
        json!({
            "textDocument": {
                "uri": document.uri
            }
        }),
    )?;
    Ok(client_response(state, vec![message], Vec::new()))
}

/// Begins the two-step LSP shutdown and exit handshake.
pub fn client_shutdown(request: ClientShutdownRequest) -> Result<LspClientResponse, CoreError> {
    let mut state = request.state;
    if state.shutdown_requested {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "LSP client shutdown has already been requested.",
        ));
    }
    let id = allocate_request(&mut state, "shutdown");
    state.shutdown_requested = true;
    let message = json_rpc_message_without_params(Some(&id), "shutdown")?;
    Ok(client_response(state, vec![message], Vec::new()))
}

/// Translates one feature request into its JSON-RPC call. Every method except
/// `workspace/symbol` must name a document the client has open.
pub fn client_feature_request(
    request: ClientFeatureRequest,
) -> Result<LspClientResponse, CoreError> {
    // `workspace/symbol` is workspace-wide: it names no document, so an empty URI is accepted.
    let workspace_wide = request.method == "workspace/symbol";
    if !workspace_wide {
        validate_uri(&request.uri)?;
    }
    validate_lsp_method(&request.method)?;
    let params = feature_request_params(&request)?;
    let uri = request.uri.clone();
    let method = request.method.clone();
    let mut state = request.state;
    if !workspace_wide && !state.open_documents.contains_key(&uri) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Cannot request LSP features for a document that is not open.",
        ));
    }
    let id = allocate_request(&mut state, &method);
    let message = json_rpc_request(&id, &method, params)?;
    Ok(client_response(state, vec![message], Vec::new()))
}

pub(crate) fn client_response(
    state: LspClientState,
    messages: Vec<String>,
    events: Vec<LspClientEvent>,
) -> LspClientResponse {
    LspClientResponse {
        state,
        messages,
        events,
    }
}

pub(crate) fn allocate_request(state: &mut LspClientState, method: &str) -> String {
    let id = state.next_request_id.to_string();
    if method == "textDocument/semanticTokens/full" {
        if let Some(legend) = state
            .semantic_token_providers
            .values()
            .rev()
            .find_map(semantic_token_legend)
        {
            state
                .pending_semantic_legends
                .insert(id.clone(), legend.clone());
        }
    }
    state.next_request_id += 1;
    state
        .pending_requests
        .insert(id.clone(), method.to_string());
    id
}
