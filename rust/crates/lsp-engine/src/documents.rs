//! Document synchronization: open, change, save and close, workspace file changes,
//! queued documents and the paths diagnostics are matched against.

use super::*;

impl RuntimeSession {
    pub(crate) fn save_document(&self, request: SaveDocumentRequest) -> Result<bool, CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if state.lifecycle != LspLifecycleState::Ready {
                return Ok(false);
            }
            let Some(include_text) = state.client.text_document_save_include_text else {
                return Ok(false);
            };
            let Some(document) = state.client.open_documents.get(&request.uri) else {
                return Ok(false);
            };
            // A server may reload its working copy from the saved text. A
            // delayed save must not discard a newer editor revision already
            // synchronized.
            if *document.text != request.text {
                return Ok(false);
            }
            let mut params = json!({"textDocument": {"uri": request.uri}});
            if include_text {
                params["text"] = json!(request.text);
            }
            json!({"jsonrpc": "2.0", "method": "textDocument/didSave", "params": params})
                .to_string()
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "saveDocument")?;
        Ok(true)
    }

    pub(crate) fn workspace_files_changed(
        &self,
        changes: Vec<WorkspaceFileChange>,
    ) -> Result<(), CoreError> {
        if changes.is_empty() {
            return Ok(());
        }
        let _indexing_order = self.lock_indexing_roots()?;
        let mut normalized = BTreeMap::new();
        for change in changes {
            if lithe_project::indexing::uri_is_excluded(&self.root_uri, &change.uri)? {
                continue;
            }
            if !change.uri.contains("://") || change.uri.contains('\0') {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Workspace file changes require absolute document URIs.",
                )
                .with_details(change.uri));
            }
            // The last event for a URI represents its final filesystem state
            // and prevents a watcher burst from causing redundant imports.
            normalized.insert(change.uri, change.kind);
        }
        if normalized.is_empty() {
            return Ok(());
        }
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if !state.client.initialized {
                state.pending_workspace_file_changes.extend(normalized);
                return Ok(());
            }
            workspace_file_changes_notification(normalized)
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "workspaceFilesChanged")
    }

    pub(crate) fn sync_document(
        &self,
        request: SyncDocumentRequest,
    ) -> Result<SyncDocumentResponse, CoreError> {
        self.sync_document_after_policy_check(request, || {})
    }

    pub(crate) fn sync_document_after_policy_check(
        &self,
        request: SyncDocumentRequest,
        after_policy_check: impl FnOnce(),
    ) -> Result<SyncDocumentResponse, CoreError> {
        let _indexing_order = self.lock_indexing_roots()?;
        if lithe_project::indexing::uri_is_excluded(&self.root_uri, &request.uri)? {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Document is excluded from indexing",
            ));
        }
        after_policy_check();
        let uri = request.uri;
        let mut messages = Vec::new();
        let mut stale_cancellations = Vec::new();
        let document_version;
        let outbound_order = self.lock_outbound_order()?;
        {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            if let Some(document) = state.client.open_documents.get(&uri) {
                // Hosts commonly publish both a ranged edit and the resulting
                // full text. Repeating that full text must not create another
                // server reconciliation cycle or advance the semantic version.
                if request.content_changes.is_empty() && *document.text == *request.text {
                    return Ok(SyncDocumentResponse {
                        document_version: document.version.max(1),
                        changed: false,
                    });
                }
            }
            if state.client.initialized {
                let response = if state
                    .client
                    .open_documents
                    .get(&uri)
                    .is_some_and(|document| document.version > 0)
                {
                    client_change_document(ClientChangeDocumentRequest {
                        state: state.client.clone(),
                        uri: uri.clone(),
                        text: request.text,
                        content_changes: request.content_changes,
                    })?
                } else {
                    let text = unopened_document_text(
                        state.client.open_documents.get(&uri),
                        request.text,
                        &request.content_changes,
                    )?;
                    client_open_document(ClientOpenDocumentRequest {
                        state: state.client.clone(),
                        uri: uri.clone(),
                        language_id: request.language_id,
                        text,
                    })?
                };
                state.client = response.state;
                messages = response.messages;
                document_version = state
                    .client
                    .open_documents
                    .get(&uri)
                    .map(|document| document.version)
                    .unwrap_or_default();
                stale_cancellations =
                    cancel_stale_document_requests_locked(self, &mut state, &uri, document_version);
            } else {
                // Version zero means the semantic document exists in the Rust
                // store but has not yet been opened on the server. The latest
                // sync wins until initialize completes.
                let text = unopened_document_text(
                    state.client.open_documents.get(&uri),
                    request.text,
                    &request.content_changes,
                )?;
                state.client.diagnostics.remove(&uri);
                state.client.diagnostic_versions.remove(&uri);
                state.client.open_documents.insert(
                    uri.clone(),
                    LspClientDocument {
                        uri,
                        language_id: request.language_id,
                        version: 0,
                        text: text.into(),
                    },
                );
                // Version zero remains an internal "not opened on the wire"
                // sentinel. Consumers receive version one, which is the exact
                // version the latest queued text will have after initialize.
                document_version = 1;
            }
        }
        messages.extend(stale_cancellations);
        self.send_messages_or_fail(&outbound_order, messages, "documentSync")?;
        Ok(SyncDocumentResponse {
            document_version,
            changed: true,
        })
    }

    pub(crate) fn close_document(&self, uri: &str) -> Result<(), CoreError> {
        let mut messages = Vec::new();
        let cleared;
        let outbound_order = self.lock_outbound_order()?;
        {
            let mut state = self.lock_state()?;
            ensure_not_terminal(state.lifecycle)?;
            let Some(document) = state.client.open_documents.get(uri).cloned() else {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Cannot close a document that is not owned by the language-server session.",
                ));
            };
            cleared = state.client.diagnostics.contains_key(uri);
            if document.version == 0 {
                state.client.open_documents.remove(uri);
                state.client.diagnostics.remove(uri);
                state.client.diagnostic_versions.remove(uri);
            } else {
                let response = client_close_document(ClientCloseDocumentRequest {
                    state: state.client.clone(),
                    uri: uri.to_string(),
                })?;
                state.client = response.state;
                messages = response.messages;
            }
            if cleared {
                push_diagnostics_event(self, &mut state, uri, None, Vec::new());
            }
        }
        self.send_messages_or_fail(&outbound_order, messages, "documentClose")
    }
}

/// The file a `file:` URI resolves to once symbolic links are followed, or
/// `None` for other schemes and for files that do not exist.
fn resolved_file_of_uri(uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    lithe_workspace::fs::canonicalize(path).ok()
}

/// Which open documents the host reached through a symbolic link.
///
/// Only such a document can be the target of a diagnostics rewrite, so the
/// rest are remembered as examined and never resolved again while open.
#[derive(Default)]
pub(crate) struct LinkedDocumentPaths {
    /// Open documents whose file has already been resolved.
    pub(crate) examined: BTreeSet<String>,
    /// Resolved file of each examined document that differs from its own
    /// path, keyed by the document's URI.
    pub(crate) resolved: BTreeMap<String, PathBuf>,
}

impl LinkedDocumentPaths {
    /// Drops closed documents and resolves each newly opened one once.
    fn refresh(&mut self, open_documents: &BTreeMap<String, LspClientDocument>) {
        self.examined.retain(|uri| open_documents.contains_key(uri));
        self.resolved
            .retain(|uri, _| open_documents.contains_key(uri));
        for uri in open_documents.keys() {
            if self.examined.contains(uri) {
                continue;
            }
            let Some(resolved) = resolved_file_of_uri(uri) else {
                // Not on disk yet (an unsaved document); look again next time.
                continue;
            };
            self.examined.insert(uri.clone());
            let own = url::Url::parse(uri)
                .ok()
                .and_then(|url| url.to_file_path().ok());
            if own.as_ref() != Some(&resolved) {
                self.resolved.insert(uri.clone(), resolved);
            }
        }
    }
}

/// Re-addresses published diagnostics to the open document they describe.
///
/// A server reports a file at its resolved location while the host may have
/// opened it through a symbolic link (a server may answer `/private/tmp/...`
/// for a document opened as `/tmp/...` on macOS). The client reducer matches URIs
/// textually and would drop such diagnostics, so when no open document carries
/// the published URI this returns the message rewritten to the URI of the open
/// document that resolves to the same file. Returns `None` when the message
/// needs no change.
///
/// Servers publish for many files that are not open, so this runs on a hot
/// path. Only documents opened through a link can be rewrite targets, and
/// `linked` remembers those, which makes the common case free of file-system
/// calls and the linked case a single `canonicalize` of the published file.
pub(crate) fn diagnostics_addressed_to_open_document(
    open_documents: &BTreeMap<String, LspClientDocument>,
    linked: &mut LinkedDocumentPaths,
    message: &Value,
) -> Option<Value> {
    let published = message.get("params")?.get("uri")?.as_str()?;
    if open_documents.contains_key(published) {
        return None;
    }
    linked.refresh(open_documents);
    if linked.resolved.is_empty() {
        return None;
    }
    let target = resolved_file_of_uri(published)?;
    let document_uri = linked
        .resolved
        .iter()
        .find(|(_, resolved)| **resolved == target)
        .map(|(uri, _)| uri.clone())?;
    let mut message = message.clone();
    message["params"]["uri"] = Value::String(document_uri);
    Some(message)
}

fn cancel_stale_document_requests_locked(
    session: &RuntimeSession,
    state: &mut SessionState,
    uri: &str,
    document_version: i64,
) -> Vec<String> {
    let stale_ids: Vec<String> = state
        .pending
        .iter()
        .filter(|(_, pending)| {
            pending.document_uri.as_deref() == Some(uri)
                && pending
                    .document_version
                    .is_some_and(|version| version < document_version)
                && is_stale_sensitive_method(&pending.method)
        })
        .map(|(request_id, _)| request_id.clone())
        .collect();
    let mut messages = Vec::new();
    for request_id in stale_ids {
        let Some(pending) = state.pending.remove(&request_id) else {
            continue;
        };
        state.client.pending_requests.remove(&request_id);
        state.client.pending_semantic_legends.remove(&request_id);
        if let Some(operation_id) = pending.operation_id.as_deref() {
            state.request_by_operation.remove(operation_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                session,
                "staleDocumentVersion",
                "request",
                "The document changed before the language-server result arrived.",
                FailureContext::request(&pending.method, Some(uri))
                    .with_underlying(Some("A newer document version superseded this request.")),
            );
            push_request_event(
                session,
                state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
        }
        messages.push(
            json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": { "id": request_id }
            })
            .to_string(),
        );
    }
    messages
}

fn is_stale_sensitive_method(method: &str) -> bool {
    matches!(
        method,
        "textDocument/completion"
            | "textDocument/hover"
            | "textDocument/inlayHint"
            | "textDocument/foldingRange"
            | "textDocument/codeLens"
            | "codeLens/resolve"
            | "textDocument/implementation"
    )
}

/// Resolves the text of a document that has not been opened on the wire yet.
///
/// Hosts may send only ranged edits, which then extend the text Core retained.
/// Edits for a document Core never received cannot be resolved, so the host
/// resends the full text.
fn unopened_document_text(
    stored: Option<&LspClientDocument>,
    text: String,
    changes: &[LspDocumentContentChange],
) -> Result<String, CoreError> {
    if !text.is_empty() || changes.is_empty() {
        return Ok(text);
    }
    let Some(stored) = stored else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Cannot apply ranged edits to a document Core has not received.",
        )
        .with_details("documentNotSynchronized"));
    };
    lithe_text::apply_content_changes_sequential(&stored.text, changes)
}

pub(crate) fn flush_queued_documents_locked(
    state: &mut SessionState,
) -> Result<Vec<String>, CoreError> {
    let queued: Vec<_> = state
        .client
        .open_documents
        .values()
        .filter(|document| document.version == 0)
        .cloned()
        .collect();
    let mut messages = Vec::new();
    for document in queued {
        let response = client_open_document(ClientOpenDocumentRequest {
            state: state.client.clone(),
            uri: document.uri,
            language_id: document.language_id,
            text: document.text.to_string(),
        })?;
        state.client = response.state;
        messages.extend(response.messages);
    }
    if !state.pending_workspace_file_changes.is_empty() {
        let changes = std::mem::take(&mut state.pending_workspace_file_changes);
        messages.push(workspace_file_changes_notification(changes));
    }
    Ok(messages)
}

fn workspace_file_changes_notification(
    changes: BTreeMap<String, WorkspaceFileChangeKind>,
) -> String {
    let changes: Vec<_> = changes
        .into_iter()
        .map(|(uri, kind)| {
            let event_type = match kind {
                WorkspaceFileChangeKind::Created => 1,
                WorkspaceFileChangeKind::Changed => 2,
                WorkspaceFileChangeKind::Deleted => 3,
            };
            json!({ "uri": uri, "type": event_type })
        })
        .collect();
    json!({
        "jsonrpc": "2.0",
        "method": "workspace/didChangeWatchedFiles",
        "params": { "changes": changes }
    })
    .to_string()
}
