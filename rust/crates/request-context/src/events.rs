//! The ordered, request-scoped event channel to the host, and the redaction
//! every event passes through.
//!
//! Events contain sanitized diagnostics, never command input or environment.
//! A request with an event consumer (see [`with_sink`]) receives
//! `requestStarted` before its work and `requestFinished` after it; domains
//! [`emit`] structured events in between, and process tracers allocate one
//! invocation per spawned child (see [`next_invocation`]).
//!
//! Hosts keep a console journal of these events. The journal's record states
//! and retention budget are part of this contract: `requestStarted` carries
//! [`JOURNAL_RETENTION`], each invocation's `finished` carries the record's
//! final state, and `requestFinished` names the records that never confirmed
//! an exit and whether a failure needs a record of its own. Hosts store, lock,
//! and throttle; they do not derive states or redact again.

use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

/// Console journal retention shared by every host, in host string length units.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalRetention {
    /// Records kept across all requests; the oldest are dropped first.
    pub max_records: usize,
    /// Output kept per record; earlier lines are dropped first.
    pub max_record_output_characters: usize,
    /// Output lines kept per record, including empty lines.
    pub max_record_output_lines: usize,
    /// Output kept across all records before the oldest record is dropped.
    pub max_total_output_characters: usize,
}

/// One budget for both products, chosen for the lower resident memory.
pub const JOURNAL_RETENTION: JournalRetention = JournalRetention {
    max_records: 200,
    max_record_output_characters: 32_768,
    max_record_output_lines: 2_000,
    max_total_output_characters: 1_048_576,
};

/// A serialized event consumer, invoked synchronously during its owning request.
pub type EventSink = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// One ordered request boundary event.
struct Event<'a> {
    operation_id: &'a str,
    #[serde(flatten)]
    kind: Kind,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Kind {
    RequestStarted {
        retention: JournalRetention,
    },
    RequestFinished {
        error: Option<serde_json::Value>,
        /// Records of this request still running; hosts mark them `unconfirmed`.
        unconfirmed_invocations: Vec<u64>,
        /// No process of this request failed or is still running: none started, or
        /// each one succeeded, such as the queries before a refusal. The failure is shown
        /// as its own record instead of on an executed command.
        failure_record: bool,
    },
}

/// Per-request identity and sequence shared by nested invocations.
struct Context {
    sink: EventSink,
    operation_id: String,
    next_invocation: u64,
    /// Transfer summaries remain attached to the transfer despite subsequent inspection queries.
    last_transfer: Option<u64>,
    /// Processes of the current request that finished without succeeding.
    failed_invocations: usize,
    /// Started processes of the current request without a `finished` event.
    open_invocations: BTreeSet<u64>,
}
thread_local! { static CURRENT: RefCell<Option<Context>> = const { RefCell::new(None) }; }

/// Restores the caller's observer after a nested request or callback returns.
struct ObserverScope(Option<Context>);
impl ObserverScope {
    fn begin(context: Option<Context>) -> Self {
        Self(CURRENT.with(|current| current.replace(context)))
    }
}
impl Drop for ObserverScope {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.0.take()));
    }
}

/// Whether the current request has an incremental event consumer.
pub fn has_sink() -> bool {
    CURRENT.with(|current| current.borrow().is_some())
}

/// Installs a consumer only for the duration of this synchronous request.
pub fn with_sink<T>(sink: EventSink, operation: impl FnOnce() -> T) -> T {
    let _scope = ObserverScope::begin(Some(Context {
        sink,
        operation_id: String::new(),
        next_invocation: 0,
        last_transfer: None,
        failed_invocations: 0,
        open_invocations: Default::default(),
    }));
    operation()
}

/// Hands one serialized event to `sink` outside the observed request's scope.
pub fn deliver(sink: &EventSink, event: &str) {
    // A host may synchronously call git.authRespond from this callback. That
    // request must not inherit or overwrite the observed operation's identity.
    let _scope = ObserverScope::begin(None);
    sink(event);
}

/// Called after cancellation registration so immediate cancellation cannot race startup.
pub fn request_started(operation_id: Option<&str>) {
    let context = CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let context = current.as_mut()?;
        context.operation_id = operation_id.unwrap_or_default().to_string();
        context.failed_invocations = 0;
        context.open_invocations.clear();
        Some((context.sink.clone(), context.operation_id.clone()))
    });
    if let Some((sink, operation_id)) = context {
        send(
            &sink,
            &operation_id,
            Kind::RequestStarted {
                retention: JOURNAL_RETENTION,
            },
        );
    }
}

/// A final event shares the same ordered channel, including preflight failures.
pub fn request_finished(response: &str) {
    let context = CURRENT.with(|current| {
        current.borrow_mut().as_mut().map(|context| {
            (
                context.sink.clone(),
                context.operation_id.clone(),
                context.failed_invocations,
                std::mem::take(&mut context.open_invocations),
            )
        })
    });
    let Some((sink, operation_id, failed, open)) = context else {
        return;
    };
    let value: serde_json::Value = serde_json::from_str(response).unwrap_or_default();
    let raw_error = value
        .get("error")
        .filter(|value| !value.is_null())
        .or_else(|| {
            value
                .pointer("/data/operationError")
                .filter(|value| !value.is_null())
        });
    let error = raw_error.map(|error| serde_json::json!({
        "code": error.get("code"),
        "message": redact(error.get("message").and_then(|value| value.as_str()).unwrap_or("Git operation failed")),
        "details": error.get("details").and_then(|value| value.as_str()).map(redact),
    }));
    let error = error.or_else(|| value.pointer("/data/exitCode").and_then(|value| value.as_i64()).filter(|code| *code != 0).map(|code| serde_json::json!({ "code": "process_failed", "message": format!("Git exited with code {code}") })));
    // A request cancelled without a failed command was superseded; it has nothing
    // to report. Any other failure that no command reported needs its own record.
    let failure_record = failed == 0
        && open.is_empty()
        && error
            .as_ref()
            .is_some_and(|error| error["code"] != "cancelled");
    send(
        &sink,
        &operation_id,
        Kind::RequestFinished {
            error,
            unconfirmed_invocations: open.into_iter().collect(),
            failure_record,
        },
    );
}

fn send(sink: &EventSink, operation_id: &str, kind: Kind) {
    if let Ok(value) = serde_json::to_value(Event { operation_id, kind }) {
        deliver(sink, &value.to_string());
    }
}

/// The channel one traced process reports on: the request's consumer and
/// operation, and the invocation number unique within that request.
pub struct InvocationChannel {
    /// Consumer of the owning request.
    pub sink: EventSink,
    /// Operation identity of the owning request.
    pub operation_id: String,
    /// Invocation number, unique within the owning request.
    pub id: u64,
}

/// Allocates the next invocation of the current request; `None` when the
/// request has no event consumer.
pub fn next_invocation() -> Option<InvocationChannel> {
    CURRENT.with(|current| {
        let mut current = current.borrow_mut();
        let context = current.as_mut()?;
        context.next_invocation += 1;
        Some(InvocationChannel {
            sink: context.sink.clone(),
            operation_id: context.operation_id.clone(),
            id: context.next_invocation,
        })
    })
}

/// Records that invocation `id` started. A `transfer` (fetch, push, pull,
/// clone) also becomes the invocation later `remoteResult` events attach to.
pub fn invocation_started(id: u64, transfer: bool) {
    CURRENT.with(|current| {
        if let Some(context) = current.borrow_mut().as_mut() {
            if transfer {
                context.last_transfer = Some(id);
            }
            context.open_invocations.insert(id);
        }
    });
}

/// Records that invocation `id` finished, and whether it failed.
pub fn invocation_finished(id: u64, failed: bool) {
    CURRENT.with(|current| {
        if let Some(context) = current.borrow_mut().as_mut() {
            context.open_invocations.remove(&id);
            context.failed_invocations += usize::from(failed);
        }
    });
}

/// Sends structured native diagnostics on the same ordered request channel.
pub fn emit(mut value: serde_json::Value) {
    let context = CURRENT.with(|current| {
        current.borrow().as_ref().map(|context| {
            (
                context.sink.clone(),
                context.operation_id.clone(),
                context.last_transfer,
            )
        })
    });
    if let Some((sink, operation_id, transfer)) = context {
        if value["type"] == "remoteResult" {
            value["invocationId"] = transfer.into();
        }
        // Structured results can carry native error text; it is redacted here
        // like every other event so hosts never redact a second time.
        if let Some(error) = value
            .get_mut("error")
            .and_then(|error| error.as_object_mut())
        {
            for key in ["message", "details"] {
                if let Some(text) = error.get(key).and_then(|text| text.as_str()) {
                    let redacted = redact(text);
                    error.insert(key.into(), redacted.into());
                }
            }
        }
        value["operationId"] = operation_id.into();
        deliver(&sink, &value.to_string());
    }
}

/// Redacts complete records, including credentials split across native chunks.
pub fn redact(text: &str) -> String {
    static URL_AUTH: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = URL_AUTH.get_or_init(|| {
        regex::Regex::new(r"(?i)(https?|ssh)://[^\s/@]+(?::[^\s/@]*)?@")
            .expect("valid URL redaction")
    });
    let text = pattern.replace_all(text, "$1://redacted@").into_owned();
    lithe_diagnostics::redaction::redact_text(lithe_diagnostics::redaction::RedactTextRequest {
        text,
    })
    .redacted
}
