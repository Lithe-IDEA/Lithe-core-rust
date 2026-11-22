//! DAP owns these fields as paths. Adapter-specific values and textual
//! diagnostics remain in the adapter's namespace.

use super::*;

/// Sends document paths to an adapter on the workspace host. Native host
/// paths already supplied by the adapter or plugin stay in their namespace.
pub(super) fn outbound(paths: &Workspace, command: &str, arguments: &mut Value) {
    match command {
        "launch" | "attach" => {
            to_host(paths, arguments, "program");
            to_host(paths, arguments, "cwd");
        }
        "setBreakpoints" | "gotoTargets" => to_host(paths, &mut arguments["source"], "path"),
        _ => {}
    }
}

fn to_host(paths: &Workspace, object: &mut Value, field: &str) {
    if let Some(path) = object.get(field).and_then(Value::as_str) {
        if let Some(path) = paths.to_host_path(path) {
            object[field] = Value::String(path);
        }
    }
}

/// Normalizes sources before matching breakpoint results or building stack
/// and inspection projections. The pending request identifies the schema.
pub(super) fn response(paths: &Workspace, request: &PendingRequest, body: &mut Value) {
    let (items, source) = match request {
        PendingRequest::SetBreakpoints { .. }
        | PendingRequest::SetFunctionBreakpoints { .. }
        | PendingRequest::SetInstructionBreakpoints { .. }
        | PendingRequest::SetDataBreakpoints { .. } => ("breakpoints", "source"),
        PendingRequest::Inspect {
            kind: DebugInspectKind::StackTrace,
            ..
        } => ("stackFrames", "source"),
        PendingRequest::Inspect {
            kind: DebugInspectKind::Disassemble,
            ..
        } => ("instructions", "location"),
        _ => return,
    };
    if let Some(items) = body.get_mut(items).and_then(Value::as_array_mut) {
        for item in items {
            to_document_source(paths, item.get_mut(source));
        }
    }
}

/// Normalizes sources on adapter events that carry source locations.
pub(super) fn event(paths: &Workspace, event: &str, body: &mut Value) {
    let source = match event {
        "breakpoint" => body
            .get_mut("breakpoint")
            .and_then(|breakpoint| breakpoint.get_mut("source")),
        "output" | "loadedSource" => body.get_mut("source"),
        _ => return,
    };
    to_document_source(paths, source);
}

fn to_document_source(paths: &Workspace, source: Option<&mut Value>) {
    let Some(source) = source.and_then(Value::as_object_mut) else {
        return;
    };
    let Some(path) = source.get("path").and_then(Value::as_str) else {
        return;
    };
    if let Some(document_path) = paths.to_document_path(path) {
        source.insert("path".into(), Value::String(document_path));
    } else {
        // The adapter's sourceReference can still identify generated content.
        // Letting an unmappable path through could open an unrelated local file.
        source.remove("path");
    }
}
