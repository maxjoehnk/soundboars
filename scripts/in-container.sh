#!/usr/bin/env bash
# Run a command inside the headless CI container (builds the image if missing).
set -euo pipefail
cd "$(dirname "$0")/.."
image=soundboars-ci
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" ci
mkdir -p .ci-cargo
exec docker run --rm \
  --user "$(id -u):$(id -g)" \
  -e CARGO_HOME=/work/.ci-cargo -e HOME=/tmp \
  -v "$PWD:/work" -w /work \
  "$image" "$@"
