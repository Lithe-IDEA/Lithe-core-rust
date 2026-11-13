//! Bounded recent-file and recent-edit history, independent of native storage and widgets.

use crate::protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_ENTRIES: usize = 80;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    path: String,
    name: String,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    column: u32,
    /// Logical recency sequence supplied by the previous reduction, never wall time.
    #[serde(default)]
    opened: u64,
    #[serde(default)]
    edited: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Event {
    kind: String,
    path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    line: u32,
    #[serde(default)]
    column: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    #[serde(default)]
    entries: Vec<Entry>,
    event: Option<Event>,
    #[serde(default)]
    query: String,
    #[serde(default)]
    only_edited: bool,
}

fn valid_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 32_768 && !path.contains(['\0', '\r', '\n'])
}

pub(crate) fn reduce_command(payload: Value) -> Result<Value, CoreError> {
    let mut request: Request = serde_json::from_value(payload)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "Invalid recent-file request"))?;
    if request.entries.len() > MAX_ENTRIES || request.query.len() > 4096 {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Recent-file request exceeds its limit",
        ));
    }
    request
        .entries
        .retain(|e| valid_path(&e.path) && e.name.len() <= 1024);
    if let Some(event) = request.event {
        if !valid_path(&event.path) || event.name.len() > 1024 {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Invalid recent-file identity",
            ));
        }
        let previous = request
            .entries
            .iter()
            .find(|e| e.path == event.path)
            .cloned();
        let sequence = request
            .entries
            .iter()
            .map(|e| e.opened.max(e.edited))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        request.entries.retain(|e| e.path != event.path);
        match event.kind.as_str() {
            "removed" => {}
            "opened" | "edited" => {
                request.entries.insert(
                    0,
                    Entry {
                        path: event.path,
                        name: event.name,
                        line: event.line,
                        column: event.column,
                        opened: sequence,
                        edited: if event.kind == "edited" {
                            sequence
                        } else {
                            previous.map_or(0, |e| e.edited)
                        },
                    },
                );
            }
            _ => {
                return Err(CoreError::new(
                    ErrorCode::InvalidRequest,
                    "Unknown recent-file event",
                ))
            }
        }
    }
    request
        .entries
        .sort_by(|a, b| b.opened.cmp(&a.opened).then(a.path.cmp(&b.path)));
    let mut seen = std::collections::BTreeSet::new();
    request
        .entries
        .retain(|entry| seen.insert(entry.path.clone()));
    request.entries.truncate(MAX_ENTRIES);
    let words: Vec<_> = request
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut results: Vec<_> = request
        .entries
        .iter()
        .filter(|entry| {
            if request.only_edited && entry.edited == 0 {
                return false;
            }
            let text = format!("{} {}", entry.name, entry.path).to_lowercase();
            words.iter().all(|word| text.contains(word))
        })
        .cloned()
        .collect();
    if request.only_edited {
        results.sort_by(|a, b| b.edited.cmp(&a.edited).then(a.path.cmp(&b.path)));
    }
    Ok(json!({"entries":request.entries,"results":results}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_another_file_does_not_reorder_the_recent_edits() {
        let mut state = json!([]);
        for (kind, path) in [("edited", "a"), ("edited", "b"), ("opened", "a")] {
            state = reduce_command(
                json!({"entries":state,"event":{"kind":kind,"path":path,"name":path}}),
            )
            .unwrap()["entries"]
                .clone();
        }
        let result = reduce_command(json!({"entries":state,"onlyEdited":true})).unwrap();
        assert_eq!(result["entries"][0]["path"], "a");
        assert_eq!(result["results"][0]["path"], "b");
        assert_eq!(result["entries"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn query_and_removal_keep_history_bounded_and_empty_results_valid() {
        let state = json!([{"path":"src/Alpha.go","name":"Alpha.go","opened":1}]);
        assert_eq!(
            reduce_command(json!({"entries":state,"query":"SRC alpha"})).unwrap()["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            reduce_command(
                json!({"entries":state,"event":{"kind":"removed","path":"src/Alpha.go"}})
            )
            .unwrap()["entries"],
            json!([])
        );
    }
}
