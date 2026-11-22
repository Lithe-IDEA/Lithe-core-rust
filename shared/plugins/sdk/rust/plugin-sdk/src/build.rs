//! Build providers: the typed `lithe/build/*` requests a plugin answers for
//! its build system, negotiated as `litheBuild` in `initialize`. The SDK
//! negotiates the version, checks each request's identity, sends progress,
//! and checks the shape of each result; the plugin supplies the build
//! semantics.
//!
//! Contract: `contract/v1/README.md` ("Build providers", "Provider task
//! execution", "Provider test execution", "Run preparation", "Native debug
//! preparation", and "Run entrypoints").

use crate::backend::{BackendHandle, Cancellation, Context};
use crate::diagnostics::Diagnostics;
use crate::error::Cancelled;
use crate::host::Host;
use crate::protocol::{bounded_text, methods, MAX_BUILD_MESSAGE_BYTES, MAX_BUILD_OUTPUT_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

/// The `version` every build result document carries.
const DOCUMENT_VERSION: u64 = 0;

/// The host that starts a contributed process. Omit the field to use the workspace host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessPlacement {
    /// The machine that owns the workspace.
    Workspace,
    /// The machine running Lithe.
    Ui,
}

/// Requests that run the build tool. Core gives each a wire `operationID`,
/// and only they report `lithe/build/progress`.
pub(crate) fn reports_progress(method: &str) -> bool {
    matches!(
        method,
        methods::BUILD_EXECUTE
            | methods::BUILD_TEST
            | methods::BUILD_PREPARE_RUN
            | methods::BUILD_PREPARE_DEBUG
    )
}

/// The `litheBuild` version that introduced `method`; a session that
/// negotiated an older one does not route it.
pub(crate) fn introduced_in(method: &str) -> u64 {
    u64::from(method == methods::BUILD_RUN_ENTRYPOINTS)
}

/// The canonical format of a typed result document.
fn document_format(method: &str) -> Option<&'static str> {
    match method {
        methods::BUILD_MODEL => Some("litheBuildModel"),
        methods::BUILD_DEPENDENCIES => Some("litheDependencies"),
        methods::BUILD_DISCOVER_TESTS => Some("litheTestDiscovery"),
        methods::BUILD_RUN_ENTRYPOINTS => Some("litheRunEntrypoints"),
        _ => None,
    }
}

/// The result the SDK answers for a handler's `value`, or `None` when its
/// shape is not the one the request needs. A document gets its canonical
/// `format` and `version`, so a handler can return the plain document. Core
/// would refuse any other shape as `plugin.build.invalidResult` or
/// `plugin.build.invalidOutput`, so the SDK fails the call with a controlled
/// diagnostic instead. Workspace containment and domain facts stay with Core
/// and the provider; the size limit is checked with the answer.
pub(crate) fn result(method: &str, value: Value) -> Option<Value> {
    let Value::Object(mut fields) = value else {
        return None;
    };
    if let Some(format) = document_format(method) {
        if fields
            .get("format")
            .is_some_and(|value| value.as_str() != Some(format))
            || fields
                .get("version")
                .is_some_and(|value| value.as_u64() != Some(DOCUMENT_VERSION))
        {
            return None;
        }
        fields.insert("format".into(), json!(format));
        fields.insert("version".into(), json!(DOCUMENT_VERSION));
    }
    let strings = |value: Option<&Value>| {
        value
            .and_then(Value::as_array)
            .is_some_and(|values| values.iter().all(Value::is_string))
    };
    let environment = |value: Option<&Value>| {
        value
            .and_then(Value::as_object)
            .is_some_and(|values| values.values().all(Value::is_string))
    };
    let valid = match method {
        methods::BUILD_EXECUTE | methods::BUILD_TEST => {
            fields
                .get("exitCode")
                .and_then(Value::as_i64)
                .is_some_and(|code| i32::try_from(code).is_ok())
                && fields.get("output").is_some_and(Value::is_string)
        }
        methods::BUILD_PREPARE_RUN => {
            fields.get("executable").is_some_and(|executable| {
                matches!(
                    executable["kind"].as_str(),
                    Some("toolchain") | Some("command")
                ) && executable["value"].is_string()
            }) && fields.get("workingDirectory").is_some_and(Value::is_string)
                && strings(fields.get("arguments"))
                && environment(fields.get("environment"))
                && fields
                    .get("placement")
                    .is_none_or(|value| ProcessPlacement::deserialize(value).is_ok())
        }
        // Native target facts only: the adapter and its DAP arguments come
        // from the manifest.
        methods::BUILD_PREPARE_DEBUG => {
            fields.get("program").is_some_and(Value::is_string)
                && fields.get("workingDirectory").is_some_and(Value::is_string)
                && strings(fields.get("arguments"))
                && environment(fields.get("environment"))
                && fields.keys().all(|key| {
                    ["program", "arguments", "workingDirectory", "environment"]
                        .contains(&key.as_str())
                })
        }
        _ => true,
    };
    valid.then_some(Value::Object(fields))
}

/// One running `lithe/build/*` request.
///
/// Every request runs on a thread of its own, so a handler may block on the
/// build tool; it should stop the tool and return promptly once
/// [`cancellation`](Self::cancellation) fires.
pub struct BuildContext {
    pub(crate) context: Context,
    reports_progress: bool,
}

impl BuildContext {
    pub(crate) fn new(context: Context, method: &str) -> Self {
        Self {
            context,
            reports_progress: reports_progress(method),
        }
    }

    /// The wire method, such as `lithe/build/model`.
    pub fn method(&self) -> &str {
        self.context.function_id()
    }

    /// The Core-assigned wire `operationID` of a request that runs the build
    /// tool (`execute`, `test`, `prepareRun`, `prepareDebug`); the SDK echoes
    /// it in every progress report.
    pub fn operation_id(&self) -> Option<&str> {
        self.context.operation_id.as_str()
    }

    /// Fires when the host cancels the request or shuts the backend down.
    /// Core has already ended the request by then.
    pub fn cancellation(&self) -> &Cancellation {
        self.context.cancellation()
    }

    /// `Err(Cancelled)` once the request is cancelled, for use with `?`.
    pub fn check_cancelled(&self) -> Result<(), Cancelled> {
        self.context.check_cancelled()
    }

    /// Structured diagnostics correlated with the request's operation.
    pub fn diagnostics(&self) -> Diagnostics {
        self.context.diagnostics()
    }

    /// Omits only this request's successful terminal fact; failures remain
    /// visible.
    pub fn suppress_success_diagnostics(&self) {
        self.context.suppress_success_diagnostics();
    }

    /// Reports what the build is doing; the host shows the latest report.
    /// `message` is cut to 4 KiB and `percentage` to 100.
    ///
    /// Only requests that run the build tool report progress; for the others
    /// (`model`, `dependencies`, `discoverTests`, `runEntrypoints`), and after
    /// the request ended or was cancelled, a report is dropped.
    pub fn progress(&self, message: &str, percentage: Option<u8>) {
        let mut report = json!({ "message": bounded_text(message, MAX_BUILD_MESSAGE_BYTES) });
        if let Some(percentage) = percentage {
            report["percentage"] = json!(percentage.min(100));
        }
        self.report(report);
    }

    /// Streams build tool output to the host's console as it arrives. Text
    /// longer than one report's 64 KiB is sent in several reports, split at
    /// character boundaries. The final result's `output` must still carry the
    /// complete text: the host decodes diagnostics from it without showing it
    /// twice. Dropped like [`progress`](Self::progress).
    pub fn output(&self, text: &str) {
        let mut rest = text;
        while !rest.is_empty() {
            let chunk = bounded_text(rest, MAX_BUILD_OUTPUT_BYTES);
            rest = &rest[chunk.len()..];
            self.report(json!({ "output": chunk }));
        }
    }

    fn report(&self, mut report: Value) {
        if !self.reports_progress
            || !self.context.reporting.load(Ordering::SeqCst)
            || self.context.cancellation().is_cancelled()
        {
            return;
        }
        report["operationID"] = self.context.operation_id.clone();
        self.context.shared.write(
            &json!({ "jsonrpc": "2.0", "method": methods::BUILD_PROGRESS, "params": report }),
        );
    }

    /// Host services for this request, bound to its cancellation. They need
    /// a host that also negotiated plugin functions with this process.
    pub fn host(&self) -> &Host {
        self.context.host()
    }

    pub fn backend(&self) -> BackendHandle {
        self.context.backend()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_get_their_canonical_format_and_version() {
        let model = result(methods::BUILD_MODEL, json!({ "projects": [] })).unwrap();
        assert_eq!(model["format"], "litheBuildModel");
        assert_eq!(model["version"], 0);
        let stamped = json!({ "format": "litheRunEntrypoints", "version": 0, "entrypoints": [] });
        assert_eq!(
            result(methods::BUILD_RUN_ENTRYPOINTS, stamped.clone()),
            Some(stamped)
        );
        assert!(result(
            methods::BUILD_MODEL,
            json!({ "format": "litheDependencies" })
        )
        .is_none());
        assert!(result(methods::BUILD_DEPENDENCIES, json!({ "version": 1 })).is_none());
        assert!(result(methods::BUILD_DISCOVER_TESTS, json!([])).is_none());
    }

    #[test]
    fn task_results_need_an_int32_exit_code_and_text_output() {
        let ok = json!({ "exitCode": -1, "output": "" });
        assert_eq!(result(methods::BUILD_EXECUTE, ok.clone()), Some(ok));
        for invalid in [
            json!({ "exitCode": 2_147_483_648_i64, "output": "" }),
            json!({ "exitCode": 1.5, "output": "" }),
            json!({ "exitCode": 0 }),
            json!({ "exitCode": 0, "output": ["text"] }),
        ] {
            assert!(
                result(methods::BUILD_TEST, invalid.clone()).is_none(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn launch_plans_and_debug_targets_need_their_portable_fields() {
        let plan = json!({ "executable": { "kind": "command", "value": "app" },
            "arguments": ["a"], "workingDirectory": "app", "environment": { "A": "1" } });
        assert!(result(methods::BUILD_PREPARE_RUN, plan.clone()).is_some());
        for (placement, wire) in [
            (ProcessPlacement::Workspace, "workspace"),
            (ProcessPlacement::Ui, "ui"),
        ] {
            let mut placed = plan.clone();
            placed["placement"] = json!(placement);
            assert_eq!(placed["placement"], wire);
            assert_eq!(
                result(methods::BUILD_PREPARE_RUN, placed.clone()),
                Some(placed)
            );
        }
        for placement in [json!("remote"), Value::Null] {
            let mut invalid = plan.clone();
            invalid["placement"] = placement;
            assert!(result(methods::BUILD_PREPARE_RUN, invalid).is_none());
        }
        let mut other = plan.clone();
        other["executable"]["kind"] = json!("shell");
        assert!(result(methods::BUILD_PREPARE_RUN, other).is_none());
        let mut environment = plan;
        environment["environment"]["A"] = json!(1);
        assert!(result(methods::BUILD_PREPARE_RUN, environment).is_none());

        let target = json!({ "program": "app/bin", "arguments": [], "workingDirectory": "app", "environment": {} });
        assert!(result(methods::BUILD_PREPARE_DEBUG, target.clone()).is_some());
        let mut adapter = target;
        adapter["adapter"] = json!("native");
        assert!(result(methods::BUILD_PREPARE_DEBUG, adapter).is_none());
    }

    #[test]
    fn only_tool_runs_report_progress() {
        assert!(reports_progress(methods::BUILD_PREPARE_DEBUG));
        assert!(!reports_progress(methods::BUILD_MODEL));
        assert_eq!(introduced_in(methods::BUILD_RUN_ENTRYPOINTS), 1);
        assert_eq!(introduced_in(methods::BUILD_MODEL), 0);
    }
}
