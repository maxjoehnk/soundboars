#!/usr/bin/env bash
# Stop/SubagentStop: refuse to finish while fmt or clippy report problems.
input=$(cat)
# Avoid loops: if we already blocked once, let the agent stop.
[[ $(jq -r '.stop_hook_active // false' <<<"$input") == "true" ]] && exit 0
cd "${CLAUDE_PROJECT_DIR:-.}"
[[ -f Cargo.toml ]] || exit 0
if ! out=$(scripts/check.sh 2>&1); then
  echo "scripts/check.sh failed — fix formatting/clippy before finishing:" >&2
  tail -n 60 <<<"$out" >&2
  exit 2
fi
