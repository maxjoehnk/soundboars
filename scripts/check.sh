#!/usr/bin/env bash
# Static checks shared by the git pre-commit hook, Claude Code hooks and CI.
set -euo pipefail
cd "$(dirname "$0")/.."

# A stale LIBCLANG_PATH (e.g. from another toolchain) breaks bindgen; fall back to auto-detection.
[[ -z "${LIBCLANG_PATH:-}" || -d "$LIBCLANG_PATH" ]] || unset LIBCLANG_PATH

cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
