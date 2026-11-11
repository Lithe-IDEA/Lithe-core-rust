//! A scripted host for driving a backend session over in-process pipes.
//! Every wait is bounded, so a broken session fails with the frames it sent
//! instead of hanging.
#![allow(dead_code)]

use lithe_plugin_sdk::Backend;
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(10);

/// The host side of one session.
pub struct Host {
    input: Option<io::PipeWriter>,
    frames: mpsc::Receiver<Value>,
    exit: mpsc::Receiver<i32>,
}

impl Host {
    pub fn start(backend: Backend) -> Self {
        let (backend_input, input) = io::pipe().expect("input pipe");
        let (output, backend_output) = io::pipe().expect("output pipe");
        let (exit_sender, exit) = mpsc::channel();
        thread::spawn(move || {
            let _ = exit_sender.send(backend.serve(backend_input, backend_output));
        });
        let (frame_sender, frames) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(output);
            while let Some(frame) = read_frame(&mut reader) {
                if frame_sender.send(frame).is_err() {
                    return;
                }
            }
        });
        Self {
            input: Some(input),
            frames,
            exit,
        }
    }

    pub fn send_raw(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().expect("open input");
        input.write_all(bytes).expect("write to the backend");
    }

    pub fn send(&mut self, message: Value) {
        let body = message.to_string();
        self.send_raw(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    }

    pub fn request(&mut self, id: impl Into<Value>, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id.into(), "method": method, "params": params }));
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn next(&self) -> Value {
        self.frames
            .recv_timeout(WAIT)
            .expect("a frame from the backend in time")
    }

    /// The answer to request `id`, after any frames before it.
    pub fn answer(&self, id: impl Into<Value>) -> (Value, Vec<Value>) {
        let id = id.into();
        let mut before = Vec::new();
        loop {
            let frame = self.next();
            if frame["id"] == id && frame.get("method").is_none() {
                return (frame, before);
            }
            before.push(frame);
        }
    }

    pub fn initialize(&mut self, versions: Value) -> Value {
        let mut capabilities = if versions.is_null() {
            json!({})
        } else {
            json!({ "experimental": { "litheFunctions": { "versions": versions } } })
        };
        capabilities["experimental"]["litheLogging"] = json!({ "versions": [0] });
        self.request(
            0,
            "initialize",
            json!({ "rootUri": "file:///tmp/state/", "capabilities": capabilities }),
        );
        self.answer(0).0["result"].clone()
    }

    pub fn invoke(&mut self, id: i64, function: &str, params: Value) {
        self.request(
            id,
            "lithe/function/invoke",
            json!({ "functionID": function, "operationID": format!("op-{id}"), "params": params }),
        );
    }

    /// An invocation with extra top-level fields (`context`, `caller`, or a
    /// different `operationID`) merged over the ones `invoke` sends.
    pub fn invoke_with(&mut self, id: i64, function: &str, params: Value, extra: Value) {
        let mut call = json!({
            "functionID": function,
            "operationID": format!("op-{id}"),
            "params": params,
        });
        for (key, value) in extra.as_object().expect("extra fields are an object") {
            call[key] = value.clone();
        }
        self.request(id, "lithe/function/invoke", call);
    }

    /// Ends the input stream and returns the backend's exit code.
    pub fn close(mut self) -> i32 {
        self.input = None;
        self.exit.recv_timeout(WAIT).expect("the backend to end")
    }

    pub fn exit_code(&self) -> i32 {
        self.exit.recv_timeout(WAIT).expect("the backend to end")
    }

    /// The next frame, if the backend sends one within `timeout`.
    pub fn next_within(&self, timeout: Duration) -> Option<Value> {
        self.frames.recv_timeout(timeout).ok()
    }

    /// Fails when the backend sends a frame within a short wait.
    pub fn assert_quiet(&self) {
        if let Some(frame) = self.next_within(Duration::from_millis(200)) {
            panic!("unexpected frame: {frame}");
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> Option<Value> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse().ok();
        }
    }
    let mut body = vec![0; length?];
    reader.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}
