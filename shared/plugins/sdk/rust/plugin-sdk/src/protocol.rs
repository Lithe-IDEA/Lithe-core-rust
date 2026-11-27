//! Wire constants and validation shared with Lithe's Rust Core
//! (`rust/crates/plugin-rpc/src/` in the Lithe
//! repository). The SDK checks the same rules locally so a plugin author sees
//! a mistake when declaring a function instead of as a refused session.

use serde_json::Value;

/// The plugin-function protocol version this SDK answers: plugin functions,
/// host services, workspace contexts, capability discovery, host
/// notifications, `$/cancelRequest` for host requests, and calls to other
/// plugins' functions through Core's relay.
pub const FUNCTIONS_VERSION: u64 = 0;
/// The `litheLogging` version this SDK requires and answers.
pub const LOGGING_VERSION: u64 = 0;
/// The `litheKeymapImport` version this SDK answers.
pub const KEYMAP_IMPORT_VERSION: u64 = 0;
/// The newest `litheBuild` version this SDK answers. The protocol is a
/// version of `buildProviders`: version 1 adds `lithe/build/runEntrypoints`
/// and changes nothing of version 0.
pub const BUILD_VERSION: u64 = 1;

/// Whether an `initialize` offer, `{ "versions": [...] }`, lists `version`.
pub(crate) fn offers(offer: Option<&Value>, version: u64) -> bool {
    offer
        .and_then(|offer| offer.get("versions"))
        .and_then(Value::as_array)
        .is_some_and(|versions| versions.iter().any(|entry| entry.as_u64() == Some(version)))
}

pub(crate) mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const SHUTDOWN: &str = "shutdown";
    pub const EXIT: &str = "exit";
    pub const CANCEL_REQUEST: &str = "$/cancelRequest";
    pub const FUNCTION_INVOKE: &str = "lithe/function/invoke";
    pub const FUNCTION_REGISTER: &str = "lithe/function/register";
    pub const FUNCTION_UNREGISTER: &str = "lithe/function/unregister";
    pub const FUNCTION_PROGRESS: &str = "lithe/function/progress";
    pub const TERMINAL_CREATE: &str = "lithe/host/terminal/create";
    pub const TERMINAL_SHOW: &str = "lithe/host/terminal/show";
    pub const TERMINAL_CLOSE: &str = "lithe/host/terminal/close";
    pub const TERMINAL_REOPEN: &str = "lithe/host/terminal/reopen";
    pub const TERMINAL_DISPOSE: &str = "lithe/host/terminal/dispose";
    pub const TERMINAL_ON_EXIT: &str = "lithe/host/terminal/onExit";
    pub const HOST_PREFIX: &str = "lithe/host/";
    pub const CREDENTIALS_GET: &str = "lithe/host/credentials/get";
    pub const CREDENTIALS_SET: &str = "lithe/host/credentials/set";
    pub const CREDENTIALS_DELETE: &str = "lithe/host/credentials/delete";
    pub const GET_CAPABILITIES: &str = "lithe/host/getCapabilities";
    pub const SUBSCRIPTIONS_DISPOSE: &str = "lithe/host/subscriptions/dispose";
    pub const WORKSPACE_LIST_CONTEXTS: &str = "lithe/host/workspace/listContexts";
    pub const WORKSPACE_GET_CONTEXT: &str = "lithe/host/workspace/getContext";
    pub const WORKSPACE_GET_FOLDERS: &str = "lithe/host/workspace/getFolders";
    pub const WORKSPACE_GET_HOST_PATHS: &str = "lithe/host/workspace/getHostPaths";
    pub const WORKSPACE_READ_DIRECTORY: &str = "lithe/host/workspace/readDirectory";
    pub const WORKSPACE_STAT: &str = "lithe/host/workspace/stat";
    pub const WORKSPACE_READ_FILE: &str = "lithe/host/workspace/readFile";
    pub const WORKSPACE_WATCH: &str = "lithe/host/workspace/watch";
    pub const WORKSPACE_FIND_FILES: &str = "lithe/host/workspace/findFiles";
    pub const WORKSPACE_FIND_TEXT: &str = "lithe/host/workspace/findText";
    pub const WORKSPACE_CREATE: &str = "lithe/host/workspace/create";
    pub const WORKSPACE_RENAME: &str = "lithe/host/workspace/rename";
    pub const WORKSPACE_COPY: &str = "lithe/host/workspace/copy";
    pub const WORKSPACE_TRASH: &str = "lithe/host/workspace/trash";
    pub const DOCUMENTS_LIST: &str = "lithe/host/documents/list";
    pub const DOCUMENTS_GET_SNAPSHOT: &str = "lithe/host/documents/getSnapshot";
    pub const DOCUMENTS_SAVE: &str = "lithe/host/documents/save";
    pub const DOCUMENTS_SAVE_AS: &str = "lithe/host/documents/saveAs";
    pub const DOCUMENTS_CLOSE: &str = "lithe/host/documents/close";
    pub const EDITOR_REGISTER_ACTIONS: &str = "lithe/host/editor/registerActions";
    pub const EDITOR_GET_ACTIVE: &str = "lithe/host/editor/getActive";
    pub const EDITOR_GET_SELECTIONS: &str = "lithe/host/editor/getSelections";
    pub const EDITOR_APPLY_EDITS: &str = "lithe/host/editor/applyEdits";
    pub const EDITOR_REPLACE_SELECTIONS: &str = "lithe/host/editor/replaceSelections";
    pub const EDITOR_INSERT_TEXT: &str = "lithe/host/editor/insertText";
    pub const EDITOR_OPEN: &str = "lithe/host/editor/open";
    pub const EDITOR_REVEAL: &str = "lithe/host/editor/reveal";
    pub const WINDOW_SHOW_QUICK_PICK: &str = "lithe/host/window/showQuickPick";
    pub const WINDOW_SHOW_INPUT_BOX: &str = "lithe/host/window/showInputBox";
    pub const PREVIEW_OPEN_MARKDOWN: &str = "lithe/host/preview/openMarkdown";
    pub const EDITOR_SHOW_DIFF: &str = "lithe/host/editor/showDiff";
    pub const VIEWS_REFRESH: &str = "lithe/host/views/refresh";
    pub const OUTPUT_CREATE: &str = "lithe/host/output/create";
    pub const OUTPUT_APPEND: &str = "lithe/host/output/append";
    pub const OUTPUT_CLEAR: &str = "lithe/host/output/clear";
    pub const OUTPUT_SHOW: &str = "lithe/host/output/show";
    pub const OUTPUT_DISPOSE: &str = "lithe/host/output/dispose";
    pub const PROGRESS_CREATE: &str = "lithe/host/progress/create";
    pub const PROGRESS_REPORT: &str = "lithe/host/progress/report";
    pub const PROGRESS_END: &str = "lithe/host/progress/end";
    pub const STATUS_BAR_SET_ITEM: &str = "lithe/host/statusBar/setItem";
    pub const STATUS_BAR_REMOVE_ITEM: &str = "lithe/host/statusBar/removeItem";
    pub const CLIPBOARD_READ_TEXT: &str = "lithe/host/clipboard/readText";
    pub const CLIPBOARD_WRITE_TEXT: &str = "lithe/host/clipboard/writeText";
    pub const WORKSPACE_REVEAL: &str = "lithe/host/workspace/reveal";
    pub const WORKSPACE_SET_DIRECTORY_TYPES: &str = "lithe/host/workspace/setDirectoryTypes";
    pub const WORKSPACE_SET_DIRECTORY_MARKS: &str = "lithe/host/workspace/setDirectoryMarks";
    pub const NETWORK_REQUEST: &str = "lithe/host/network/request";
    pub const PREVIEW_OPEN_HTML: &str = "lithe/host/preview/openHTML";
    pub const BROWSER_OPEN_EXTERNAL: &str = "lithe/host/browser/openExternal";
    pub const WINDOW_SHOW_INFORMATION: &str = "lithe/host/window/showInformation";
    pub const WINDOW_SHOW_WARNING: &str = "lithe/host/window/showWarning";
    pub const WINDOW_SHOW_ERROR: &str = "lithe/host/window/showError";
    pub const DID_CHANGE_CONTEXTS: &str = "lithe/host/workspace/didChangeContexts";
    pub const SUBSCRIPTION_NOTIFY: &str = "lithe/host/subscriptions/notify";
    pub const COMMAND_DID_RUN: &str = "lithe/host/commands/didRun";
    pub const FUNCTIONS_PROVIDERS: &str = "lithe/host/functions/providers";
    pub const AGENTS_WRITE: &str = "lithe/host/agents/write";
    pub const AGENTS_REPORT: &str = "lithe/host/agents/report";
    pub const AGENTS_USAGE: &str = "lithe/host/agents/usage";
    pub const AGENTS_DID_START: &str = "lithe/host/agents/didStart";
    pub const AGENTS_DID_OUTPUT: &str = "lithe/host/agents/didOutput";
    pub const AGENTS_DID_PROMPT: &str = "lithe/host/agents/didPrompt";
    pub const AGENTS_DID_CANCEL: &str = "lithe/host/agents/didCancel";
    pub const AGENTS_DID_DECIDE_PERMISSION: &str = "lithe/host/agents/didDecidePermission";
    pub const AGENTS_DID_EXIT: &str = "lithe/host/agents/didExit";
    pub const FUNCTIONS_INVOKE: &str = "lithe/host/functions/invoke";
    pub const BUILD_MODEL: &str = "lithe/build/model";
    pub const BUILD_DEPENDENCIES: &str = "lithe/build/dependencies";
    pub const BUILD_EXECUTE: &str = "lithe/build/execute";
    pub const BUILD_TEST: &str = "lithe/build/test";
    pub const BUILD_DISCOVER_TESTS: &str = "lithe/build/discoverTests";
    pub const BUILD_RUN_ENTRYPOINTS: &str = "lithe/build/runEntrypoints";
    pub const BUILD_PREPARE_RUN: &str = "lithe/build/prepareRun";
    pub const BUILD_PREPARE_DEBUG: &str = "lithe/build/prepareDebug";
    pub const BUILD_PROGRESS: &str = "lithe/build/progress";
}

/// JSON-RPC and LSP error codes the SDK answers with.
pub(crate) mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    pub const REQUEST_FAILED: i64 = -32000;
    pub const REQUEST_CANCELLED: i64 = -32800;
}

pub(crate) const MAX_FUNCTIONS: usize = 256;
const MAX_FUNCTION_ID_BYTES: usize = 128;
const MAX_PLUGIN_ERROR_CODE_BYTES: usize = 64;
pub(crate) const MAX_MESSAGE_BYTES: usize = 1024;
pub(crate) const MAX_RESULT_BYTES: usize = 1 << 20;
/// A build result document, including the complete `output` of a task or test.
pub(crate) const MAX_BUILD_RESULT_BYTES: usize = 32 << 20;
/// The `message` of one `lithe/build/progress` report.
pub(crate) const MAX_BUILD_MESSAGE_BYTES: usize = 4 * 1024;
/// The `output` of one `lithe/build/progress` report.
pub(crate) const MAX_BUILD_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_CREDENTIAL_KEY_BYTES: usize = 128;
pub(crate) const MAX_CREDENTIAL_SECRET_BYTES: usize = 16 * 1024;
/// Host requests a backend may have unanswered at once; later ones wait.
pub(crate) const MAX_PENDING_HOST_REQUESTS: usize = 16;
/// Events kept for subscriptions whose answer has not been read yet.
pub(crate) const MAX_EARLY_EVENTS: usize = 128;
const MAX_COMMAND_ID_BYTES: usize = 128;
/// The longest `parentOperationID` Core accepts on a relayed call.
const MAX_PARENT_OPERATION_ID_BYTES: usize = 128;
/// The most root IDs one `getHostPaths` request names.
pub(crate) const MAX_HOST_PATH_ROOTS: usize = 256;
/// The largest integer JSON carries exactly.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Dot-separated lowerCamelCase segments, for example `snapshot.refresh`.
pub fn is_function_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FUNCTION_ID_BYTES
        && value.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
                && characters.all(|character| character.is_ascii_alphanumeric())
        })
}

/// The plugin error code alphabet Core forwards as `pluginErrorCode`.
pub fn is_plugin_error_code(value: &str) -> bool {
    let mut characters = value.chars();
    value.len() <= MAX_PLUGIN_ERROR_CODE_BYTES
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// A credential key: 1–128 ASCII letters, digits, `.`, `_`, `:`, or `-`.
pub fn is_credential_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CREDENTIAL_KEY_BYTES
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character))
}

/// A command ID from the plugin's manifest: 1–128 of `[a-z0-9.-]`.
pub fn is_command_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COMMAND_ID_BYTES
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || ".-".contains(character)
        })
}

/// A `parentOperationID`: the `operationID` of a relayed call, a string of
/// 1-128 bytes.
pub(crate) fn is_parent_operation_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_PARENT_OPERATION_ID_BYTES
}

/// A context generation: a positive integer JSON carries exactly.
pub(crate) fn generation(value: &Value) -> Option<u64> {
    let generation = match value.as_u64() {
        Some(number) => number,
        None => {
            let number = value.as_f64()?;
            if number.fract() != 0.0 || !(1.0..=MAX_SAFE_INTEGER as f64).contains(&number) {
                return None;
            }
            number as u64
        }
    };
    (1..=MAX_SAFE_INTEGER)
        .contains(&generation)
        .then_some(generation)
}

/// Cuts `value` to `limit` UTF-8 bytes without splitting a character.
pub(crate) fn bounded_text(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_protocol_this_sdk_speaks_is_version_zero() {
        assert_eq!(FUNCTIONS_VERSION, 0);
        assert_eq!(LOGGING_VERSION, 0);
        assert_eq!(KEYMAP_IMPORT_VERSION, 0);
    }

    #[test]
    fn the_build_protocol_follows_build_providers_one() {
        assert_eq!(BUILD_VERSION, 1);
    }

    #[test]
    fn an_offer_lists_the_version_exactly() {
        use serde_json::json;
        assert!(offers(Some(&json!({ "versions": [0] })), 0));
        assert!(offers(Some(&json!({ "versions": [99, 0] })), 0));
        for invalid in [
            json!({ "versions": [1] }),
            json!({ "versions": [] }),
            json!({ "versions": ["0", true, null, 0.5] }),
            json!({ "versions": 0 }),
            json!({ "version": 0 }),
            json!(0),
            json!(null),
        ] {
            assert!(!offers(Some(&invalid), 0), "{invalid}");
        }
        assert!(!offers(None, 0));
    }

    #[test]
    fn function_ids_are_dot_separated_lower_camel_case() {
        assert!(is_function_id("snapshot.refresh"));
        assert!(is_function_id("query.runAll2"));
        for invalid in [
            "",
            "Snapshot",
            "snapshot.",
            ".a",
            "a..b",
            "a-b",
            "a.B",
            "1a",
        ] {
            assert!(!is_function_id(invalid), "{invalid}");
        }
        assert!(!is_function_id(&"a".repeat(129)));
    }

    #[test]
    fn plugin_error_codes_and_credential_keys_use_their_alphabets() {
        assert!(is_plugin_error_code("db.connection-failed_2"));
        assert!(!is_plugin_error_code("2db"));
        assert!(!is_plugin_error_code(&"a".repeat(65)));
        assert!(is_credential_key("connection:prod.password"));
        assert!(!is_credential_key(""));
        assert!(!is_credential_key("has space"));
        assert!(!is_credential_key(&"k".repeat(129)));
        assert!(is_command_id("db.refresh-all2"));
        for invalid in ["", "DB.refresh", "db_refresh", "db refresh"] {
            assert!(!is_command_id(invalid), "{invalid}");
        }
        assert!(!is_command_id(&"a".repeat(129)));
    }

    #[test]
    fn parent_operation_ids_are_one_to_one_hundred_twenty_eight_bytes() {
        assert!(is_parent_operation_id("17"));
        assert!(is_parent_operation_id(&"o".repeat(128)));
        assert!(is_parent_operation_id(&"é".repeat(64)));
        assert!(!is_parent_operation_id(""));
        assert!(!is_parent_operation_id(&"o".repeat(129)));
        assert!(!is_parent_operation_id(&"é".repeat(65)));
    }

    #[test]
    fn generations_are_positive_integers_json_carries_exactly() {
        use serde_json::json;
        assert_eq!(generation(&json!(2)), Some(2));
        assert_eq!(generation(&json!(2.0)), Some(2));
        assert_eq!(generation(&json!(MAX_SAFE_INTEGER)), Some(MAX_SAFE_INTEGER));
        for invalid in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("2"),
            json!(null),
            json!(MAX_SAFE_INTEGER + 1),
        ] {
            assert_eq!(generation(&invalid), None, "{invalid}");
        }
    }

    #[test]
    fn bounded_text_never_splits_a_character() {
        assert_eq!(bounded_text("abc", 8), "abc");
        assert_eq!(bounded_text("aé", 2), "a");
        assert_eq!(bounded_text("日本語", 7), "日本");
    }
}
