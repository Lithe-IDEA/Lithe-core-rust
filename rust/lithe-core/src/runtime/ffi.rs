//! Ownership-safe C ABI wrappers for the JSON command and cancellation APIs.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;

/// Returns a pointer to the static, NUL-terminated Core ABI version.
///
/// The pointer remains valid for the lifetime of the process and must not be
/// passed to [`lithe_core_free_string`].
#[no_mangle]
pub extern "C" fn lithe_core_version() -> *const c_char {
    static VERSION: &[u8] = b"0.1.0\0";
    VERSION.as_ptr().cast()
}

/// Classifies a borrowed UTF-8 document without copying it into a JSON envelope.
/// Returns 1 for plain text, 0 for binary control characters, and -1 for invalid input.
/// An empty slice is plain text; embedded NUL is inspected rather than terminating the input.
///
/// # Safety
/// `bytes` must point to `length` readable bytes for this call, or be null when length is zero.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_is_plain_text(bytes: *const u8, length: usize) -> i32 {
    if length == 0 {
        return 1;
    }
    if bytes.is_null() || length > isize::MAX as usize {
        return -1;
    }
    match std::str::from_utf8(std::slice::from_raw_parts(bytes, length)) {
        Ok(text) => i32::from(crate::project::is_plain_text(text)),
        Err(_) => -1,
    }
}

/// Executes one JSON request through the stable C ABI.
///
/// The returned string is owned by the caller and must be released exactly
/// once with [`lithe_core_free_string`].
///
/// # Safety
///
/// `request` must be null or point to a readable, NUL-terminated byte string
/// for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_execute_json(request: *const c_char) -> *mut c_char {
    if request.is_null() {
        return response_pointer(
            r#"{"id":null,"ok":false,"error":{"code":"invalid_request","message":"Request pointer is null"}}"#,
        );
    }
    let request = CStr::from_ptr(request).to_string_lossy();
    response_pointer(&crate::execute_json(&request))
}

/// Loads the merged language-provider catalog for an optional workspace root.
///
/// The returned string is owned by the caller and must be released exactly
/// once with [`lithe_core_free_string`].
///
/// # Safety
///
/// `workspace_root` must be null or point to a readable, NUL-terminated byte
/// string for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_lsp_provider_catalog_json(
    workspace_root: *const c_char,
) -> *mut c_char {
    let root = if workspace_root.is_null() {
        None
    } else {
        let value = CStr::from_ptr(workspace_root).to_string_lossy();
        if value.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(value.as_ref()))
        }
    };
    response_pointer(&crate::lsp::provider_catalog_json(root.as_deref()))
}

/// Requests cooperative cancellation of an in-flight operation. The call is
/// thread-safe and returns 1 when an active operation was found.
///
/// # Safety
///
/// `operation_id` must be null or point to a readable, NUL-terminated byte
/// string for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_cancel(operation_id: *const c_char) -> i32 {
    if operation_id.is_null() {
        return 0;
    }
    let operation_id = CStr::from_ptr(operation_id).to_string_lossy();
    crate::cancel_operation(&operation_id) as i32
}

/// Releases a string returned by a Core C ABI function.
///
/// # Safety
///
/// `value` must be null or a pointer returned by this library that has not
/// already been freed. Static pointers such as [`lithe_core_version`] are not
/// owned strings and must not be passed here.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_free_string(value: *mut c_char) {
    if !value.is_null() {
        drop(CString::from_raw(value));
    }
}

fn response_pointer(value: &str) -> *mut c_char {
    CString::new(value)
        .unwrap_or_else(|_| CString::new("{\"id\":null,\"ok\":false}").expect("fallback is valid"))
        .into_raw()
}

/// Executes JSON with request-scoped Git diagnostics and Agent install progress. Each event string
/// is borrowed only during the callback; the response uses the normal ownership.
///
/// # Safety
///
/// `request` follows `lithe_core_execute_json`'s contract. `callback` must not
/// unwind, and `context` must remain valid until this synchronous call returns.
/// Callbacks run serially on the calling thread and must copy retained strings.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_execute_json_with_events(
    request: *const c_char,
    callback: Option<unsafe extern "C" fn(*const c_char, *mut std::ffi::c_void)>,
    context: *mut std::ffi::c_void,
) -> *mut c_char {
    let Some(callback) = callback else {
        return lithe_core_execute_json(request);
    };
    if request.is_null() {
        return lithe_core_execute_json(request);
    }
    let request = CStr::from_ptr(request).to_string_lossy();
    // The callback is scoped to this synchronous call, never a worker thread.
    let context_address = context as usize;
    let sink = std::sync::Arc::new(move |event: &str| {
        if let Ok(event) = CString::new(event) {
            callback(event.as_ptr(), context_address as *mut std::ffi::c_void);
        }
    });
    response_pointer(&crate::execute_json_with_events(&request, sink))
}

/// Installs the host log writer's callback for pending Core diagnostic facts,
/// replacing any earlier writer. Core calls it at most once per drain cycle,
/// after a fact or overflow count is queued, so the host can schedule its
/// existing drain instead of polling. A null callback is ignored.
///
/// # Safety
///
/// `callback` may run on any Core thread, must not unwind, and must not call
/// back into Core synchronously; it should only schedule host work. `context`
/// is passed back unchanged and must remain valid until
/// [`lithe_core_remove_log_waker`] returns for it or another install replaces it.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_install_log_waker(
    callback: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    context: *mut std::ffi::c_void,
) {
    if let Some(callback) = callback {
        crate::diagnostics::core_events::install_waker(callback, context as usize);
    }
}

/// Removes the log waker installed with `context`. Returns 1 when it was the
/// current waker. After return, Core never invokes the callback with `context`,
/// so the host may release it.
#[no_mangle]
pub extern "C" fn lithe_core_remove_log_waker(context: *mut std::ffi::c_void) -> i32 {
    crate::diagnostics::core_events::remove_waker(context as usize) as i32
}

/// Runs the native authentication helper without initializing the application.
///
/// # Safety
/// `prompt` must be a readable NUL-terminated string for this synchronous call.
#[no_mangle]
pub unsafe extern "C" fn lithe_core_git_askpass(prompt: *const c_char) -> i32 {
    if prompt.is_null() {
        return 1;
    }
    crate::git_askpass_main(&CStr::from_ptr(prompt).to_string_lossy())
}

#[cfg(test)]
mod text_content_tests {
    use super::lithe_core_is_plain_text;

    #[test]
    fn decoded_text_contract_matches_c_abi_and_json() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/editor/text-content-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let expected = case["isPlainText"].as_bool().unwrap();
            assert_eq!(
                unsafe { lithe_core_is_plain_text(text.as_ptr(), text.len()) },
                i32::from(expected),
                "{}",
                case["name"]
            );
            let request = serde_json::json!({ "command": "document.classifyText", "payload": { "text": text } });
            let response: serde_json::Value =
                serde_json::from_str(&crate::execute_json(&request.to_string())).unwrap();
            assert_eq!(
                response["data"]["isPlainText"], expected,
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn invalid_utf8_is_not_silently_replaced() {
        let bytes = [0xff, 0xfe];
        assert_eq!(
            unsafe { lithe_core_is_plain_text(bytes.as_ptr(), bytes.len()) },
            -1
        );
        assert_eq!(unsafe { lithe_core_is_plain_text(std::ptr::null(), 1) }, -1);
        assert_eq!(unsafe { lithe_core_is_plain_text(std::ptr::null(), 0) }, 1);
        let response: serde_json::Value = serde_json::from_str(&crate::execute_json(
            r#"{"command":"document.classifyText","payload":{"text":1}}"#,
        ))
        .unwrap();
        assert_eq!(response["error"]["code"], "invalid_request");
    }
}
