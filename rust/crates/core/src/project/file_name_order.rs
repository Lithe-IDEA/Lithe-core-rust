//! `workspace.orderEntries`: orders natively enumerated siblings with the shared
//! file name order in `lithe_core_rules::file_names`.

use crate::protocol::{CoreError, ErrorCode};
use lithe_core_rules::file_names::FileNameKey;
use serde::Deserialize;
use serde_json::{json, Value};

/// Upper bound for one `workspace.orderEntries` request, matching the plugin directory scan
/// budget; hosts send one directory at a time.
const MAX_ORDER_ENTRIES: usize = 100_000;
/// Longest accepted sibling name, in UTF-8 bytes; longer names are rejected, never truncated.
const MAX_ORDER_NAME_BYTES: usize = 4_096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Siblings of one natively enumerated directory, in enumeration order.
struct OrderEntriesRequest {
    entries: Vec<OrderEntry>,
    /// Ranks directories before files, the Project view default; `false` orders by name only.
    #[serde(default = "directories_first_default")]
    directories_first: bool,
}

fn directories_first_default() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One sibling name and its kind; the host keeps the full entry and only uses the index.
struct OrderEntry {
    name: String,
    is_directory: bool,
}

/// Orders natively enumerated siblings for `workspace.orderEntries`.
///
/// Returns `{ order }`, the request indices in display order. The order is total and
/// deterministic: names compare with IDEA's `FileNameComparator`, and identical names
/// keep their request order. Oversized requests fail with `invalid_request`.
pub(crate) fn order_entries_command(payload: Value) -> Result<Value, CoreError> {
    let request: OrderEntriesRequest = serde_json::from_value(payload).map_err(|error| {
        CoreError::new(ErrorCode::InvalidRequest, "Invalid entry order request")
            .with_details(error.to_string())
    })?;
    if request.entries.len() > MAX_ORDER_ENTRIES {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Too many entries to order",
        ));
    }
    if request
        .entries
        .iter()
        .any(|entry| entry.name.len() > MAX_ORDER_NAME_BYTES)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Entry name is too long to order",
        ));
    }
    let directories_first = request.directories_first;
    let mut keyed = request
        .entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                directories_first && !entry.is_directory,
                FileNameKey::new(&entry.name),
                index,
            )
        })
        .collect::<Vec<_>>();
    keyed.sort();
    Ok(json!({ "order": keyed.into_iter().map(|(_, _, index)| index).collect::<Vec<_>>() }))
}
