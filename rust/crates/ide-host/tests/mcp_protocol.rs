//! Exercises MCP discovery over the same stdio interface used by external clients.

use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{ChildStdin, ChildStdout, Command},
    time::timeout,
};

struct Client {
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    id: u64,
}

impl Client {
    async fn send(&mut self, message: Value) {
        self.input
            .write_all(format!("{message}\n").as_bytes())
            .await
            .expect("write MCP message");
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}))
            .await;
        let line = self.output.next_line().await.unwrap().expect("MCP reply");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.id, "{method}: {reply}");
        reply
            .get("result")
            .unwrap_or_else(|| panic!("{method}: {reply}"))
            .clone()
    }
}

fn tool_names(tools: &Value) -> Vec<&str> {
    let mut names: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    names
}

async fn check_session(mode: &'static str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/ide-api/mcp-protocol.json")).unwrap();
    assert!(fixture["modes"].as_array().unwrap().contains(&json!(mode)));
    let hosts: Value = serde_json::from_str(include_str!(
        "../../../fixtures/ide-api/host-tool-exposure.json"
    ))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let descriptor = directory.path().join("connection.json");
    let descriptor_path = descriptor.clone();
    let write_descriptor =
        move |exposure: &Value| {
            std::fs::write(&descriptor_path, json!({
                "endpoint": "http://127.0.0.1:1/call", "token": "fixture", "toolExposure": exposure
            }).to_string()).unwrap();
        };
    write_descriptor(&hosts["macos"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_lithe-mcp"))
        .arg("--connection")
        .arg(&descriptor)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start lithe-mcp");
    let mut client = Client {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()).lines(),
        id: 0,
    };

    // Keep ownership of the child outside the client task so even a failed
    // protocol assertion closes both pipes and reaps the helper.
    let exchange = tokio::spawn(timeout(Duration::from_secs(10), async move {
        let modern = &fixture["modernProtocolVersion"];
        if mode != "modern" {
            let version = if mode == "legacy" {
                &fixture["legacyProtocolVersion"]
            } else {
                modern
            };
            let initialized = client
                .request(
                    "initialize",
                    json!({
                        "protocolVersion": version, "capabilities": {},
                        "clientInfo": {"name": "fixture", "version": "1"}
                    }),
                )
                .await;
            assert_eq!(initialized["serverInfo"]["name"], "lithe");
            if mode == "legacy" {
                assert_eq!(&initialized["protocolVersion"], version);
            }
            client
                .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .await;
        }
        let mut params = json!({});
        if mode != "legacy" {
            params = json!({"_meta": {
                "io.modelcontextprotocol/protocolVersion": modern,
                "io.modelcontextprotocol/clientCapabilities": {}
            }});
            let discovery = client.request("server/discover", params.clone()).await;
            assert!(discovery["supportedVersions"]
                .as_array()
                .unwrap()
                .contains(modern));
        }
        for platform in ["macos", "windows"] {
            let exposure = &hosts[platform];
            write_descriptor(exposure);
            let list = client.request("tools/list", params.clone()).await;
            assert_eq!(list["ttlMs"], fixture["cachePolicy"]["ttlMs"]);
            assert_eq!(list["cacheScope"], fixture["cachePolicy"]["cacheScope"]);
            let names = tool_names(&list["tools"]);
            assert_eq!(names, tool_names(exposure), "{platform} tool exposure");
            for provider in fixture["providerTools"].as_array().unwrap() {
                assert!(
                    names.contains(&provider.as_str().unwrap()),
                    "missing {provider}"
                );
            }
        }
        // Both platforms currently expose the full catalog. A restricted
        // descriptor makes a cached or unfiltered tools/list observable.
        let restricted = json!([hosts["windows"][0]]);
        write_descriptor(&restricted);
        let list = client.request("tools/list", params).await;
        assert_eq!(tool_names(&list["tools"]), tool_names(&restricted));
    }))
    .await;
    timeout(Duration::from_secs(5), child.kill())
        .await
        .expect("reap lithe-mcp before deadline")
        .unwrap();
    exchange
        .expect("MCP client assertions")
        .expect("MCP session deadline");
}

#[tokio::test]
async fn legacy() {
    check_session("legacy").await;
}

#[tokio::test]
async fn modern() {
    check_session("modern").await;
}

#[tokio::test]
async fn modern_after_initialize() {
    check_session("modern-after-initialize").await;
}
