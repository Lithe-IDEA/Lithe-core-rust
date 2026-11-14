//! JSON command dispatch and the exported C ABI.

mod dispatcher;
mod ffi;
pub(crate) mod helper_failures;
pub(crate) mod request_trace;

pub(crate) use dispatcher::execute_json;
