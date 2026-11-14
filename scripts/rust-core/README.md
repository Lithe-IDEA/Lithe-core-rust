# Verify Rust Core comments before compiling tests

Run `scripts/rust-core/verify-rust-core-comments.sh` from any directory. It checks
module documentation, English comments, unsafe API safety sections, exported
Rustdoc, and intra-doc links in every crate under `rust/crates/`. Every crate
must pass `missing_docs`; there is no exemption list.

The small `comment-lexer` helper uses pinned `rustc_lexer` tokens to distinguish
comments from ordinary, raw, and byte string literals. It does not parse source
with delimiter regular expressions or exempt Unicode test data. Run its tests
with `cargo test --locked --manifest-path scripts/rust-core/comment-lexer/Cargo.toml`.

The gate uses `CARGO_TARGET_DIR` when supplied. Otherwise it writes only to this
checkout's `.artifacts/rust-core-checks/`. Cargo state and generated Rustdoc stay
local to the checkout, are ignored by Git, and must not be shared with another
worktree or installed with the application.
