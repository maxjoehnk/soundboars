#!/usr/bin/env bash
# Run a command inside the headless CI container (builds the image if missing).
set -euo pipefail
cd "$(dirname "$0")/.."
image=soundboars-ci
docker image inspect "$image" >/dev/null 2>&1 || docker build -t "$image" ci
mkdir -p .ci-cargo
# The MIDI integration tests need an ALSA sequencer. The kernel's sequencer has no namespaces, so
# passing it through briefly adds virtual test ports to the *host's* MIDI graph. Opt-in only:
# SOUNDBOARS_HOST_MIDI=1 scripts/in-container.sh scripts/test-headless.sh
devices=()
if [[ "${SOUNDBOARS_HOST_MIDI:-0}" == 1 && -e /dev/snd/seq ]]; then
  devices=(--device /dev/snd/seq --group-add "$(stat -c %g /dev/snd/seq)")
fi
exec docker run --rm \
  --user "$(id -u):$(id -g)" \
  "${devices[@]}" \
  -e CARGO_HOME=/work/.ci-cargo -e HOME=/tmp \
  -v "$PWD:/work" -w /work \
  "$image" "$@"
