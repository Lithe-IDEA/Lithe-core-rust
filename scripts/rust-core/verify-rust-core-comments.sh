#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/../.." && pwd)"
# Keep this helper, Rustdoc and subsequent checks in one native build-state
# directory. A desktop worktree supplies its own existing Cargo target.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/.artifacts/rust-core-checks}"
# lithe-core-rules is part of Core and holds reusable editor/UI rules; lithe-ai holds the
# ai.* commands.
SOURCE_DIRS=("$ROOT_DIR/rust/lithe-core/src" "$ROOT_DIR/rust/lithe-core-rules/src" "$ROOT_DIR/rust/lithe-ai/src")
cd "$ROOT_DIR"

rust_files=()
production_files=()
while IFS= read -r -d '' file; do
    rust_files+=("$file")
done < <(find "${SOURCE_DIRS[@]}" -type f -name '*.rs' -print0)
while IFS= read -r -d '' file; do
    production_files+=("$file")
done < <(find "${SOURCE_DIRS[@]}" -type f -name '*.rs' \
    ! -path '*/tests/*' \
    ! -name 'tests.rs' \
    -print0)

if (( ${#production_files[@]} == 0 )); then
    printf '%s\n' "Rust Core comment verification found no production modules" >&2
    exit 1
fi

failures=0

# Module documentation is a cheap textual check and should fail before Rustdoc runs.
for file in "${production_files[@]}"; do
    first_source_line="$(awk 'NF { sub(/^[[:space:]]*/, ""); print; exit }' "$file")"
    if [[ "$first_source_line" != "//!"* ]]; then
        relative_file="${file#"$ROOT_DIR"/}"
        printf '%s\n' "$relative_file:1: production modules must start with //! documentation" >&2
        failures=$((failures + 1))
    fi
done

# Rust lexical tokens distinguish nested comments from ordinary, raw and byte
# strings. Textual delimiter matching misclassified Unicode .gitignore fixtures
# containing "/*" and could also miss an actual comment after a string literal.
if ! cargo run --quiet --locked \
    --manifest-path "$SCRIPT_DIR/comment-lexer/Cargo.toml" -- "${rust_files[@]}"; then
    failures=$((failures + 1))
fi

# The C ABI lives behind a private Rust module, so rustdoc's missing_docs lint
# cannot enforce its safety sections. Check public unsafe functions explicitly.
unsafe_without_safety="$(awk '
    function reset_docs() {
        documented = 0
        safety = 0
    }

    FNR == 1 {
        reset_docs()
        in_attribute = 0
    }

    /^[[:space:]]*\/\/\/([^!]|$)/ {
        documented = 1
        if ($0 ~ /#[[:space:]]*Safety/) {
            safety = 1
        }
        next
    }

    /^[[:space:]]*#\[/ {
        in_attribute = ($0 !~ /\][[:space:]]*$/)
        next
    }

    in_attribute {
        if ($0 ~ /\][[:space:]]*$/) {
            in_attribute = 0
        }
        next
    }

    /^[[:space:]]*$/ {
        reset_docs()
        next
    }

    {
        public_unsafe_function = $0 ~ /^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+(async[[:space:]]+)?(const[[:space:]]+)?unsafe[[:space:]]+(extern[[:space:]]+"[^"]+"[[:space:]]+)?fn[[:space:]]+/
        if (public_unsafe_function && (!documented || !safety)) {
            print FILENAME ":" FNR ": public unsafe functions require /// documentation with a # Safety section"
        }
        reset_docs()
    }
' "${rust_files[@]}")"
if [[ -n "$unsafe_without_safety" ]]; then
    printf '%s\n' "$unsafe_without_safety" >&2
    failures=$((failures + 1))
fi

if (( failures > 0 )); then
    exit 1
fi

# Let Rust understand visibility instead of treating pub items in private
# modules as exported APIs. This covers public functions, types, variants, and
# fields without requiring documentation for every internal helper.
for manifest in rust/lithe-core/Cargo.toml rust/lithe-core-rules/Cargo.toml rust/lithe-ai/Cargo.toml; do
    cargo rustdoc --quiet \
        --manifest-path "$manifest" \
        --lib -- \
        -D missing_docs \
        -D rustdoc::broken_intra_doc_links
done

printf '%s\n' "Rust Core comment verification passed"
