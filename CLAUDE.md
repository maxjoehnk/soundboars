# SoundBoars

A Linux soundboard (in the spirit of Soundux) written in Rust. It plays sounds locally **and**
injects them into a chosen voice-chat application through PipeWire.

- Crate / binary: `soundboars` — display name **SoundBoars**
- App ID (portal, `.desktop`, config dirs): `me.maxjoehnk.soundboars`
- Linux + PipeWire only. No Windows/macOS/PulseAudio-only/JACK-only support.

## Decisions (already made — don't re-litigate)

| Area | Decision |
|---|---|
| GUI | `eframe`/`egui` 0.37, default look. Prototypes in `prototypes/` are throwaway references for milestones 4–7 (trim editor, URL dialog, hotkey/MIDI settings); delete them once those exist. |
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

## Running checks and tests

```sh
scripts/in-container.sh scripts/check.sh          # fmt + clippy, same as CI
scripts/in-container.sh scripts/test-headless.sh  # all tests against private D-Bus/PipeWire/WirePlumber/Xvfb
scripts/in-container.sh scripts/test-headless.sh --test pipewire_inject -- --nocapture  # extra args go to cargo test
scripts/in-container.sh scripts/test-headless.sh run cargo run --example play         # any command against those services
```

- `in-container.sh` builds the `soundboars-ci` image from `ci/Dockerfile` only if it's missing; after
  changing the Dockerfile run `docker build -t soundboars-ci ci`. Container cargo state lives in
  `.ci-cargo/`, build output shares `target/` with the host.
- `test-headless.sh` refuses to run outside a container and sets `SOUNDBOARS_REQUIRE_PIPEWIRE=1`.
  Tests that create PipeWire nodes/links run only when that variable is `1` (and then fail if
  PipeWire is unreachable); a plain `cargo test` on the host skips them and never touches the
  desktop's audio graph.

## Audio threading

- `audio::AudioEngine` is the app's API (`Send`, every call non-blocking, results as `EngineEvent`s).
  It runs an `audio-control` thread owning the `Controller` (sound cache, mixer handles, what's
  playing); everything reaches it as `controller::Message`s on one `mpsc` channel (façade calls,
  PipeWire events, decode results). It polls the mixers every 10 ms only while voices play.
- `audio::PipeWireService` owns a `pipewire` thread running a `MainLoop`; all PipeWire objects
  (`!Send`) live there. Commands go in via `pipewire::channel`, `audio::Event`s go to a callback.
- Two output streams (`audio::output`, `RT_PROCESS`, `node.always-process` so they render while
  unlinked): `soundboars-inject` (linked to the target) and `soundboars-local` (autoconnected).
  Each render closure is a `mixer::Mixer` on PipeWire's data thread: commands arrive via an `rtrb`
  ring; ended voices go back (with their `Arc<[f32]>`, so the last `Arc` is never dropped on the RT
  thread) via a second ring. Call volume is applied after the limiter and halved for targets that
  sum both channels into one port (`link_plan::sums_channels`).
- Decoding (`audio::decode`, symphonia + rubato) runs on the `sound-decoder` thread
  (`loader::SoundCache`); `invalidate` drops cached sounds and discards in-flight stale decodes.
- Previews: `PlayRequest { destination: Destination::Preview, .. }` plays only on the local
  stream (even with "Local" disabled), as one shared sound id with `Restart` retriggering, so a new
  preview replaces the old one; `stop_preview()` stops it. `Playing::destination` tells previews
  apart (the board ignores them). `load_waveform(path)` reuses the decode cache and computes
  `audio::Waveform` (min/max peaks + length) on a short-lived thread → `EngineEvent::Waveform`.
- Node details (app name, binary, media name) are not in registry globals; recording nodes are bound
  and their `info` (PROPS change only) is watched. Links are reconciled against a pure plan
  (`audio::link_plan`) on every graph change; a link lives as long as its proxy (`object.linger = false`).

## App architecture

- `config` (`Config`, `ConfigStore`): one TOML file, every struct `#[serde(default)]` (new fields
  must have defaults; unknown fields are ignored). Per-sound data: `tabs[i].sounds["<file>"]`
  (`SoundConfig`, keyed by path relative to the tab folder). `$SOUNDBOARS_CONFIG` overrides the path.
- `library`: tab = one folder, non-recursive, sorted by name; `watch::FolderWatcher` (impl
  `NotifyWatcher`) reports `FolderEvent::{Listing, Contents}`. `durations::DurationProbe` measures
  every listed sound once on a background thread (`audio::decode::probe_duration`: container frame
  count, full decode as fallback); tiles show the trimmed length (`SoundConfig::trimmed_length`).
- `app`: `App` owns `AppState` (pure, unit-tested logic: visible sounds, play requests, progress)
  plus `Services` (`AudioBackend` = `AudioEngine` or a fake, `FolderWatcher`, `FolderPicker`,
  `ConfigStore`, `MidiService` — started in eframe's creator closure, as it needs the
  `egui::Context` to wake the UI). Each frame: `poll()` services → `ui::draw(&AppState) -> Vec<Action>` → `apply()`
  → save config when dirty and no pointer button is held. UI code (`app/ui/*`) only reads state
  and emits `Action`s; it never mutates state or calls services. Settings go through
  `Action::ChangeSetting(Setting)`. `app::targets::Targets` decides auto-relinking.
- Dialogs are OS windows: `ui::dialog::Dialog { id, title, size }.show(ctx, |ui| …)` opens an
  immediate viewport (app ID + icon set) and returns `true` when the user closed the window — treat
  it as Cancel. Kittest embeds viewports as `egui::Window`s, so GUI tests work unchanged (a dialog's
  window title is its label there). Size is fixed; adjust `size` when adding content.
- "Edit sound" (`app/sound_editor.rs` state/logic, `app/ui/sound_editor.rs` drawing): opened by
  `Action::EditSound(path)` (tile context menu; rescans the folder if the file isn't listed yet),
  edits a copy (`AppState::editor`) via `Action::Editor(EditorAction)`, Save writes it into the
  sound's `SoundConfig` with `SoundEditor::apply_to`. **Extension point:** `ui::sound_editor::body`
  calls one function per section (`name_section`, `trim_section`, `volume_section`, …); a new
  section adds one call there plus its own file, its fields to `SoundEditor` (+ `open`/`apply_to`)
  and may emit its own `Action`s. Tile context menu items go into `ui::board::context_menu`.
- New features: add an `Action` variant + `apply` arm, a module under `app/` for logic, and a file
  under `app/ui/` for drawing; keep edits to shared files (`app/mod.rs`, `app/ui/mod.rs`,
  `config.rs`) to small, additive insertions.
- GUI tests: `tests/gui.rs` (`egui_kittest`, fakes, AccessKit queries — every widget needs an
  accessible name or kittest fails). `tests/app_smoke.rs` runs the real binary on the container's
  Xvfb (wgpu on Mesa lavapipe). Icons in labels come from egui's bundled emoji fonts
  (`egui/monochrome_emoji_fonts`); kittest panics on glyphs no bundled font has.

## MIDI

- `midi::MidiService` runs a `midi` thread around a `MidiInput` (`MidirInput` = ALSA sequencer via
  `midir`, one client for listing plus one per connection; fakes in tests). It follows
  `Settings::midi_input` (`MidiDevice::{Off, All, Port(name)}`, default `Off`) and polls the port
  list every `HOTPLUG_POLL` (2 s); `midi::hotplug::plan` (pure) says what to connect/drop. Ports are
  matched by their stable name (`midir`'s name without the `client:port` address,
  `stable_port_name`). Events (`MidiEvent::{Ports, Connected, Trigger, Error}`) wake the UI
  (`request_repaint`); errors are reported once until the same thing worked again.
- `midi::message::Parser` (one per port): Note On with velocity > 0 triggers; CCs trigger on the
  rising edge across 64, tracked per channel + CC; everything else is ignored. Channels are 1–16.
- `MidiBinding { port: Option<String>, channel, control }` on `SoundConfig::midi` and
  `Settings::midi_stop_all`; `port: None` matches any port ("Any device" in the editor). It's the
  one config struct without `#[serde(default)]` (a binding without channel/control means nothing).
- App side (`app/midi.rs`): `MidiState::learning` captures the next trigger instead of acting on
  it (`Action::Midi(MidiAction::Learn(..))`; closing the window or Esc cancels). Otherwise
  `AppState::midi_actions` turns a trigger into `Action::StopAll`/`Action::Play`, like a click.
  UI: `ui/midi_section.rs` (editor section, settings rows, shared learn widget).
- `tests/midi_alsa.rs` needs `/dev/snd/seq` (passed through by `in-container.sh` when the host has
  one; the kernel sequencer isn't namespaced, so the test only touches its own virtual port) and
  runs only inside the container; it skips otherwise.

## Tiling window managers

winit doesn't set a parent for dialog windows on Wayland, so tiling compositors tile them. Users
can float them by app ID (`me.maxjoehnk.soundboars`) and title, e.g. Hyprland 0.53–0.54 (the rule
syntax changes between versions — see the wiki for yours):

```
windowrule = match:class ^(me\.maxjoehnk\.soundboars)$, match:title ^(Settings|Edit sound.*)$, float on
```

## Milestones

| # | Milestone | Status |
|---|---|---|
| 1 | Docker/CI harness + spike: inject stream, list recording apps, link into one; portal shortcut registration | done |
| 2 | Audio engine: decoding + cache, both streams, voices, retrigger modes, volumes, limiter | done |
| 3 | App UI: folder tabs, `notify`, config, target picker with auto-relink | done |
| 4 | Hotkeys: trait, portal backend (with Hyprland bind hints), X11 backend | todo |
| 5 | MIDI: midir, learn mode, hotplug | todo |
| 6 | Trim editor: waveform peaks, non-destructive trim, preview | done |
| 7 | URL import: yt-dlp runner, progress, errors → trim editor | todo |
| 8 | Polish: tray, multi-target, Opus, export trimmed copy | later |
