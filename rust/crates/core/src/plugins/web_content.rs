//! Plugin web content (`ui`): the rules a host applies around a plugin page.
//!
//! The native host owns the web view, file reads, storage, dialogs and the
//! calls a page makes; this module decides what a page is served, which bridge
//! requests it may make with which parameters, and how replies and events are
//! framed, so every client answers a page the same way. Pages are registered
//! here for their lifetime because some bridge rules outlive a request: a page
//! never reuses a call ID, and it holds a bounded number of calls and one file
//! dialog at a time.
//!
//! Note: the decision is recorded in Lithe-IDEA's
//! .agents/notes/implemented/architecture/2026-10-18-plugin-web-content-in-host-windows.md

use super::declarative::{decode_payload, validate_manifest_value};
use crate::protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// URL scheme of plugin pages; the host is the plugin ID, one origin per plugin.
const SCHEME: &str = "lithe-plugin";
/// Version of the page bridge, the `ui` capability's web content version.
const BRIDGE_VERSION: u32 = 0;
/// Package path of the host's bridge script. No package file under its
/// directory is served: the host answers this path with the bootstrap script.
const BOOTSTRAP_PATH: &str = ".lithe-host/bridge.js";
/// Largest package file a page is served.
const FILE_LIMIT: u64 = 16 * 1024 * 1024;
/// Largest stored page state, serialized, per window and workspace.
const STATE_LIMIT: usize = 64 * 1024;
/// Largest bridge message a page may post.
const MESSAGE_LIMIT: usize = 1024 * 1024;
/// Function calls one page may have accepted at once, including calls still
/// awaiting consent.
const CALL_LIMIT: usize = 16;
/// Longest page-chosen call ID and function ID.
const ID_LIMIT: usize = 128;
/// Call IDs one page may use in its lifetime. The set of used IDs is what
/// makes a reused ID detectable after its call ended, so it must be bounded;
/// past the bound a page makes calls without IDs or is reloaded.
const USED_CALL_LIMIT: usize = 65_536;
/// Pages registered at once across all windows; protects Core from a host
/// that forgets to close pages.
const PAGE_LIMIT: usize = 512;
/// Longest workspace or package path a page may name.
const PATH_LIMIT: usize = 4096;
/// File name extensions a dialog accepts, and their length.
const EXTENSION_COUNT: usize = 16;
const EXTENSION_LIMIT: usize = 16;

/// Methods a page may request through the bridge.
const METHODS: &[&str] = &[
    "build.instances",
    "build.refresh",
    "build.runTask",
    "build.runTests",
    "build.cancel",
    "build.dependencies",
    "build.setOptions",
    "workspace.openFile",
    "ui.getState",
    "ui.setState",
    "functions.list",
    "functions.invoke",
    "functions.cancel",
    "dialog.openFile",
    "dialog.saveFile",
];

/// Events a host may deliver to a page.
const EVENTS: &[&str] = &[
    "build.instancesChanged",
    "build.runChanged",
    "functions.progress",
    "ui.themeChanged",
];

/// Theme a page is shown with: `light` or `dark`, and the host's `--lithe-*`
/// CSS variables.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Theme {
    theme: String,
    #[serde(default)]
    variables: BTreeMap<String, String>,
}

impl Theme {
    fn validated(self) -> Result<Self, CoreError> {
        if self.theme != "light" && self.theme != "dark" {
            return Err(invalid("theme must be light or dark"));
        }
        for (name, value) in &self.variables {
            let valid_name = name.len() <= 64
                && name.strip_prefix("--lithe-").is_some_and(|rest| {
                    !rest.is_empty()
                        && rest.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        })
                });
            // A value lands in a style property, never in markup or script
            // source, but stays printable and short so a host bug cannot
            // smuggle a document into it.
            let valid_value = value.len() <= 256 && !value.chars().any(char::is_control);
            if !valid_name || !valid_value {
                return Err(invalid(
                    "theme variables are --lithe-* names with short printable values",
                ));
            }
        }
        Ok(self)
    }

    fn payload(&self) -> Value {
        json!({ "theme": self.theme, "variables": self.variables })
    }
}

/// One open page: who it belongs to and the bridge state that outlives a request.
struct Page {
    plugin_id: String,
    window_id: String,
    platform: String,
    locale: String,
    theme: Theme,
    /// Whether this host serves `build.*`, and `functions.*` with `instance`.
    build_systems: bool,
    /// Accepted function calls by call ID until the host replies to them.
    calls: HashSet<String>,
    /// Hashes of every page-chosen call ID; see [`USED_CALL_LIMIT`].
    used_calls: HashSet<u64>,
    /// Keyed per page so a page cannot choose IDs that collide in `used_calls`.
    hasher: RandomState,
    /// Counter of host-made call IDs for calls the page sent without one.
    host_calls: u64,
    /// A page has at most one file dialog open.
    dialog_open: bool,
}

#[derive(Default)]
struct Registry {
    pages: HashMap<u64, Page>,
    next: u64,
}

fn registry() -> MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, message.to_owned())
}

fn unknown_page() -> CoreError {
    CoreError::new(ErrorCode::InvalidRequest, "The plugin page is not open.")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenPayload {
    /// The installed plugin's manifest.
    manifest: Value,
    #[serde(rename = "windowID")]
    window_id: String,
    /// Host platform label shown to the page, e.g. `macos`.
    platform: String,
    /// BCP 47 language of the host interface, e.g. `zh-CN`.
    locale: String,
    theme: Theme,
    /// Whether the host serves build systems to pages; `build.*` are unknown
    /// methods otherwise.
    #[serde(default)]
    build_systems: bool,
}

/// `plugins.web.open`: registers a page for one web tool window and returns
/// what the host needs to load it.
pub fn open_command(payload: Value) -> Result<Value, CoreError> {
    let payload: OpenPayload = decode_payload(payload, "plugins.web.open")?;
    let manifest = validate_manifest_value(payload.manifest).map_err(|rejection| {
        CoreError::new(ErrorCode::InvalidRequest, rejection.message.clone())
            .with_details(rejection.code)
    })?;
    let entry = manifest
        .web_window_entry(&payload.window_id)
        .ok_or_else(|| invalid("The plugin declares no web window with this ID."))?
        .to_owned();
    let label_ok = |value: &str| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    };
    if !label_ok(&payload.platform) || !label_ok(&payload.locale) {
        return Err(invalid("platform and locale are short identifiers"));
    }
    let theme = payload.theme.validated()?;
    let mut registry = registry();
    if registry.pages.len() >= PAGE_LIMIT {
        return Err(invalid("Too many plugin pages are open; close one first."));
    }
    registry.next += 1;
    let id = registry.next;
    let plugin_id = manifest.id().to_owned();
    let url = format!("{SCHEME}://{plugin_id}/{}", encode_path(&entry));
    registry.pages.insert(
        id,
        Page {
            plugin_id: plugin_id.clone(),
            window_id: payload.window_id.clone(),
            platform: payload.platform,
            locale: payload.locale,
            theme,
            build_systems: payload.build_systems,
            calls: HashSet::new(),
            used_calls: HashSet::new(),
            hasher: RandomState::new(),
            host_calls: 0,
            dialog_open: false,
        },
    );
    Ok(json!({
        "page": id,
        "pluginID": plugin_id,
        "windowID": payload.window_id,
        "scheme": SCHEME,
        "url": url,
        "bridgeVersion": BRIDGE_VERSION,
        "fileLimit": FILE_LIMIT,
        "stateLimit": STATE_LIMIT,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageRef {
    page: u64,
}

/// `plugins.web.close`: forgets a page. Its accepted calls are returned so
/// the host cancels them; closing an unknown page is not an error.
pub fn close_command(payload: Value) -> Result<Value, CoreError> {
    let payload: PageRef = decode_payload(payload, "plugins.web.close")?;
    let page = registry().pages.remove(&payload.page);
    let mut calls: Vec<String> = page
        .map(|page| page.calls.into_iter().collect())
        .unwrap_or_default();
    calls.sort();
    Ok(json!({ "cancelCalls": calls }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourcePayload {
    page: u64,
    /// Decoded URL path after the host, with or without its leading `/`.
    path: String,
}

/// `plugins.web.resource`: decides how one request of a page is answered.
///
/// `bootstrap` is the bridge script itself; `file` names package path
/// `segments` the host reads (refusing symbolic links, non-regular files and
/// files over `fileLimit`) and, when `document` is true, passes through
/// `plugins.web.document`; `refused` is answered as a missing file.
pub fn resource_command(payload: Value) -> Result<Value, CoreError> {
    let payload: ResourcePayload = decode_payload(payload, "plugins.web.resource")?;
    let registry = registry();
    let page = registry.pages.get(&payload.page).ok_or_else(unknown_page)?;
    let path = payload.path.strip_prefix('/').unwrap_or(&payload.path);
    if path == BOOTSTRAP_PATH {
        return Ok(json!({
            "kind": "bootstrap",
            "mime": "text/javascript; charset=utf-8",
            "body": bootstrap_script(page),
        }));
    }
    let reserved = BOOTSTRAP_PATH.split_once('/').map(|(prefix, _)| prefix);
    let Some(segments) =
        package_segments(path).filter(|segments| segments.first().copied() != reserved)
    else {
        return Ok(json!({ "kind": "refused" }));
    };
    let extension = segments
        .last()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    Ok(json!({
        "kind": "file",
        "segments": segments,
        "mime": mime_type(&extension),
        "document": extension == "html" || extension == "htm",
    }))
}

/// Splits a request path into package path segments, or `None` when it could
/// leave the package: empty, `.` and `..` segments, backslashes, drive or
/// stream markers, and control characters are refused.
fn package_segments(path: &str) -> Option<Vec<&str>> {
    if path.is_empty() || path.len() > PATH_LIMIT {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    segments
        .iter()
        .all(|segment| {
            !segment.is_empty()
                && *segment != "."
                && *segment != ".."
                && !segment.contains(['\\', ':'])
                && !segment.chars().any(char::is_control)
        })
        .then_some(segments)
}

fn mime_type(extension: &str) -> &'static str {
    match extension {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Percent-encodes a package path for the page URL, keeping `/`.
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The page's content security policy: only its own package, no network,
/// remote resources, frames, inline or evaluated scripts.
fn content_security_policy(plugin_id: &str) -> String {
    let origin = format!("'self' {SCHEME}://{plugin_id}");
    [
        "default-src 'none'".to_owned(),
        format!("script-src {origin}"),
        format!("style-src {origin} 'unsafe-inline'"),
        format!("img-src {origin} data: blob:"),
        format!("font-src {origin} data:"),
        format!("media-src {origin} blob:"),
        format!("connect-src {origin}"),
        "frame-src 'none'".to_owned(),
        "child-src 'none'".to_owned(),
        "worker-src 'none'".to_owned(),
        "object-src 'none'".to_owned(),
        "manifest-src 'none'".to_owned(),
        "base-uri 'none'".to_owned(),
        "form-action 'none'".to_owned(),
    ]
    .join("; ")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentPayload {
    page: u64,
    html: String,
}

/// `plugins.web.document`: prepares an HTML document of the package for the
/// page. The policy and the bridge script go first in the document, before
/// anything the package wrote, because a host that serves a custom scheme
/// cannot always attach response headers or document-start scripts.
pub fn document_command(payload: Value) -> Result<Value, CoreError> {
    let payload: DocumentPayload = decode_payload(payload, "plugins.web.document")?;
    let registry = registry();
    let page = registry.pages.get(&payload.page).ok_or_else(unknown_page)?;
    let head = format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <script src=\"/{BOOTSTRAP_PATH}\"></script>",
        content_security_policy(&page.plugin_id)
    );
    Ok(json!({ "html": inject_head(&payload.html, &head) }))
}

/// Inserts `head` right after the document's opening `<head>` tag when it
/// comes before `<body>`, else after `<html>`, else after a leading doctype,
/// else at the very start. A meta policy governs only what follows it, so it
/// must precede the package's own elements.
fn inject_head(html: &str, head: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let after_tag = |name: &str| -> Option<usize> {
        let mut from = 0;
        while let Some(found) = lower[from..].find(name) {
            let start = from + found;
            let next = lower.as_bytes().get(start + name.len()).copied();
            if matches!(next, Some(b'>' | b' ' | b'\t' | b'\n' | b'\r' | b'/')) {
                return lower[start..].find('>').map(|end| start + end + 1);
            }
            from = start + name.len();
        }
        None
    };
    let body = lower.find("<body");
    let before_body = |end: &usize| body.is_none_or(|body| *end <= body);
    let position = after_tag("<head")
        .filter(before_body)
        .or_else(|| after_tag("<html").filter(before_body))
        .or_else(|| {
            let leading = lower.len() - lower.trim_start().len();
            lower[leading..]
                .starts_with("<!doctype")
                .then(|| lower[leading..].find('>').map(|end| leading + end + 1))
                .flatten()
        })
        .unwrap_or(0);
    let mut prepared = String::with_capacity(html.len() + head.len());
    prepared.push_str(&html[..position]);
    prepared.push_str(head);
    prepared.push_str(&html[position..]);
    prepared
}

/// The bridge script, defining a frozen `window.lithe` over the web view's
/// message channel (`window.viva`). It runs before the package's scripts.
fn bootstrap_script(page: &Page) -> String {
    let context = json!({
        "pluginID": page.plugin_id,
        "windowID": page.window_id,
        "platform": page.platform,
        "locale": page.locale,
    });
    // JSON is a JavaScript expression; `<` is escaped so the text can never
    // close a script element even if it were inlined.
    let script_json = |value: Value| value.to_string().replace('<', "\\u003c");
    BOOTSTRAP
        .replace("__LITHE_VERSION__", &BRIDGE_VERSION.to_string())
        .replace("__LITHE_CONTEXT__", &script_json(context))
        .replace("__LITHE_THEME__", &script_json(page.theme.payload()))
}

/// Requests are `{ id, method, params }`; the host answers `{ id, ok }` or
/// `{ id, error }` and sends `{ event, payload }`. The channel is taken
/// before any package script runs and its `onmessage` cannot be replaced.
const BOOTSTRAP: &str = r#"(() => {
  'use strict';
  const channel = window.viva;
  if (!channel || window.lithe) return;
  const post = channel.postMessage.bind(channel);
  const parse = JSON.parse, stringify = JSON.stringify, freeze = Object.freeze;
  const initial = __LITHE_CONTEXT__;
  let theme = __LITHE_THEME__;
  let next = 0;
  const pending = new Map();
  const listeners = new Map();
  const failure = (code, message, pluginErrorCode) =>
    freeze(typeof pluginErrorCode === 'string' ? { code, message, pluginErrorCode } : { code, message });
  const applyTheme = () => {
    const root = document.documentElement;
    if (!root) { document.addEventListener('readystatechange', applyTheme, { once: true }); return; }
    root.setAttribute('data-theme', theme.theme);
    for (const [name, value] of Object.entries(theme.variables)) root.style.setProperty(name, value);
  };
  const notify = (event, payload) => {
    for (const listener of [...(listeners.get(event) || [])]) {
      try { listener(parse(stringify(payload))); } catch (error) { setTimeout(() => { throw error; }); }
    }
  };
  Object.defineProperty(channel, 'onmessage', {
    value: (message) => {
      let value;
      try { value = parse(message.data); } catch (error) { return; }
      if (typeof value.id === 'number') {
        const waiting = pending.get(value.id);
        if (!waiting) return;
        pending.delete(value.id);
        if (value.error) waiting.reject(failure(value.error.code, value.error.message, value.error.pluginErrorCode));
        else waiting.resolve(value.ok);
      } else if (typeof value.event === 'string') {
        if (value.event === 'ui.themeChanged') {
          theme = value.payload;
          applyTheme();
          notify(value.event, { theme: theme.theme });
        } else {
          notify(value.event, value.payload);
        }
      }
    }
  });
  const lithe = freeze({
    version: __LITHE_VERSION__,
    context: freeze({
      pluginID: initial.pluginID, windowID: initial.windowID, platform: initial.platform, locale: initial.locale,
      get theme() { return theme.theme; }
    }),
    request(method, params) {
      const id = ++next;
      let message;
      try {
        message = stringify({ id, method: String(method), params: params === undefined ? null : params });
      } catch (error) {
        return Promise.reject(failure('plugin.web.invalidRequest', 'The parameters are not JSON.'));
      }
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        post(message);
      });
    },
    on(event, listener) {
      if (typeof listener !== 'function') throw new TypeError('listener must be a function');
      const name = String(event);
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
      return () => { listeners.get(name)?.delete(listener); };
    }
  });
  Object.defineProperty(window, 'lithe', { value: lithe, enumerable: true });
  applyTheme();
})();
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestPayload {
    page: u64,
    /// The text the page posted.
    message: String,
}

/// A bridge failure with its stable code.
struct Refusal {
    code: &'static str,
    message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

fn invalid_request(message: impl Into<String>) -> Refusal {
    refuse("plugin.web.invalidRequest", message)
}

/// `plugins.web.request`: checks one message a page posted.
///
/// The outcome is `{ reply }`, a message to post back at once (a refusal),
/// `{ action: { id, method, params } }`, a checked request for the host to
/// carry out and answer with `plugins.web.reply`, or `{ ignored }` with the
/// reason when the message cannot be answered.
pub fn request_command(payload: Value) -> Result<Value, CoreError> {
    let payload: RequestPayload = decode_payload(payload, "plugins.web.request")?;
    let mut registry = registry();
    let Some(page) = registry.pages.get_mut(&payload.page) else {
        return Ok(json!({ "ignored": "The plugin page is not open." }));
    };
    if payload.message.len() > MESSAGE_LIMIT {
        return Ok(json!({ "ignored": "The message is larger than 1 MiB." }));
    }
    let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(&payload.message) else {
        return Ok(json!({ "ignored": "The message is not a JSON object." }));
    };
    let Some(id) = body
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id <= (1 << 53))
    else {
        return Ok(json!({ "ignored": "The message has no request ID." }));
    };
    let outcome = (|| {
        if body
            .keys()
            .any(|key| !matches!(key.as_str(), "id" | "method" | "params"))
        {
            return Err(invalid_request("A request is { id, method, params }."));
        }
        let Some(Value::String(method)) = body.remove("method") else {
            return Err(invalid_request("A request names its method."));
        };
        let params = match body.remove("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(params)) => params,
            Some(_) => return Err(invalid_request("The parameters must be an object.")),
        };
        check(page, &method, params).map(|params| (method, params))
    })();
    Ok(match outcome {
        Ok((method, params)) => {
            json!({ "action": { "id": id, "method": method, "params": params } })
        }
        Err(refusal) => {
            json!({ "reply": error_reply(id, refusal.code, &refusal.message, None) })
        }
    })
}

/// Checks the parameters of one method and returns them normalized, taking
/// the page resources the request holds until its reply.
fn check(
    page: &mut Page,
    method: &str,
    params: Map<String, Value>,
) -> Result<Map<String, Value>, Refusal> {
    if !METHODS.contains(&method) || (method.starts_with("build.") && !page.build_systems) {
        return Err(refuse(
            "plugin.web.unknownMethod",
            format!("This host does not answer {method}."),
        ));
    }
    let params = Params(params);
    match method {
        "workspace.openFile" => {
            params.only(&["path", "line", "column"])?;
            let path = params.string("path", PATH_LIMIT)?;
            let line = params.position("line")?;
            let column = params.position("column")?;
            // The host still resolves the path against the workspace and
            // refuses one that escapes it through a symbolic link.
            if path.split(['/', '\\']).any(|part| part == "..") {
                return Err(refuse(
                    "plugin.web.forbidden",
                    format!("{path} is outside the workspace."),
                ));
            }
            Ok(object([
                ("path", json!(path)),
                ("line", json!(line)),
                ("column", json!(column)),
            ]))
        }
        "ui.getState" => {
            params.only(&[])?;
            Ok(Map::new())
        }
        "ui.setState" => {
            params.only(&["state"])?;
            let Some(state) = params.0.get("state") else {
                return Err(invalid_request("state is required; use null to clear it."));
            };
            // The host stores the text as given; `null` clears the state.
            let encoded = (!state.is_null()).then(|| state.to_string());
            if encoded
                .as_ref()
                .is_some_and(|text| text.len() > STATE_LIMIT)
            {
                return Err(invalid_request("The state is larger than 64 KiB."));
            }
            Ok(object([("state", json!(encoded))]))
        }
        "functions.list" => {
            params.only(&["instance"])?;
            let instance = params.instance(page)?;
            Ok(object([("instance", json!(instance))]))
        }
        "functions.invoke" => {
            params.only(&["instance", "function", "params", "call"])?;
            let instance = params.instance(page)?;
            let function = params.string("function", ID_LIMIT)?;
            let input = match params.0.get("params") {
                None | Some(Value::Null) => Value::Object(Map::new()),
                Some(value @ Value::Object(_)) => value.clone(),
                Some(_) => return Err(invalid_request("params must be an object.")),
            };
            let page_call = params.optional_string("call", ID_LIMIT)?;
            if let Some(call) = &page_call {
                if page.used_calls.contains(&page.hasher.hash_one(call)) {
                    return Err(invalid_request(format!(
                        "The call ID {call} was already used."
                    )));
                }
                if page.used_calls.len() >= USED_CALL_LIMIT {
                    return Err(invalid_request(
                        "This page used too many call IDs; reload it.",
                    ));
                }
            }
            if page.calls.len() >= CALL_LIMIT {
                return Err(refuse(
                    "plugin.build.unavailable",
                    format!("This page already runs {CALL_LIMIT} function calls."),
                ));
            }
            let call = match &page_call {
                Some(call) => {
                    page.used_calls.insert(page.hasher.hash_one(call));
                    call.clone()
                }
                None => {
                    // A page ID is printable, so a host-made ID starting with
                    // a control character never collides with one.
                    page.host_calls += 1;
                    format!("\u{1}host-{}", page.host_calls)
                }
            };
            page.calls.insert(call.clone());
            Ok(object([
                ("instance", json!(instance)),
                ("function", json!(function)),
                ("params", input),
                ("call", json!(call)),
                ("progress", json!(page_call.is_some())),
            ]))
        }
        "functions.cancel" => {
            params.only(&["call"])?;
            let call = params.string("call", ID_LIMIT)?;
            // A call that already ended, or was never made, is cancelled as
            // a no-op, so the host acts only on a live call.
            let active = page.calls.contains(&call);
            Ok(object([("call", json!(call)), ("active", json!(active))]))
        }
        "dialog.openFile" | "dialog.saveFile" => {
            let save = method == "dialog.saveFile";
            params.only(if save {
                &["title", "suggestedName", "extensions"]
            } else {
                &["title", "extensions"]
            })?;
            let title = params.optional_string("title", 256)?;
            let suggested = if save {
                params.optional_string("suggestedName", 255)?
            } else {
                None
            };
            if suggested
                .as_ref()
                .is_some_and(|name| name.contains(['/', '\\', ':']) || name.starts_with('.'))
            {
                return Err(invalid_request("suggestedName must be a file name."));
            }
            let extensions = params.extensions()?;
            if page.dialog_open {
                return Err(invalid_request(
                    "A file dialog of this page is already open.",
                ));
            }
            page.dialog_open = true;
            Ok(object([
                ("title", json!(title)),
                ("suggestedName", json!(suggested)),
                ("extensions", json!(extensions)),
            ]))
        }
        // build.*: answered only by hosts that serve build systems; their
        // build bridge checks the selections against the loaded model.
        _ => Ok(params.0),
    }
}

/// An object of the given fields, leaving out the ones that are null.
fn object<const N: usize>(fields: [(&str, Value); N]) -> Map<String, Value> {
    fields
        .into_iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// Parameters of one request.
struct Params(Map<String, Value>);

impl Params {
    fn only(&self, allowed: &[&str]) -> Result<(), Refusal> {
        let mut keys: Vec<&String> = self.0.keys().collect();
        keys.sort();
        match keys
            .into_iter()
            .find(|key| !allowed.contains(&key.as_str()))
        {
            Some(key) => Err(invalid_request(format!("Unknown parameter {key}."))),
            None => Ok(()),
        }
    }

    fn optional_string(&self, key: &str, limit: usize) -> Result<Option<String>, Refusal> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value))
                if !value.is_empty()
                    && value.chars().count() <= limit
                    && !value.chars().any(char::is_control) =>
            {
                Ok(Some(value.clone()))
            }
            Some(_) => Err(invalid_request(format!(
                "{key} must be a string of 1 to {limit} characters."
            ))),
        }
    }

    fn string(&self, key: &str, limit: usize) -> Result<String, Refusal> {
        self.optional_string(key, limit)?
            .ok_or_else(|| invalid_request(format!("{key} is required.")))
    }

    fn position(&self, key: &str) -> Result<Option<u64>, Refusal> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => match value.as_u64() {
                Some(position @ 1..=10_000_000) => Ok(Some(position)),
                _ => Err(invalid_request(format!(
                    "{key} must be a positive integer."
                ))),
            },
        }
    }

    /// A build instance ID, which only a host serving build systems accepts.
    fn instance(&self, page: &Page) -> Result<Option<String>, Refusal> {
        let instance = self.optional_string("instance", 1024)?;
        if instance.is_some() && !page.build_systems {
            return Err(refuse(
                "plugin.web.forbidden",
                "This host serves no build-system instances to plugin pages.",
            ));
        }
        Ok(instance)
    }

    fn extensions(&self) -> Result<Vec<String>, Refusal> {
        let refusal =
            || invalid_request("extensions must be at most 16 file name extensions without a dot.");
        match self.0.get("extensions") {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) if items.len() <= EXTENSION_COUNT => items
                .iter()
                .map(|item| match item {
                    Value::String(value)
                        if !value.is_empty()
                            && value.len() <= EXTENSION_LIMIT
                            && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) =>
                    {
                        Ok(value.clone())
                    }
                    _ => Err(refusal()),
                })
                .collect(),
            Some(_) => Err(refusal()),
        }
    }
}

fn error_reply(id: u64, code: &str, message: &str, plugin_error_code: Option<&str>) -> String {
    let mut error = json!({ "code": code, "message": message });
    if let Some(plugin_error_code) = plugin_error_code {
        error["pluginErrorCode"] = json!(plugin_error_code);
    }
    json!({ "id": id, "error": error }).to_string()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplyPayload {
    page: u64,
    id: u64,
    /// The method of the action answered.
    method: String,
    /// The call of a `functions.invoke` action, released by this reply.
    #[serde(default)]
    call: Option<String>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ReplyError>,
}

/// A failure the host reports for an action; codes pass through unchanged
/// (`plugin.web.*`, `plugin.function.*`, `requestCancelled`, ...).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplyError {
    code: String,
    message: String,
    /// A failure the plugin itself reported; never interpreted by the host.
    #[serde(default)]
    plugin_error_code: Option<String>,
}

/// `plugins.web.reply`: frames the host's answer to an action and releases
/// what the action held (its function call or the page's file dialog).
/// `message` is null when the page closed meanwhile: the answer is dropped.
pub fn reply_command(payload: Value) -> Result<Value, CoreError> {
    let payload: ReplyPayload = decode_payload(payload, "plugins.web.reply")?;
    let message = match (payload.result, payload.error) {
        (Some(result), None) => json!({ "id": payload.id, "ok": result }).to_string(),
        (None, Some(error)) => {
            let code = if error.code.is_empty() || error.code.len() > 128 {
                "plugin.web.invalidRequest".to_owned()
            } else {
                error.code
            };
            error_reply(
                payload.id,
                &code,
                &error.message,
                error.plugin_error_code.as_deref(),
            )
        }
        _ => return Err(invalid("A reply carries exactly one of result and error.")),
    };
    let mut registry = registry();
    let Some(page) = registry.pages.get_mut(&payload.page) else {
        return Ok(json!({ "message": null }));
    };
    if let Some(call) = &payload.call {
        page.calls.remove(call);
    }
    if payload.method.starts_with("dialog.") {
        page.dialog_open = false;
    }
    Ok(json!({ "message": message }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventPayload {
    page: u64,
    event: String,
    payload: Value,
}

/// `plugins.web.event`: frames one event for a page. `functions.progress`
/// is delivered only for a running call the page made with its own ID;
/// `ui.themeChanged` also becomes the theme later bridge scripts start with.
/// `message` is null when nothing is to be delivered.
pub fn event_command(payload: Value) -> Result<Value, CoreError> {
    let payload: EventPayload = decode_payload(payload, "plugins.web.event")?;
    if !EVENTS.contains(&payload.event.as_str()) {
        return Err(invalid("Unknown plugin page event."));
    }
    let mut registry = registry();
    let Some(page) = registry.pages.get_mut(&payload.page) else {
        return Ok(json!({ "message": null }));
    };
    let body = match payload.event.as_str() {
        "ui.themeChanged" => {
            let theme: Theme = serde_json::from_value(payload.payload)
                .map_err(|_| invalid("ui.themeChanged carries { theme, variables }."))
                .and_then(Theme::validated)?;
            let body = theme.payload();
            page.theme = theme;
            body
        }
        "functions.progress" => {
            let call = payload.payload.get("call").and_then(Value::as_str);
            let page_made = call.is_some_and(|call| {
                page.calls.contains(call) && !call.starts_with(char::is_control)
            });
            if !page_made {
                return Ok(json!({ "message": null }));
            }
            payload.payload
        }
        _ => payload.payload,
    };
    Ok(json!({ "message": json!({ "event": payload.event, "payload": body }).to_string() }))
}

#[cfg(test)]
#[path = "web_content_tests.rs"]
mod tests;
