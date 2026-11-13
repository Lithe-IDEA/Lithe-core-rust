//! Recovery decisions and storage budgets; native adapters own atomic file I/O.

use crate::protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    /// Checkpointed content and the disk version it was edited against.
    base_hash: String,
    disk_hash: Option<String>,
    #[serde(default)]
    scratch: bool,
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    discarded: bool,
}

pub(crate) fn decide_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload)
        .map_err(|_| CoreError::new(ErrorCode::InvalidRequest, "Invalid recovery decision"))?;
    if request.base_hash.len() > 128 || request.disk_hash.as_ref().is_some_and(|h| h.len() > 128) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid recovery identity",
        ));
    }
    let action = if request.saved || request.discarded {
        "remove"
    } else if request.bytes > 64 * 1024 * 1024 {
        "tooLarge"
    } else if request.scratch {
        "restoreScratch"
    } else if request.disk_hash.as_deref() == Some(request.base_hash.as_str()) {
        "restore"
    } else {
        // A changed or deleted source never gets silently overwritten by a checkpoint.
        "restoreConflict"
    };
    Ok(
        json!({"action":action,"maxDocumentBytes":64*1024*1024,"maxStorageBytes":256*1024*1024,"maxDocuments":64}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_or_deleted_disk_files_restore_as_conflicts() {
        for disk in [json!(null), json!("new")] {
            assert_eq!(
                decide_command(json!({"baseHash":"old","diskHash":disk})).unwrap()["action"],
                "restoreConflict"
            );
        }
        assert_eq!(
            decide_command(json!({"baseHash":"old","diskHash":"old"})).unwrap()["action"],
            "restore"
        );
    }
    #[test]
    fn scratch_and_saved_checkpoints_have_different_lifecycles() {
        assert_eq!(
            decide_command(json!({"baseHash":"","scratch":true})).unwrap()["action"],
            "restoreScratch"
        );
        assert_eq!(
            decide_command(json!({"baseHash":"","scratch":true,"saved":true})).unwrap()["action"],
            "remove"
        );
        assert_eq!(
            decide_command(json!({"baseHash":"","bytes":64*1024*1024+1})).unwrap()["action"],
            "tooLarge"
        );
    }
}
