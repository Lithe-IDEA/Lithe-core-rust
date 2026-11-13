use super::*;
use crate::projection_v2::{Projection, SessionProjection, Source};

fn request_message(sequence: i64, command: &str, arguments: Value) -> Value {
    json!({
        "seq": sequence,
        "type": "request",
        "command": command,
        "arguments": arguments
    })
}

fn response_message(request_sequence: i64, command: &str, body: Value) -> Value {
    json!({
        "seq": 100 + request_sequence,
        "type": "response",
        "request_seq": request_sequence,
        "success": true,
        "command": command,
        "body": body
    })
}

fn receive_messages(session_id: &str, messages: Vec<Value>) -> DebugSessionUpdate {
    let bytes = messages
        .into_iter()
        .flat_map(|message| frame_message(&message).unwrap())
        .collect::<Vec<_>>();
    receive(ReceiveRequest {
        session_id: session_id.to_string(),
        data_base64: BASE64.encode(bytes),
    })
    .unwrap()
}

fn decode_frame(frame: &str) -> Value {
    let bytes = BASE64.decode(frame).unwrap();
    let body_start = bytes
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .unwrap()
        + 4;
    serde_json::from_slice(&bytes[body_start..]).unwrap()
}

#[test]
fn delayed_launch_response_preserves_a_newer_stopped_or_terminal_event() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/debug/launch-event-order.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let session_id = case["id"].as_str().unwrap();
        let event = case["event"].clone();
        let expected = case["state"].clone();
        create_session(child_request(session_id, None)).unwrap();
        struct Cleanup<'a>(&'a str);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                // log-exempt: Expected absence if the test already disposed this process-free session.
                let _ = destroy_session(SessionRequest {
                    session_id: self.0.into(),
                });
            }
        }
        let _cleanup = Cleanup(session_id);
        receive_messages(
            session_id,
            vec![response_message(1, "initialize", json!({}))],
        );
        let launching = launch(LaunchRequest {
            session_id: session_id.into(),
            operation_id: "launch".into(),
            configuration: DebugLaunchConfiguration {
                name: "native target".into(),
                request: DebugRequestKind::Launch,
                arguments: Map::new(),
                stepping_filters: None,
            },
        })
        .unwrap();
        let frame = decode_frame(&launching.outbound_frames[0]);
        let stopped = receive_messages(session_id, vec![event]);
        assert_eq!(serde_json::to_value(stopped.state).unwrap(), expected);
        // LLDB can report the first stop before acknowledging the launch request.
        let acknowledged = receive_messages(
            session_id,
            vec![response_message(
                frame["seq"].as_i64().unwrap(),
                "launch",
                json!({}),
            )],
        );
        assert_eq!(
            serde_json::to_value(acknowledged.state).unwrap(),
            expected,
            "a response must not resume or resurrect the target"
        );
    }
}

#[test]
fn dap_logging_keeps_catalog_identity_and_cancellation_private() {
    let adapter = "lldb+private-adapter";
    let subject = identity_segment(adapter);
    assert!(subject.starts_with("invalid-"));
    assert_eq!(subject, identity_segment(adapter));
    assert!(!subject.contains("private"));
    let mut fact = lithe_diagnostics::record::StructuredLogEvent {
        ts: "2026-11-04T00:00:00.000Z".into(),
        level: lithe_diagnostics::record::LogLevel::Debug,
        seq: 1,
        event: "debug.operation.cancelled".into(),
        op: Some("owned-operation".into()),
        msg: String::new(),
        fields: BTreeMap::from([
            ("adapter".into(), json!(subject)),
            ("failure_code".into(), json!("cancelled")),
        ]),
    };
    let channel = lithe_diagnostics::record::prepare_catalog_event(&mut fact, &json!({})).unwrap();
    assert_eq!(fact.level, lithe_diagnostics::record::LogLevel::Debug);
    assert_eq!(fact.op.as_deref(), Some("owned-operation"));
    assert_eq!(fact.fields.len(), 2);
    assert!(channel.starts_with("debug/invalid-"));
    fact.fields
        .insert("message".into(), json!("forbidden protocol payload"));
    assert!(lithe_diagnostics::record::prepare_catalog_event(&mut fact, &json!({})).is_err());
}

#[test]
fn dap_logging_failed_launch_owns_one_terminal_and_cancel_is_not_error() {
    let session_id = "debug-log-owner";
    create_session(child_request(session_id, None)).unwrap();
    let result = with_session(session_id, |session| {
        session.state = DebugSessionState::Launching;
        session.emit(DebugEventBody::OperationFailed {
            operation_id: "cancelled-launch".into(),
            command: "launch".into(),
            code: DebugOperationFailureCode::Cancelled,
            message: "private adapter payload must never be logged".into(),
        });
        assert!(session.logged_terminal_failure);
        session.transition(DebugSessionState::Failed);
        Ok(())
    });
    destroy_session(SessionRequest {
        session_id: session_id.into(),
    })
    .unwrap();
    result.unwrap();
}

#[test]
fn stack_frame_reports_adapter_served_source_references() {
    let filters = DebugSteppingFilters::unfiltered();
    let body = json!({"stackFrames": [
        {"id": 1, "name": "main", "line": 9, "column": 1,
         "source": {"name": "main.c", "path": "/workspace/main.c"}},
        {"id": 2, "name": "start", "line": 1664, "column": 1,
         "source": {"name": "start", "path": "/usr/lib/dyld`start", "sourceReference": 1}},
        {"id": 3, "name": "stale", "line": 1, "column": 1,
         "source": {"name": "stale.c", "path": "/workspace/stale.c", "sourceReference": 0}},
    ]});
    let frames =
        normalize_inspection(DebugInspectKind::StackTrace, &body, &filters, "/workspace").unwrap();
    let value = serde_json::to_value(frames).unwrap();
    let frames = &value["stackFrames"];
    // A path with a positive reference is adapter-served content, not a file on disk.
    assert!(frames[0].get("sourceReference").is_none());
    assert_eq!(frames[1]["sourceReference"], 1);
    assert_eq!(frames[1]["sourcePath"], "/usr/lib/dyld`start");
    assert!(frames[2].get("sourceReference").is_none());
}

#[test]
fn hot_replace_normalizes_success_empty_and_error_bodies() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/hot-code-replace-v1.json"
    )))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let result = normalize_inspection(
            DebugInspectKind::RedefineClasses,
            &case["body"],
            &DebugSteppingFilters::unfiltered(),
            "/workspace",
        );
        if case["failure"].as_bool() == Some(true) {
            assert!(result.is_err(), "{}", case["name"]);
        } else {
            let value = serde_json::to_value(result.unwrap()).unwrap();
            assert_eq!(value, case["result"], "{}", case["name"]);
        }
    }
}

#[test]
fn hot_replace_requires_the_declaration_and_preserves_paused_state() {
    let hot_swapping: DeclaredAdapterBehavior = serde_json::from_value(json!({
        "features": {
            "hotCodeReplace": {
                "request": "redefineClasses",
                "changedField": "changedClasses",
                "errorField": "errorMessage"
            }
        }
    }))
    .unwrap();
    for (id, adapter, declared) in [
        ("hcr-declared", "hotswap", hot_swapping),
        ("hcr-other", "python", DeclaredAdapterBehavior::default()),
    ] {
        let declares_hot_replace = declared
            .features
            .as_ref()
            .is_some_and(|features| features.hot_code_replace.is_some());
        create_session(CreateSessionRequest {
            session_id: id.into(),
            adapter_id: adapter.into(),
            root_path: "/workspace".into(),
            supports_run_in_terminal_request: false,
            supports_start_debugging_request: false,
            disabled_capabilities: Vec::new(),
            parent_session_id: None,
            declared,
        })
        .unwrap();
        // The guard always removes the process-free session, including on assertion failure.
        struct Cleanup(&'static str);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = destroy_session(SessionRequest {
                    session_id: self.0.into(),
                });
            }
        }
        let _cleanup = Cleanup(id);
        receive_messages(id, vec![response_message(1, "initialize", json!({}))]);
        let launched = launch(LaunchRequest {
            session_id: id.into(),
            operation_id: "launch".into(),
            configuration: DebugLaunchConfiguration {
                name: "service".into(),
                request: DebugRequestKind::Launch,
                arguments: serde_json::from_value(json!({"vmArgs":"-Xmx256m"})).unwrap(),
                stepping_filters: None,
            },
        })
        .unwrap();
        let launch_frame = decode_frame(&launched.outbound_frames[0]);
        // Launch arguments pass through untouched for every adapter.
        assert_eq!(launch_frame["arguments"]["vmArgs"], "-Xmx256m");
        receive_messages(
            id,
            vec![response_message(
                launch_frame["seq"].as_i64().unwrap(),
                "launch",
                json!({}),
            )],
        );
        receive_messages(
            id,
            vec![
                json!({"type":"event", "event":"stopped", "body":{"reason":"breakpoint","threadId":1}}),
            ],
        );
        let request: InspectRequest = serde_json::from_value(json!({
            "sessionId":id,"operationId":"apply","kind":"redefineClasses"
        }))
        .unwrap();
        let update = inspect(request);
        if !declares_hot_replace {
            assert!(update.is_err());
            continue;
        }
        let update = update.unwrap();
        let frame = decode_frame(&update.outbound_frames[0]);
        assert_eq!(frame["command"], "redefineClasses");
        assert_eq!(frame["arguments"], json!({}));
        let result = receive_messages(
            id,
            vec![response_message(
                frame["seq"].as_i64().unwrap(),
                "redefineClasses",
                json!({"changedClasses":["example.Service"]}),
            )],
        );
        assert_eq!(result.state, DebugSessionState::Paused);
        assert!(result.events.iter().any(|e| matches!(&e.body,
            DebugEventBody::OperationCompleted { operation_id, result: DebugOperationResult::RedefineClasses { changed_classes } }
            if operation_id == "apply" && changed_classes == &["example.Service".to_string()])));
        // A success envelope with a runtime error must not tear down the debug transport.
        let rejected = inspect(
            serde_json::from_value(json!({
                "sessionId":id,"operationId":"rejected","kind":"redefineClasses"
            }))
            .unwrap(),
        )
        .unwrap();
        let frame = decode_frame(&rejected.outbound_frames[0]);
        let result = receive_messages(
            id,
            vec![response_message(
                frame["seq"].as_i64().unwrap(),
                "redefineClasses",
                json!({"changedClasses":[], "errorMessage":"Schema change not supported"}),
            )],
        );
        assert_eq!(result.state, DebugSessionState::Paused);
        assert!(result.events.iter().any(|e| matches!(&e.body,
            DebugEventBody::OperationFailed { operation_id, message, .. }
            if operation_id == "rejected" && message == "Schema change not supported")));
    }
}

#[test]
fn debug_update_serializes_variant_fields_with_contract_casing() {
    let update = DebugSessionUpdate {
        session_id: "debug-contract-casing".to_string(),
        state: DebugSessionState::Paused,
        outbound_frames: Vec::new(),
        events: vec![
            DebugEvent {
                sequence: 1,
                body: DebugEventBody::Stopped {
                    reason: "breakpoint".to_string(),
                    thread_id: Some(13),
                    description: None,
                    stop_generation: 1,
                },
            },
            DebugEvent {
                sequence: 2,
                body: DebugEventBody::OperationCompleted {
                    operation_id: "stack-1".to_string(),
                    result: DebugOperationResult::StackTrace {
                        stack_frames: vec![DebugStackFrame {
                            id: 7,
                            name: "example.Main.run".to_string(),
                            source_path: Some("/workspace/Main.c".to_string()),
                            source_reference: None,
                            line: 12,
                            column: 1,
                            is_filtered: false,
                            instruction_pointer_reference: None,
                        }],
                    },
                },
            },
        ],
    };

    let value = serde_json::to_value(update).unwrap();
    assert_eq!(value["events"][0]["threadId"], 13);
    assert!(value["events"][0].get("thread_id").is_none());
    assert_eq!(value["events"][1]["operationId"], "stack-1");
    assert!(value["events"][1].get("operation_id").is_none());
    assert_eq!(value["events"][1]["result"]["kind"], "stackTrace");
    assert_eq!(value["events"][1]["result"]["stackFrames"][0]["id"], 7);
    assert_eq!(
        value["events"][1]["result"]["stackFrames"][0]["isFiltered"],
        false
    );
    assert!(value["events"][1]["result"].get("stack_frames").is_none());
}

#[test]
fn disconnect_policy_matches_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/disconnect-policy-v1.json"
    )))
    .unwrap();

    for case in fixture["cases"].as_array().unwrap() {
        let request_name = case["request"].as_str().unwrap_or("unstarted");
        let session_id = format!("debug-disconnect-{request_name}");
        create_session(CreateSessionRequest {
            session_id: session_id.clone(),
            adapter_id: "lldb".to_string(),
            root_path: "/workspace".to_string(),
            supports_run_in_terminal_request: false,
            supports_start_debugging_request: false,
            disabled_capabilities: Vec::new(),
            parent_session_id: None,
            declared: Default::default(),
        })
        .unwrap();

        if case["request"].is_string() {
            let request_kind = serde_json::from_value(case["request"].clone()).unwrap();
            launch(LaunchRequest {
                session_id: session_id.clone(),
                operation_id: format!("{request_name}-main"),
                configuration: DebugLaunchConfiguration {
                    name: "Main".to_string(),
                    request: request_kind,
                    arguments: Map::new(),
                    stepping_filters: None,
                },
            })
            .unwrap();
            let initialized = receive_messages(
                &session_id,
                vec![response_message(1, "initialize", json!({}))],
            );
            assert_eq!(
                decode_frame(&initialized.outbound_frames[0])["command"],
                request_name
            );
        }

        let disconnected = disconnect(DisconnectRequest {
            session_id: session_id.clone(),
            terminate_debuggee: case["requestedTerminateDebuggee"].as_bool(),
        })
        .unwrap();
        let request = decode_frame(&disconnected.outbound_frames[0]);
        assert_eq!(request["command"], "disconnect");
        assert_eq!(request["arguments"], case["expectedArguments"]);
        destroy_session(SessionRequest { session_id }).unwrap();
    }
}

#[test]
fn variable_paging_arguments_reject_invalid_combinations() {
    let base = InspectRequest {
        session_id: "debug-variable-validation".to_string(),
        operation_id: "variables".to_string(),
        kind: DebugInspectKind::Variables,
        thread_id: None,
        frame_id: None,
        variables_reference: Some(700),
        variable_filter: Some(DebugVariableFilter::Indexed),
        start: Some(0),
        count: Some(100),
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    };

    let mut negative_start = base.clone();
    negative_start.start = Some(-1);
    let error = inspect_arguments(&negative_start).unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));

    let mut zero_count = base.clone();
    zero_count.count = Some(0);
    let error = inspect_arguments(&zero_count).unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));

    let mut non_variable_request = base;
    non_variable_request.kind = DebugInspectKind::Threads;
    non_variable_request.variables_reference = None;
    non_variable_request.start = None;
    non_variable_request.count = None;
    let error = inspect_arguments(&non_variable_request).unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
}

#[test]
fn variable_child_counts_are_normalized_for_scopes_and_evaluation() {
    let scopes = normalize_inspection(
        DebugInspectKind::Scopes,
        &json!({
            "scopes": [{
                "name": "Locals",
                "variablesReference": 700,
                "expensive": false,
                "namedVariables": -2,
                "indexedVariables": 250
            }]
        }),
        &DebugSteppingFilters::default(),
        "/workspace",
    )
    .unwrap();
    let scopes = serde_json::to_value(scopes).unwrap();
    assert_eq!(scopes["scopes"][0]["namedVariables"], 0);
    assert_eq!(scopes["scopes"][0]["indexedVariables"], 250);

    let evaluation = normalize_inspection(
        DebugInspectKind::Evaluate,
        &json!({
            "result": "Customer[250]",
            "variablesReference": 701,
            "namedVariables": 4,
            "indexedVariables": -1
        }),
        &DebugSteppingFilters::default(),
        "/workspace",
    )
    .unwrap();
    let evaluation = serde_json::to_value(evaluation).unwrap();
    assert_eq!(evaluation["variable"]["namedVariables"], 4);
    assert_eq!(evaluation["variable"]["indexedVariables"], 0);
}

#[test]
fn stepping_filters_are_normalized_projected_and_mark_stack_frames() {
    // Without a declaration there are no built-in defaults for any adapter.
    let defaults = stepping_filters(DebugSteppingFiltersRequest {
        adapter_id: "lldb".to_string(),
        filters: None,
    })
    .unwrap();
    assert_eq!(defaults, DebugSteppingFilters::unfiltered());

    let filters = stepping_filters(DebugSteppingFiltersRequest {
        adapter_id: "lldb".to_string(),
        filters: Some(DebugSteppingFilters {
            class_name_filters: vec![
                " org.mocking.* ".to_string(),
                "lib.core.*".to_string(),
                "org.mocking.*".to_string(),
                String::new(),
            ],
            skip_synthetics: true,
            skip_static_initializers: false,
            skip_constructors: true,
            hide_filtered_stack_frames: true,
        }),
    })
    .unwrap();
    assert_eq!(filters.class_name_filters, ["lib.core.*", "org.mocking.*"]);

    let multiline_error = stepping_filters(DebugSteppingFiltersRequest {
        adapter_id: "lldb".to_string(),
        filters: Some(DebugSteppingFilters {
            class_name_filters: vec!["example.Valid\nexample.Invalid".to_string()],
            skip_synthetics: false,
            skip_static_initializers: false,
            skip_constructors: false,
            hide_filtered_stack_frames: false,
        }),
    })
    .unwrap_err();
    assert!(matches!(multiline_error.code, ErrorCode::InvalidRequest));

    let excessive_filter_error = stepping_filters(DebugSteppingFiltersRequest {
        adapter_id: "lldb".to_string(),
        filters: Some(DebugSteppingFilters {
            class_name_filters: (0..257)
                .map(|index| format!("example.Type{index}"))
                .collect(),
            skip_synthetics: false,
            skip_static_initializers: false,
            skip_constructors: false,
            hide_filtered_stack_frames: false,
        }),
    })
    .unwrap_err();
    assert!(matches!(
        excessive_filter_error.code,
        ErrorCode::InvalidRequest
    ));

    let session_id = "debug-stepping-filters";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: serde_json::from_value(json!({
            "features": {"stepFilters": {"argument": "stepFilters"}}
        }))
        .unwrap(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: Some(filters),
        },
    })
    .unwrap();
    let initialized = receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    let launch_request = decode_frame(&initialized.outbound_frames[0]);
    assert_eq!(launch_request["command"], "launch");
    assert_eq!(
        launch_request["arguments"]["stepFilters"]["skipClasses"],
        json!(["lib.core.*", "org.mocking.*"])
    );
    assert_eq!(
        launch_request["arguments"]["stepFilters"]["skipConstructors"],
        true
    );
    receive_messages(
        session_id,
        vec![
            response_message(2, "launch", json!({})),
            json!({
                "seq": 103,
                "type": "event",
                "event": "stopped",
                "body": {"reason": "breakpoint", "threadId": 11}
            }),
        ],
    );
    inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "stack".to_string(),
        kind: DebugInspectKind::StackTrace,
        thread_id: Some(11),
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let stack = receive_messages(
        session_id,
        vec![response_message(
            3,
            "stackTrace",
            json!({"stackFrames": [
                {
                    "id": 7,
                    "name": "lib.core.reflect.Method.invoke(Object,Object[])",
                    "source": {
                        "path": "/deps/lib/core/reflect/Method.class"
                    },
                    "line": 1,
                    "column": 1
                },
                {
                    "id": 8,
                    "name": "MockMethodInterceptor.intercept(Object,Method,Object[],Invoker)",
                    "source": {
                        "path": "/deps/mocking-core/org/mocking/internal/MockMethodInterceptor.class"
                    },
                    "line": 1,
                    "column": 1
                },
                {
                    "id": 9,
                    "name": "example.LoginService.authenticate",
                    "source": {"path": "/workspace/LoginService.c"},
                    "line": 24,
                    "column": 5
                }
            ]}),
        )],
    );
    assert!(stack.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::StackTrace { stack_frames }
        } if operation_id == "stack"
            && stack_frames.len() == 3
            && stack_frames[0].is_filtered
            && stack_frames[1].is_filtered
            && !stack_frames[2].is_filtered
    )));
    let library_filters = DebugSteppingFilters {
        class_name_filters: vec!["$Libraries".to_string()],
        skip_synthetics: false,
        skip_static_initializers: false,
        skip_constructors: false,
        hide_filtered_stack_frames: true,
    };
    assert!(stack_frame_matches_filters(
        "Dependency.call()",
        Some("/dependencies/example/Dependency.c"),
        None,
        &library_filters,
        "/workspace"
    ));
    assert!(!stack_frame_matches_filters(
        "LoginService.authenticate()",
        Some("/workspace/LoginService.c"),
        None,
        &library_filters,
        "/workspace"
    ));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn initialize_launch_breakpoints_and_inspection_are_reduced_in_order() {
    let session_id = "debug-engine-flow";
    let created = create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    assert_eq!(created.state, DebugSessionState::Initializing);
    assert_eq!(
        decode_frame(&created.outbound_frames[0])["command"],
        "initialize"
    );

    let queued = launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch-1".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::from_iter([("mainClass".to_string(), json!("example.Main"))]),
            stepping_filters: None,
        },
    })
    .unwrap();
    assert!(queued.outbound_frames.is_empty());
    set_breakpoints(SetBreakpointsRequest {
        session_id: session_id.to_string(),
        source_path: "/workspace/src/Main.c".to_string(),
        breakpoints: vec![
            SourceBreakpoint {
                line: 12,
                column: None,
                enabled: true,
                condition: Some("value > 1".to_string()),
                hit_condition: Some("3".to_string()),
                log_message: Some("value = {value}".to_string()),
            },
            SourceBreakpoint {
                line: 14,
                column: None,
                enabled: false,
                condition: None,
                hit_condition: None,
                log_message: None,
            },
        ],
    })
    .unwrap();
    set_exception_breakpoints(SetExceptionBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: vec![
            ExceptionBreakpoint {
                filter: "uncaught".to_string(),
                enabled: false,
                condition: None,
            },
            ExceptionBreakpoint {
                filter: "caught".to_string(),
                enabled: true,
                condition: Some(" example.CustomException ".to_string()),
            },
        ],
    })
    .unwrap();
    set_function_breakpoints(SetFunctionBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: vec![
            FunctionBreakpoint {
                name: " example.Main.run ".to_string(),
                enabled: true,
                condition: Some("ready".to_string()),
                hit_condition: Some("2".to_string()),
            },
            FunctionBreakpoint {
                name: "example.Main.skip".to_string(),
                enabled: false,
                condition: None,
                hit_condition: None,
            },
        ],
    })
    .unwrap();

    let initialized = receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({
                "supportsConfigurationDoneRequest": true,
                "supportsConditionalBreakpoints": true,
                "supportsHitConditionalBreakpoints": true,
                "supportsLogPoints": true,
                "supportsFunctionBreakpoints": true,
                "supportsDataBreakpoints": true,
                "supportsSetVariable": true,
                "supportsRestartRequest": true,
                "supportsExceptionFilterOptions": true,
                "exceptionBreakpointFilters": [{
                    "filter": "caught",
                    "label": "Caught Exceptions",
                    "default": false,
                    "supportsCondition": true
                }]
            }),
        )],
    );
    assert_eq!(initialized.state, DebugSessionState::Launching);
    assert!(initialized.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::Capabilities { capabilities }
            if capabilities.supports_conditional_breakpoints
                && capabilities.supports_hit_conditional_breakpoints
                && capabilities.supports_log_points
                && capabilities.supports_function_breakpoints
                && capabilities.supports_data_breakpoints
                && capabilities.supports_set_variable
                && capabilities.supports_restart_request
                && capabilities.exception_breakpoint_filters.len() == 1
    )));
    assert_eq!(
        decode_frame(&initialized.outbound_frames[0])["command"],
        "launch"
    );

    let configured = receive_messages(
        session_id,
        vec![json!({"seq": 102, "type": "event", "event": "initialized"})],
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[0])["command"],
        "setExceptionBreakpoints"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["command"],
        "setFunctionBreakpoints"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[2])["command"],
        "setDataBreakpoints"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[3])["command"],
        "setBreakpoints"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[4])["command"],
        "configurationDone"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[0])["arguments"]["filters"],
        json!(["caught"])
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[0])["arguments"]["filterOptions"][0]["condition"],
        "example.CustomException"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["arguments"]["breakpoints"][0]["name"],
        "example.Main.run"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["arguments"]["breakpoints"][0]["condition"],
        "ready"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["arguments"]["breakpoints"][0]["hitCondition"],
        "2"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["arguments"]["breakpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[3])["arguments"]["breakpoints"][0]["condition"],
        "value > 1"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[3])["arguments"]["breakpoints"][0]["hitCondition"],
        "3"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[3])["arguments"]["breakpoints"][0]["logMessage"],
        "value = {value}"
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[3])["arguments"]["breakpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let running = receive_messages(
        session_id,
        vec![
            response_message(2, "launch", json!({})),
            response_message(3, "setExceptionBreakpoints", json!({})),
            response_message(
                4,
                "setFunctionBreakpoints",
                json!({"breakpoints": [{"id": 8, "verified": true}]}),
            ),
            response_message(5, "setDataBreakpoints", json!({"breakpoints": []})),
            response_message(
                6,
                "setBreakpoints",
                json!({"breakpoints": [{"id": 7, "verified": true, "line": 12}]}),
            ),
            response_message(7, "configurationDone", json!({})),
        ],
    );
    assert_eq!(running.state, DebugSessionState::Running);
    assert!(running
        .events
        .iter()
        .any(|event| matches!(event.body, DebugEventBody::Breakpoint { .. })));
    assert!(running.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::Breakpoint { breakpoint }
            if breakpoint.function_name.as_deref() == Some("example.Main.run")
                && breakpoint.verified
    )));

    let inspection = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "threads-1".to_string(),
        kind: DebugInspectKind::Threads,
        thread_id: None,
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    assert_eq!(
        decode_frame(&inspection.outbound_frames[0])["command"],
        "threads"
    );
    let completed = receive_messages(
        session_id,
        vec![response_message(
            8,
            "threads",
            json!({"threads": [{"id": 1, "name": "main"}]}),
        )],
    );
    assert!(completed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted { operation_id, result: DebugOperationResult::Threads { threads } }
            if operation_id == "threads-1" && threads.len() == 1
    )));

    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn source_breakpoint_binding_preserves_requested_identity_and_rejects_stale_replies() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/breakpoint-binding-v1.json"
    )))
    .unwrap();
    let session_id = "source-breakpoint-binding";
    let source = fixture["sourcePath"].as_str().unwrap();
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    // This reducer owns no processes; always destroy its registry entry below.
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    receive_messages(
        session_id,
        vec![json!({"type":"event", "event":"initialized"})],
    );
    let set = || {
        set_breakpoints(SetBreakpointsRequest {
            session_id: session_id.to_string(),
            source_path: source.to_string(),
            breakpoints: vec![SourceBreakpoint {
                line: fixture["requestedLine"].as_i64().unwrap(),
                column: None,
                enabled: true,
                condition: None,
                hit_condition: None,
                log_message: None,
            }],
        })
        .unwrap()
    };
    let sequence = |update: DebugSessionUpdate| {
        decode_frame(&update.outbound_frames[0])["seq"]
            .as_i64()
            .unwrap()
    };
    let old_sequence = sequence(set());
    let latest_sequence = sequence(set());
    let stale = receive_messages(
        session_id,
        vec![response_message(
            old_sequence,
            "setBreakpoints",
            json!({"breakpoints":[{"id":40,"verified":true,"line":8}]}),
        )],
    );
    assert!(!stale
        .events
        .iter()
        .any(|e| matches!(e.body, DebugEventBody::Breakpoint { .. })));
    receive_messages(
        session_id,
        vec![response_message(
            latest_sequence,
            "setBreakpoints",
            json!({"breakpoints":[fixture["response"].clone()]}),
        )],
    );
    let changed = receive_messages(
        session_id,
        vec![json!({"type":"event", "event":"breakpoint",
        "body":{"reason":"changed", "breakpoint":fixture["changed"]}})],
    );
    let result = changed
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::Breakpoint { breakpoint } => {
                Some(serde_json::to_value(breakpoint).unwrap())
            }
            _ => None,
        })
        .unwrap();
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
    for key in ["requestedLine", "line", "sourcePath", "verified"] {
        assert_eq!(result[key], fixture["expected"][key], "{key}");
    }
}

#[test]
fn start_debugging_request_is_normalized_completed_and_stale_safe() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/start-debugging-v1.json"
    )))
    .unwrap();
    let session_id = "debug-start-debugging";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "node".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: true,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    // This reducer owns no processes; always destroy its registry entry below.
    let requested = receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
    let request_id = requested
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::StartDebuggingRequested {
                request_id,
                request,
            } => {
                assert_eq!(
                    serde_json::to_value(request).unwrap(),
                    fixture["expectedRequest"]
                );
                Some(request_id.clone())
            }
            _ => None,
        })
        .unwrap();

    let duplicate = receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
    assert!(duplicate.outbound_frames.is_empty() && duplicate.events.is_empty());

    let success = start_debugging_response(DebugStartDebuggingResponseRequest {
        session_id: session_id.to_string(),
        request_id: request_id.clone(),
        success: true,
        message: None,
    })
    .unwrap();
    let response = decode_frame(&success.outbound_frames[0]);
    assert_eq!(response["request_seq"], fixture["adapterRequest"]["seq"]);
    assert_eq!(response["command"], "startDebugging");
    assert_eq!(response["success"], true);

    let stale = start_debugging_response(DebugStartDebuggingResponseRequest {
        session_id: session_id.to_string(),
        request_id,
        success: false,
        message: Some(fixture["failureMessage"].as_str().unwrap().to_string()),
    })
    .unwrap();
    assert!(stale.outbound_frames.is_empty());
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn start_debugging_failure_and_unsupported_hosts_answer_the_adapter() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/start-debugging-v1.json"
    )))
    .unwrap();
    for (session_id, supported) in [
        ("debug-start-debugging-unsupported", false),
        ("debug-start-debugging-failure", true),
    ] {
        create_session(CreateSessionRequest {
            session_id: session_id.to_string(),
            adapter_id: "node".to_string(),
            root_path: "/workspace".to_string(),
            supports_run_in_terminal_request: false,
            supports_start_debugging_request: supported,
            disabled_capabilities: Vec::new(),
            parent_session_id: None,
            declared: Default::default(),
        })
        .unwrap();
        let update = receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
        if supported {
            let request_id = update
                .events
                .iter()
                .find_map(|event| match &event.body {
                    DebugEventBody::StartDebuggingRequested { request_id, .. } => {
                        Some(request_id.clone())
                    }
                    _ => None,
                })
                .unwrap();
            let failed = start_debugging_response(DebugStartDebuggingResponseRequest {
                session_id: session_id.to_string(),
                request_id,
                success: false,
                message: Some(fixture["failureMessage"].as_str().unwrap().to_string()),
            })
            .unwrap();
            let response = decode_frame(&failed.outbound_frames[0]);
            assert_eq!(response["success"], false);
            assert_eq!(response["message"], fixture["failureMessage"]);
        } else {
            let response = decode_frame(&update.outbound_frames[0]);
            assert_eq!(response["success"], false);
            assert!(update.events.is_empty());
        }
        destroy_session(SessionRequest {
            session_id: session_id.to_string(),
        })
        .unwrap();
    }
}

#[test]
fn disconnect_fails_a_pending_start_debugging_request() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/start-debugging-v1.json"
    )))
    .unwrap();
    let session_id = "debug-start-debugging-disconnect";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "node".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: true,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
    let update = disconnect(DisconnectRequest {
        session_id: session_id.to_string(),
        terminate_debuggee: None,
    })
    .unwrap();
    let response = decode_frame(&update.outbound_frames[0]);
    assert_eq!(response["command"], "startDebugging");
    assert_eq!(response["success"], false);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn node_source_mapped_locations_pass_through_unchanged() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/node-source-map-v1.json"
    )))
    .unwrap();
    let session_id = "debug-node-source-map";
    let source = fixture["sourcePath"].as_str().unwrap();
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "node".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    // This reducer owns no processes; always destroy its registry entry below.
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    receive_messages(
        session_id,
        vec![json!({"type":"event", "event":"initialized"})],
    );
    let sent = set_breakpoints(SetBreakpointsRequest {
        session_id: session_id.to_string(),
        source_path: source.to_string(),
        breakpoints: vec![SourceBreakpoint {
            line: fixture["requestedLine"].as_i64().unwrap(),
            column: None,
            enabled: true,
            condition: None,
            hit_condition: None,
            log_message: None,
        }],
    })
    .unwrap();
    let request = decode_frame(&sent.outbound_frames[0]);
    // Core never rewrites TypeScript to the generated file; the adapter owns mapping.
    assert_eq!(request["arguments"]["source"]["path"], source);
    let answered = receive_messages(
        session_id,
        vec![response_message(
            request["seq"].as_i64().unwrap(),
            "setBreakpoints",
            json!({"breakpoints":[fixture["response"].clone()]}),
        )],
    );
    let breakpoint = answered
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::Breakpoint { breakpoint } => {
                Some(serde_json::to_value(breakpoint).unwrap())
            }
            _ => None,
        })
        .unwrap();
    for key in ["sourcePath", "requestedLine", "line", "verified"] {
        assert_eq!(breakpoint[key], fixture["expectedBreakpoint"][key], "{key}");
    }

    receive_messages(
        session_id,
        vec![json!({"seq": 103, "type": "event", "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 1}})],
    );
    inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "stack".to_string(),
        kind: DebugInspectKind::StackTrace,
        thread_id: Some(1),
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let stack_sequence = 4;
    let stack = receive_messages(
        session_id,
        vec![response_message(
            stack_sequence,
            "stackTrace",
            json!({"stackFrames": fixture["stackFrames"].clone()}),
        )],
    );
    let frame = stack
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::OperationCompleted {
                result: DebugOperationResult::StackTrace { stack_frames },
                ..
            } => Some(serde_json::to_value(&stack_frames[0]).unwrap()),
            _ => None,
        })
        .unwrap();
    assert_eq!(frame["sourcePath"], fixture["expectedFrame"]["sourcePath"]);
    assert_eq!(frame["line"], fixture["expectedFrame"]["line"]);
    assert_eq!(frame["column"], fixture["expectedFrame"]["column"]);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn data_breakpoint_identity_and_verification_are_correlated() {
    let session_id = "debug-data-breakpoint";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    set_data_breakpoints(SetDataBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: vec![
            DataBreakpoint {
                data_id: " field:count ".to_string(),
                label: Some("count".to_string()),
                enabled: true,
                access_type: Some(" write ".to_string()),
                condition: Some(" count > 1 ".to_string()),
                hit_condition: Some(" 2 ".to_string()),
            },
            DataBreakpoint {
                data_id: "field:ignored".to_string(),
                label: None,
                enabled: false,
                access_type: None,
                condition: None,
                hit_condition: None,
            },
        ],
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({"supportsDataBreakpoints": true}),
        )],
    );
    let configured = receive_messages(
        session_id,
        vec![json!({"seq": 102, "type": "event", "event": "initialized"})],
    );
    assert_eq!(
        decode_frame(&configured.outbound_frames[1])["command"],
        "setDataBreakpoints"
    );
    let arguments = &decode_frame(&configured.outbound_frames[1])["arguments"]["breakpoints"];
    assert_eq!(arguments.as_array().unwrap().len(), 1);
    assert_eq!(arguments[0]["dataId"], "field:count");
    assert_eq!(arguments[0]["accessType"], "write");
    assert_eq!(arguments[0]["condition"], "count > 1");
    assert_eq!(arguments[0]["hitCondition"], "2");

    let verified = receive_messages(
        session_id,
        vec![
            response_message(2, "setExceptionBreakpoints", json!({})),
            response_message(
                3,
                "setDataBreakpoints",
                json!({"breakpoints": [{"id": 9, "verified": true}]}),
            ),
        ],
    );
    assert!(verified.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::Breakpoint { breakpoint }
            if breakpoint.data_id.as_deref() == Some("field:count")
                && breakpoint.verified
    )));

    let info = data_breakpoint_info(DataBreakpointInfoRequest {
        session_id: session_id.to_string(),
        operation_id: "field-info".to_string(),
        name: " count ".to_string(),
        variables_reference: Some(42),
        frame_id: Some(7),
    })
    .unwrap();
    let request = decode_frame(&info.outbound_frames[0]);
    assert_eq!(request["command"], "dataBreakpointInfo");
    assert_eq!(request["arguments"]["name"], "count");
    assert_eq!(request["arguments"]["variablesReference"], 42);
    assert_eq!(request["arguments"]["frameId"], 7);
    let completed = receive_messages(
        session_id,
        vec![response_message(
            4,
            "dataBreakpointInfo",
            json!({
                "dataId": "field:count",
                "description": "Main.count",
                "accessTypes": ["read", "write"],
                "canPersist": true
            }),
        )],
    );
    assert!(completed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::DataBreakpointInfo {
                data_id,
                description,
                access_types,
                can_persist
            }
        } if operation_id == "field-info"
            && data_id.as_deref() == Some("field:count")
            && description == "Main.count"
            && access_types == &["read", "write"]
            && *can_persist
    )));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn variable_mutation_is_capability_gated_and_correlated() {
    let session_id = "debug-set-variable";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({"supportsSetVariable": true}),
        )],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 11}
        })],
    );

    let update = set_variable(SetVariableRequest {
        session_id: session_id.to_string(),
        operation_id: "set-count".to_string(),
        variables_reference: 42,
        name: " count ".to_string(),
        value: "7".to_string(),
    })
    .unwrap();
    let request = decode_frame(&update.outbound_frames[0]);
    assert_eq!(request["command"], "setVariable");
    assert_eq!(request["arguments"]["variablesReference"], 42);
    assert_eq!(request["arguments"]["name"], "count");
    assert_eq!(request["arguments"]["value"], "7");

    let completed = receive_messages(
        session_id,
        vec![response_message(
            3,
            "setVariable",
            json!({
                "value": "7",
                "type": "int",
                "variablesReference": 0,
                "namedVariables": -1,
                "indexedVariables": 2
            }),
        )],
    );
    assert!(completed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::SetVariable { variable }
        } if operation_id == "set-count"
            && variable.name == "count"
            && variable.value == "7"
            && variable.r#type.as_deref() == Some("int")
            && variable.named_variables == 0
            && variable.indexed_variables == 2
    )));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

// Regression: adapters that explain a failure only in `body.error.format` used to surface
// as the generic "rejected the request" line, hiding the actionable reason from the user.
#[test]
fn adapter_failure_detail_prefers_the_user_visible_error_format() {
    let response = json!({
        "success": false,
        "message": "launch failed",
        "body": {"error": {
            "id": 2,
            "format": "program '{path}' does not exist ({path})",
            "variables": {"path": "/work/a.out"}
        }}
    });
    assert_eq!(
        adapter_failure_detail(&response).as_deref(),
        Some("program '/work/a.out' does not exist (/work/a.out)")
    );
}

#[test]
fn adapter_failure_detail_falls_back_to_the_short_message_and_keeps_unknown_placeholders() {
    let short_only = json!({"success": false, "message": "  notStopped  "});
    assert_eq!(
        adapter_failure_detail(&short_only).as_deref(),
        Some("notStopped")
    );

    // A placeholder without a matching variable must stay readable, not vanish.
    let unknown = json!({
        "success": false,
        "body": {"error": {"format": "cannot read {file} {", "variables": {"other": "x"}}}
    });
    assert_eq!(
        adapter_failure_detail(&unknown).as_deref(),
        Some("cannot read {file} {")
    );

    let empty = json!({"success": false, "message": "  ", "body": {"error": {"format": ""}}});
    assert_eq!(adapter_failure_detail(&empty), None);
    assert_eq!(adapter_failure_detail(&json!({"success": false})), None);
}

#[test]
fn rejected_launch_reports_the_adapters_own_explanation() {
    let session_id = "debug-launch-rejected-detail";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    let update = receive_messages(
        session_id,
        vec![json!({
            "seq": 102,
            "type": "response",
            "request_seq": 2,
            "success": false,
            "command": "launch",
            "message": "launch failed",
            "body": {"error": {"id": 1, "format": "no such file: {path}", "variables": {"path": "/work/a.out"}}}
        })],
    );
    let failure = update.events.iter().find_map(|event| match &event.body {
        DebugEventBody::OperationFailed {
            operation_id,
            code,
            message,
            ..
        } => Some((operation_id.clone(), *code, message.clone())),
        _ => None,
    });
    assert_eq!(
        failure,
        Some((
            "launch".to_string(),
            DebugOperationFailureCode::AdapterRejected,
            "no such file: /work/a.out".to_string()
        ))
    );
    assert_eq!(session_state(session_id), DebugSessionState::Failed);
    cleanup(session_id);
}

#[test]
fn cancelled_operation_forwards_dap_cancel_and_ignores_late_response() {
    let session_id = "debug-cancel-operation";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({
                "supportsCancelRequest": true,
                "supportsSingleThreadExecutionRequests": true
            }),
        )],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 11}
        })],
    );
    let pending = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "threads-timeout".to_string(),
        kind: DebugInspectKind::Threads,
        thread_id: None,
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    assert_eq!(decode_frame(&pending.outbound_frames[0])["seq"], 3);

    let cancelled = cancel_operation(CancelOperationRequest {
        session_id: session_id.to_string(),
        operation_id: "threads-timeout".to_string(),
        reason: DebugCancellationReason::TimedOut,
    })
    .unwrap();
    assert!(cancelled.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed { operation_id, command, code, message }
            if operation_id == "threads-timeout"
                && command == "threads"
                && *code == DebugOperationFailureCode::TimedOut
                && message == "Debug operation timed out."
    )));
    let cancel = decode_frame(&cancelled.outbound_frames[0]);
    assert_eq!(cancel["command"], "cancel");
    assert_eq!(cancel["arguments"]["requestId"], 3);

    let late = receive_messages(
        session_id,
        vec![
            response_message(3, "threads", json!({"threads": []})),
            response_message(4, "cancel", json!({})),
        ],
    );
    assert!(!late
        .events
        .iter()
        .any(|event| matches!(event.body, DebugEventBody::OperationCompleted { .. })));
    let resumed = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "resume-main-thread".to_string(),
        command: DebugExecutionCommand::Continue,
        thread_id: Some(11),
        target_id: None,
        single_thread: true,
        granularity: None,
    })
    .unwrap();
    let resumed_request = decode_frame(&resumed.outbound_frames[0]);
    assert_eq!(resumed_request["command"], "continue");
    assert_eq!(resumed_request["arguments"]["threadId"], 11);
    assert_eq!(resumed_request["arguments"]["singleThread"], true);
    let resumed = receive_messages(
        session_id,
        vec![
            response_message(5, "continue", json!({})),
            json!({
                "seq": 108,
                "type": "event",
                "event": "continued",
                "body": {"threadId": 11, "allThreadsContinued": false}
            }),
        ],
    );
    assert_eq!(resumed.state, DebugSessionState::Paused);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn advanced_execution_controls_are_capability_gated_and_correlated() {
    let session_id = "debug-advanced-control";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({
                "supportsStepBack": true,
                "supportsRestartRequest": true,
                "supportsTerminateRequest": true
            }),
        )],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 11}
        })],
    );

    let step_back = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "step-back".to_string(),
        command: DebugExecutionCommand::StepBack,
        thread_id: Some(11),
        target_id: None,
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    let request = decode_frame(&step_back.outbound_frames[0]);
    assert_eq!(request["command"], "stepBack");
    assert_eq!(request["arguments"]["threadId"], 11);
    assert_eq!(request["arguments"]["singleThread"], false);
    let stepped = receive_messages(session_id, vec![response_message(3, "stepBack", json!({}))]);
    assert!(stepped.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted { operation_id, result: DebugOperationResult::Acknowledged { command } }
            if operation_id == "step-back" && command == "stepBack"
    )));

    let restart = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "restart".to_string(),
        command: DebugExecutionCommand::Restart,
        thread_id: Some(11),
        target_id: None,
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    let request = decode_frame(&restart.outbound_frames[0]);
    assert_eq!(request["command"], "restart");
    assert!(request["arguments"].get("threadId").is_none());
    receive_messages(session_id, vec![response_message(4, "restart", json!({}))]);

    let terminate = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "terminate".to_string(),
        command: DebugExecutionCommand::Terminate,
        thread_id: Some(11),
        target_id: None,
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    let request = decode_frame(&terminate.outbound_frames[0]);
    assert_eq!(request["command"], "terminate");
    assert!(request["arguments"].get("threadId").is_none());
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn smart_step_and_goto_targets_are_normalized_before_targeted_execution() {
    let session_id = "debug-targeted-control";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({
                "supportsStepInTargetsRequest": true,
                "supportsGotoTargetsRequest": true
            }),
        )],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 11}
        })],
    );

    let step_targets = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "step-targets".to_string(),
        kind: DebugInspectKind::StepInTargets,
        thread_id: None,
        frame_id: Some(7),
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let request = decode_frame(&step_targets.outbound_frames[0]);
    assert_eq!(request["command"], "stepInTargets");
    assert_eq!(request["arguments"]["frameId"], 7);
    let step_targets = receive_messages(
        session_id,
        vec![response_message(
            3,
            "stepInTargets",
            json!({"targets": [{
                "id": 21,
                "label": "service.load()",
                "line": 12,
                "column": 9,
                "endLine": 12,
                "endColumn": 23
            }]}),
        )],
    );
    assert!(step_targets.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::StepInTargets { targets }
        } if operation_id == "step-targets"
            && targets.first().map(|target| target.id) == Some(21)
    )));
    let targeted_step = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "targeted-step".to_string(),
        command: DebugExecutionCommand::StepIn,
        thread_id: Some(11),
        target_id: Some(21),
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    assert_eq!(
        decode_frame(&targeted_step.outbound_frames[0])["arguments"]["targetId"],
        21
    );
    receive_messages(session_id, vec![response_message(4, "stepIn", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 105,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "step", "threadId": 11}
        })],
    );

    let goto_targets = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "goto-targets".to_string(),
        kind: DebugInspectKind::GotoTargets,
        thread_id: None,
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: Some("/workspace/src/Main.c".to_string()),
        line: Some(20),
        column: Some(5),
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let request = decode_frame(&goto_targets.outbound_frames[0]);
    assert_eq!(request["command"], "gotoTargets");
    assert_eq!(
        request["arguments"]["source"]["path"],
        "/workspace/src/Main.c"
    );
    assert_eq!(request["arguments"]["line"], 20);
    let goto_targets = receive_messages(
        session_id,
        vec![response_message(
            5,
            "gotoTargets",
            json!({"targets": [{"id": 31, "label": "main.c:20", "line": 20}]}),
        )],
    );
    assert!(goto_targets.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::GotoTargets { targets }
        } if operation_id == "goto-targets"
            && targets.first().map(|target| target.id) == Some(31)
    )));
    let goto = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "goto".to_string(),
        command: DebugExecutionCommand::Goto,
        thread_id: Some(11),
        target_id: Some(31),
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    let request = decode_frame(&goto.outbound_frames[0]);
    assert_eq!(request["command"], "goto");
    assert_eq!(request["arguments"]["targetId"], 31);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

fn completions_request(session_id: &str, text: &str, column: Option<i64>) -> InspectRequest {
    InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "complete".to_string(),
        kind: DebugInspectKind::Completions,
        thread_id: None,
        frame_id: Some(7),
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: Some(text.to_string()),
        context: None,
        source_path: None,
        line: None,
        column,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    }
}

#[test]
fn console_completions_are_capability_gated_and_normalized() {
    let unsupported = "debug-completions-unsupported";
    paused_session(unsupported);
    assert!(inspect(completions_request(unsupported, "fra", None)).is_err());
    destroy_session(SessionRequest {
        session_id: unsupported.to_string(),
    })
    .unwrap();

    let session_id = "debug-completions";
    paused_session_with_capabilities(session_id, json!({"supportsCompletionsRequest": true}));
    // The caret must lie inside the typed text, counted in UTF-16 units.
    assert!(inspect(completions_request(session_id, "fra", Some(5))).is_err());
    assert!(inspect(completions_request(session_id, "fra", Some(0))).is_err());
    assert!(inspect(completions_request(session_id, "", None)).is_err());

    let update = inspect(completions_request(session_id, "p 😀fra", None)).unwrap();
    let request = decode_frame(&update.outbound_frames[0]);
    assert_eq!(request["command"], "completions");
    assert_eq!(request["arguments"]["text"], "p 😀fra");
    // "p ", the emoji (two UTF-16 units) and "fra" make seven units; the caret is the eighth.
    assert_eq!(request["arguments"]["column"], 8);
    assert_eq!(request["arguments"]["frameId"], 7);

    let completed = receive_messages(
        session_id,
        vec![response_message(
            3,
            "completions",
            json!({"targets": [
                {"label": "frame -- Commands for stack frames.", "text": "frame"},
                {"label": "format", "start": 3, "length": 2, "type": "function"},
                {"label": ""},
                {"text": "no label"}
            ]}),
        )],
    );
    let items = completed
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::OperationCompleted {
                operation_id,
                result: DebugOperationResult::Completions { items },
            } if operation_id == "complete" => Some(items),
            _ => None,
        })
        .expect("completions result");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "frame");
    assert_eq!(items[0].start, None);
    // Without `text` the label is inserted; the 1-based wire range becomes 0-based.
    assert_eq!(items[1].text, "format");
    assert_eq!(items[1].start, Some(2));
    assert_eq!(items[1].length, Some(2));
    assert_eq!(items[1].kind.as_deref(), Some("function"));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn exception_information_is_capability_gated_and_normalized() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/exception-info-v1.json"
    )))
    .unwrap();
    let session_id = "debug-exception-info";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    let initialized = receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({"supportsExceptionInfoRequest": true}),
        )],
    );
    assert!(initialized.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::Capabilities { capabilities }
            if capabilities.supports_exception_info_request
    )));
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "exception", "threadId": fixture["request"]["threadId"]}
        })],
    );

    let inspection = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: fixture["request"]["operationId"]
            .as_str()
            .unwrap()
            .to_string(),
        kind: DebugInspectKind::ExceptionInfo,
        thread_id: fixture["request"]["threadId"].as_i64(),
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let request = decode_frame(&inspection.outbound_frames[0]);
    assert_eq!(request["command"], "exceptionInfo");
    assert_eq!(
        request["arguments"]["threadId"],
        fixture["request"]["threadId"]
    );

    let completed = receive_messages(
        session_id,
        vec![response_message(
            3,
            "exceptionInfo",
            fixture["adapterResponse"].clone(),
        )],
    );
    let result = completed.events.iter().find_map(|event| match &event.body {
        DebugEventBody::OperationCompleted {
            operation_id,
            result,
        } if operation_id == fixture["request"]["operationId"].as_str().unwrap() => Some(result),
        _ => None,
    });
    assert_eq!(
        serde_json::to_value(result.unwrap()).unwrap(),
        fixture["expected"]
    );
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn variable_paging_is_forwarded_and_normalized_from_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/variable-paging-v1.json"
    )))
    .unwrap();
    let session_id = "debug-variable-paging";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 11}
        })],
    );

    let inspection = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: fixture["request"]["operationId"]
            .as_str()
            .unwrap()
            .to_string(),
        kind: DebugInspectKind::Variables,
        thread_id: None,
        frame_id: None,
        variables_reference: fixture["request"]["variablesReference"].as_i64(),
        variable_filter: serde_json::from_value(fixture["request"]["variableFilter"].clone())
            .unwrap(),
        start: fixture["request"]["start"].as_i64(),
        count: fixture["request"]["count"].as_i64(),
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap();
    let request = decode_frame(&inspection.outbound_frames[0]);
    assert_eq!(request["command"], "variables");
    assert_eq!(request["arguments"]["variablesReference"], 700);
    assert_eq!(request["arguments"]["filter"], "indexed");
    assert_eq!(request["arguments"]["start"], 100);
    assert_eq!(request["arguments"]["count"], 2);

    let completed = receive_messages(
        session_id,
        vec![response_message(
            3,
            "variables",
            fixture["adapterResponse"].clone(),
        )],
    );
    let result = completed.events.iter().find_map(|event| match &event.body {
        DebugEventBody::OperationCompleted {
            operation_id,
            result,
        } if operation_id == fixture["request"]["operationId"].as_str().unwrap() => Some(result),
        _ => None,
    });
    assert_eq!(
        serde_json::to_value(result.unwrap()).unwrap(),
        fixture["expected"]
    );
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn exception_information_is_rejected_when_the_adapter_does_not_support_it() {
    let session_id = "debug-exception-info-unsupported";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(
        session_id,
        vec![json!({
            "seq": 103,
            "type": "event",
            "event": "stopped",
            "body": {"reason": "exception", "threadId": 13}
        })],
    );

    let error = inspect(InspectRequest {
        session_id: session_id.to_string(),
        operation_id: "exception-main".to_string(),
        kind: DebugInspectKind::ExceptionInfo,
        thread_id: Some(13),
        frame_id: None,
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    })
    .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn run_in_terminal_request_is_normalized_completed_and_stale_safe() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/run-in-terminal-v1.json"
    )))
    .unwrap();
    let session_id = "debug-run-in-terminal";
    let created = create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: true,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    assert_eq!(
        decode_frame(&created.outbound_frames[0])["arguments"]["supportsRunInTerminalRequest"],
        true
    );

    let requested = receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
    assert!(requested.outbound_frames.is_empty());
    let request_id = requested
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::RunInTerminalRequested {
                request_id,
                request,
            } => {
                assert_eq!(
                    serde_json::to_value(request).unwrap(),
                    fixture["expectedRequest"]
                );
                Some(request_id.clone())
            }
            _ => None,
        })
        .unwrap();

    let duplicate = receive_messages(session_id, vec![fixture["adapterRequest"].clone()]);
    assert!(duplicate.outbound_frames.is_empty());
    assert!(duplicate.events.is_empty());

    let invalid = run_in_terminal_response(DebugRunInTerminalResponseRequest {
        session_id: session_id.to_string(),
        request_id: request_id.clone(),
        success: true,
        process_id: Some(0),
        shell_process_id: None,
        message: None,
    })
    .unwrap_err();
    assert!(matches!(invalid.code, ErrorCode::InvalidRequest));

    let success = run_in_terminal_response(DebugRunInTerminalResponseRequest {
        session_id: session_id.to_string(),
        request_id: request_id.clone(),
        success: true,
        process_id: fixture["successResponse"]["processId"].as_i64(),
        shell_process_id: fixture["successResponse"]["shellProcessId"].as_i64(),
        message: None,
    })
    .unwrap();
    let response = decode_frame(&success.outbound_frames[0]);
    assert_eq!(response["request_seq"], fixture["adapterRequest"]["seq"]);
    assert_eq!(response["success"], true);
    assert_eq!(response["body"], fixture["expectedSuccessBody"]);

    let stale = run_in_terminal_response(DebugRunInTerminalResponseRequest {
        session_id: session_id.to_string(),
        request_id,
        success: false,
        process_id: None,
        shell_process_id: None,
        message: Some(fixture["failureMessage"].as_str().unwrap().to_string()),
    })
    .unwrap();
    assert!(stale.outbound_frames.is_empty());
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn malformed_run_in_terminal_request_gets_an_explicit_failure_response() {
    let session_id = "debug-run-in-terminal-invalid";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: true,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();

    let update = receive_messages(
        session_id,
        vec![request_message(
            45,
            "runInTerminal",
            json!({"cwd": "/workspace", "args": []}),
        )],
    );

    let response = decode_frame(&update.outbound_frames[0]);
    assert_eq!(response["request_seq"], 45);
    assert_eq!(response["success"], false);
    assert!(response["message"]
        .as_str()
        .unwrap()
        .contains("between 1 and 4096"));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn unknown_server_request_gets_an_explicit_failure_response() {
    let session_id = "debug-server-request";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();

    let update = receive_messages(
        session_id,
        vec![request_message(44, "runInTerminal", json!({}))],
    );

    let response = decode_frame(&update.outbound_frames[0]);
    assert_eq!(response["request_seq"], 44);
    assert_eq!(response["success"], false);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

fn snapshot_of(update: &DebugSessionUpdate) -> Option<(u64, BTreeMap<String, CapabilityState>)> {
    update.events.iter().find_map(|event| match &event.body {
        DebugEventBody::Projection {
            projection:
                SessionProjection {
                    projection: Projection::Capabilities(payload),
                    ..
                },
        } => Some((payload.revision, payload.capabilities.clone())),
        _ => None,
    })
}

#[test]
fn initialize_publishes_a_snapshot_and_forces_disabled_capabilities_off() {
    let session_id = "debug-capability-snapshot";
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: vec!["supportsRestartRequest".to_string()],
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();

    let update = receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({"supportsRestartRequest": true, "supportsWriteMemoryRequest": true}),
        )],
    );
    let (revision, states) = snapshot_of(&update).expect("initialize publishes a snapshot");
    assert_eq!(revision, 1);
    let restart = &states["supportsRestartRequest"];
    assert!(restart.adapter && restart.extension && !restart.authorized && !restart.supported);
    // The adapter reports memory reads, but no extension in this build handles it.
    let step_back = &states["supportsWriteMemoryRequest"];
    assert!(step_back.adapter && !step_back.extension && !step_back.supported);

    let late = receive_messages(
        session_id,
        vec![json!({
            "seq": 90,
            "type": "event",
            "event": "capabilities",
            "body": {"capabilities": {"supportsRestartRequest": true}}
        })],
    );
    assert!(
        snapshot_of(&late).is_none(),
        "an unchanged state must not bump the revision"
    );
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

fn child_request(session_id: &str, parent: Option<&str>) -> CreateSessionRequest {
    CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: parent.map(str::to_string),
        declared: Default::default(),
    }
}

fn child_events(update: &DebugSessionUpdate) -> Vec<String> {
    update
        .events
        .iter()
        .filter_map(|event| match &event.body {
            DebugEventBody::ChildSessionStopRequested { child_session_id } => {
                Some(format!("stop:{child_session_id}"))
            }
            _ => None,
        })
        .collect()
}

fn destroy(session_id: &str) -> Vec<String> {
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap()
}

#[test]
fn child_sessions_stop_with_the_parent_and_leave_the_tree_when_destroyed() {
    let parent = "debug-tree-parent";
    let child = "debug-tree-child";
    create_session(child_request(parent, None)).unwrap();
    assert!(create_session(child_request("debug-tree-orphan", Some("missing"))).is_err());
    create_session(child_request(child, Some(parent))).unwrap();

    let stopping = disconnect(DisconnectRequest {
        session_id: parent.to_string(),
        terminate_debuggee: None,
    })
    .unwrap();
    assert_eq!(child_events(&stopping), vec![format!("stop:{child}")]);
    let again = disconnect(DisconnectRequest {
        session_id: parent.to_string(),
        terminate_debuggee: None,
    })
    .unwrap();
    assert!(child_events(&again).is_empty(), "stop is requested once");
    assert!(
        create_session(child_request("debug-tree-late", Some(parent))).is_err(),
        "a stopping parent accepts no new children"
    );

    assert!(destroy(child).is_empty());
    with_session(parent, |session| {
        assert!(session.child_ids.is_empty());
        Ok(())
    })
    .unwrap();
    assert!(destroy(parent).is_empty());
}

#[test]
fn destroying_a_parent_destroys_its_whole_child_tree() {
    let (root, child, grandchild) = (
        "debug-cascade-root",
        "debug-cascade-child",
        "debug-cascade-leaf",
    );
    create_session(child_request(root, None)).unwrap();
    create_session(child_request(child, Some(root))).unwrap();
    create_session(child_request(grandchild, Some(child))).unwrap();

    assert_eq!(
        destroy(root),
        vec![grandchild.to_string(), child.to_string()]
    );
    for id in [root, child, grandchild] {
        assert!(
            with_session(id, |_| Ok(())).is_err(),
            "{id} must not outlive the root"
        );
    }
}

#[test]
fn stack_trace_and_scopes_emit_v2_projections_with_one_source_identity() {
    let session_id = "debug-v2-projection";
    paused_session(session_id);
    let mut request = evaluate_request(session_id, "stack", None);
    request.kind = DebugInspectKind::StackTrace;
    request.thread_id = Some(11);
    request.expression = None;
    inspect(request).unwrap();
    let update = receive_messages(
        session_id,
        vec![response_message(
            3,
            "stackTrace",
            json!({"totalFrames": 3, "stackFrames": [
                {"id": 1, "name": "main", "line": 4, "column": 2,
                 "source": {"path": "/workspace/src/Main.c"}},
                {"id": 2, "name": "decompiled", "line": 9,
                 "source": {"name": "Lib.class", "sourceReference": 77}},
                {"id": 3, "name": "jit", "line": 5,
                 "source": {"path": "/elsewhere/Lib.c"},
                 "instructionPointerReference": "0xFFFFFFFFFFFFFFFF"}
            ]}),
        )],
    );
    let projection = update
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::Projection { projection } => Some(projection.clone()),
            _ => None,
        })
        .expect("stack trace emits a v2 projection");
    assert_eq!(projection.stop_generation, 1);
    let Projection::StackTrace(payload) = projection.projection else {
        panic!("expected a stack trace projection");
    };
    assert_eq!(payload.thread_id, 11);
    assert!(matches!(
        &payload.frames[0].source,
        Some(Source::Workspace(source)) if source.path == "src/Main.c"
    ));
    assert!(matches!(
        &payload.frames[1].source,
        Some(Source::Session(source)) if source.source_reference == 77
    ));
    // A path outside the workspace has no grant, so the frame carries no location.
    assert!(payload.frames[2].source.is_none() && payload.frames[2].line.is_none());
    assert_eq!(
        payload.frames[2].instruction_pointer_reference.as_deref(),
        Some("0xFFFFFFFFFFFFFFFF")
    );
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

/// Creates a paused session so reference-bound inspections can be issued.
fn paused_session(session_id: &str) {
    paused_session_with_capabilities(session_id, json!({}));
}

/// A paused session whose adapter answered `initialize` with `capabilities`.
fn paused_session_with_capabilities(session_id: &str, capabilities: Value) {
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", capabilities)],
    );
    receive_messages(session_id, vec![response_message(2, "launch", json!({}))]);
    receive_messages(session_id, vec![stopped_event(103)]);
}

fn stopped_event(sequence: i64) -> Value {
    json!({
        "seq": sequence,
        "type": "event",
        "event": "stopped",
        "body": {"reason": "breakpoint", "threadId": 11}
    })
}

fn evaluate_request(session_id: &str, operation_id: &str, context: Option<&str>) -> InspectRequest {
    InspectRequest {
        session_id: session_id.to_string(),
        operation_id: operation_id.to_string(),
        kind: DebugInspectKind::Evaluate,
        thread_id: None,
        frame_id: Some(7),
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: Some("a + b".to_string()),
        context: context.map(str::to_string),
        source_path: None,
        line: None,
        column: None,
        memory_reference: None,
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    }
}

#[test]
fn evaluate_forwards_each_supported_context_and_rejects_unknown_ones() {
    let session_id = "debug-evaluate-context";
    paused_session(session_id);
    let update = inspect(evaluate_request(session_id, "default", None)).unwrap();
    assert_eq!(
        decode_frame(&update.outbound_frames[0])["arguments"]["context"],
        "watch"
    );
    for context in ["watch", "hover", "repl", "clipboard", "variables"] {
        let update = inspect(evaluate_request(session_id, context, Some(context))).unwrap();
        assert_eq!(
            decode_frame(&update.outbound_frames[0])["arguments"]["context"],
            context
        );
    }
    assert!(inspect(evaluate_request(session_id, "bad", Some("debugger"))).is_err());
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn reference_bound_replies_after_a_new_stop_fail_as_stale() {
    let session_id = "debug-stale-reference";
    paused_session(session_id);
    let update = inspect(evaluate_request(session_id, "old", None)).unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    receive_messages(session_id, vec![stopped_event(104)]);
    let update = receive_messages(
        session_id,
        vec![response_message(
            request_seq,
            "evaluate",
            json!({"result": "3"}),
        )],
    );
    assert!(update.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed { operation_id, code: DebugOperationFailureCode::StaleReference, .. }
            if operation_id == "old"
    )));
    assert!(!update
        .events
        .iter()
        .any(|event| matches!(event.body, DebugEventBody::OperationCompleted { .. })));

    let update = inspect(evaluate_request(session_id, "fresh", None)).unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    let update = receive_messages(
        session_id,
        vec![response_message(
            request_seq,
            "evaluate",
            json!({"result": "3"}),
        )],
    );
    assert!(update.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted { operation_id, .. } if operation_id == "fresh"
    )));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn stale_reference_fixture_matches_core_behavior() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/debug/stale-reference-v1.json"
    )))
    .unwrap();
    let session_id = "debug-stale-fixture";
    paused_session(session_id);
    let stale_id = fixture["staleOperationId"].as_str().unwrap();
    let update = inspect(evaluate_request(session_id, stale_id, None)).unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    receive_messages(session_id, vec![stopped_event(104)]);
    let update = receive_messages(
        session_id,
        vec![response_message(
            request_seq,
            "evaluate",
            fixture["evaluateResponseBody"].clone(),
        )],
    );
    let failed = update
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::OperationFailed {
                operation_id, code, ..
            } => Some((operation_id.clone(), serde_json::to_value(code).unwrap())),
            _ => None,
        })
        .expect("stale reply must fail");
    assert_eq!(failed.0, stale_id);
    assert_eq!(failed.1, fixture["expectedFailureCode"]);
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

fn session_state(session_id: &str) -> DebugSessionState {
    with_session(session_id, |session| Ok(session.state)).unwrap()
}

fn cleanup(session_id: &str) {
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

#[test]
fn step_reply_arriving_after_the_stopped_event_keeps_the_session_paused() {
    let session_id = "debug-step-race";
    paused_session(session_id);
    let update = execute(ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "step".to_string(),
        command: DebugExecutionCommand::Next,
        thread_id: Some(11),
        target_id: None,
        single_thread: false,
        granularity: None,
    })
    .unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    let stopped = receive_messages(session_id, vec![stopped_event(104)]);
    let generation = stopped
        .events
        .iter()
        .find_map(|event| match event.body {
            DebugEventBody::Stopped {
                stop_generation, ..
            } => Some(stop_generation),
            _ => None,
        })
        .unwrap();
    receive_messages(
        session_id,
        vec![response_message(request_seq, "next", json!({}))],
    );
    assert_eq!(session_state(session_id), DebugSessionState::Paused);
    let current = with_session(session_id, |session| Ok(session.stop_generation)).unwrap();
    assert_eq!(current, generation);
    cleanup(session_id);
}

#[test]
fn single_thread_continue_event_keeps_other_threads_references_valid() {
    let session_id = "debug-single-thread-continue";
    paused_session(session_id);
    let update = inspect(evaluate_request(session_id, "other", None)).unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    receive_messages(
        session_id,
        vec![json!({"seq": 105, "type": "event", "event": "continued",
            "body": {"threadId": 12, "allThreadsContinued": false}})],
    );
    let update = receive_messages(
        session_id,
        vec![response_message(
            request_seq,
            "evaluate",
            json!({"result": "1"}),
        )],
    );
    assert!(update.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted { operation_id, .. } if operation_id == "other"
    )));
    assert_eq!(session_state(session_id), DebugSessionState::Paused);
    cleanup(session_id);
}

#[test]
fn stale_stack_trace_reply_is_not_published_under_the_new_generation() {
    let session_id = "debug-stale-stack";
    paused_session(session_id);
    let mut request = evaluate_request(session_id, "stack", None);
    request.kind = DebugInspectKind::StackTrace;
    request.thread_id = Some(11);
    request.expression = None;
    let update = inspect(request).unwrap();
    let request_seq = decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap();
    receive_messages(session_id, vec![stopped_event(104)]);
    let update = receive_messages(
        session_id,
        vec![response_message(
            request_seq,
            "stackTrace",
            json!({"stackFrames": [{"id": 1, "name": "main", "line": 1}]}),
        )],
    );
    assert!(update.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed {
            code: DebugOperationFailureCode::StaleReference,
            ..
        }
    )));
    assert!(!update
        .events
        .iter()
        .any(|event| matches!(event.body, DebugEventBody::Projection { .. })));
    cleanup(session_id);
}

#[test]
fn capabilities_event_with_new_exception_filters_is_published() {
    let session_id = "debug-late-filters";
    paused_session(session_id);
    let update = receive_messages(
        session_id,
        vec![json!({"seq": 106, "type": "event", "event": "capabilities",
        "body": {"capabilities": {"exceptionBreakpointFilters": [
            {"filter": "uncaught", "label": "Uncaught", "default": true}
        ]}}})],
    );
    let filters = update.events.iter().find_map(|event| match &event.body {
        DebugEventBody::Capabilities { capabilities } => {
            Some(capabilities.exception_breakpoint_filters.len())
        }
        _ => None,
    });
    assert_eq!(filters, Some(1));
    cleanup(session_id);
}

#[test]
fn terminated_parent_asks_each_child_to_stop_once() {
    let parent = "debug-term-parent";
    let child = "debug-term-child";
    create_session(child_request(parent, None)).unwrap();
    create_session(child_request(child, Some(parent))).unwrap();
    let update = receive_messages(
        parent,
        vec![json!({"seq": 9, "type": "event", "event": "terminated", "body": {}})],
    );
    assert_eq!(child_events(&update), vec![format!("stop:{child}")]);
    let update = disconnect(DisconnectRequest {
        session_id: parent.to_string(),
        terminate_debuggee: None,
    })
    .unwrap();
    assert!(child_events(&update).is_empty());
    cleanup(child);
    cleanup(parent);
}

#[test]
fn stop_generation_increments_on_every_stop() {
    let session_id = "debug-stop-generation";
    paused_session(session_id);
    let update = receive_messages(session_id, vec![stopped_event(104)]);
    assert!(update.events.iter().any(|event| matches!(
        event.body,
        DebugEventBody::Stopped {
            stop_generation: 2,
            ..
        }
    )));
    destroy_session(SessionRequest {
        session_id: session_id.to_string(),
    })
    .unwrap();
}

/// Removes the process-free session even when an assertion fails first.
struct SessionGuard(&'static str);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = destroy_session(SessionRequest {
            session_id: self.0.to_string(),
        });
    }
}

fn memory_request(session_id: &str, operation_id: &str, kind: DebugInspectKind) -> InspectRequest {
    InspectRequest {
        session_id: session_id.to_string(),
        operation_id: operation_id.to_string(),
        kind,
        thread_id: Some(11),
        frame_id: Some(7),
        variables_reference: None,
        variable_filter: None,
        start: None,
        count: None,
        expression: None,
        context: None,
        source_path: None,
        line: None,
        column: None,
        memory_reference: Some("0x100003f50".to_string()),
        offset: None,
        instruction_offset: None,
        instruction_count: None,
        resolve_symbols: None,
    }
}

fn step_request(
    session_id: &str,
    command: DebugExecutionCommand,
    granularity: Option<DebugSteppingGranularity>,
) -> ExecuteRequest {
    ExecuteRequest {
        session_id: session_id.to_string(),
        operation_id: "step".to_string(),
        command,
        thread_id: Some(11),
        target_id: None,
        single_thread: false,
        granularity,
    }
}

fn instruction_breakpoint(
    reference: &str,
    offset: Option<i64>,
    enabled: bool,
    condition: Option<&str>,
) -> InstructionBreakpoint {
    InstructionBreakpoint {
        instruction_reference: reference.to_string(),
        offset,
        enabled,
        condition: condition.map(str::to_string),
        hit_condition: None,
    }
}

fn request_sequence(update: &DebugSessionUpdate) -> i64 {
    decode_frame(&update.outbound_frames[0])["seq"]
        .as_i64()
        .unwrap()
}

#[test]
fn sourceless_frames_keep_the_instruction_pointer_without_a_fake_line() {
    let filters = DebugSteppingFilters::unfiltered();
    let body = json!({"stackFrames": [
        {"id": 1, "name": "main", "line": 9, "column": 1,
         "instructionPointerReference": "0x100003f50",
         "source": {"name": "main.c", "path": "/workspace/main.c"}},
        {"id": 2, "name": "start",
         "instructionPointerReference": "0xffffffffffffffff"},
        {"id": 3, "name": "no-line",
         "source": {"name": "a.c", "path": "/workspace/a.c"}},
        {"id": 4, "name": "empty-reference", "instructionPointerReference": ""},
    ]});
    let frames =
        normalize_inspection(DebugInspectKind::StackTrace, &body, &filters, "/workspace").unwrap();
    let value = serde_json::to_value(frames).unwrap();
    let frames = &value["stackFrames"];
    assert_eq!(frames[0]["line"], 9);
    assert_eq!(frames[0]["instructionPointerReference"], "0x100003f50");
    // No file means no line: the UI shows the function and its address instead.
    assert_eq!(frames[1]["line"], 0);
    assert!(frames[1]["sourcePath"].is_null());
    // A 64-bit address stays an opaque string; it never passes through a JSON number.
    assert_eq!(
        frames[1]["instructionPointerReference"],
        "0xffffffffffffffff"
    );
    // A frame that does name a file keeps the historical line-1 fallback.
    assert_eq!(frames[2]["line"], 1);
    assert!(frames[2].get("instructionPointerReference").is_none());
    assert!(frames[3].get("instructionPointerReference").is_none());
}

#[test]
fn registers_scope_keeps_its_presentation_hint() {
    let filters = DebugSteppingFilters::unfiltered();
    let body = json!({"scopes": [
        {"name": "Locals", "variablesReference": 3},
        {"name": "Registers", "variablesReference": 4,
         "presentationHint": "registers", "expensive": true},
        {"name": "Blank", "variablesReference": 5, "presentationHint": ""},
    ]});
    let scopes =
        normalize_inspection(DebugInspectKind::Scopes, &body, &filters, "/workspace").unwrap();
    let value = serde_json::to_value(scopes).unwrap();
    let scopes = &value["scopes"];
    assert!(scopes[0].get("presentationHint").is_none());
    assert_eq!(scopes[1]["presentationHint"], "registers");
    assert!(scopes[2].get("presentationHint").is_none());
}

#[test]
fn stepping_granularity_is_gated_and_forwarded() {
    let unsupported = "debug-granularity-unsupported";
    let _unsupported_guard = SessionGuard(unsupported);
    paused_session(unsupported);
    let error = execute(step_request(
        unsupported,
        DebugExecutionCommand::StepIn,
        Some(DebugSteppingGranularity::Instruction),
    ))
    .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    // The rejected request left the session paused, so a plain step still works.
    let update = execute(step_request(unsupported, DebugExecutionCommand::Next, None)).unwrap();
    let request = decode_frame(&update.outbound_frames[0]);
    assert_eq!(request["command"], "next");
    assert!(request["arguments"].get("granularity").is_none());

    let supported = "debug-granularity-supported";
    let _supported_guard = SessionGuard(supported);
    paused_session_with_capabilities(supported, json!({"supportsSteppingGranularity": true}));
    // Granularity only describes how far a step moves.
    for command in [
        DebugExecutionCommand::Continue,
        DebugExecutionCommand::Pause,
        DebugExecutionCommand::Restart,
    ] {
        assert!(execute(step_request(
            supported,
            command,
            Some(DebugSteppingGranularity::Instruction)
        ))
        .is_err());
    }
    let update = execute(step_request(
        supported,
        DebugExecutionCommand::StepIn,
        Some(DebugSteppingGranularity::Instruction),
    ))
    .unwrap();
    let request = decode_frame(&update.outbound_frames[0]);
    assert_eq!(request["command"], "stepIn");
    assert_eq!(request["arguments"]["granularity"], "instruction");
    assert_eq!(request["arguments"]["threadId"], 11);
}

#[test]
fn instruction_breakpoints_are_validated_deduplicated_and_sent_after_initialized() {
    let session_id = "debug-instruction-breakpoints";
    let _guard = SessionGuard(session_id);
    create_session(CreateSessionRequest {
        session_id: session_id.to_string(),
        adapter_id: "lldb".to_string(),
        root_path: "/workspace".to_string(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared: Default::default(),
    })
    .unwrap();
    launch(LaunchRequest {
        session_id: session_id.to_string(),
        operation_id: "launch".to_string(),
        configuration: DebugLaunchConfiguration {
            name: "Main".to_string(),
            request: DebugRequestKind::Launch,
            arguments: Map::new(),
            stepping_filters: None,
        },
    })
    .unwrap();
    for invalid in ["  ", "0x1000\n0x2000", &"9".repeat(300)] {
        assert!(
            set_instruction_breakpoints(SetInstructionBreakpointsRequest {
                session_id: session_id.to_string(),
                breakpoints: vec![instruction_breakpoint(invalid, None, true, None)],
            })
            .is_err(),
            "{invalid:?}"
        );
    }
    let queued = set_instruction_breakpoints(SetInstructionBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: vec![
            instruction_breakpoint(" 0x100003f60 ", Some(4), true, None),
            instruction_breakpoint(" 0x100003f50 ", None, true, Some(" x > 1 ")),
            instruction_breakpoint("0x100003f50", None, true, Some("x > 1")),
            instruction_breakpoint("0x100003f70", None, false, None),
        ],
    })
    .unwrap();
    // The session has not seen `initialized`, so nothing is sent yet.
    assert!(queued.outbound_frames.is_empty());

    receive_messages(
        session_id,
        vec![response_message(
            1,
            "initialize",
            json!({
                "supportsConfigurationDoneRequest": true,
                "supportsInstructionBreakpoints": true
            }),
        )],
    );
    let configured = receive_messages(
        session_id,
        vec![json!({"seq": 102, "type": "event", "event": "initialized"})],
    );
    let frames: Vec<Value> = configured
        .outbound_frames
        .iter()
        .map(|frame| decode_frame(frame))
        .collect();
    let sent = frames
        .iter()
        .find(|frame| frame["command"] == "setInstructionBreakpoints")
        .expect("instruction breakpoints are sent after initialized");
    let breakpoints = sent["arguments"]["breakpoints"].as_array().unwrap();
    // The duplicate address collapsed and the disabled one is not sent.
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0]["instructionReference"], "0x100003f50");
    assert_eq!(breakpoints[0]["condition"], "x > 1");
    assert!(breakpoints[0].get("offset").is_none());
    assert_eq!(breakpoints[1]["instructionReference"], "0x100003f60");
    assert_eq!(breakpoints[1]["offset"], 4);

    let results = receive_messages(
        session_id,
        vec![response_message(
            sent["seq"].as_i64().unwrap(),
            "setInstructionBreakpoints",
            json!({"breakpoints": [
                {"id": 5, "verified": true, "instructionReference": "0x100003f50"},
                {"verified": false, "message": "no code at this address"}
            ]}),
        )],
    );
    let verified: Vec<(Option<String>, bool)> = results
        .events
        .iter()
        .filter_map(|event| match &event.body {
            DebugEventBody::Breakpoint { breakpoint } => Some((
                breakpoint.instruction_reference.clone(),
                breakpoint.verified,
            )),
            _ => None,
        })
        .collect();
    // The second reply omitted the address, so the request at the same index names it.
    assert_eq!(
        verified,
        vec![
            (Some("0x100003f50".to_string()), true),
            (Some("0x100003f60".to_string()), false)
        ]
    );

    // Once initialized, a change is sent immediately and an empty list clears the set.
    let cleared = set_instruction_breakpoints(SetInstructionBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: Vec::new(),
    })
    .unwrap();
    let request = decode_frame(&cleared.outbound_frames[0]);
    assert_eq!(request["command"], "setInstructionBreakpoints");
    assert_eq!(request["arguments"]["breakpoints"], json!([]));
}

#[test]
fn instruction_breakpoints_are_not_sent_without_adapter_support() {
    let session_id = "debug-instruction-breakpoints-unsupported";
    let _guard = SessionGuard(session_id);
    paused_session(session_id);
    let update = set_instruction_breakpoints(SetInstructionBreakpointsRequest {
        session_id: session_id.to_string(),
        breakpoints: vec![instruction_breakpoint("0x100003f50", None, true, None)],
    })
    .unwrap();
    assert!(update.outbound_frames.is_empty());
}

#[test]
fn disassemble_is_gated_bounded_and_normalized() {
    let unsupported = "debug-disassemble-unsupported";
    let _unsupported_guard = SessionGuard(unsupported);
    paused_session(unsupported);
    let mut request = memory_request(unsupported, "dis", DebugInspectKind::Disassemble);
    request.instruction_count = Some(4);
    assert!(inspect(request).is_err());

    let session_id = "debug-disassemble";
    let _guard = SessionGuard(session_id);
    paused_session_with_capabilities(session_id, json!({"supportsDisassembleRequest": true}));
    let valid = |count: Option<i64>| {
        let mut request = memory_request(session_id, "dis", DebugInspectKind::Disassemble);
        request.instruction_count = count;
        request
    };
    assert!(inspect(valid(None)).is_err());
    assert!(inspect(valid(Some(0))).is_err());
    assert!(inspect(valid(Some(1001))).is_err());
    let mut no_reference = valid(Some(4));
    no_reference.memory_reference = None;
    assert!(inspect(no_reference).is_err());
    // Memory fields belong to memory requests only.
    let mut misplaced = memory_request(session_id, "threads", DebugInspectKind::Threads);
    misplaced.memory_reference = Some("0x100003f50".to_string());
    assert!(inspect(misplaced).is_err());

    let mut request = valid(Some(5));
    request.instruction_offset = Some(-2);
    request.offset = Some(0);
    let update = inspect(request).unwrap();
    let sent = decode_frame(&update.outbound_frames[0]);
    assert_eq!(sent["command"], "disassemble");
    assert_eq!(sent["arguments"]["memoryReference"], "0x100003f50");
    assert_eq!(sent["arguments"]["instructionOffset"], -2);
    assert_eq!(sent["arguments"]["instructionCount"], 5);
    assert_eq!(sent["arguments"]["resolveSymbols"], true);

    let completed = receive_messages(
        session_id,
        vec![response_message(
            sent["seq"].as_i64().unwrap(),
            "disassemble",
            json!({"instructions": [
                {"address": "0x100003f48", "instructionBytes": "fd 7b bf a9",
                 "instruction": "stp x29, x30, [sp, #-0x10]!", "symbol": "main"},
                {"address": "0xffffffffffffff00", "instruction": "mov x29, sp",
                 "location": {"path": "/workspace/main.c"}, "line": 9},
                {"instruction": "no address is dropped"},
                {"address": "0x100003f50"}
            ]}),
        )],
    );
    let instructions = completed
        .events
        .iter()
        .find_map(|event| match &event.body {
            DebugEventBody::OperationCompleted {
                operation_id,
                result: DebugOperationResult::Disassemble { instructions },
            } if operation_id == "dis" => Some(instructions),
            _ => None,
        })
        .expect("disassemble result");
    assert_eq!(instructions.len(), 2);
    assert_eq!(instructions[0].address, "0x100003f48");
    assert_eq!(
        instructions[0].instruction_bytes.as_deref(),
        Some("fd 7b bf a9")
    );
    assert_eq!(instructions[0].symbol.as_deref(), Some("main"));
    assert_eq!(instructions[1].address, "0xffffffffffffff00");
    assert_eq!(
        instructions[1].source_path.as_deref(),
        Some("/workspace/main.c")
    );
    assert_eq!(instructions[1].line, Some(9));
}

#[test]
fn disassemble_failure_and_stale_replies_are_operation_failures() {
    let session_id = "debug-disassemble-failure";
    let _guard = SessionGuard(session_id);
    paused_session_with_capabilities(session_id, json!({"supportsDisassembleRequest": true}));
    let mut request = memory_request(session_id, "bad", DebugInspectKind::Disassemble);
    request.instruction_count = Some(3);
    // A body without `instructions` is a rejected request, not a broken session.
    let update = inspect(request.clone()).unwrap();
    let failed = receive_messages(
        session_id,
        vec![response_message(
            request_sequence(&update),
            "disassemble",
            json!({}),
        )],
    );
    assert!(failed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed { operation_id, code: DebugOperationFailureCode::AdapterRejected, .. }
            if operation_id == "bad"
    )));
    assert_eq!(session_state(session_id), DebugSessionState::Paused);

    // Addresses belong to one stop: a reply after the next stop is stale.
    request.operation_id = "old".to_string();
    let update = inspect(request).unwrap();
    let sequence = request_sequence(&update);
    receive_messages(session_id, vec![stopped_event(104)]);
    let stale = receive_messages(
        session_id,
        vec![response_message(
            sequence,
            "disassemble",
            json!({"instructions": [{"address": "0x1", "instruction": "nop"}]}),
        )],
    );
    assert!(stale.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed { operation_id, code: DebugOperationFailureCode::StaleReference, .. }
            if operation_id == "old"
    )));
}

#[test]
fn read_memory_is_gated_bounded_and_returns_validated_bytes() {
    let unsupported = "debug-read-memory-unsupported";
    let _unsupported_guard = SessionGuard(unsupported);
    paused_session(unsupported);
    let mut request = memory_request(unsupported, "mem", DebugInspectKind::ReadMemory);
    request.count = Some(16);
    assert!(inspect(request).is_err());

    let session_id = "debug-read-memory";
    let _guard = SessionGuard(session_id);
    paused_session_with_capabilities(session_id, json!({"supportsReadMemoryRequest": true}));
    let sized = |count: Option<i64>| {
        let mut request = memory_request(session_id, "mem", DebugInspectKind::ReadMemory);
        request.count = count;
        request
    };
    assert!(inspect(sized(None)).is_err());
    assert!(inspect(sized(Some(0))).is_err());
    assert!(inspect(sized(Some(65_537))).is_err());

    let mut request = sized(Some(16));
    request.offset = Some(8);
    let update = inspect(request).unwrap();
    let sent = decode_frame(&update.outbound_frames[0]);
    assert_eq!(sent["command"], "readMemory");
    assert_eq!(sent["arguments"]["memoryReference"], "0x100003f50");
    assert_eq!(sent["arguments"]["offset"], 8);
    assert_eq!(sent["arguments"]["count"], 16);

    let completed = receive_messages(
        session_id,
        vec![response_message(
            sent["seq"].as_i64().unwrap(),
            "readMemory",
            json!({"address": "0x100003f58", "unreadableBytes": 8, "data": "3q2+7w=="}),
        )],
    );
    assert!(completed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::ReadMemory { address, unreadable_bytes: 8, data_base64 }
        } if operation_id == "mem" && address == "0x100003f58" && data_base64 == "3q2+7w=="
    )));

    // Entirely unreadable memory has no `data`; invalid base64 is rejected, not forwarded.
    let update = inspect(sized(Some(4))).unwrap();
    let unreadable = receive_messages(
        session_id,
        vec![response_message(
            request_sequence(&update),
            "readMemory",
            json!({"address": "0x100003f50", "unreadableBytes": 4}),
        )],
    );
    assert!(unreadable.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            result: DebugOperationResult::ReadMemory { unreadable_bytes: 4, data_base64, .. },
            ..
        } if data_base64.is_empty()
    )));
    let update = inspect(sized(Some(4))).unwrap();
    let malformed = receive_messages(
        session_id,
        vec![response_message(
            request_sequence(&update),
            "readMemory",
            json!({"address": "0x100003f50", "data": "***not base64***"}),
        )],
    );
    assert!(malformed.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed {
            code: DebugOperationFailureCode::AdapterRejected,
            ..
        }
    )));
    assert_eq!(session_state(session_id), DebugSessionState::Paused);
}

/// A scripting-language adapter that declares the DAP extensions it implements.
fn scripting_adapter() -> DeclaredAdapterBehavior {
    serde_json::from_value(json!({
        "features": {
            "hotCodeReplace": {
                "request": "script.reload",
                "event": "script.reloaded",
                "changedField": "modules",
                "errorField": "error"
            },
            "stepFilters": {"argument": "skipFilters"}
        },
        "defaults": {
            "stepFilters": {
                "classNameFilters": ["site-packages/*"],
                "skipSynthetics": true
            }
        }
    }))
    .unwrap()
}

/// Creates a session, answers `initialize`, and launches with `arguments`.
/// Returns the launch request the adapter received.
fn launch_session(
    session_id: &str,
    adapter_id: &str,
    declared: DeclaredAdapterBehavior,
    arguments: Value,
) -> Value {
    create_session(CreateSessionRequest {
        session_id: session_id.into(),
        adapter_id: adapter_id.into(),
        root_path: "/workspace".into(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared,
    })
    .unwrap();
    receive_messages(
        session_id,
        vec![response_message(1, "initialize", json!({}))],
    );
    let launched = launch(LaunchRequest {
        session_id: session_id.into(),
        operation_id: "launch".into(),
        configuration: DebugLaunchConfiguration {
            name: "script".into(),
            request: DebugRequestKind::Launch,
            arguments: serde_json::from_value(arguments).unwrap(),
            stepping_filters: None,
        },
    })
    .unwrap();
    decode_frame(&launched.outbound_frames[0])
}

/// Answers the launch request and stops the debuggee at a breakpoint.
fn pause_session(session_id: &str, launch_frame: &Value) {
    receive_messages(
        session_id,
        vec![
            response_message(launch_frame["seq"].as_i64().unwrap(), "launch", json!({})),
            json!({"type": "event", "event": "stopped",
                   "body": {"reason": "breakpoint", "threadId": 1}}),
        ],
    );
}

fn redefine_request(session_id: &str, operation_id: &str) -> InspectRequest {
    serde_json::from_value(json!({
        "sessionId": session_id, "operationId": operation_id, "kind": "redefineClasses"
    }))
    .unwrap()
}

#[test]
fn declared_hot_code_replace_sends_the_declared_request_and_reads_its_fields() {
    let session_id = "debug-declared-hot-replace";
    let _guard = SessionGuard(session_id);
    let launched = launch_session(session_id, "debugpy", scripting_adapter(), json!({}));
    pause_session(session_id, &launched);

    let update = inspect(redefine_request(session_id, "apply")).unwrap();
    let frame = decode_frame(&update.outbound_frames[0]);
    assert_eq!(frame["command"], "script.reload");
    let replaced = receive_messages(
        session_id,
        vec![response_message(
            frame["seq"].as_i64().unwrap(),
            "script.reload",
            json!({"modules": ["pkg.util", "pkg.app", "pkg.util"]}),
        )],
    );
    assert_eq!(replaced.state, DebugSessionState::Paused);
    assert!(replaced.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationCompleted {
            operation_id,
            result: DebugOperationResult::RedefineClasses { changed_classes },
        } if operation_id == "apply"
            && changed_classes == &["pkg.app".to_string(), "pkg.util".to_string()]
    )));

    // A failure the adapter reports inside a successful response fails the
    // operation without breaking the session.
    let update = inspect(redefine_request(session_id, "rejected")).unwrap();
    let rejected = receive_messages(
        session_id,
        vec![response_message(
            request_sequence(&update),
            "script.reload",
            json!({"modules": [], "error": "Syntax error in pkg.app"}),
        )],
    );
    assert_eq!(rejected.state, DebugSessionState::Paused);
    assert!(rejected.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::OperationFailed { operation_id, message, .. }
            if operation_id == "rejected" && message == "Syntax error in pkg.app"
    )));
}

#[test]
fn declared_hot_code_replace_event_message_reaches_the_debug_console() {
    let session_id = "debug-declared-hot-replace-event";
    let _guard = SessionGuard(session_id);
    let launched = launch_session(session_id, "debugpy", scripting_adapter(), json!({}));
    pause_session(session_id, &launched);

    let update = receive_messages(
        session_id,
        vec![json!({"type": "event", "event": "script.reloaded",
                    "body": {"message": "Reloaded 2 modules"}})],
    );
    assert!(update.events.iter().any(|event| matches!(
        &event.body,
        DebugEventBody::Output { category: Some(category), output }
            if category == "console" && output == "Reloaded 2 modules\n"
    )));
}

#[test]
fn an_adapter_without_the_declaration_has_no_hot_code_replace() {
    let session_id = "debug-undeclared-hot-replace";
    let _guard = SessionGuard(session_id);
    let launched = launch_session(
        session_id,
        "debugpy",
        DeclaredAdapterBehavior::default(),
        json!({}),
    );
    pause_session(session_id, &launched);

    assert!(inspect(redefine_request(session_id, "apply")).is_err());
}

#[test]
fn declared_step_filters_use_the_declared_argument_and_defaults() {
    let session_id = "debug-declared-step-filters";
    let _guard = SessionGuard(session_id);
    let launched = launch_session(session_id, "debugpy", scripting_adapter(), json!({}));

    let filters = &launched["arguments"]["skipFilters"];
    assert_eq!(filters["skipClasses"], json!(["site-packages/*"]));
    assert_eq!(filters["skipSynthetics"], true);
    assert_eq!(filters["skipConstructors"], false);
    // The declaration names the argument; no other argument is invented.
    assert!(launched["arguments"].get("stepFilters").is_none());
    assert!(launched["arguments"].get("vmArgs").is_none());
}

#[test]
fn declared_step_filters_keep_what_the_launch_configuration_sets() {
    let session_id = "debug-declared-step-filters-explicit";
    let _guard = SessionGuard(session_id);
    let launched = launch_session(
        session_id,
        "debugpy",
        scripting_adapter(),
        json!({"skipFilters": {"skipClasses": ["mine/*"]}}),
    );

    assert_eq!(
        launched["arguments"]["skipFilters"],
        json!({"skipClasses": ["mine/*"]})
    );
}

#[test]
fn invalid_declarations_are_refused_when_the_session_is_created() {
    let declared = serde_json::from_value(json!({
        "features": {"hotCodeReplace": {"request": "not a command"}}
    }))
    .unwrap();
    let error = create_session(CreateSessionRequest {
        session_id: "debug-invalid-declaration".into(),
        adapter_id: "debugpy".into(),
        root_path: "/workspace".into(),
        supports_run_in_terminal_request: false,
        supports_start_debugging_request: false,
        disabled_capabilities: Vec::new(),
        parent_session_id: None,
        declared,
    })
    .unwrap_err();
    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    assert_eq!(
        error.details.as_deref(),
        Some("features.hotCodeReplace.request")
    );
}

fn sent(update: &DebugSessionUpdate) -> Vec<(String, Value)> {
    update
        .outbound_frames
        .iter()
        .map(|frame| {
            let message = decode_frame(frame);
            (
                message["command"].as_str().unwrap_or_default().to_string(),
                message["arguments"].clone(),
            )
        })
        .collect()
}

// Muting sends every kind of stored breakpoint empty, the adapter's default exception
// filters included, and unmuting sends them as stored; a child starts muted.
#[test]
fn muting_empties_every_breakpoint_kind_and_children_inherit_it() {
    let parent = "debug-mute-parent";
    let child = "debug-mute-child";
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for session_id in ["debug-mute-child", "debug-mute-parent"] {
                // log-exempt: Expected absence if the test already disposed this process-free session.
                let _ = destroy_session(SessionRequest {
                    session_id: session_id.into(),
                });
            }
        }
    }
    let _cleanup = Cleanup;
    let capabilities = json!({
        "supportsFunctionBreakpoints": true,
        "supportsInstructionBreakpoints": true,
        "supportsDataBreakpoints": true,
        "exceptionBreakpointFilters": [{
            "filter": "uncaught",
            "label": "Uncaught Exceptions",
            "default": true
        }]
    });
    create_session(child_request(parent, None)).unwrap();
    receive_messages(
        parent,
        vec![response_message(1, "initialize", capabilities.clone())],
    );
    let line = |line: i64, enabled: bool| SourceBreakpoint {
        line,
        column: None,
        enabled,
        condition: None,
        hit_condition: None,
        log_message: None,
    };
    set_breakpoints(SetBreakpointsRequest {
        session_id: parent.into(),
        source_path: "/workspace/main.c".into(),
        breakpoints: vec![line(4, true), line(9, false)],
    })
    .unwrap();
    set_breakpoints(SetBreakpointsRequest {
        session_id: parent.into(),
        source_path: "/workspace/off.c".into(),
        breakpoints: vec![line(2, false)],
    })
    .unwrap();
    set_function_breakpoints(SetFunctionBreakpointsRequest {
        session_id: parent.into(),
        breakpoints: vec![FunctionBreakpoint {
            name: "main".into(),
            enabled: true,
            condition: None,
            hit_condition: None,
        }],
    })
    .unwrap();
    set_instruction_breakpoints(SetInstructionBreakpointsRequest {
        session_id: parent.into(),
        breakpoints: vec![InstructionBreakpoint {
            instruction_reference: "0x1000".into(),
            offset: None,
            enabled: true,
            condition: None,
            hit_condition: None,
        }],
    })
    .unwrap();
    let configured = receive_messages(
        parent,
        vec![json!({"seq": 102, "type": "event", "event": "initialized"})],
    );
    assert_eq!(sent(&configured)[0].1["filters"], json!(["uncaught"]));

    let muted = set_breakpoints_muted(SetBreakpointsMutedRequest {
        session_id: parent.into(),
        muted: true,
    })
    .unwrap();
    // The data set and the file holding only a disabled line send the same either way.
    assert_eq!(
        sent(&muted),
        vec![
            ("setExceptionBreakpoints".into(), json!({"filters": []})),
            ("setFunctionBreakpoints".into(), json!({"breakpoints": []})),
            (
                "setInstructionBreakpoints".into(),
                json!({"breakpoints": []})
            ),
            (
                "setBreakpoints".into(),
                json!({
                    "source": {"name": "main.c", "path": "/workspace/main.c"},
                    "breakpoints": [],
                    "sourceModified": false
                })
            ),
        ]
    );
    let again = set_breakpoints_muted(SetBreakpointsMutedRequest {
        session_id: parent.into(),
        muted: true,
    })
    .unwrap();
    assert!(again.outbound_frames.is_empty());
    // A set replaced while muted is stored and sent empty.
    let replaced = set_function_breakpoints(SetFunctionBreakpointsRequest {
        session_id: parent.into(),
        breakpoints: vec![FunctionBreakpoint {
            name: "worker".into(),
            enabled: true,
            condition: None,
            hit_condition: None,
        }],
    })
    .unwrap();
    assert_eq!(sent(&replaced)[0].1, json!({"breakpoints": []}));

    create_session(child_request(child, Some(parent))).unwrap();
    receive_messages(child, vec![response_message(1, "initialize", capabilities)]);
    set_breakpoints(SetBreakpointsRequest {
        session_id: child.into(),
        source_path: "/workspace/main.c".into(),
        breakpoints: vec![line(4, true)],
    })
    .unwrap();
    let child_configured = receive_messages(
        child,
        vec![json!({"seq": 102, "type": "event", "event": "initialized"})],
    );
    let child_sent = sent(&child_configured);
    assert_eq!(
        child_sent[0],
        ("setExceptionBreakpoints".into(), json!({"filters": []}))
    );
    assert!(child_sent
        .iter()
        .filter(|(command, _)| command == "setBreakpoints")
        .all(|(_, arguments)| arguments["breakpoints"] == json!([])));

    let unmuted = set_breakpoints_muted(SetBreakpointsMutedRequest {
        session_id: parent.into(),
        muted: false,
    })
    .unwrap();
    let unmuted = sent(&unmuted);
    assert_eq!(unmuted[0].1["filters"], json!(["uncaught"]));
    assert_eq!(unmuted[1].1["breakpoints"][0]["name"], "worker");
    assert_eq!(
        unmuted[2].1["breakpoints"][0]["instructionReference"],
        "0x1000"
    );
    assert_eq!(unmuted[3].1["breakpoints"], json!([{"line": 4}]));
    assert_eq!(unmuted.len(), 4);
}
