//! Tests that need a real Windows or Unix host.
//!
//! Every module shares one test binary, so the helpers in `support` and the
//! dependencies are built once.

mod askpass;
mod git_executable;
mod lsp_process;
mod permissions;
mod process_tree;
mod support;
