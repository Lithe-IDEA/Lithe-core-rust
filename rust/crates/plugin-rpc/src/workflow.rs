//! Strict host workflow contracts and bounded projections. Native hosts own UI,
//! sockets and resource lifetimes; this module owns grants, shapes and budgets.

use crate::{
    codes, host_rpc_code, is_command_id, is_context_reference, is_credential_key,
    HostRequestRefusal,
};
use lithe_protocol::{CoreError, ErrorCode};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Maximum UTF-8 bytes in an inline document, HTTP body or clipboard transfer.
pub const TEXT_BYTES: usize = 128 * 1024;
/// Maximum response header bytes accepted by the desktop HTTP transport.
pub const HTTP_HEADER_BYTES: usize = 16 * 1024;
/// Maximum retained output lines per channel.
pub const OUTPUT_LINES: usize = 4000;
/// Maximum retained output bytes per channel.
pub const OUTPUT_BYTES: usize = 512 * 1024;
/// Maximum bytes of an unfinished output line.
pub const OUTPUT_LINE_BYTES: usize = 16 * 1024;
/// Maximum channels, progress handles and status items per backend session (shared with other limited host registrations).
pub const RESOURCE_LIMIT: usize = 16;

fn invalid() -> HostRequestRefusal {
    HostRequestRefusal {
        rpc_code: -32602,
        code: codes::HOST_INVALID_PARAMS,
        message: "Invalid workflow parameters.",
        details: None,
    }
}

fn object<'a>(
    value: &'a Value,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, HostRequestRefusal> {
    let map = value.as_object().ok_or_else(invalid)?;
    if let Some(key) = map.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(HostRequestRefusal {
            rpc_code: host_rpc_code(codes::HOST_UNSUPPORTED_OPTION),
            code: codes::HOST_UNSUPPORTED_OPTION,
            message: "Unsupported workflow option.",
            details: Some(json!({"option":key})),
        });
    }
    Ok(map)
}

fn text(map: &Map<String, Value>, key: &str, max: usize, required: bool, line: bool) -> bool {
    match map.get(key) {
        None => !required,
        Some(value) => value.as_str().is_some_and(|text| {
            text.len() <= max
                && (!required || !text.is_empty())
                && (!line || !text.chars().any(char::is_control))
        }),
    }
}
fn flag(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).is_none_or(Value::is_boolean)
}
fn key(map: &Map<String, Value>, name: &str) -> bool {
    map.get(name)
        .and_then(Value::as_str)
        .is_some_and(is_credential_key)
}
fn resource(value: &Value) -> bool {
    value.as_object().is_some_and(|map| {
        map.len() == 2
            && key(map, "rootID")
            && map.get("path").and_then(Value::as_str).is_some_and(|path| {
                path.len() <= 4096
                    && !path.contains(['\\', '\0', ':'])
                    && !path.starts_with('/')
                    && (path.is_empty()
                        || path
                            .split('/')
                            .all(|p| !p.is_empty() && p != "." && p != ".."))
            })
    })
}

/// Checks one backend request before the session forwards it to native code.
/// Unknown fields retain the protocol's distinct unsupported-option error.
pub fn check_params(method: &str, params: &Value) -> Result<(), HostRequestRefusal> {
    let name = method.strip_prefix("lithe/host/").unwrap_or(method);
    let fields: &[&str] = match name {
        "window/showQuickPick" => &["context", "title", "placeholder", "items", "canPickMany"],
        "window/showInputBox" => &[
            "context",
            "title",
            "prompt",
            "placeholder",
            "value",
            "password",
        ],
        "preview/openMarkdown" | "preview/openHTML" => &[
            "context",
            "resource",
            "text",
            "title",
            "id",
            "presentation",
            "range",
        ],
        "editor/showDiff" => &[
            "context",
            "title",
            "original",
            "modified",
            "originalTitle",
            "modifiedTitle",
        ],
        "views/refresh" => &["context", "instance"],
        "output/create" => &["context", "name"],
        "output/append" => &["context", "channelID", "text"],
        "output/clear" | "output/show" | "output/dispose" => &["context", "channelID"],
        "progress/create" => &["context", "title", "cancellable"],
        "progress/report" => &["context", "progressID", "message", "percentage"],
        "progress/end" => &["context", "progressID"],
        "statusBar/setItem" => &["context", "id", "text", "tooltip", "commandID", "priority"],
        "statusBar/removeItem" => &["context", "id"],
        "clipboard/readText" => &["context"],
        "clipboard/writeText" => &["context", "text"],
        "workspace/reveal" => &["context", "resource"],
        "workspace/setDirectoryTypes" => &["context", "types"],
        "workspace/setDirectoryMarks" => &["context", "marks"],
        "browser/openExternal" => &["context", "url", "resource", "unsaved"],
        "network/request" => &[
            "context",
            "url",
            "method",
            "headers",
            "body",
            "timeoutMilliseconds",
            "maximumResponseBytes",
        ],
        _ => return Err(invalid()),
    };
    let p = object(params, fields)?;
    if !p.get("context").is_some_and(is_context_reference) {
        return Err(invalid());
    }
    let valid = match name {
        "window/showQuickPick" => {
            let mut ids = BTreeSet::new();
            text(p, "title", 512, false, true)
                && text(p, "placeholder", 512, false, true)
                && flag(p, "canPickMany")
                && p.get("items")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items.len() <= 1000
                            && items.iter().all(|item| {
                                let Ok(item) = object(
                                    item,
                                    &["id", "label", "description", "detail", "picked"],
                                ) else {
                                    return false;
                                };
                                key(item, "id")
                                    && ids.insert(item["id"].as_str().unwrap_or_default())
                                    && text(item, "label", 512, true, true)
                                    && text(item, "description", 512, false, true)
                                    && text(item, "detail", 1024, false, true)
                                    && flag(item, "picked")
                            })
                    })
        }
        "window/showInputBox" => {
            text(p, "title", 512, false, true)
                && text(p, "prompt", 2048, false, true)
                && text(p, "placeholder", 512, false, true)
                && text(p, "value", 16 * 1024, false, true)
                && flag(p, "password")
                && !(p.get("password") == Some(&Value::Bool(true))
                    && p.get("value")
                        .and_then(Value::as_str)
                        .is_some_and(|v| !v.is_empty()))
        }
        "preview/openMarkdown" | "preview/openHTML" => {
            let source = match (p.get("resource"), p.get("text")) {
                (Some(value), None) => resource(value),
                (None, Some(_)) => {
                    text(p, "text", TEXT_BYTES, false, false)
                        && key(p, "id")
                        && text(p, "title", 256, true, true)
                }
                _ => false,
            };
            source
                && p.get("presentation")
                    .is_none_or(|v| matches!(v.as_str(), Some("editorAndPreview" | "previewOnly")))
                && p.get("range").is_none_or(|v| {
                    let position = |v: &Value| {
                        v.as_object().is_some_and(|o| {
                            o.len() == 2
                                && ["line", "column"].iter().all(|key| {
                                    o.get(*key)
                                        .and_then(Value::as_u64)
                                        .is_some_and(|n| n > 0 && n <= u32::MAX as u64)
                                })
                        })
                    };
                    v.as_object().is_some_and(|o| {
                        o.len() == 2
                            && o.get("start").is_some_and(position)
                            && o.get("end").is_some_and(position)
                            && (o["start"]["line"].as_u64(), o["start"]["column"].as_u64())
                                <= (o["end"]["line"].as_u64(), o["end"]["column"].as_u64())
                    })
                })
        }
        "editor/showDiff" => {
            ["original", "modified"].iter().all(|k| {
                p.get(*k)
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.len() <= TEXT_BYTES)
            }) && ["title", "originalTitle", "modifiedTitle"]
                .iter()
                .all(|k| text(p, k, 256, false, true))
        }
        "views/refresh" => key(p, "instance"),
        "output/create" => text(p, "name", 256, true, true),
        "output/append" => {
            key(p, "channelID")
                && text(p, "text", TEXT_BYTES, false, false)
                && p.contains_key("text")
        }
        "output/clear" | "output/show" | "output/dispose" => key(p, "channelID"),
        "progress/create" => text(p, "title", 256, true, true) && flag(p, "cancellable"),
        "progress/report" => {
            key(p, "progressID")
                && text(p, "message", 1024, false, true)
                && p.get("percentage").is_none_or(|v| {
                    v.as_f64()
                        .is_some_and(|n| n.is_finite() && (0.0..=100.0).contains(&n))
                })
        }
        "progress/end" => key(p, "progressID"),
        "statusBar/setItem" => {
            key(p, "id")
                && text(p, "text", 256, true, true)
                && text(p, "tooltip", 1024, false, true)
                && p.get("commandID")
                    .is_none_or(|v| v.as_str().is_some_and(is_command_id))
                && p.get("priority")
                    .is_none_or(|v| v.as_i64().is_some_and(|n| (-1000..=1000).contains(&n)))
        }
        "statusBar/removeItem" => key(p, "id"),
        "clipboard/readText" => true,
        "clipboard/writeText" => {
            p.contains_key("text") && text(p, "text", TEXT_BYTES, false, false)
        }
        "workspace/reveal" => p.get("resource").is_some_and(resource),
        "workspace/setDirectoryTypes" => directory_types_valid(p),
        "workspace/setDirectoryMarks" => directory_marks_valid(p),
        "browser/openExternal" => match (p.get("url"), p.get("resource")) {
            (Some(Value::String(url)), None) => !p.contains_key("unsaved") && http_address(url),
            (None, Some(value)) => {
                resource(value)
                    && p.get("unsaved").is_none_or(|v| {
                        matches!(v.as_str(), Some("openSavedFile" | "requireSaved"))
                    })
            }
            _ => false,
        },
        "network/request" => network_valid(p),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn directory_types_valid(p: &Map<String, Value>) -> bool {
    let Some(items) = p.get("types").and_then(Value::as_array) else {
        return false;
    };
    if items.len() > 32 {
        return false;
    }
    let mut ids = BTreeSet::new();
    items.iter().all(|item| {
        let Ok(obj) = object(
            item,
            &[
                "id",
                "name",
                "tag",
                "icon",
                "color",
                "tintBg",
                "tintText",
                "inheritBg",
                "excludeIndexing",
                "readOnlyHint",
                "matchGlob",
            ],
        ) else {
            return false;
        };
        let valid_id = obj
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| is_plugin_directory_type_id(id) && ids.insert(id.to_owned()));
        let valid_icon = obj.get("icon").and_then(Value::as_str).is_some_and(|ic| {
            matches!(
                ic,
                "folder"
                    | "sources"
                    | "tests"
                    | "resources"
                    | "generated"
                    | "excluded"
                    | "module"
                    | "package"
                    | "star"
                    | "book"
                    | "archive"
                    | "database"
                    | "gear"
            )
        });
        let valid_color = obj
            .get("color")
            .and_then(Value::as_str)
            .is_some_and(is_hex_color);
        valid_id
            && text(obj, "name", 64, true, true)
            && text(obj, "tag", 24, false, true)
            && valid_icon
            && valid_color
            && flag(obj, "tintBg")
            && flag(obj, "tintText")
            && flag(obj, "inheritBg")
            && flag(obj, "excludeIndexing")
            && flag(obj, "readOnlyHint")
            && text(obj, "matchGlob", 256, false, true)
    })
}

fn directory_marks_valid(p: &Map<String, Value>) -> bool {
    let Some(items) = p.get("marks").and_then(Value::as_array) else {
        return false;
    };
    if items.len() > 500 {
        return false;
    }
    let mut paths = BTreeSet::new();
    items.iter().all(|item| {
        let Ok(obj) = object(item, &["path", "role", "color"]) else {
            return false;
        };
        let valid_path = obj.get("path").and_then(Value::as_str).is_some_and(|raw| {
            !raw.is_empty()
                && raw.len() <= 1024
                && !raw.starts_with('/')
                && !raw.ends_with('/')
                && !raw.contains('\\')
                && !raw.chars().any(char::is_control)
                && raw
                    .split('/')
                    .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
                && paths.insert(raw.to_owned())
        });
        let valid_role = obj.get("role").and_then(Value::as_str).is_some_and(|role| {
            matches!(
                role,
                "plain" | "sources" | "tests" | "resources" | "testResources" | "generated" | "excluded"
            ) || is_plugin_directory_type_id(role)
        });
        let valid_color = obj
            .get("color")
            .is_none_or(|v| v.as_str().is_some_and(is_hex_color));
        valid_path && valid_role && valid_color
    })
}

fn is_plugin_directory_type_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("plugin.") else {
        return false;
    };
    !rest.is_empty()
        && id.len() <= 64
        && rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

fn is_hex_color(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

fn http_address(address: &str) -> bool {
    address.len() <= 8192
        && !address.chars().any(char::is_control)
        && url::Url::parse(address).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn network_valid(p: &Map<String, Value>) -> bool {
    let address = p
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| http_address(s))
        .and_then(|s| url::Url::parse(s).ok());
    let url_valid = address.is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    });
    let method = p.get("method").is_none_or(|v| {
        v.as_str().is_some_and(|s| {
            matches!(
                s,
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
            )
        })
    });
    let headers = p.get("headers").is_none_or(|headers| {
        headers.as_object().is_some_and(|headers| {
            headers.len() <= 64
                && headers.iter().all(|(name, value)| {
                    !name.is_empty()
                        && name.len() <= 128
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                        && !matches!(
                            name.to_ascii_lowercase().as_str(),
                            "host"
                                | "connection"
                                | "content-length"
                                | "transfer-encoding"
                                | "proxy-authorization"
                                | "proxy-connection"
                                | "upgrade"
                                | "trailer"
                                | "te"
                        )
                        && value.as_str().is_some_and(|v| {
                            v.len() <= 8192
                                && !v.chars().any(|c| c == '\r' || c == '\n' || c == '\0')
                        })
                })
        })
    });
    url_valid
        && method
        && headers
        && text(p, "body", TEXT_BYTES, false, false)
        && p.get("timeoutMilliseconds")
            .is_none_or(|v| v.as_u64().is_some_and(|n| (1..=30_000).contains(&n)))
        && p.get("maximumResponseBytes").is_none_or(|v| {
            v.as_u64()
                .is_some_and(|n| (1..=TEXT_BYTES as u64).contains(&n))
        })
}

fn core_invalid() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRequest,
        "Invalid plugin workflow projection.",
    )
}
fn cut_tail(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Plans native requests, filters bounded quick-pick candidates, and splits one
/// output increment. No full output history crosses the application boundary.
pub fn command(payload: Value) -> Result<Value, CoreError> {
    let p = payload.as_object().ok_or_else(core_invalid)?;
    match p.get("operation").and_then(Value::as_str) {
        Some("prepare") => {
            object(&payload, &["operation", "method", "params", "manifest"])
                .map_err(|_| core_invalid())?;
            let method = p
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(core_invalid)?;
            let mut params = p.get("params").cloned().ok_or_else(core_invalid)?;
            check_params(method, &params).map_err(|failure| {
                CoreError::new(ErrorCode::InvalidRequest, failure.message)
                    .with_details(failure.code)
            })?;
            let name = method.strip_prefix("lithe/host/").unwrap_or(method);
            if name == "statusBar/setItem" {
                if let Some(id) = params.get("commandID").and_then(Value::as_str) {
                    let declared = p
                        .get("manifest")
                        .and_then(|m| m.pointer("/ui/commands"))
                        .and_then(Value::as_array)
                        .is_some_and(|commands| {
                            commands.iter().any(|c| c["id"].as_str() == Some(id))
                        });
                    if !declared {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The status command is not declared by this plugin.",
                        )
                        .with_details(codes::HOST_INVALID_PARAMS));
                    }
                }
            }
            if matches!(
                name,
                "preview/openMarkdown" | "preview/openHTML" | "browser/openExternal"
            ) {
                if let Some(path) = params.pointer("/resource/path").and_then(Value::as_str) {
                    let extension = path
                        .rsplit('.')
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let supported = if name == "preview/openMarkdown" {
                        matches!(extension.as_str(), "md" | "markdown" | "mdown")
                    } else {
                        matches!(extension.as_str(), "html" | "htm" | "xhtml")
                    };
                    if !supported {
                        return Err(CoreError::new(
                            ErrorCode::InvalidRequest,
                            "The resource does not support this preview kind.",
                        )
                        .with_details(codes::HOST_UNSUPPORTED_OPTION));
                    }
                }
                if name.starts_with("preview/") {
                    params
                        .as_object_mut()
                        .ok_or_else(core_invalid)?
                        .entry("presentation")
                        .or_insert(json!("editorAndPreview"));
                } else if params.get("resource").is_some() {
                    params
                        .as_object_mut()
                        .ok_or_else(core_invalid)?
                        .entry("unsaved")
                        .or_insert(json!("requireSaved"));
                }
            }
            if method.ends_with("network/request") {
                let map = params.as_object_mut().ok_or_else(core_invalid)?;
                map.entry("method").or_insert(json!("GET"));
                map.entry("headers").or_insert(json!({}));
                map.entry("timeoutMilliseconds").or_insert(json!(30_000));
                map.entry("maximumResponseBytes")
                    .or_insert(json!(TEXT_BYTES));
            }
            Ok(
                json!({"params":params,"limits":{"textBytes":TEXT_BYTES,"httpHeaderBytes":HTTP_HEADER_BYTES,"outputLines":OUTPUT_LINES,"outputBytes":OUTPUT_BYTES,"resources":RESOURCE_LIMIT}}),
            )
        }
        Some("quickPick") => {
            object(&payload, &["operation", "items", "query"]).map_err(|_| core_invalid())?;
            let items = p
                .get("items")
                .and_then(Value::as_array)
                .filter(|v| v.len() <= 1000)
                .ok_or_else(core_invalid)?;
            let query = p
                .get("query")
                .and_then(Value::as_str)
                .filter(|q| q.len() <= 1024)
                .ok_or_else(core_invalid)?
                .to_lowercase();
            let words: Vec<_> = query.split_whitespace().collect();
            let indices: Vec<_> = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let search = ["label", "description", "detail"]
                        .iter()
                        .filter_map(|k| item.get(*k).and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase();
                    words
                        .iter()
                        .all(|word| search.contains(word))
                        .then_some(index)
                })
                .collect();
            Ok(json!({"indices":indices}))
        }
        Some("status") => {
            object(&payload, &["operation", "items"]).map_err(|_| core_invalid())?;
            let items = p
                .get("items")
                .and_then(Value::as_array)
                .filter(|items| items.len() <= RESOURCE_LIMIT * 16)
                .ok_or_else(core_invalid)?;
            let mut order = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let item = object(item, &["id", "priority"]).map_err(|_| core_invalid())?;
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| id.len() <= 512)
                    .ok_or_else(core_invalid)?;
                let priority = item
                    .get("priority")
                    .and_then(Value::as_i64)
                    .filter(|n| (-1000..=1000).contains(n))
                    .ok_or_else(core_invalid)?;
                order.push((index, priority, id));
            }
            order.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(b.2)));
            Ok(json!({"ids":order.into_iter().map(|(_,_,id)|id).collect::<Vec<_>>()}))
        }
        Some("input") => {
            object(&payload, &["operation", "pick", "value"]).map_err(|_| core_invalid())?;
            let pick = p
                .get("pick")
                .and_then(Value::as_bool)
                .ok_or_else(core_invalid)?;
            let value = p
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(core_invalid)?;
            let valid = value.len() <= if pick { 1024 } else { 16 * 1024 }
                && !value.chars().any(char::is_control);
            Ok(json!({"valid":valid}))
        }
        Some("treePage") => {
            object(
                &payload,
                &[
                    "operation",
                    "cursor",
                    "lastCursor",
                    "count",
                    "lastCount",
                    "rows",
                    "lastVisible",
                ],
            )
            .map_err(|_| core_invalid())?;
            let cursor = p
                .get("cursor")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 256)
                .ok_or_else(core_invalid)?;
            let last = p
                .get("lastCursor")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= 256)
                .ok_or_else(core_invalid)?;
            let number = |key| {
                p.get(key)
                    .and_then(Value::as_u64)
                    .filter(|n| *n <= 2000)
                    .ok_or_else(core_invalid)
            };
            let count = number("count")?;
            let previous = number("lastCount")?;
            let visible = number("lastVisible")?;
            let rows = number("rows")?;
            // A page advances the cursor and grows the retained range. A plugin
            // replaces its full loaded range or patches it; the host never merges pages.
            Ok(
                json!({"load":!cursor.is_empty() && cursor!=last && (last.is_empty() || count>previous) && rows>0 && visible>=rows.saturating_sub(8)}),
            )
        }
        Some("output") => {
            object(&payload, &["operation", "open", "text"]).map_err(|_| core_invalid())?;
            let open = p
                .get("open")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= OUTPUT_LINE_BYTES)
                .ok_or_else(core_invalid)?;
            let text = p
                .get("text")
                .and_then(Value::as_str)
                .filter(|s| s.len() <= TEXT_BYTES)
                .ok_or_else(core_invalid)?;
            let joined = format!("{open}{text}");
            let mut lines: Vec<&str> = joined.split('\n').collect();
            let last = lines.pop().unwrap_or_default();
            let lines: Vec<_> = lines
                .into_iter()
                .map(|line| cut_tail(line.trim_end_matches('\r'), OUTPUT_LINE_BYTES))
                .collect();
            Ok(
                json!({"lines":lines,"open":cut_tail(last,OUTPUT_LINE_BYTES),"maxLines":OUTPUT_LINES,"maxBytes":OUTPUT_BYTES}),
            )
        }
        _ => Err(core_invalid()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{host_request, HostRoute};

    #[test]
    fn grants_are_individual_and_workflow_params_are_strict() {
        let method = "lithe/host/window/showInputBox";
        let context = json!({"contextID":"context-1","generation":1});
        let params = json!({"context":context,"password":true});
        assert_eq!(
            host_request(
                method,
                Some(&params),
                Some(0),
                &["window".to_string()].into_iter().collect()
            )
            .unwrap_err()
            .code,
            codes::HOST_PERMISSION_DENIED
        );
        let grants = [method.to_string()].into_iter().collect();
        assert_eq!(
            host_request(method, Some(&params), Some(0), &grants).unwrap(),
            HostRoute::Forward("window")
        );
        let mut unknown = params.clone();
        unknown["secret"] = json!("hidden");
        assert_eq!(
            check_params(method, &unknown).unwrap_err().code,
            codes::HOST_UNSUPPORTED_OPTION
        );
        let mut seeded = params;
        seeded["value"] = json!("do not reveal stored secrets");
        assert!(check_params(method, &seeded).is_err());
    }

    #[test]
    fn managed_http_rejects_unsafe_headers_urls_and_budgets() {
        let base = json!({"context":{"contextID":"c","generation":1},"url":"https://example.test/query","method":"POST","headers":{"Authorization":"credential","Content-Type":"application/json"},"body":"{}"});
        assert!(check_params("network/request", &base).is_ok());
        for patch in [
            json!({"url":"file:///etc/hosts"}),
            json!({"url":"https://user:secret@example.test"}),
            json!({"headers":{"Host":"other.test"}}),
            json!({"headers":{"X-Test":"bad\r\nheader"}}),
            json!({"maximumResponseBytes":TEXT_BYTES+1}),
            json!({"timeoutMilliseconds":30001}),
        ] {
            let mut value = base.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert!(check_params("network/request", &value).is_err());
        }
        let planned =
            command(json!({"operation":"prepare","method":"network/request","params":base}))
                .unwrap();
        assert_eq!(planned["params"]["maximumResponseBytes"], TEXT_BYTES);
    }

    #[test]
    fn projections_keep_search_order_and_bound_unfinished_output() {
        let filtered = command(json!({"operation":"quickPick","items":[{"label":"Alpha","detail":"Rust"},{"label":"Beta","description":"Rust"},{"label":"Gamma"}],"query":"rust beta"})).unwrap();
        assert_eq!(filtered["indices"], json!([1]));
        assert_eq!(command(json!({"operation":"status","items":[{"id":"b","priority":2},{"id":"a","priority":2},{"id":"c","priority":3}]})).unwrap()["ids"],json!(["c","a","b"]));
        assert_eq!(command(json!({"operation":"treePage","cursor":"next","lastCursor":"previous","count":20,"lastCount":20,"rows":20,"lastVisible":19})).unwrap()["load"],false);
        assert_eq!(
            command(json!({"operation":"input","pick":false,"value":"a\nb"})).unwrap()["valid"],
            false
        );
        let result = command(json!({"operation":"output","open":"first","text":format!(" second\n{}", "字".repeat(20_000))})).unwrap();
        assert_eq!(result["lines"], json!(["first second"]));
        assert!(result["open"].as_str().unwrap().len() <= OUTPUT_LINE_BYTES);
    }

    #[test]
    fn native_plans_check_preview_type_and_own_command_before_side_effects() {
        let context = json!({"contextID":"workspace","generation":1});
        let preview = json!({"operation":"prepare","method":"preview/openHTML","params":{
            "context":context,"resource":{"rootID":"root","path":"notes.txt"}}});
        assert!(command(preview.clone()).is_err());
        let mut valid = preview;
        valid["params"]["resource"]["path"] = json!("page.HTML");
        assert_eq!(
            command(valid).unwrap()["params"]["presentation"],
            "editorAndPreview"
        );
        let mut browser = json!({"operation":"prepare","method":"browser/openExternal","params":{
            "context":context,"resource":{"rootID":"root","path":"page.html"}}});
        assert_eq!(
            command(browser.clone()).unwrap()["params"]["unsaved"],
            "requireSaved"
        );
        browser["params"]["unsaved"] = json!("openSavedFile");
        assert_eq!(
            command(browser).unwrap()["params"]["unsaved"],
            "openSavedFile"
        );
        let mut status = json!({"operation":"prepare","method":"statusBar/setItem","params":{
            "context":context,"id":"state","text":"Ready","commandID":"run"},
            "manifest":{"ui":{"commands":[{"id":"other"}]}}});
        assert!(command(status.clone()).is_err());
        status["manifest"]["ui"]["commands"][0]["id"] = json!("run");
        assert!(command(status).is_ok());

        let valid_types = json!({
            "operation": "prepare",
            "method": "workspace/setDirectoryTypes",
            "params": {
                "context": context,
                "types": [{
                    "id": "plugin.migrations",
                    "name": "SQL Migrations",
                    "tag": "migrations",
                    "icon": "database",
                    "color": "#0d9488",
                    "tintBg": true
                }]
            }
        });
        assert!(command(valid_types).is_ok());

        let invalid_type_id = json!({
            "operation": "prepare",
            "method": "workspace/setDirectoryTypes",
            "params": {
                "context": context,
                "types": [{
                    "id": "custom.notallowed",
                    "name": "Bad",
                    "icon": "folder",
                    "color": "#0d9488"
                }]
            }
        });
        assert!(command(invalid_type_id).is_err());

        let valid_marks = json!({
            "operation": "prepare",
            "method": "workspace/setDirectoryMarks",
            "params": {
                "context": context,
                "marks": [
                    {"path": "db/migrations", "role": "plugin.migrations"},
                    {"path": "src/main", "role": "sources", "color": "#3b82f6"}
                ]
            }
        });
        assert!(command(valid_marks).is_ok());

        let invalid_mark_path = json!({
            "operation": "prepare",
            "method": "workspace/setDirectoryMarks",
            "params": {
                "context": context,
                "marks": [{"path": "../escape", "role": "sources"}]
            }
        });
        assert!(command(invalid_mark_path).is_err());
    }

    #[test]
    fn workflow_fixture_covers_projection_and_paging_boundaries() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../fixtures/plugins/workflow-v1.json"))
                .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                command(case["request"].clone()).unwrap(),
                case["result"],
                "{}",
                case["name"]
            );
        }
    }
}
