#!/usr/bin/env bash
# Run a command inside the headless CI container (builds the image if missing).
set -euo pipefail
cd "$(dirname "$0")/.."
image=soundboars-ci
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" ci
mkdir -p .ci-cargo
# The MIDI integration tests need an ALSA sequencer. It's the host's (the kernel's sequencer has no
# namespaces), so they only ever use the virtual ports they create themselves.
devices=()
if [[ -e /dev/snd/seq ]]; then
  devices=(--device /dev/snd/seq --group-add "$(stat -c %g /dev/snd/seq)")
fi
exec docker run --rm \
  --user "$(id -u):$(id -g)" \
  "${devices[@]}" \
  -e CARGO_HOME=/work/.ci-cargo -e HOME=/tmp \
  -v "$PWD:/work" -w /work \
  "$image" "$@"
