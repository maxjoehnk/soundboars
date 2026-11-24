# SoundBoars

A Linux soundboard (in the spirit of Soundux) written in Rust. It plays sounds locally **and**
injects them into a chosen voice-chat application through PipeWire.

- Crate / binary: `soundboars` — display name **SoundBoars**
- App ID (portal, `.desktop`, config dirs): `me.maxjoehnk.soundboars`
- Linux + PipeWire only. No Windows/macOS/PulseAudio-only/JACK-only support.

## Decisions (already made — don't re-litigate)

| Area | Decision |
|---|---|
| GUI | `eframe`/`egui` 0.37, default look. Prototypes in `prototypes/` are throwaway and get deleted once the real UI exists. |
| Injection | Soundux-style "pick an app": an output stream `soundboars-inject` (`Stream/Output/Audio`, no autoconnect) is linked via `link-factory` to the target app's `Stream/Input/Audio` node ports. The app's real mic link (WirePlumber) stays untouched. Target is remembered as `(application.process.binary, application.name)` and auto-relinked when the node reappears. |
| Local playback | Separate stream `soundboars-local`, autoconnected to the default sink. Each stream mixes independently over shared decoded `Arc<[f32]>` buffers (no cross-clock mixing). |
| Hotkeys | `trait HotkeyBackend` with two impls: XDG GlobalShortcuts portal (`ashpd`) and X11 (`global-hotkey`, X11-only; won't fire on Wayland-native focus). Backends report capabilities (e.g. Hyprland's portal ignores `preferred_trigger` → UI shows the `hyprland.conf` bind line instead of a record button). |
| MIDI | `midir`, MIDI-learn mode, `(port name, channel, note/CC)` mapping, hotplug by polling. |
| Library | Folder-based: every added folder is a tab, watched with `notify`. Per-sound metadata (hotkey, MIDI, trim, volume) lives in `config.toml`, keyed by path relative to its tab folder. |
| Trimming | **Non-destructive**: `start_ms`/`end_ms` stored in config, audio files never modified. Waveform = min/max peaks computed in the background. |
| URL import | Spawn `yt-dlp -x --audio-format mp3 …` into the chosen tab folder, parse progress, then open the trim editor. |
| Decoding | `symphonia` (no Opus decoder in 0.6!), resample with `rubato` to 48 kHz stereo f32. |
| Config | `serde` + `toml` in `$XDG_CONFIG_HOME/soundboars/config.toml` (`directories` crate). |

## Code style

- SOLID, SLA (single level of abstraction per function), KISS, YAGNI. Simple over clever.
- Talk to external systems (PipeWire, portal, X11, MIDI, yt-dlp, filesystem) behind small traits so
  logic can be unit-tested with fakes. Don't add a trait with only one impl unless a test needs it.
- No allocation, locking, logging or decoding in real-time audio callbacks.
- `unwrap()`/`expect()` only in tests or with a comment proving it can't fail. Errors: `thiserror`
  in library modules, `anyhow` at the binary edge.
- Match existing patterns in the codebase before inventing new ones.

## Checks (enforced)

- `scripts/check.sh` = `cargo fmt --check` + `cargo clippy --all-targets --all-features -D warnings`.
  It runs as git pre-commit hook (`core.hooksPath = .githooks`), as a Claude Code Stop/SubagentStop
  hook, and in CI. Don't bypass it (`--no-verify`) and don't silence lints with `#[allow]` unless
  there's a written reason next to it.
- Rust files are auto-formatted after every Claude edit (`.claude/hooks/rustfmt-edited.sh`).

## Testing

- Write unit tests next to the code, integration tests in `tests/`, end-to-end tests where feasible
  (GUI via `egui_kittest`, PipeWire via a real daemon in the container).
- **All tests must run headless.** The container in `ci/` provides PipeWire + WirePlumber + D-Bus,
  Xvfb + xdotool, ffmpeg + yt-dlp. CI (GitHub Actions) runs the same container.
- **Never interact with the user's desktop session**: no `hyprctl`, no opening windows on the host
  display, no moving/focusing windows, no creating links/nodes in the host PipeWire graph. Run
  anything that needs a display or audio server inside the container (or with `DISPLAY`/
  `WAYLAND_DISPLAY` unset and a private Xvfb/PipeWire).
- Tests that need `/dev/snd/seq` (MIDI) must skip cleanly when it doesn't exist (CI runners).
- Tests must not need network access (serve yt-dlp test media from a local HTTP server).
- Manual verification against real apps (Discord etc.) is done by the user.

## Milestones

| # | Milestone | Status |
|---|---|---|
| 1 | Docker/CI harness + spike: inject stream, list recording apps, link into one; portal shortcut registration | todo |
| 2 | Audio engine: decoding + cache, both streams, voices, retrigger modes, volumes, limiter | todo |
| 3 | App UI: folder tabs, `notify`, config, target picker with auto-relink | todo |
| 4 | Hotkeys: trait, portal backend (with Hyprland bind hints), X11 backend | todo |
| 5 | MIDI: midir, learn mode, hotplug | todo |
| 6 | Trim editor: waveform peaks, non-destructive trim, preview | todo |
| 7 | URL import: yt-dlp runner, progress, errors → trim editor | todo |
| 8 | Polish: tray, multi-target, Opus, export trimmed copy | later |
