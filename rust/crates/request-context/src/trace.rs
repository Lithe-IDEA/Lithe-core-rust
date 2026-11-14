//! Trace correlation of the running request, inherited by the process traces
//! started inside it.

use crate::Source;
use std::cell::RefCell;

/// Correlation inherited by Git process traces started inside the request.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Operation shared by the user action, its requests and their Git processes.
    pub op: Option<String>,
    /// Provenance resolved once from the request envelope.
    pub source: Source,
}

thread_local! {
    static CURRENT: RefCell<Context> = RefCell::new(Context::default());
}

/// Snapshot for traces that finish on another thread, such as streamed history.
pub fn current() -> Context {
    CURRENT.with(|current| current.borrow().clone())
}

/// Makes `context` the running request's correlation and returns the enclosing
/// one, which the caller restores with another `replace` when its request ends.
pub fn replace(context: Context) -> Context {
    CURRENT.with(|current| current.replace(context))
}
