//! Project services: workspace files and visibility, indexing policy, search, local history,
//! document persistence decisions, and Markdown rendering and heading outlines.
//!
//! Language-server sessions sit above this crate. Calls that change indexing or the search index
//! take an `update_providers` callback, which lithe-core uses to tell running sessions which
//! directories are now excluded.

pub mod contracts;
pub mod directory_actions;
mod document_lifecycle;
pub mod document_save_batch;
mod encoding;
pub mod file_name_order;
pub mod file_operations;
pub mod files;
mod history;
pub mod indexing;
mod markdown;
mod markdown_outline;
mod markdown_reference;
mod name_matching;
pub mod name_ranking;
pub mod recent_projects;
mod search_index;
mod search_paging;

pub use document_lifecycle::*;
pub use files::*;
pub use history::*;
pub use markdown::*;
pub use markdown_outline::*;
pub use markdown_reference::*;
pub use search_paging::{search_page, SearchPageRequest};

#[cfg(test)]
mod test_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh path under the system temporary directory that no other fixture shares.
    pub(crate) fn temporary_root(label: &str) -> PathBuf {
        static NONCE: OnceLock<u128> = OnceLock::new();
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        // A clock tick is not a unique ID: parallel fixtures must never share cleanup ownership.
        let nonce = NONCE.get_or_init(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be valid")
                .as_nanos()
        });
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "lithe-project-{label}-{}-{nonce}-{sequence}",
            std::process::id()
        ))
    }
}
