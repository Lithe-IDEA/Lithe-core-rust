//! Requests sent to a session: semantic features, provider requests, plugin functions,
//! host replies and notifications, and cancellation.

use super::*;

impl RuntimeSession {
    pub(crate) fn request(
        &self,
        request: SemanticRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let document_version = request.uri.as_deref().and_then(|uri| {
            self.lock_state().ok().and_then(|state| {
                state
                    .client
                    .open_documents
                    .get(uri)
                    .map(|document| document.version.max(1))
            })
        });
        self.request_with_kind(
            request,
            operation_id,
            PendingKind::Feature,
            document_version,
        )
    }

    /// Writes a build-provider request. It needs no open document, so it is
    /// not a semantic feature request; only the negotiated protocol's methods
    /// are accepted, so the host cannot use it as a generic JSON-RPC tunnel.
    pub(crate) fn build_request(
        &self,
        request: BuildProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        if !lithe_lsp_client::build_provider::is_build_method(&request.method) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "This is not a build-provider request.",
            )
            .with_details(request.method));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) =
            {
                let mut state = self.lock_state()?;
                if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Language server is not ready.",
                    ));
                }
                if state.request_by_operation.contains_key(&operation_id) {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "The language-server operation ID is already pending.",
                    ));
                }
                if !state.client.server_capabilities.iter().any(|feature| {
                    feature == lithe_lsp_client::build_provider::BUILD_PROVIDER_FEATURE
                }) {
                    return Err(CoreError::new(
                        ErrorCode::NotSupported,
                        "The process did not agree on a build-provider protocol version.",
                    )
                    .with_details(lithe_lsp_client::build_provider::BUILD_PROVIDER_FEATURE));
                }
                if request.params.get("launchOptions").is_some()
                    && request.method != lithe_lsp_client::build_provider::PREPARE_RUN_METHOD
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Launch options apply only to run preparation.",
                    ));
                }
                if (request.params.get("selectorKind").is_some()
                    || request.params.get("selectors").is_some())
                    && request.method != lithe_lsp_client::build_provider::TEST_METHOD
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Opaque selectors require a provider test request.",
                    ));
                }
                if request.params.get("testResults").is_some()
                    && (request.method != lithe_lsp_client::build_provider::TEST_METHOD
                        || request.params["testResults"] != json!(true))
                {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Inline test reports require a provider test request.",
                    ));
                }
                let mut params = request.params;
                if lithe_lsp_client::build_provider::is_progress_method(&request.method) {
                    let object = params.as_object_mut().ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "Build task parameters must be an object.",
                        )
                    })?;
                    object.insert(
                        "operationID".to_string(),
                        json!(state.client.next_request_id.to_string()),
                    );
                }
                let timeout = request
                    .timeout_milliseconds
                    .map(|milliseconds| {
                        Duration::from_millis(milliseconds.clamp(1, MAX_BUILD_PROVIDER_TIMEOUT_MS))
                    })
                    .unwrap_or(state.request_timeout);
                let response = allocate_raw_request(state.client.clone(), &request.method, params)?;
                let request_id = (response.state.next_request_id - 1).to_string();
                let now = Instant::now();
                state.client = response.state;
                state.pending.insert(
                    request_id.clone(),
                    PendingRequest {
                        kind: PendingKind::Feature,
                        operation_id: Some(operation_id.clone()),
                        method: request.method,
                        document_uri: None,
                        document_version: None,
                        created_at: now,
                        deadline: now + timeout,
                    },
                );
                state
                    .request_by_operation
                    .insert(operation_id, request_id.clone());
                (response.messages, request_id)
            };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    pub(crate) fn keymap_import_request(
        &self,
        request: KeymapImportProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        // A snapshot reference is an opaque token, never a filesystem path.
        // The future host service must separately enforce its session ownership.
        if request.snapshot_id.is_empty()
            || request.snapshot_id.len() > 128
            || !request
                .snapshot_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid import snapshot reference.",
            ));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Plugin backend is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The operation ID is already pending.",
                ));
            }
            if !state
                .client
                .keymap_import_providers
                .contains(&request.provider_id)
            {
                return Err(CoreError::new(
                    ErrorCode::NotSupported,
                    "The plugin did not negotiate this import provider.",
                )
                .with_details("plugin.keymapImport.providerUnavailable"));
            }
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_KEYMAP_IMPORT_TIMEOUT_MS))
                })
                .unwrap_or(
                    state
                        .request_timeout
                        .min(Duration::from_millis(MAX_KEYMAP_IMPORT_TIMEOUT_MS)),
                );
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_lsp_client::keymap_provider::IMPORT_METHOD,
                json!({"providerID": request.provider_id, "snapshotID": request.snapshot_id}),
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_lsp_client::keymap_provider::IMPORT_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes a VCS-provider request planned by `lithe_vcs::plan`, which
    /// checks the operation's arguments and negotiated capability.
    pub(crate) fn toolchain_request(
        &self,
        request: ToolchainProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let provider = request.params["providerID"].as_str().unwrap_or_default();
            if !state
                .client
                .toolchain_providers
                .iter()
                .any(|negotiated| negotiated == provider)
            {
                return Err(CoreError::new(
                    ErrorCode::NotSupported,
                    "The process did not negotiate this toolchain provider.",
                )
                .with_details("plugin.toolchain.providerUnavailable"));
            }
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_TOOLCHAIN_QUERY_TIMEOUT_MS))
                })
                .unwrap_or(state.request_timeout);
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_lsp_client::toolchain_provider::QUERY_METHOD,
                request.params,
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_lsp_client::toolchain_provider::QUERY_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    pub(crate) fn vcs_request(
        &self,
        request: VcsProviderRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let operation = lithe_vcs::Operation::parse(&request.operation).ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Unknown VCS operation")
                .with_details(request.operation.clone())
        })?;
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let negotiation = state
                .client
                .vcs_provider
                .as_ref()
                .filter(|negotiation| negotiation.vcs == request.vcs)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotSupported,
                        format!("The process is not a negotiated {} provider.", request.vcs),
                    )
                    .with_details("vcs.provider.unavailable")
                })?;
            let planned = lithe_vcs::plan(
                operation,
                &request.root,
                request.tool.as_deref(),
                request.arguments,
                &negotiation.capabilities,
            )?;
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(milliseconds.clamp(1, MAX_VCS_PROVIDER_TIMEOUT_MS))
                })
                .unwrap_or(state.request_timeout);
            let response =
                allocate_raw_request(state.client.clone(), planned.method, planned.params)?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: planned.method.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes a `lithe/function/invoke` request. The plan comes from
    /// `plugin_function::plan_invocation`, so plugin identity, negotiation,
    /// and registration are checked before anything reaches the process.
    ///
    /// `caller` is the plugin whose call the relay forwards, and `None` for a
    /// call the host starts. Only the relay passes `Some`, so a host cannot
    /// name a plugin as the caller and a plugin cannot choose its own name.
    pub(crate) fn function_request(
        &self,
        request: PluginFunctionRequest,
        operation_id: String,
        caller: Option<&str>,
    ) -> Result<(), CoreError> {
        if caller.is_none() && relay_hub::is_relay_operation_id(&operation_id) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "This language-server operation ID is reserved for relayed plugin calls.",
            ));
        }
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let negotiated = state
                .client
                .server_capabilities
                .iter()
                .any(|feature| feature == lithe_plugin_rpc::FUNCTIONS_FEATURE);
            // Progress echoes this per-request wire ID, never the reusable host ID.
            let wire_operation_id = state.client.next_request_id.to_string();
            let params = lithe_plugin_rpc::plan_invocation(
                &lithe_plugin_rpc::FunctionSession {
                    plugin_id: self.plugin_id.as_deref(),
                    negotiated,
                    functions: &state.client.plugin_functions,
                },
                &request.function,
                &wire_operation_id,
                &request.params,
            )?;
            let mut params = params;
            lithe_plugin_rpc::attach_context(&mut params, request.context.as_ref())?;
            lithe_plugin_rpc::attach_caller(&mut params, caller);
            let timeout = request
                .timeout_milliseconds
                .map(|milliseconds| {
                    Duration::from_millis(
                        milliseconds.clamp(1, lithe_plugin_rpc::MAX_FUNCTION_TIMEOUT_MS),
                    )
                })
                .unwrap_or(state.request_timeout);
            let response = allocate_raw_request(
                state.client.clone(),
                lithe_plugin_rpc::INVOKE_METHOD,
                params,
            )?;
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            state.client = response.state;
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    kind: PendingKind::Feature,
                    operation_id: Some(operation_id.clone()),
                    method: lithe_plugin_rpc::INVOKE_METHOD.to_string(),
                    document_uri: None,
                    document_version: None,
                    created_at: now,
                    deadline: now + timeout,
                },
            );
            state
                .request_by_operation
                .insert(operation_id.clone(), request_id.clone());
            state.function_by_operation.insert(
                operation_id,
                PendingFunctionCall {
                    function_id: request.function.function_id,
                    request_id: request_id.clone(),
                },
            );
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Sends the backend the host's answer to one forwarded host-service
    /// request. An unknown ID, including one already answered or of a session
    /// that restarted, is refused so a late answer never reaches another
    /// request.
    pub(crate) fn host_response(&self, response: PluginHostResponse) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let mut state = self.lock_state()?;
            let backend_id = state
                .host_requests
                .remove(&response.host_request_id)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "No host-service request with this ID is waiting for an answer.",
                    )
                })?;
            match (response.result, response.error) {
                (_, Some(failure)) => {
                    let (code, rpc_code) = lithe_plugin_rpc::host_failure(failure.code.as_deref());
                    let mut data = json!({ "code": code });
                    if let Some(details) =
                        lithe_plugin_rpc::bounded_host_details(failure.details.as_ref())
                    {
                        data["details"] = details;
                    }
                    json_rpc_error_with_data(
                        &backend_id,
                        rpc_code,
                        &lithe_plugin_rpc::bounded_host_message(&failure.message),
                        data,
                    )?
                }
                (result, None) => {
                    let result = result.unwrap_or(Value::Null);
                    if result.to_string().len() > lithe_plugin_rpc::MAX_RESULT_BYTES {
                        json_rpc_error_with_code(
                            &backend_id,
                            -32014,
                            "Host result exceeds the byte limit.",
                            lithe_plugin_rpc::codes::HOST_LIMIT_EXCEEDED,
                        )?
                    } else {
                        json_rpc_result(&backend_id, result)?
                    }
                }
            }
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "hostResponse")
    }

    /// Writes one host notification to a ready plugin-owned backend.
    /// Returns `false`, writing nothing, when the backend negotiated no
    /// plugin-function protocol version.
    pub(crate) fn host_notify(
        &self,
        notification: PluginHostNotification,
    ) -> Result<bool, CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let message = {
            let state = self.lock_state()?;
            if self.plugin_id.is_none() {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Only a plugin process receives host notifications.",
                ));
            }
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            let delivered = lithe_plugin_rpc::host_notification(
                &notification.method,
                &notification.params,
                state.client.plugin_function_version,
                &state.client.host_services,
            )?;
            if !delivered {
                return Ok(false);
            }
            json_rpc_notification(&notification.method, notification.params)?
        };
        self.send_messages_or_fail(&outbound_order, vec![message], "hostNotify")?;
        Ok(true)
    }

    /// Admits a backend's request to its language server: it is sent now,
    /// held behind a gate, or joined to a command that already runs.
    pub(crate) fn plugin_server_request(
        &self,
        request: PluginServerRequest,
        operation_id: String,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let server = plugin_server_target(
                &mut state,
                self.plugin_id.as_deref(),
                &request.plugin_id,
                &request.server_id,
            )?;
            let now = Instant::now();
            let deadline = now + plugin_server_timeout(request.timeout_milliseconds);
            let admission = server.admit_request(Arrival {
                operation_id: &operation_id,
                method: &request.method,
                params: &request.params,
                now,
                deadline,
            })?;
            if admission != Admission::Send {
                // Held and joined requests are written later, or never.
                return Ok(());
            }
            match register_plugin_server_request(
                &mut state,
                &operation_id,
                &request.method,
                request.params,
                now,
                deadline,
            ) {
                Ok(registered) => registered,
                Err(error) => {
                    if let Some(server) = state.plugin_server.as_mut() {
                        server.finish(&operation_id);
                    }
                    return Err(error);
                }
            }
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Sends a backend's notification to its language server. Returns whether
    /// it was sent now.
    pub(crate) fn plugin_server_notify(
        &self,
        request: PluginServerNotify,
    ) -> Result<bool, CoreError> {
        {
            let mut state = self.lock_state()?;
            let server = plugin_server_target(
                &mut state,
                self.plugin_id.as_deref(),
                &request.plugin_id,
                &request.server_id,
            )?;
            server.admit_notification(&request.method, &request.params)?;
        }
        if request.method == DID_CHANGE_CONFIGURATION {
            // The settings also answer the server's own `workspace/configuration`
            // requests, so they go through the plugin configuration.
            let settings = request
                .params
                .get("settings")
                .filter(|settings| settings.is_object())
                .cloned()
                .ok_or_else(|| {
                    CoreError::new(ErrorCode::InvalidRequest, "settings must be an object.")
                        .with_details(lithe_plugin_rpc::codes::HOST_INVALID_PARAMS)
                })?;
            return Ok(self.update_plugin_configuration(settings)?.sent);
        }
        let message = json_rpc_notification(&request.method, request.params)?;
        let outbound_order = self.lock_outbound_order()?;
        self.send_messages_or_fail(&outbound_order, vec![message], "pluginServerNotify")?;
        Ok(true)
    }

    fn request_with_kind(
        &self,
        request: SemanticRequest,
        operation_id: String,
        requested_kind: PendingKind,
        requested_document_version: Option<i64>,
    ) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let (messages, request_id) = {
            let mut state = self.lock_state()?;
            if state.lifecycle != LspLifecycleState::Ready || !state.client.initialized {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Language server is not ready.",
                ));
            }
            if state.request_by_operation.contains_key(&operation_id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "The language-server operation ID is already pending.",
                ));
            }
            let uri = request.uri.clone();
            if let (Some(uri), Some(expected_version)) =
                (uri.as_deref(), requested_document_version)
            {
                let current_version = state
                    .client
                    .open_documents
                    .get(uri)
                    .map(|document| document.version.max(1))
                    .ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The document is not synchronized with the language server.",
                        )
                    })?;
                if current_version != expected_version {
                    return Err(CoreError::new(
                        ErrorCode::Cancelled,
                        "The document changed before the language-server request started.",
                    )
                    .with_details(format!(
                        "expectedVersion={expected_version}, currentVersion={current_version}"
                    )));
                }
            }
            let method = semantic_method(request.operation);
            let required_capability = semantic_capability(request.operation);
            if let Some(capability) = required_capability {
                if !state
                    .client
                    .server_capabilities
                    .iter()
                    .any(|candidate| candidate == capability)
                {
                    return Err(CoreError::new(
                        ErrorCode::NotSupported,
                        "The language server did not advertise this capability.",
                    )
                    .with_details(capability));
                }
            }

            let pending_kind = requested_kind;
            let response = match request.operation {
                LspSemanticOperation::WorkspaceSymbols => {
                    let query = request.query.clone().unwrap_or_default();
                    allocate_raw_request(state.client.clone(), method, json!({ "query": query }))?
                }
                LspSemanticOperation::ExecuteCommand => {
                    let command = request.command.ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "This language-server request requires a command.",
                        )
                    })?;
                    allocate_raw_request(state.client.clone(), method, command)?
                }
                _ => {
                    let uri = uri.clone().ok_or_else(|| {
                        CoreError::new(
                            ErrorCode::InvalidRequest,
                            "This language-server operation requires a document URI.",
                        )
                    })?;
                    let document_is_open = state
                        .client
                        .open_documents
                        .get(&uri)
                        .is_some_and(|document| document.version > 0);
                    if !document_is_open {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The document is not open in the language server.",
                        ));
                    }
                    let feature_request = ClientFeatureRequest {
                        state: state.client.clone(),
                        uri,
                        method: method.to_string(),
                        position: request.position,
                        new_name: request.new_name,
                        query: request.query,
                        range: request.range,
                        diagnostics: request.diagnostics,
                        completion_context: request.completion_context,
                        completion_item: request.completion_item,
                        code_action: request.code_action,
                        command: request.command,
                        formatting_options: request.formatting_options,
                        include_declaration: request.include_declaration,
                    };
                    client_feature_request(feature_request)?
                }
            };
            let request_id = (response.state.next_request_id - 1).to_string();
            let now = Instant::now();
            let pending = PendingRequest {
                kind: pending_kind,
                operation_id: Some(operation_id.clone()),
                method: method.to_string(),
                document_uri: uri.clone(),
                document_version: requested_document_version.or_else(|| {
                    uri.as_deref()
                        .and_then(|uri| state.client.open_documents.get(uri))
                        .map(|document| document.version)
                }),
                created_at: now,
                deadline: now + state.request_timeout,
            };
            state.client = response.state;
            state.pending.insert(request_id.clone(), pending);
            state
                .request_by_operation
                .insert(operation_id, request_id.clone());
            (response.messages, request_id)
        };
        self.send_request_or_fail(&outbound_order, messages, &request_id)
    }

    /// Writes an allocated request. A write failure completes the request
    /// with an error and fails the session, because the server's view of the
    /// conversation is no longer known.
    fn send_request_or_fail(
        &self,
        outbound_order: &MutexGuard<'_, ()>,
        messages: Vec<String>,
        request_id: &str,
    ) -> Result<(), CoreError> {
        if let Err(error) = self.send_messages(outbound_order, messages) {
            self.complete_request_with_error(
                request_id,
                "transportFailed",
                "request",
                "Could not write the language-server request.",
                Some(core_error_detail(&error)),
                None,
            );
            self.fail(
                "transportFailed",
                "request",
                "Language-server stdin failed.",
                Some(core_error_detail(&error)),
                None,
            );
            self.kill_process();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn cancel_operation(&self, operation_id: &str) -> Result<(), CoreError> {
        let outbound_order = self.lock_outbound_order()?;
        let request_id = {
            let mut state = self.lock_state()?;
            if state
                .plugin_server
                .as_ref()
                .is_some_and(|server| server.is_unsent(operation_id))
            {
                cancel_unsent_plugin_server_request(self, &mut state, operation_id);
                return Ok(());
            }
            let request_id = state
                .request_by_operation
                .remove(operation_id)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Unknown pending language-server operation.",
                    )
                })?;
            let pending = state.pending.remove(&request_id).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unknown pending language-server request.",
                )
            })?;
            state.client.pending_requests.remove(&request_id);
            state.client.pending_semantic_legends.remove(&request_id);
            state.function_by_operation.remove(operation_id);
            let error = runtime_error(
                self,
                "requestCancelled",
                "request",
                "Language-server request was cancelled.",
                FailureContext::request(&pending.method, pending.document_uri.as_deref()),
            );
            push_request_event(
                self,
                &mut state,
                operation_id,
                &pending.method,
                None,
                Some(error),
            );
            request_id
        };
        let cancellation = json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": request_id }
        })
        .to_string();
        self.send_messages_or_fail(&outbound_order, vec![cancellation], "requestCancel")
    }
}

pub(crate) fn semantic_method(operation: LspSemanticOperation) -> &'static str {
    match operation {
        LspSemanticOperation::Completion => "textDocument/completion",
        LspSemanticOperation::Hover => "textDocument/hover",
        LspSemanticOperation::Definition => "textDocument/definition",
        LspSemanticOperation::Declaration => "textDocument/declaration",
        LspSemanticOperation::TypeDefinition => "textDocument/typeDefinition",
        LspSemanticOperation::References => "textDocument/references",
        LspSemanticOperation::Implementation => "textDocument/implementation",
        LspSemanticOperation::Rename => "textDocument/rename",
        LspSemanticOperation::Formatting => "textDocument/formatting",
        LspSemanticOperation::RangeFormatting => "textDocument/rangeFormatting",
        LspSemanticOperation::DocumentSymbols => "textDocument/documentSymbol",
        LspSemanticOperation::CodeActions => "textDocument/codeAction",
        LspSemanticOperation::ResolveCompletion => "completionItem/resolve",
        LspSemanticOperation::ResolveCodeAction => "codeAction/resolve",
        LspSemanticOperation::ExecuteCommand => "workspace/executeCommand",
        LspSemanticOperation::InlayHints => "textDocument/inlayHint",
        LspSemanticOperation::FoldingRanges => "textDocument/foldingRange",
        LspSemanticOperation::DocumentLinks => "textDocument/documentLink",
        LspSemanticOperation::WorkspaceSymbols => "workspace/symbol",
        LspSemanticOperation::SemanticTokens => "textDocument/semanticTokens/full",
        LspSemanticOperation::CodeLens => "textDocument/codeLens",
    }
}

fn semantic_capability(operation: LspSemanticOperation) -> Option<&'static str> {
    match operation {
        LspSemanticOperation::Completion => Some("completion"),
        LspSemanticOperation::Hover => Some("hover"),
        LspSemanticOperation::Definition => Some("definition"),
        LspSemanticOperation::Declaration => Some("declaration"),
        LspSemanticOperation::TypeDefinition => Some("typeDefinition"),
        LspSemanticOperation::References => Some("references"),
        LspSemanticOperation::Implementation => Some("implementation"),
        LspSemanticOperation::Rename => Some("rename"),
        LspSemanticOperation::Formatting => Some("formatting"),
        LspSemanticOperation::RangeFormatting => Some("rangeFormatting"),
        LspSemanticOperation::DocumentSymbols => Some("documentSymbols"),
        LspSemanticOperation::CodeActions => Some("codeActions"),
        LspSemanticOperation::ResolveCompletion => Some("completionResolve"),
        LspSemanticOperation::ResolveCodeAction => Some("codeActionResolve"),
        LspSemanticOperation::ExecuteCommand => Some("executeCommand"),
        LspSemanticOperation::InlayHints => Some("inlayHints"),
        LspSemanticOperation::FoldingRanges => Some("foldingRanges"),
        LspSemanticOperation::DocumentLinks => Some("documentLinks"),
        LspSemanticOperation::WorkspaceSymbols => Some("workspaceSymbols"),
        LspSemanticOperation::SemanticTokens => Some("semanticTokens"),
        LspSemanticOperation::CodeLens => Some("codeLens"),
    }
}

pub(crate) fn allocate_raw_request(
    mut state: LspClientState,
    method: &str,
    params: Value,
) -> Result<lithe_lsp_client::LspClientResponse, CoreError> {
    let id = state.next_request_id.to_string();
    state.next_request_id += 1;
    state
        .pending_requests
        .insert(id.clone(), method.to_string());
    let message = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    }))
    .map_err(|error| {
        CoreError::new(
            ErrorCode::Unknown,
            "Could not encode a language-server request.",
        )
        .with_details(error.to_string())
    })?;
    Ok(lithe_lsp_client::LspClientResponse {
        state,
        messages: vec![message],
        events: Vec::new(),
    })
}
