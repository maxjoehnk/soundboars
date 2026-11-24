#!/usr/bin/env bash
# PostToolUse: format a Rust file right after Claude edits it.
file=$(jq -r '.tool_input.file_path // empty')
[[ "$file" == *.rs && -f "$file" ]] || exit 0
rustfmt --edition 2024 "$file" 2>/dev/null || true
