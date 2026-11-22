//! Workspace roots through the SDK, the Core relay and an empty transport chain.

use super::backend::{invoke_in, respond, take_host_request};
use super::{Fixture, Session, WAIT};
use lithe_remote::client::session::ConnectOptions;
use lithe_remote::client::{RemoteSession, TransportChain};
use serde_json::json;
use std::fs;

pub(super) fn run(backend: &mut Session, fixture: &Fixture) {
    let local_root = fixture.root.join("local-root");
    let remote_root = fixture.root.join("host-root");
    fs::create_dir(&local_root).expect("local workspace root");
    fs::create_dir(&remote_root).expect("remote workspace root");
    let marker = remote_root.join("marker");
    fs::write(&marker, "remote workspace").expect("remote workspace marker");

    let server = std::env::var("LITHE_PLUGIN_SDK_REMOTE_SERVER")
        .expect("run shared/plugins/scripts/sdk-e2e.mjs to build the remote server");
    let (remote, _events) = RemoteSession::connect(
        &TransportChain::local(),
        &[server, "--stdio".into()],
        &ConnectOptions {
            handshake_timeout: WAIT,
            ..ConnectOptions::default()
        },
    )
    .expect("empty-chain handshake");

    let local = json!({ "contextID": "workspace-local", "generation": 1 });
    invoke_in(backend, "local-roots", "workspace.processRoots", &local);
    let request = take_host_request(backend, "lithe/host/workspace/getContext");
    assert_eq!(request["result"]["params"], json!({ "context": local }));
    respond(
        backend,
        &request,
        Ok(json!({
            "contextID": local["contextID"],
            "generation": local["generation"],
            "name": "Local workspace",
            "authority": { "kind": "local" },
            "folders": [{ "rootID": "root", "name": "local", "path": local_root }]
        })),
    );
    let local_result = backend.completion("local-roots");
    assert_eq!(
        local_result["result"]["value"],
        json!([{ "rootID": "root", "path": local_root }]),
        "{local_result}"
    );

    let context = json!({ "contextID": "workspace-remote", "generation": 2 });
    invoke_in(backend, "host-roots", "workspace.processRoots", &context);
    let request = take_host_request(backend, "lithe/host/workspace/getContext");
    assert_eq!(request["result"]["params"], json!({ "context": context }));
    let arch = match remote.hello().host.arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        actual => panic!("unsupported fixture host architecture {actual}"),
    };
    respond(
        backend,
        &request,
        Ok(json!({
            "contextID": context["contextID"],
            "generation": context["generation"],
            "name": "Remote workspace",
            // The public workspace authority models Linux hosts. The same
            // empty-chain fixture is also runnable on a developer's macOS host.
            "authority": { "kind": "remote", "id": "remote-fixture", "platform": { "os": "linux", "arch": arch } },
            "folders": [{ "rootID": "root", "name": "remote" }]
        })),
    );
    let request = take_host_request(backend, "lithe/host/workspace/getHostPaths");
    assert_eq!(
        request["result"]["params"],
        json!({ "context": context }),
        "the SDK sent anything other than the context reference"
    );
    let (bytes, _) = remote
        .read_file(marker.to_str().expect("UTF-8 fixture path"))
        .expect("the remote root is reachable on the real session");
    assert_eq!(bytes, b"remote workspace");
    respond(
        backend,
        &request,
        Ok(json!({ "folders": [{ "rootID": "root", "hostPath": remote_root }] })),
    );
    let host_result = backend.completion("host-roots");
    assert_eq!(
        host_result["result"]["value"],
        json!([{ "rootID": "root", "hostPath": remote_root }]),
        "{host_result}"
    );
    assert!(
        !backend
            .events
            .iter()
            .any(|event| event["type"] == "hostRequest"
                && event["method"] == "lithe/host/workspace/getHostPaths"),
        "an extra host-path request escaped the workspace helper"
    );
    remote.shutdown(WAIT);
}
