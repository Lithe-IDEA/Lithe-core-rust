//! Pure, synchronous product rules shared by every host.
//!
//! This crate holds rules that are small, deterministic and sit on a synchronous hot path
//! (for example a gesture recognizer that runs on every native key event). It performs no
//! file-system, process, thread or clock access: time is always supplied by the caller.
//!
//! `lithe-core` links this crate as a normal dependency and exposes each rule through its JSON
//! command dispatch, so hosts reach the rules over the usual Core C ABI. Rule failures use
//! [`RuleError`], which `lithe-core` maps one to one onto its stable error codes.

pub mod action_search;
mod dispatch;
pub mod editor;
mod error;
pub mod file_names;
pub mod keymap;
pub mod visibility;
pub mod workbench;

pub use dispatch::{execute, COMMANDS};
pub use error::{RuleError, RuleErrorCode};
