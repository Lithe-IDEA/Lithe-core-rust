//! Git process invocations on the request's event channel: the `started`,
//! `output` and `finished` events of each traced child, with bounded,
//! incremental output decoding.
//!
//! The channel itself, its request boundary events and redaction live in
//! `lithe_request_context::events`. Native process ownership belongs to the
//! `lithe-git-host` adapter.

use crate::protocol::CoreError;
use lithe_request_context::events::{deliver, EventSink};
pub(crate) use lithe_request_context::events::{emit, has_sink, redact};
use serde::Serialize;
use std::cell::Cell;
use std::time::Instant;

const MAX_LINE_BYTES: usize = 16 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 512 * 1024;
const MAX_OUTPUT_EVENTS: usize = 4096;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// One ordered diagnostic event of a Git invocation.
struct Event<'a> {
    operation_id: &'a str,
    #[serde(flatten)]
    kind: Kind<'a>,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
/// Process start is evidence of a spawned child; failure may have no exit code.
enum Kind<'a> {
    Started {
        source: super::console::Source,
        executable: Option<String>,
        temporary_config: Vec<(String, String)>,
        invocation_id: u64,
        working_directory: &'a str,
        arguments: &'a [String],
        /// Native presentation folds global options without rewriting actual argv.
        display_arguments: Vec<String>,
        global_arguments: Vec<String>,
        /// An internal query reported through `reportQueries`, as opposed to a command the
        /// workflow runs visibly; consoles fold a query's output by default.
        query: bool,
    },
    Output {
        invocation_id: u64,
        stream: &'a str,
        text: String,
        progress: bool,
        truncated: bool,
    },
    Finished {
        /// An operation-defined nonzero result, such as a missing optional config key.
        expected_exit: bool,
        invocation_id: u64,
        exit_code: Option<i32>,
        duration_milliseconds: u128,
        error: Option<&'a CoreError>,
        /// `completed` with an exit code; `unconfirmed` when the exit was not observed.
        state: &'static str,
    },
}

/// A traced invocation. The byte decoders are independent for stdout/stderr.
pub(super) struct Invocation {
    sink: EventSink,
    operation_id: String,
    id: u64,
    clock: Instant,
    output_bytes: usize,
    output_events: usize,
    omitted: bool,
    stdout: LineDecoder,
    stderr: LineDecoder,
    pending_progress: Option<PendingProgress>,
    /// Broad config reads can contain arbitrary credentials and scripts. The command
    /// stays visible while stdout is replaced with one explicit redaction marker.
    redact_configuration: Cell<bool>,
    configuration_marker_sent: bool,
    /// The child is an internal query reported only because the request asked for queries.
    query: bool,
}
impl Invocation {
    /// The invocation that reports a child: visible commands always report, and internal
    /// queries only when the request set `reportQueries` and is not a background refresh.
    pub(super) fn reported(visible: bool) -> Option<Self> {
        use super::{console::Source, execution_policy};
        let query = || {
            execution_policy::current().report_queries
                && execution_policy::source() != Source::Background
        };
        let mut invocation = (visible || query()).then(Self::current).flatten()?;
        invocation.query = !visible;
        Some(invocation)
    }
    /// Whether an internal query's nonzero exit answered its question, such as a missing
    /// config key or an absent ref. A visible command's nonzero exit is always Git's failure.
    pub(super) fn answered(visible: bool, arguments: &[String], exit_code: Option<i32>) -> bool {
        !visible
            && exit_code.is_some_and(|code| {
                code != 0 && super::command_trace::exit_is_answer(arguments, code)
            })
    }
    pub(super) fn current() -> Option<Self> {
        lithe_request_context::events::next_invocation().map(|channel| Self {
            sink: channel.sink,
            operation_id: channel.operation_id,
            id: channel.id,
            clock: Instant::now(),
            output_bytes: 0,
            output_events: 0,
            omitted: false,
            stdout: LineDecoder::default(),
            stderr: LineDecoder::default(),
            pending_progress: None,
            redact_configuration: Cell::new(false),
            configuration_marker_sent: false,
            query: false,
        })
    }
    pub(super) fn started(&self, root: &str, arguments: &[String]) {
        let executable = lithe_git_host::configuration::executable(
            super::execution_policy::current().executable.as_deref(),
        )
        .map(|path| path.to_string_lossy().into_owned());
        self.started_on_host(root, arguments, executable);
    }

    /// Reports the executable actually selected by a workspace host.
    pub(super) fn started_on_host(
        &self,
        root: &str,
        arguments: &[String],
        executable: Option<String>,
    ) {
        let config = super::execution_policy::command_index(arguments)
            .is_some_and(|index| arguments[index] == "config");
        let safe_config = arguments
            .iter()
            .any(|arg| arg == "--get" || arg == "--get-all")
            && arguments.last().is_some_and(|key| {
                matches!(
                    key.as_str(),
                    "user.name"
                        | "user.email"
                        | "remote.pushDefault"
                        | "push.default"
                        | "pull.rebase"
                        | "pull.ff"
                ) || key.starts_with("lithe.fetch.")
                    || (key.starts_with("remote.")
                        && (key.ends_with(".url") || key.ends_with(".skipFetchAll")))
                    || (key.starts_with("branch.")
                        && (key.ends_with(".remote")
                            || key.ends_with(".merge")
                            || key.ends_with(".pushRemote")))
            });
        self.redact_configuration.set(config && !safe_config);
        let transfer = super::execution_policy::command_index(arguments).is_some_and(|index| {
            matches!(
                arguments[index].as_str(),
                "fetch" | "push" | "pull" | "clone"
            )
        });
        lithe_request_context::events::invocation_started(self.id, transfer);
        let arguments = arguments
            .iter()
            .map(|argument| redact(argument))
            .collect::<Vec<_>>();
        let temporary_config = if super::execution_policy::command_index(&arguments)
            .is_some_and(|index| arguments[index] != "config")
        {
            super::execution_policy::temporary_config()
        } else {
            Vec::new()
        };
        let (display_arguments, global_arguments) =
            super::execution_policy::console_arguments(&arguments, &temporary_config);
        send(
            &self.sink,
            &self.operation_id,
            Kind::Started {
                source: super::execution_policy::current().source,
                executable,
                temporary_config,
                display_arguments,
                global_arguments,
                invocation_id: self.id,
                working_directory: root,
                arguments: &arguments,
                query: self.query,
            },
        );
    }
    pub(super) fn output(&mut self, stream: lithe_git_host::Stream, bytes: &[u8]) {
        if self.redact_configuration.get() && matches!(stream, lithe_git_host::Stream::Stdout) {
            if !bytes.is_empty() && !self.configuration_marker_sent {
                self.configuration_marker_sent = true;
                emit_output(&self.sink, &self.operation_id, self.id, &mut self.pending_progress,
                    "stdout", "[Git configuration values redacted; inspect Git settings for supported values]".into(), false, false);
            }
            return;
        }
        if self.omitted {
            return;
        }
        self.output_bytes += bytes.len();
        if self.output_bytes > MAX_DIAGNOSTIC_BYTES {
            self.omitted = true;
            self.stdout = LineDecoder::default();
            self.stderr = LineDecoder::default();
            emit_output(
                &self.sink,
                &self.operation_id,
                self.id,
                &mut self.pending_progress,
                "stderr",
                "[Further Git output omitted: diagnostic limit]".into(),
                false,
                true,
            );
            return;
        }
        let (decoder, name) = match stream {
            lithe_git_host::Stream::Stdout => (&mut self.stdout, "stdout"),
            lithe_git_host::Stream::Stderr => (&mut self.stderr, "stderr"),
        };
        let sink = &self.sink;
        let operation_id = &self.operation_id;
        let id = self.id;
        let output_events = &mut self.output_events;
        let omitted = &mut self.omitted;
        let pending_progress = &mut self.pending_progress;
        decoder.push(bytes, &mut |text, progress, truncated| {
            if *omitted {
                return;
            }
            *output_events += 1;
            if *output_events > MAX_OUTPUT_EVENTS {
                *omitted = true;
                emit_output(
                    sink,
                    operation_id,
                    id,
                    pending_progress,
                    name,
                    "[Further Git output omitted: diagnostic limit]".into(),
                    false,
                    true,
                );
                return;
            }
            emit_output(
                sink,
                operation_id,
                id,
                pending_progress,
                name,
                text,
                progress,
                truncated,
            );
        });
    }
    #[cfg(test)]
    pub(super) fn finished(self, exit_code: Option<i32>, error: Option<&CoreError>) {
        self.finished_with_expected_exit(exit_code, error, false);
    }
    pub(super) fn finished_with_expected_exit(
        mut self,
        exit_code: Option<i32>,
        error: Option<&CoreError>,
        expected_exit: bool,
    ) {
        if self.omitted {
            self.stdout = LineDecoder::default();
            self.stderr = LineDecoder::default();
        }
        for (name, decoder) in [("stdout", &mut self.stdout), ("stderr", &mut self.stderr)] {
            decoder.flush(&mut |text, progress, truncated| {
                emit_output(
                    &self.sink,
                    &self.operation_id,
                    self.id,
                    &mut self.pending_progress,
                    name,
                    text,
                    progress,
                    truncated,
                )
            });
        }
        if let Some(pending) = self.pending_progress.take() {
            send(
                &self.sink,
                &self.operation_id,
                Kind::Output {
                    invocation_id: self.id,
                    stream: pending.stream,
                    text: pending.text,
                    progress: false,
                    truncated: false,
                },
            );
        }
        // Errors may include native details. They use the same redaction as output.
        let error = error.map(|error| CoreError {
            code: error.code.clone(),
            message: redact(&error.message),
            details: error.details.as_ref().map(|value| redact(value)),
        });
        let failed = error.is_some() || !(exit_code == Some(0) || expected_exit);
        lithe_request_context::events::invocation_finished(self.id, failed);
        send(
            &self.sink,
            &self.operation_id,
            Kind::Finished {
                expected_exit,
                invocation_id: self.id,
                exit_code,
                duration_milliseconds: self.clock.elapsed().as_millis(),
                error: error.as_ref(),
                state: if exit_code.is_some() {
                    "completed"
                } else {
                    "unconfirmed"
                },
            },
        );
    }
}

/// The current phase is revised in place; its last retained line is finalized on
/// a phase transition or process completion, including a CR-only final update.
struct PendingProgress {
    stream: &'static str,
    text: String,
    stage: String,
}
fn emit_output(
    sink: &EventSink,
    operation_id: &str,
    id: u64,
    pending: &mut Option<PendingProgress>,
    stream: &'static str,
    text: String,
    progress: bool,
    truncated: bool,
) {
    let stage = super::progress::parse(&text)
        .map(|p| p.stage.to_string())
        .or_else(|| text.split_once(':').map(|(prefix, _)| prefix.to_string()))
        .unwrap_or_default();
    if let Some(previous) = pending.take() {
        let changed_stage = previous.stage != stage || previous.stream != stream;
        if (changed_stage || (!progress && stage.is_empty())) && previous.text != text {
            send(
                sink,
                operation_id,
                Kind::Output {
                    invocation_id: id,
                    stream: previous.stream,
                    text: previous.text,
                    progress: false,
                    truncated: false,
                },
            );
        }
    }
    if progress && !truncated {
        *pending = Some(PendingProgress {
            stream,
            text: text.clone(),
            stage,
        });
    }
    send(
        sink,
        operation_id,
        Kind::Output {
            invocation_id: id,
            stream,
            text,
            progress,
            truncated,
        },
    );
}

fn send(sink: &EventSink, operation_id: &str, kind: Kind<'_>) {
    if let Ok(mut value) = serde_json::to_value(Event { operation_id, kind }) {
        if value["type"] == "output" {
            if let Some(progress) = value["text"].as_str().and_then(super::progress::parse) {
                value["progressDetails"] = serde_json::to_value(progress).unwrap_or_default();
            }
        }
        deliver(sink, &value.to_string());
    }
}

#[derive(Default)]
/// Complete-line decoding avoids exposing a secret before its delimiter arrives.
/// Overlong lines are discarded in full rather than publishing a secret prefix.
/// NUL ends a line like LF: `-z` output separates its records with NUL.
struct LineDecoder {
    bytes: Vec<u8>,
    cr: bool,
    overflow: bool,
}
impl LineDecoder {
    fn push(&mut self, bytes: &[u8], emit: &mut impl FnMut(String, bool, bool)) {
        for &byte in bytes {
            if self.cr {
                self.emit(byte != b'\n', emit);
                self.cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\r' => self.cr = true,
                b'\n' | 0 => self.emit(false, emit),
                _ if self.bytes.len() < MAX_LINE_BYTES && !self.overflow => self.bytes.push(byte),
                _ => {
                    self.bytes.clear();
                    self.overflow = true;
                }
            }
        }
    }
    fn emit(&mut self, progress: bool, emit: &mut impl FnMut(String, bool, bool)) {
        let text = if self.overflow {
            "[Git output line omitted: size limit]".to_string()
        } else {
            redact(&String::from_utf8_lossy(&self.bytes))
        };
        emit(text, progress, self.overflow);
        self.bytes.clear();
        self.overflow = false;
    }
    fn flush(&mut self, emit: &mut impl FnMut(String, bool, bool)) {
        if !self.bytes.is_empty() || self.overflow || self.cr {
            self.emit(self.cr, emit);
        }
        self.cr = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lithe_request_context::events::{request_finished, request_started, with_sink};
    use std::sync::Arc;

    /// A query's nonzero exit that answers it is expected; the same exit of a visible
    /// command, or an exit Git means as failure, is not.
    #[test]
    fn only_answering_query_exits_are_expected() {
        let answered = |visible, arguments: &[&str], code| {
            let arguments: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
            Invocation::answered(visible, &arguments, Some(code))
        };
        assert!(answered(false, &["config", "--get", "user.signingkey"], 1));
        assert!(answered(
            false,
            &["rev-parse", "--verify", "--quiet", "refs/stash"],
            1
        ));
        assert!(!answered(false, &["status", "--porcelain=v2"], 128));
        assert!(!answered(false, &["config", "--get", "user.name"], 0));
        assert!(!answered(true, &["config", "--get", "user.name"], 1));
    }

    #[test]
    fn nul_separates_lines() {
        let mut decoder = LineDecoder::default();
        let mut lines = Vec::new();
        decoder.push(b"? first.txt\0? second", &mut |text, _, _| lines.push(text));
        decoder.flush(&mut |text, _, _| lines.push(text));
        assert_eq!(lines, ["? first.txt", "? second"]);
    }

    #[test]
    fn emitted_events_match_the_shared_fixture_and_observer_scope_is_released() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let sink_capture = captured.clone();
        with_sink(
            Arc::new(move |json| {
                sink_capture
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(json).unwrap())
            }),
            || {
                request_started(Some("fixture"));
                let mut invocation = Invocation::current().unwrap();
                invocation.started("/workspace", &["fetch".into(), "--progress".into()]);
                invocation.output(
                    lithe_git_host::Stream::Stderr,
                    b"Receiving: 50%\rReceiving: 100%\n",
                );
                invocation.output(lithe_git_host::Stream::Stdout, b"reference updated\n");
                invocation.finished(Some(0), None);
                request_finished(r#"{"ok":true,"data":{"exitCode":0}}"#);
            },
        );
        assert!(Invocation::current().is_none());
        let mut events = captured.lock().unwrap().clone();
        for event in &mut events {
            if event["type"] == "started" {
                event["executable"] = serde_json::Value::Null;
            }
            if event.get("durationMilliseconds").is_some() {
                event["durationMilliseconds"] = serde_json::json!(0);
            }
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/git/execution-events-v1.json"
        ))
        .unwrap();
        assert_eq!(serde_json::json!(events), fixture["events"]);
    }

    fn capture_request(operation: impl FnOnce()) -> Vec<serde_json::Value> {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let sink_capture = captured.clone();
        with_sink(
            Arc::new(move |json| {
                sink_capture
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(json).unwrap())
            }),
            operation,
        );
        let events = captured.lock().unwrap().clone();
        events
    }

    /// Hosts no longer infer record states; a process whose exit was never
    /// observed must be named by Core or it would stay "running" forever.
    /// A refusal after queries that all succeeded is its own record; a failure after a
    /// failed command belongs to that command.
    #[test]
    fn request_failure_after_successful_queries_is_its_own_record() {
        let error = r#"{"ok":false,"error":{"code":"invalid_request","message":"locked"}}"#;
        let events = capture_request(|| {
            request_started(Some("refused"));
            let query = Invocation::current().unwrap();
            query.started("/workspace", &["worktree".into(), "list".into()]);
            query.finished(Some(0), None);
            request_finished(error);
            request_started(Some("failed"));
            let command = Invocation::current().unwrap();
            command.started("/workspace", &["worktree".into(), "remove".into()]);
            command.finished(Some(128), None);
            request_finished(error);
        });
        let records: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "requestFinished")
            .map(|event| event["failureRecord"].clone())
            .collect();
        assert_eq!(records, [true, false]);
    }

    #[test]
    fn reported_queries_are_marked_apart_from_visible_commands() {
        let options = crate::git::execution_policy::GitExecutionOptions {
            report_queries: true,
            ..Default::default()
        };
        let _policy = crate::git::execution_policy::Scope::begin(Some(options)).unwrap();
        let events = capture_request(|| {
            request_started(Some("marked"));
            let query = Invocation::reported(false).unwrap();
            query.started("/workspace", &["log".into()]);
            query.finished(Some(0), None);
            let command = Invocation::reported(true).unwrap();
            command.started("/workspace", &["commit".into()]);
            command.finished(Some(0), None);
            request_finished(r#"{"ok":true,"data":{}}"#);
        });
        let marks: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "started")
            .map(|event| (event["arguments"][0].clone(), event["query"].clone()))
            .collect();
        assert_eq!(
            marks,
            [
                (serde_json::json!("log"), serde_json::json!(true)),
                (serde_json::json!("commit"), serde_json::json!(false))
            ]
        );
    }

    #[test]
    fn request_finished_names_unconfirmed_records_and_early_failures() {
        let events = capture_request(|| {
            request_started(Some("lost"));
            let invocation = Invocation::current().unwrap();
            invocation.started("/workspace", &["fetch".into()]);
            drop(invocation);
            request_finished(r#"{"ok":false,"error":{"code":"timed_out","message":"late"}}"#);
            request_started(Some("preflight"));
            request_finished(
                r#"{"ok":false,"error":{"code":"invalid_request","message":"https://u:p@h/x?client_secret=s"}}"#,
            );
            request_started(Some("superseded"));
            request_finished(r#"{"ok":false,"error":{"code":"cancelled","message":"Cancelled"}}"#);
        });
        let finished = events
            .iter()
            .filter(|event| event["type"] == "requestFinished")
            .collect::<Vec<_>>();
        assert_eq!(
            finished[0]["unconfirmedInvocations"],
            serde_json::json!([1])
        );
        assert_eq!(finished[0]["failureRecord"], false);
        assert_eq!(finished[1]["unconfirmedInvocations"], serde_json::json!([]));
        assert_eq!(finished[1]["failureRecord"], true);
        assert_eq!(
            finished[1]["error"]["message"],
            "https://redacted@h/x?client_secret=<redacted>"
        );
        assert_eq!(finished[2]["failureRecord"], false);
        let started = events
            .iter()
            .find(|event| event["type"] == "requestStarted")
            .unwrap();
        assert_eq!(started["retention"]["maxRecordOutputCharacters"], 32_768);
    }

    #[test]
    fn diagnostic_budget_keeps_the_final_event_after_output_is_omitted() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let sink_capture = captured.clone();
        with_sink(
            Arc::new(move |json| {
                sink_capture
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(json).unwrap())
            }),
            || {
                request_started(Some("budget"));
                let mut invocation = Invocation::current().unwrap();
                invocation.started("/workspace", &["fetch".into()]);
                invocation.output(
                    lithe_git_host::Stream::Stdout,
                    &vec![b'\n'; MAX_OUTPUT_EVENTS + 100],
                );
                invocation.output(lithe_git_host::Stream::Stdout, b"must stay omitted\n");
                invocation.finished(Some(0), None);
            },
        );
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), MAX_OUTPUT_EVENTS + 4);
        assert_eq!(events[events.len() - 2]["truncated"], true);
        assert_eq!(events.last().unwrap()["type"], "finished");
    }

    #[test]
    fn progress_keeps_each_final_phase_and_transfer_identity_across_queries() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let sink_capture = captured.clone();
        with_sink(
            Arc::new(move |json| {
                sink_capture
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(json).unwrap())
            }),
            || {
                request_started(Some("phases"));
                let mut transfer = Invocation::current().unwrap();
                transfer.started("/repo", &["fetch".into(), "origin".into()]);
                for byte in b"Receiving objects: 50% (1/2)\rReceiving objects: 100% (2/2)\rResolving deltas: 50% (1/2)\rResolving deltas: 100% (2/2)\r" {
                transfer.output(lithe_git_host::Stream::Stderr, &[*byte]);
            }
                transfer.finished(Some(0), None);
                let query = Invocation::current().unwrap();
                query.started("/repo", &["for-each-ref".into()]);
                query.finished(Some(0), None);
                emit(
                    serde_json::json!({"type":"remoteResult", "remote":"origin", "succeeded":true}),
                );
            },
        );
        let events = captured.lock().unwrap();
        let settled = events
            .iter()
            .filter(|event| event["type"] == "output" && event["progress"] == false)
            .map(|event| event["text"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            settled,
            [
                "Receiving objects: 100% (2/2)",
                "Resolving deltas: 100% (2/2)"
            ]
        );
        assert_eq!(events.last().unwrap()["invocationId"], 1);
    }

    #[test]
    fn broad_configuration_queries_are_visible_without_leaking_unrelated_secrets() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let sink_capture = captured.clone();
        with_sink(
            Arc::new(move |json| {
                sink_capture
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(json).unwrap())
            }),
            || {
                request_started(Some("config"));
                let mut query = Invocation::current().unwrap();
                query.started(
                    "/repo",
                    &["config".into(), "--null".into(), "--list".into()],
                );
                query.output(
                    lithe_git_host::Stream::Stdout,
                    b"custom.key\nfixture-private-value\0",
                );
                query.output(
                    lithe_git_host::Stream::Stdout,
                    b"credential.helper\n!echo fixture-helper-secret\0",
                );
                query.finished(Some(0), None);
            },
        );
        let events = captured.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "started")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "output")
                .count(),
            1
        );
        let text = serde_json::to_string(&*events).unwrap();
        assert!(text.contains("values redacted"));
        assert!(!text.contains("fixture-private-value") && !text.contains("fixture-helper-secret"));
    }

    #[test]
    fn output_handles_split_utf8_crlf_progress_and_credentials() {
        let mut decoder = LineDecoder::default();
        let mut lines = Vec::new();
        let mut emit = |text, progress, truncated| lines.push((text, progress, truncated));
        for byte in "获取\r\nReceiving: 10%\rReceiving: 20%\nhttps://user:secret@example.invalid/repo?token=fake\nlast".as_bytes() {
            decoder.push(&[*byte], &mut emit);
        }
        decoder.flush(&mut emit);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], ("获取".into(), false, false));
        assert_eq!(lines[1], ("Receiving: 10%".into(), true, false));
        assert!(!lines[3].0.contains("secret"));
        assert!(!lines[3].0.contains("fake"));
        assert_eq!(lines[4].0, "last");
    }
    #[test]
    fn overlong_output_is_bounded_and_recovers_at_next_line() {
        let mut decoder = LineDecoder::default();
        let mut lines = Vec::new();
        let mut emit = |text, progress, truncated| lines.push((text, progress, truncated));
        decoder.push(&vec![b'x'; MAX_LINE_BYTES * 3], &mut emit);
        assert!(decoder.bytes.is_empty());
        decoder.push(b"\nsafe\n", &mut emit);
        assert!(lines[0].2);
        assert_eq!(lines[1].0, "safe");
    }
}
