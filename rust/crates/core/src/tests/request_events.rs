//! Event channels of requests run through `execute_json_with_events`: a host
//! callback that answers through Core keeps the outer request's events and
//! cancellation, and a nested observed request gets its own channel.

use std::sync::Arc;

#[test]
fn callback_auth_reply_preserves_outer_events_and_cancellation() {
    let events = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = events.clone();
    let response = crate::execute_json_with_events(
        r#"{"id":"callback-cancellation","command":"core.ping","timeoutMilliseconds":1000}"#,
        Arc::new(move |event| {
            let event: serde_json::Value = serde_json::from_str(event).unwrap();
            captured.lock().unwrap().push(event.clone());
            if event["type"] == "requestStarted" && event["operationId"] == "callback-cancellation"
            {
                // Like the C AskPass host, synchronously reply through Core
                // before requesting cancellation of the observed operation.
                let reply: serde_json::Value = serde_json::from_str(&crate::execute_json(
                    r#"{"id":"callback-auth-reply","command":"git.authRespond","payload":{"requestId":"expired-fixture","answer":null}}"#,
                )).unwrap();
                assert_eq!(reply["ok"], true);
                assert_eq!(reply["data"]["accepted"], false);
                assert!(crate::cancel_operation("callback-cancellation"));
            }
        }),
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], "cancelled", "{response}");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0]["type"], "requestStarted");
    assert_eq!(events[1]["type"], "requestFinished");
    assert!(events
        .iter()
        .all(|event| event["operationId"] == "callback-cancellation"));
    assert!(!crate::cancel_operation("callback-cancellation"));
}

#[test]
fn callback_can_observe_a_nested_request_without_mixing_channels() {
    let outer = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let inner = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let outer_capture = outer.clone();
    let inner_capture = inner.clone();
    let response = crate::execute_json_with_events(
        r#"{"id":"outer-observer","command":"core.ping"}"#,
        Arc::new(move |event| {
            let event: serde_json::Value = serde_json::from_str(event).unwrap();
            outer_capture.lock().unwrap().push(event.clone());
            if event["type"] == "requestStarted" {
                let captured = inner_capture.clone();
                let response = crate::execute_json_with_events(
                    r#"{"id":"inner-observer","command":"core.ping"}"#,
                    Arc::new(move |event| {
                        captured
                            .lock()
                            .unwrap()
                            .push(serde_json::from_str(event).unwrap())
                    }),
                );
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&response).unwrap()["ok"],
                    true
                );
            }
        }),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap()["ok"],
        true
    );
    for (events, id) in [(outer, "outer-observer"), (inner, "inner-observer")] {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "requestStarted");
        assert_eq!(events[1]["type"], "requestFinished");
        assert!(events.iter().all(|event| event["operationId"] == id));
    }
}
