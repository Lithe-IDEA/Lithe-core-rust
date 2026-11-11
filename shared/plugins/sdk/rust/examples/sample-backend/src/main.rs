//! Sample plugin backend for the Rust SDK: the `backend` process of the
//! sample plugin in this directory. Lithe starts it when the plugin's web
//! page first calls a function or the user runs its command, with
//! `--state ${stateDir}`. It:
//!
//! - keeps notes in its state directory, with progress (`note.save`,
//!   `note.read`), and registers `note.clear` once there is a note to clear;
//! - keeps a secret in the host's credential store, never in its state
//!   (`secret.remember`, `secret.check`, `secret.forget`);
//! - waits until it is cancelled (`work.wait`), and fails unexpectedly
//!   without leaking details to the page (`fail.unexpectedly`);
//! - watches the files of the page's workspace through the host
//!   (`files.nextChange`), answers its command with a message in the window
//!   it ran in, and lists the open workspaces when they change.

use lithe_plugin_sdk::{
    BackendHandle, Cancelled, Context, FunctionError, PluginError, SdkError, Value,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::Duration;

/// The command `plugin.json` declares.
const GREET: &str = "org.example.sample-backend.greet";
const WATCH: &str = "lithe/host/workspace/watch";

fn main() {
    let state = match state_directory() {
        Ok(state) => Arc::new(state),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let mut backend =
        lithe_plugin_sdk::Backend::new("sample-backend").version(env!("CARGO_PKG_VERSION"));

    // Exercised only when the Core end-to-end harness offers this provider.
    backend.keymap_import_provider("fixture", |snapshot, context| {
        let files = snapshot.list_inputs(context.host())?;
        let bytes = snapshot.read_chunk(context.host(), &files[0].id, 0, 4)?;
        if bytes != b"data" { return Err(FunctionError::internal("Unexpected fixture source")); }
        Ok(json!({"bindings": [{"commandID": "find-next", "keys": [{"key": "f3", "modifiers": []}]}]}))
    }).expect("valid fixture provider");

    let notes = Arc::clone(&state);
    backend
        .function("note.save", move |params, context| {
            let name = note_name(&params)?;
            let text = params["text"].as_str().ok_or_else(|| {
                PluginError::new("sample.invalidParams", "\"text\" must be a string.")
            })?;
            context.progress("Saving", Some(0));
            let directory = notes.join("notes");
            fs::create_dir_all(&directory)?;
            // Written beside the note and renamed, so a reader never sees half a note.
            let temporary = directory.join(format!(".{name}.tmp"));
            fs::write(&temporary, text)?;
            fs::rename(&temporary, directory.join(format!("{name}.txt")))?;
            context.progress("Saved", Some(100));
            offer_clear(&context.backend(), &notes);
            Ok(json!({ "name": name, "bytes": text.len() }))
        })
        .expect("valid function");

    let notes = Arc::clone(&state);
    backend
        .function("note.read", move |params, _| {
            let name = note_name(&params)?;
            match fs::read_to_string(notes.join("notes").join(format!("{name}.txt"))) {
                Ok(text) => Ok(json!({ "text": text })),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    Err(PluginError::new(
                        "sample.noteMissing",
                        format!("There is no note named {name}."),
                    )
                    .into())
                }
                Err(error) => Err(error.into()),
            }
        })
        .expect("valid function");

    backend
        .function("secret.remember", |params, context| {
            let (key, secret) = (credential_key(&params)?, secret(&params)?);
            context
                .credentials()
                .set(key, secret)
                .map_err(credentials_failure)?;
            Ok(json!({}))
        })
        .expect("valid function");

    backend
        .function("secret.check", |params, context| {
            let (key, secret) = (credential_key(&params)?, secret(&params)?);
            let stored = context
                .credentials()
                .get(key)
                .map_err(credentials_failure)?;
            // The page learns whether its input matches, never the secret.
            Ok(json!({ "stored": stored.is_some(), "matches": stored.as_deref() == Some(secret) }))
        })
        .expect("valid function");

    backend
        .function("secret.forget", |params, context| {
            let key = credential_key(&params)?;
            context
                .credentials()
                .delete(key)
                .map_err(credentials_failure)?;
            Ok(Value::Null)
        })
        .expect("valid function");

    let markers = Arc::clone(&state);
    backend
        .function("work.wait", move |_, context| {
            fs::write(markers.join("waiting"), "")?;
            // Blocks without spinning until the host cancels the call.
            if context
                .cancellation()
                .wait_timeout(Duration::from_secs(600))
            {
                fs::write(markers.join("cancelled"), "")?;
                return Err(Cancelled.into());
            }
            Ok(json!("finished"))
        })
        .expect("valid function");

    let hidden = Arc::clone(&state);
    backend
        .function("fail.unexpectedly", move |_, _| {
            Err(FunctionError::internal(format!(
                "could not open {}",
                hidden.join("missing").display()
            )))
        })
        .expect("valid function");

    backend
        .function("files.nextChange", |_, context| next_change(context))
        .expect("valid function");

    // The terminal API is a host service. The lifecycle sample deliberately
    // uses a harmless interactive client and display-only history; it never
    // executes retained text as input.
    backend
        .function("terminal.cycle", |_, context| {
            let workspace = context
                .workspace_context()
                .ok_or_else(|| PluginError::new("sample.noWorkspace", "Open a workspace first."))?;
            let host = context.host();
            if !host.supports("lithe/host/terminal/reopen")? {
                return Err(PluginError::new(
                    "sample.terminalUnavailable",
                    "The host has no native terminal API.",
                )
                .into());
            }
            let terminal = host.terminal();
            let created = terminal.create(
                workspace,
                json!({ "executable": "example-client", "arguments": ["--interactive"],
            "retainedContextBytes": 4096, "show": true }),
            )?;
            let id = created["terminalID"]
                .as_str()
                .ok_or_else(|| FunctionError::internal("missing terminalID"))?;
            let subscription = terminal.on_exit(workspace, id)?;
            let closed = terminal.close(workspace, id, Some(4096))?;
            let event = subscription
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| FunctionError::internal("terminal exit did not arrive"))?;
            let reopened = terminal.reopen(workspace, id)?;
            terminal.dispose(workspace, id)?;
            subscription.dispose()?;
            Ok(json!({ "closed": closed, "reopened": reopened, "event": event.event() }))
        })
        .expect("valid function");

    // Notifications are events: each handler runs on a thread of its own and
    // may ask the host for more, outside any call.
    let handle = backend.handle();
    backend
        .on_command(GREET, move |_, context| {
            let Some(context) = context else {
                return Ok(());
            };
            let host = handle.host();
            let folders = host.workspace().get_folders(&context)?;
            let names: Vec<&str> = folders["folders"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|folder| folder["name"].as_str())
                .collect();
            let text = if names.is_empty() {
                "Sample Backend sees no folders.".to_string()
            } else {
                format!("Sample Backend sees {}.", names.join(", "))
            };
            host.window()
                .show_information(&context, json!({ "text": text }))?;
            Ok(())
        })
        .expect("valid command");

    // A backend that keeps state per workspace would refresh it here.
    let handle = backend.handle();
    backend.on_contexts_changed(move |_| {
        let _contexts = handle.host().workspace().list_contexts()?;
        Ok(())
    });

    let notes = Arc::clone(&state);
    backend.on_initialize(move |initialize| {
        if has_notes(&notes) {
            let notes = Arc::clone(&notes);
            initialize.function("note.clear", move |_, context| clear(&notes, context))?;
        }
        Ok(())
    });

    std::process::exit(backend.listen());
}

fn state_directory() -> Result<PathBuf, String> {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (Some("--state"), Some(path)) if arguments.next().is_none() => Ok(PathBuf::from(path)),
        _ => Err("usage: sample-backend --state <directory>".into()),
    }
}

/// Registers `note.clear` the first time there is a note to clear.
fn offer_clear(backend: &BackendHandle, state: &Arc<PathBuf>) {
    if backend.function_ids().iter().any(|id| id == "note.clear") {
        return;
    }
    let notes = Arc::clone(state);
    if backend
        .register_function("note.clear", move |_, context| clear(&notes, context))
        .is_err()
    {
        backend
            .diagnostics()
            .degraded("initialize", "registration_failed", None)
            .expect("fixed diagnostic");
    }
}

fn clear(state: &Path, context: &Context) -> Result<Value, FunctionError> {
    context.check_cancelled()?;
    match fs::remove_dir_all(state.join("notes")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(Value::Null),
    }
}

/// Waits for the next change under the first folder of the page's
/// workspace, through a host file watch.
fn next_change(context: &Context) -> Result<Value, FunctionError> {
    let Some(workspace) = context.workspace_context() else {
        return Err(PluginError::new("sample.noWorkspace", "Open a workspace first.").into());
    };
    let host = context.host();
    // Core answers capability discovery itself, from the plugin's permissions.
    if !host.supports(WATCH)? {
        return Err(PluginError::new(
            "sample.watchUnavailable",
            "Lithe does not let this plugin watch files.",
        )
        .into());
    }
    let folders = host
        .workspace()
        .get_folders(workspace)
        .map_err(workspace_failure)?;
    let Some(root) = folders["folders"][0]["rootID"].as_str() else {
        return Err(PluginError::new("sample.noWorkspace", "The workspace has no folders.").into());
    };
    let watch = host
        .workspace()
        .watch(
            workspace,
            json!({ "resource": { "rootID": root, "path": "" }, "recursive": true }),
        )
        .map_err(workspace_failure)?;
    let outcome = loop {
        if context.cancellation().is_cancelled() {
            break Err(Cancelled.into());
        }
        match watch.recv_timeout(Duration::from_millis(200)) {
            Ok(event) => break Ok(json!({ "changedPaths": event.event()["changedPaths"] })),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break Err(FunctionError::internal("the watch ended"))
            }
        }
    };
    // A subscription outlives the call until it is disposed.
    if watch.dispose().is_err() {
        context
            .diagnostics()
            .degraded("files.nextChange", "subscription_dispose_failed", None)
            .expect("fixed diagnostic");
    }
    outcome
}

/// A window that switched folders makes the page's context stale
/// (`plugin.host.contextExpired`), which the page can recover from.
fn workspace_failure(error: SdkError) -> FunctionError {
    if error.code() == "plugin.host.contextExpired" {
        PluginError::new(
            "sample.workspaceChanged",
            "The workspace changed; try again.",
        )
        .into()
    } else {
        error.into()
    }
}

fn has_notes(state: &Path) -> bool {
    fs::read_dir(state.join("notes")).is_ok_and(|mut entries| entries.next().is_some())
}

/// A note name is a file name the sample chooses the alphabet of, so a page
/// can never name a path outside the notes directory.
fn note_name(params: &Value) -> Result<&str, PluginError> {
    params["name"]
        .as_str()
        .filter(|name| {
            (1..=64).contains(&name.len())
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .ok_or_else(|| {
            PluginError::new(
                "sample.invalidParams",
                "\"name\" must be 1-64 lowercase letters, digits, or \"-\".",
            )
        })
}

fn credential_key(params: &Value) -> Result<&str, PluginError> {
    params["key"]
        .as_str()
        .filter(|key| lithe_plugin_sdk::is_credential_key(key))
        .ok_or_else(|| PluginError::new("sample.invalidParams", "\"key\" is not a credential key."))
}

fn secret(params: &Value) -> Result<&str, PluginError> {
    params["secret"]
        .as_str()
        .ok_or_else(|| PluginError::new("sample.invalidParams", "\"secret\" must be a string."))
}

/// A host that offers no credential store (`plugin.host.unavailable`), or a
/// plugin installed without the `credentials.store` permission, is a state
/// the page can explain; other failures stay internal.
fn credentials_failure(error: SdkError) -> FunctionError {
    if error.code().starts_with("plugin.host.") {
        PluginError::new(
            "sample.credentialsUnavailable",
            "Lithe cannot store credentials for this plugin.",
        )
        .into()
    } else {
        error.into()
    }
}
