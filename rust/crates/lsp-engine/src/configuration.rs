//! Plugin workspace configuration and indexing scope pushed into running sessions.

use super::*;

/// Rejects plugin configuration that is not a JSON object, the only shape
/// `workspace/configuration` sections can be looked up in.
pub(crate) fn checked_workspace_configuration(
    configuration: Option<Value>,
) -> Result<Option<Value>, CoreError> {
    match configuration {
        None | Some(Value::Null) => Ok(None),
        Some(configuration @ Value::Object(_)) => Ok(Some(configuration)),
        Some(_) => Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "workspaceConfiguration must be a JSON object.",
        )),
    }
}

/// Answers one `workspace/configuration` item from plugin configuration.
///
/// A section names a dotted path (`python.analysis`); an exact top-level key
/// wins over the path so servers that use dotted keys still match. A missing
/// section answers `null`, which tells the server to use its own default.
pub(crate) fn configuration_section(configuration: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|section| !section.is_empty()) else {
        return configuration.clone();
    };
    if let Some(value) = configuration.get(section) {
        return value.clone();
    }
    section
        .split('.')
        .try_fold(configuration, |value, key| value.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `workspace/didChangeConfiguration` carrying the whole plugin configuration.
pub(crate) fn plugin_configuration_notification(configuration: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "workspace/didChangeConfiguration",
        "params": { "settings": configuration }
    })
    .to_string()
}

pub(crate) fn apply_indexing_to_sessions(
    root: &Path,
    sessions: Vec<Arc<RuntimeSession>>,
) -> Result<(), CoreError> {
    let root_uri = url::Url::from_directory_path(root).ok();
    let canonical_root = root_uri
        .as_ref()
        .and_then(|uri| canonical_workspace_root(uri.as_str()));
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            canonical_workspace_root(&session.root_uri) == canonical_root
                && session.active.load(Ordering::Acquire)
        })
        .collect::<Vec<_>>();
    if sessions.is_empty() {
        return Ok(());
    }
    let prepared = lithe_project::indexing::prepare_provider_exclusions(root)?;
    let roots = lithe_project::indexing::excluded_roots(root);
    let revision = prepared.as_ref().map_or(0, |policy| policy.revision);
    if let Some(policy) = &prepared {
        policy.check()?;
    }
    for session in sessions {
        if let Some(policy) = &prepared {
            policy.check()?;
        }
        if ensure_not_terminal(session.lock_state()?.lifecycle).is_err() {
            continue;
        }
        if canonical_workspace_root(&session.root_uri) != canonical_root {
            continue;
        }
        let mut previous = session.lock_indexing_roots()?;
        if *previous == roots && session.indexing_revision.load(Ordering::Acquire) == revision {
            continue;
        }
        let documents = session
            .lock_state()?
            .client
            .open_documents
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in documents {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                session.close_document(&uri)?;
            }
        }
        let diagnostic_uris = session
            .lock_state()?
            .client
            .diagnostics
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in diagnostic_uris {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                let mut state = session.lock_state()?;
                state.client.diagnostics.remove(&uri);
                state.client.diagnostic_versions.remove(&uri);
                push_diagnostics_event(&session, &mut state, &uri, None, Vec::new());
            }
        }
        let pending_uris = session
            .lock_state()?
            .pending_workspace_file_changes
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for uri in pending_uris {
            if lithe_project::indexing::uri_is_excluded(&session.root_uri, &uri)? {
                session
                    .lock_state()?
                    .pending_workspace_file_changes
                    .remove(&uri);
            }
        }
        // Exclusions are folded into the configuration answered through
        // `workspace/configuration`, so an initialized server is told to pull
        // the changed sections again instead of keeping its old import scope.
        {
            let order = session.lock_outbound_order()?;
            if session.lock_state()?.client.initialized {
                if let Some(configuration) = session.plugin_configuration() {
                    session.send_messages_or_fail(
                        &order,
                        vec![plugin_configuration_notification(&configuration)],
                        "indexingChanged",
                    )?;
                }
            }
        }
        *previous = roots.clone();
        session.indexing_revision.store(revision, Ordering::Release);
    }
    Ok(())
}

impl RuntimeSession {
    /// Plugin configuration the server currently sees, if any.
    pub(crate) fn plugin_configuration(&self) -> Option<Arc<Value>> {
        let base = self
            .plugin_configuration
            .lock()
            .map(|configuration| configuration.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        crate::scan_settings::configuration(&self.provider_id, &self.root_uri, base.as_deref())
            .map(Arc::new)
    }

    pub(crate) fn update_plugin_configuration(
        &self,
        configuration: Value,
    ) -> Result<UpdateWorkspaceConfigurationResponse, CoreError> {
        if self.plugin_id.is_none() {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Only plugin-provided language servers accept plugin configuration.",
            ));
        }
        let configuration = Arc::new(configuration);
        let outbound_order = self.lock_outbound_order()?;
        let initialized = {
            let state = self.lock_state()?;
            if matches!(
                state.lifecycle,
                LspLifecycleState::Stopping
                    | LspLifecycleState::Stopped
                    | LspLifecycleState::Failed
            ) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language session is no longer running.",
                ));
            }
            // Under `outbound_order`, an uninitialized client sends the stored
            // configuration with its `initialized` notification instead.
            state.client.initialized
        };
        *self
            .plugin_configuration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(configuration.clone());
        let messages = if initialized {
            vec![plugin_configuration_notification(
                self.plugin_configuration()
                    .as_deref()
                    .unwrap_or(&configuration),
            )]
        } else {
            Vec::new()
        };
        self.send_messages_or_fail(&outbound_order, messages, "workspaceConfigurationUpdate")?;
        Ok(UpdateWorkspaceConfigurationResponse { sent: initialized })
    }
}
