# SoundBoars

A soundboard for Linux. It plays sounds on your speakers **and** injects them into a voice-chat
app of your choice (Discord, Mumble, a browser call, …) through PipeWire, without touching that
app's real microphone.

## Features

- Folders as tabs: add a folder and its sounds show up as tiles; changes on disk appear live.
- Inject into any app that records audio; the chosen app is remembered and relinked when it
  restarts. Separate volumes for the call and local playback; local playback can be turned off.
- Non-destructive trimming with a waveform editor, per-sound volume, preview.
- Global hotkeys (XDG GlobalShortcuts portal or X11) and MIDI controllers (learn mode, hotplug).
  Both work while the window is hidden or on another workspace.
- "Stop all" button, hotkey and MIDI binding.
- Import sounds from YouTube or any other site yt-dlp supports, then trim them.

## Requirements

- Linux with PipeWire (WirePlumber as session manager).
- For URL import: `yt-dlp`, `ffmpeg` and `ffprobe` on `$PATH` (the button is disabled otherwise).
- For global hotkeys: an XDG desktop portal with GlobalShortcuts (KDE, GNOME, Hyprland, …) or an
  X11 session. The X11 backend can't see key presses while a Wayland window has focus.
- For MIDI: the ALSA sequencer (`/dev/snd/seq`).

Build dependencies (Debian/Ubuntu names): a Rust toolchain, `pkg-config`, `clang`,
`libclang-dev`, `libpipewire-0.3-dev`, `libspa-0.2-dev`, `libasound2-dev`, `libxkbcommon-dev`,
`libwayland-dev`, X11 headers (`libx11-dev`, `libxi-dev`, `libxtst-dev`). `ci/Dockerfile` has the
full list.

## Build and run

```sh
cargo build --release
./target/release/soundboars
```

The config lives in `~/.config/soundboars/config.toml` (`$SOUNDBOARS_CONFIG` overrides it). Logs
go to stderr; `RUST_LOG=debug` shows more.

## Desktop entry and icon

Install the `.desktop` file and icon so the desktop (and its shortcut settings, via the portal)
show SoundBoars with its name and icon:

```sh
install -Dm755 target/release/soundboars ~/.local/bin/soundboars
install -Dm644 assets/me.maxjoehnk.soundboars.desktop ~/.local/share/applications/me.maxjoehnk.soundboars.desktop
install -Dm644 assets/me.maxjoehnk.soundboars.svg ~/.local/share/icons/hicolor/scalable/apps/me.maxjoehnk.soundboars.svg
install -Dm644 assets/icon-256.png ~/.local/share/icons/hicolor/256x256/apps/me.maxjoehnk.soundboars.png
```

`Exec=soundboars` expects the binary on `$PATH`.

## Hyprland

Hyprland's portal ignores the keys an app asks for, so global shortcuts are bound in your config.
Open "Edit sound" (right-click a tile), click "Add global shortcut" and copy the line it shows,
e.g. (Hyprland ≥ 0.55, Lua config):

```lua
hl.bind("CTRL + F13", hl.dsp.global("me.maxjoehnk.soundboars:play-airhorn"))
hl.bind("CTRL + F12", hl.dsp.global("me.maxjoehnk.soundboars:stop-all"))
```

Older hyprlang configs: `bind = CTRL, F13, global, me.maxjoehnk.soundboars:play-airhorn`.

Dialogs (Settings, Edit sound, Add sound from URL) open as their own windows; to float them
instead of tiling:

```lua
hl.window_rule({
  match = {
    class = [[^me\.maxjoehnk\.soundboars$]],
    title = [[^(Settings|Edit sound.*|Add sound from URL)$]],
  },
  float = true,
})
```

Hyprland 0.53–0.54 (hyprlang):
`windowrule = match:class ^(me\.maxjoehnk\.soundboars)$, match:title ^(Settings|Edit sound.*|Add sound from URL)$, float on`

## Development

```sh
scripts/check.sh                                     # cargo fmt --check + clippy -D warnings
scripts/in-container.sh scripts/check.sh             # the same in the CI container
scripts/in-container.sh scripts/test-headless.sh     # all tests, headless
```

Tests that need PipeWire, D-Bus, X11 or the ALSA sequencer run only in the container
(`ci/Dockerfile`: PipeWire + WirePlumber, D-Bus, Xvfb, ffmpeg, yt-dlp), never against your desktop
session; a plain `cargo test` skips them. Enable the pre-commit hook with
`git config core.hooksPath .githooks`. `CLAUDE.md` describes the architecture and conventions.

## License

GPL-3.0-only, see [LICENSE](LICENSE).
