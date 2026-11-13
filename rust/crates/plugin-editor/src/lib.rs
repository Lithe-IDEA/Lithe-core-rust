//! Editor contributions of plugins.
//!
//! [`actions`] covers the independent, host-rendered editor actions of ordinary
//! tool plugins; [`providers`] covers the language-feature providers a plugin
//! backend registers through `lithe/host/editor/registerProviders`. Core checks
//! declarations and results; hosts own documents, calls and presentation.

pub mod actions;
pub mod providers;
