#!/usr/bin/env bash
# Runs the test suite against private headless services: a D-Bus session bus, PipeWire +
# WirePlumber with a null sink, and Xvfb. Run it inside the CI container:
#   scripts/in-container.sh scripts/test-headless.sh [cargo test args…]
# or, to run any other command against those services instead of the tests:
#   scripts/in-container.sh scripts/test-headless.sh run CMD…
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ! -e /.dockerenv ]]; then
  echo "test-headless.sh starts its own audio/display servers; run it via scripts/in-container.sh" >&2
  exit 1
fi

# dbus-daemon needs a passwd entry for the (host) user the container runs as.
if ! id -un >/dev/null 2>&1; then
  echo "ci:x:$(id -u):$(id -g):CI:${HOME:-/tmp}:/bin/sh" >>/etc/passwd
fi

runtime=$(mktemp -d)
export XDG_RUNTIME_DIR=$runtime
export XDG_CONFIG_HOME=$runtime/config
export XDG_STATE_HOME=$runtime/state
export DBUS_SESSION_BUS_ADDRESS=unix:path=$runtime/bus
export DISPLAY=:99
unset WAYLAND_DISPLAY

pids=()
cleanup() {
  kill "${pids[@]}" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$runtime"
}
trap cleanup EXIT

# start NAME CMD… — runs CMD in the background, logging to $runtime/NAME.log.
start() {
  local name=$1
  shift
  "$@" >"$runtime/$name.log" 2>&1 &
  pids+=($!)
}

# wait_for NAME CMD… — retries CMD for up to 10 s.
wait_for() {
  local name=$1
  shift
  for _ in $(seq 100); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.1
  done
  echo "timed out waiting for $name; logs:" >&2
  tail -n +1 "$runtime"/*.log >&2
  exit 1
}

# A null sink gives WirePlumber a default sink and the graph a driver.
mkdir -p "$XDG_CONFIG_HOME/pipewire/pipewire.conf.d"
cat >"$XDG_CONFIG_HOME/pipewire/pipewire.conf.d/headless-sink.conf" <<'CONF'
context.objects = [
  { factory = adapter
    args = {
      factory.name   = support.null-audio-sink
      node.name      = headless-sink
      media.class    = Audio/Sink
      audio.position = [ FL FR ]
    }
  }
]
CONF

start dbus dbus-daemon --session --nofork --nopidfile --address="$DBUS_SESSION_BUS_ADDRESS"
wait_for dbus test -S "$runtime/bus"
start pipewire pipewire
wait_for pipewire test -S "$runtime/pipewire-0"
start wireplumber wireplumber
wait_for wireplumber wpctl inspect @DEFAULT_AUDIO_SINK@
start xvfb Xvfb "$DISPLAY" -nolisten tcp -screen 0 1280x800x24
wait_for xvfb xdotool getdisplaygeometry

export SOUNDBOARS_REQUIRE_PIPEWIRE=1
if [[ ${1:-} == run ]]; then
  shift
  "$@"
else
  cargo test --locked --all-features "$@"
fi
