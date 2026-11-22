//! `remote.attachProcess` over a resumed persistent session: a scripted host
//! replays what its processes did while nobody was connected.

use lithe_remote::client::session::{ConnectOptions, ResumeMode};
use lithe_remote::client::RemoteSession;
use lithe_remote::protocol::frame::{read_frame, write_frame};
use lithe_remote::protocol::{methods, Message, PREAMBLE};
use lithe_workspace::sessions::History;
use lithe_workspace::test_support;
use serde_json::{json, Value};
use std::io::Write;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const STEP: Duration = Duration::from_secs(10);

/// Process 3 exited while the connection was down, 5 runs quietly and 6
/// ended before the events this Core received.
fn scripted_host(mut server: UnixStream, done: mpsc::Sender<()>) {
    let send = |server: &mut UnixStream, message: Message| {
        write_frame(server, &serde_json::to_vec(&message).unwrap()).unwrap();
        server.flush().unwrap();
    };
    let event = |event: &str, params: Value, seq: u64| Message::Event {
        event: event.into(),
        params,
        seq: Some(seq),
    };
    let mut states = 0;
    server.write_all(PREAMBLE).unwrap();
    while let Ok(Some(frame)) = read_frame(&mut server) {
        let Message::Request { id, method, params } = serde_json::from_slice(&frame).unwrap()
        else {
            continue;
        };
        let result = match method.as_str() {
            methods::SERVER_HELLO => json!({
                "protocolVersion": 1, "serverVersion": "test",
                "host": {"os": "linux", "arch": "x86_64", "hostName": "build.example",
                    "userName": "dev", "homeDirectory": "/home/dev", "defaultShell": "/bin/sh",
                    "pathSeparator": "/", "process": {"orphanProtection": "parentDeathSignal"},
                    "watch": {"backend": "inotify", "directoryLimit": 128}, "features": []},
                "session": {"persistent": true, "resumed": true, "nextResourceId": 7, "lastEventSeq": 6}
            }),
            methods::SERVER_RESUME => {
                assert_eq!(params["afterSeq"], 4);
                let data = methods::encode_bytes(b"while away\n");
                send(
                    &mut server,
                    event(
                        methods::EVENT_PROCESS_OUTPUT,
                        json!({"processId": 3, "stream": "stdout", "data": data}),
                        5,
                    ),
                );
                send(
                    &mut server,
                    event(
                        methods::EVENT_PROCESS_EXIT,
                        json!({"processId": 3, "exitCode": 0, "signal": null}),
                        6,
                    ),
                );
                json!({"truncated": false})
            }
            methods::SERVER_STATE => {
                states += 1;
                let processes = if states == 1 {
                    json!([{"processId": 5, "pid": 55, "argv": ["build"], "runningMs": 9,
                        "leaderExited": false}])
                } else {
                    json!([])
                };
                send(
                    &mut server,
                    Message::success(
                        id,
                        json!({"processes": processes, "terminals": [], "watches": []}),
                    ),
                );
                if states == 1 {
                    // The quiet process speaks and exits only after the attach
                    // checked that it still runs.
                    let data = methods::encode_bytes(b"live\n");
                    send(
                        &mut server,
                        event(
                            methods::EVENT_PROCESS_OUTPUT,
                            json!({"processId": 5, "stream": "stdout", "data": data}),
                            7,
                        ),
                    );
                    send(
                        &mut server,
                        event(
                            methods::EVENT_PROCESS_EXIT,
                            json!({"processId": 5, "exitCode": 2, "signal": null}),
                            8,
                        ),
                    );
                }
                continue;
            }
            // The attach owns the process: its end releases it as a run does.
            methods::PROCESS_KILL => Value::Null,
            methods::SERVER_SHUTDOWN => {
                send(&mut server, Message::success(id, Value::Null));
                break;
            }
            other => panic!("unexpected method {other}"),
        };
        send(&mut server, Message::success(id, result));
    }
    let _ = done.send(());
}

/// Runs `remote.attachProcess` and returns its response with the remote
/// events it relayed.
fn attach(session_id: &str, process_id: u64) -> (Value, Vec<Value>) {
    let relayed = Arc::new(Mutex::new(Vec::new()));
    let records = Arc::clone(&relayed);
    let response = crate::execute_json_with_events(
        &json!({"id": test_support::unique_id(), "command": "remote.attachProcess",
            "timeoutMilliseconds": 30_000,
            "payload": {"sessionId": session_id, "processId": process_id}})
        .to_string(),
        Arc::new(move |raw: &str| {
            let event: Value = serde_json::from_str(raw).unwrap();
            if event["type"] == "remote" {
                records.lock().unwrap().push(event);
            }
        }),
    );
    let events = relayed.lock().unwrap().clone();
    (serde_json::from_str(&response).unwrap(), events)
}

fn output(event: &Value) -> Vec<u8> {
    methods::decode_bytes(event["params"]["data"].as_str().unwrap()).unwrap()
}

#[test]
fn attaching_after_a_reconnection_relays_what_each_process_did_meanwhile() {
    let (client, server) = UnixStream::pair().unwrap();
    server.set_read_timeout(Some(STEP)).unwrap();
    server.set_write_timeout(Some(STEP)).unwrap();
    let socket = client.try_clone().unwrap();
    let (done, finished) = mpsc::channel();
    let host = std::thread::spawn(move || scripted_host(server, done));
    let (session, events) = RemoteSession::from_streams(
        client.try_clone().unwrap(),
        client,
        &ConnectOptions {
            handshake_timeout: STEP,
            resume: ResumeMode::Deferred,
            ..ConnectOptions::default()
        },
    )
    .unwrap();
    session.park("process");
    drop(events);
    assert!(!session.resume(Some(4)).unwrap().truncated);
    let id = test_support::register_in_process_with(
        session,
        History {
            resumed: true,
            complete: true,
        },
    );

    // Exited meanwhile: its output and exit, then the request ends.
    let (response, relayed) = attach(&id, 3);
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["data"]["exitCode"], 0, "{response}");
    let names: Vec<_> = relayed.iter().map(|event| &event["event"]).collect();
    assert_eq!(
        names,
        ["process.attached", "process.output", "process.exit"],
        "{relayed:?}"
    );
    assert_eq!(
        relayed[0]["params"],
        json!({"processId": 3, "truncated": false})
    );
    assert_eq!(output(&relayed[1]), b"while away\n");

    // Its attach ended, so nothing keeps its events any more.
    let (again, _) = attach(&id, 3);
    assert_eq!(again["error"]["code"], "remote_process_gone", "{again}");

    // Quiet since the reconnection: the host confirms it runs, then its
    // live output follows.
    let (response, relayed) = attach(&id, 5);
    assert_eq!(response["data"]["exitCode"], 2, "{response}");
    assert_eq!(relayed.len(), 3, "{relayed:?}");
    assert_eq!(relayed[0]["event"], "process.attached");
    assert_eq!(output(&relayed[1]), b"live\n");

    // Ended before what this Core received, and created by this connection.
    for process_id in [6, 7] {
        let (gone, relayed) = attach(&id, process_id);
        assert_eq!(gone["error"]["code"], "remote_process_gone", "{gone}");
        assert!(relayed.is_empty(), "{relayed:?}");
    }

    test_support::unregister(&id);
    let (unknown, _) = attach(&id, 5);
    assert_eq!(unknown["error"]["code"], "connection_closed", "{unknown}");
    finished.recv_timeout(STEP).expect("the host finished");
    let _ = socket.shutdown(Shutdown::Both);
    host.join().unwrap();
}

#[test]
fn a_connection_that_resumed_nothing_has_no_process_to_attach_to() {
    let (client, server) = UnixStream::pair().unwrap();
    server.set_read_timeout(Some(STEP)).unwrap();
    server.set_write_timeout(Some(STEP)).unwrap();
    let worker = std::thread::spawn(move || {
        let _ = lithe_remote::server::serve_stdio(server.try_clone().unwrap(), server);
    });
    let (session, events) = RemoteSession::from_streams(
        client.try_clone().unwrap(),
        client,
        &ConnectOptions {
            handshake_timeout: STEP,
            ..ConnectOptions::default()
        },
    )
    .unwrap();
    drop(events);
    let id = test_support::register_in_process(session);
    let (gone, _) = attach(&id, 1);
    assert_eq!(gone["error"]["code"], "remote_process_gone", "{gone}");
    test_support::unregister(&id);
    worker.join().unwrap();
}
