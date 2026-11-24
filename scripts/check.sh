#!/usr/bin/env bash
# Static checks shared by the git pre-commit hook, Claude Code hooks and CI.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
