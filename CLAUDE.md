# SoundBoars

A Linux soundboard (in the spirit of Soundux) written in Rust. It plays sounds locally **and**
gets them into voice chats through PipeWire: by default through a virtual microphone
("SoundBoars Microphone") that also carries the user's real microphone, or by linking straight into
a chosen app's recording stream.

- Crate / binary: `soundboars` — display name **SoundBoars**
- App ID (portal, `.desktop`, config dirs): `me.maxjoehnk.soundboars`
- Linux + PipeWire only. No Windows/macOS/PulseAudio-only/JACK-only support.

## Decisions (already made — don't re-litigate)

| Area | Decision |
|---|---|
| GUI | `eframe`/`egui` 0.37, egui's default colors and widgets with more room (`app/ui/style.rs`); no custom design system. Dialogs are separate OS windows (deferred viewports); eframe is vendored with one patch (`vendor/README.md`). |
| Injection | Two modes (`Config::injection`). **Virtual mic** (default): a `support.null-audio-sink` adapter node `soundboars-mic` ("SoundBoars Microphone", `Audio/Source/Virtual`, FL/FR) that call apps pick as their input; `soundboars-inject` and the default source (the user's mic, followed via the `default` metadata) are linked into it with `link-factory`, the mic only while something records the virtual mic. **App** (Soundux-style "pick an app"): `soundboars-inject` (`Stream/Output/Audio`, no autoconnect) is linked straight to the target app's `Stream/Input/Audio` ports; the app's real mic link (WirePlumber) stays untouched. The app target is remembered as `(application.process.binary, application.name)` and auto-relinked when the node reappears. The app-linking mode proved unstable with some apps (lost links, teams-for-linux), hence the virtual mic. |
| Local playback | Separate stream `soundboars-local`, autoconnected to the default sink. Each stream mixes independently over shared decoded `Arc<[f32]>` buffers (no cross-clock mixing). |
| Hotkeys | `trait HotkeyBackend` with two impls: XDG GlobalShortcuts portal (`ashpd`) and X11 (`global-hotkey`, X11-only; won't fire on Wayland-native focus). Backends report capabilities (e.g. Hyprland's portal ignores `preferred_trigger` → UI shows the Hyprland config bind line instead of a record button). |
| MIDI | `midir`, MIDI-learn mode, `(port name, channel, note/CC)` mapping, hotplug by polling. |
| Library | Folder-based: every added folder is a tab, watched with `notify`. Per-sound metadata (name, color, hotkey, MIDI, trim, volume) lives in `config.toml`, keyed by path relative to its tab folder. A file can be on the board several times as **clips** (config only, the file is never copied); renaming never renames the file. |
| Trimming | **Non-destructive**: `start_ms`/`end_ms` stored in config, audio files never modified. Waveform = min/max peaks computed in the background. |
| URL import | Spawn `yt-dlp -x --audio-format mp3 …` into the chosen tab folder (via a hidden staging folder), parse progress, then open "Edit sound" (trim editor). |
| Decoding | `symphonia` (no Opus decoder in 0.6!), resample with `rubato` to 48 kHz stereo f32. |
| Config | `serde` + `toml` in `$XDG_CONFIG_HOME/soundboars/config.toml` (`directories` crate). |
| Sharing (planned, milestone 9) | Self-hosted `soundboars-hub` in this repo (cargo workspace: app, hub, shared API types crate). axum + SQLite, blobs on a PVC addressed by sha256 (deduplicated; several shared sounds may point to one blob). Single replica on k3s. No users: API keys `sbk_<id>_<secret>` (only the hash is stored), created/listed/revoked with the hub's CLI via `kubectl exec`, with no admin HTTP API. The **original file** is uploaded with name, volume, trim and tags. Color, hotkey and MIDI stay local. Uploaders can delete their own sounds. The app stores the API key in the Secret Service keychain, never in `config.toml`. |

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
- Recorders (`tests/common::Recorder`, `pw-record`) need a moment after being linked before audio
  flows; call `wait_until_receiving()` before playing anything short.
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

- `audio::AudioEngine` is the app's API (`Send`, every call non-blocking, results as `EngineEvent`s;
  `play_handle()` gives a cloneable `Send + Sync` `PlayHandle` for play/stop-all from other threads).
  It runs an `audio-control` thread owning the `Controller` (sound cache, mixer handles, what's
  playing); everything reaches it as `controller::Message`s on one `mpsc` channel (façade calls,
  PipeWire events, decode results). It polls the mixers every 10 ms only while voices play.
- `audio::PipeWireService` owns a `pipewire` thread running a `MainLoop`; all PipeWire objects
  (`!Send`) live there. Commands go in via `pipewire::channel`, `audio::Event`s go to a callback.
- Two output streams (`audio::output`, `RT_PROCESS`, `node.always-process` so they render while
  unlinked): `soundboars-inject` (linked to the target) and `soundboars-local` (autoconnected).
  The target is `None`, an app node, or the virtual mic (`AudioEngine::use_virtual_mic()`;
  `audio::virtual_mic` creates the node, it lives as long as its proxy). The service also binds the
  sources and the `default` metadata (`default.audio.source`) and reports the mic it mixes in as
  `EngineEvent::Microphone` (`link_plan::choose_microphone`: the default source, else the highest
  `priority.session`, never `soundboars-mic` itself). `link_plan::plan_virtual_mic` always links
  inject → virtual mic and the mic → virtual mic only while the virtual mic has consumers
  (`Graph::has_consumers`; a pavucontrol level meter counts too).
  PipeWire < 1.6.9 can silently stop sending metadata changes to every bound client (a client that
  binds and leaves before the owner's pong leaks `pending` in module-metadata, PipeWire #5445,
  fixed by commit 06de0ed2/b784720b); short-lived `pw-metadata`/`wpctl` calls trigger it. On such
  servers (`graph::may_lose_metadata_updates`, version from the core info) the service re-binds the
  `default` metadata every 2 s while the virtual mic is the target (a new binding always gets the
  current values). The CI container has 1.4.2, so `tests/virtual_mic.rs` exercises this path.
  Each render closure is a `mixer::Mixer` on PipeWire's data thread: commands arrive via an `rtrb`
  ring; ended voices go back (with their `Arc<[f32]>`, so the last `Arc` is never dropped on the RT
  thread) via a second ring. Call volume is applied after the limiter and halved for targets that
  sum both channels into one port (`link_plan::sums_channels`).
- Decoding (`audio::decode`, symphonia + rubato) runs on the `sound-decoder` thread
  (`loader::SoundCache`, a priority queue: `load` jumps ahead of `preload`); `invalidate` drops
  cached sounds and discards queued and in-flight stale decodes. The app preloads every listed
  sound (`AudioEngine::preload`, after every library change) so a click plays right away; the cache
  then `retain`s only those. Decoded audio is f32 48 kHz stereo (~23 MB per minute).
- `PlayRequest`/`Playing` carry a `clip` id besides the path: several clips share one file (and its
  decoded buffer), but retrigger and stop per `(path, clip)`; stopping a file (it changed on disk)
  stops all its clips.
- The dev profile builds dependencies with `opt-level = 3` and the crate with `1` (rubato's generics
  are monomorphized in our crate; unoptimized decoding was far too slow for `cargo run`).
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
  (`SoundConfig`, keyed by path relative to the tab folder): `name` (shown instead of the file
  name), `color` (`color::TileColor`: a preset name like `"red"` or `"#ff8800"`; invalid values are
  dropped with a warning), volume, trim, hotkey, MIDI, and `clips` (`ClipConfig { id ≥ 1,
  #[serde(flatten)] config }`: more clips of the same file; nested clips and bad ids are dropped
  on load). `tabs[i].order` is the dragged tile order as `ClipKey`s (`"Airhorn.wav"`,
  `"Airhorn.wav#2"`), empty = by name. `$SOUNDBOARS_CONFIG` overrides the path.
- `library`: tab = one folder, non-recursive, sorted by name; `watch::FolderWatcher` (impl
  `NotifyWatcher`) reports `FolderEvent::{Listing, Contents}`. `durations::DurationProbe` measures
  every listed sound once on a background thread (`audio::decode::probe_duration`: container frame
  count, full decode as fallback); tiles show the trimmed length (`SoundConfig::trimmed_length`).
- `app`: `App` owns `AppState` (pure, unit-tested logic: visible clips, play requests, progress)
  plus `Services` (`AudioBackend` = `AudioEngine` or a fake, `FolderWatcher`, `FolderPicker`,
  `ConfigStore`, `Downloader`, `HotkeyBackends`, `MidiInput`). `App::new(services, ctx)` starts
  the background threads with `ctx.request_repaint` as their wake-up. `AppWindows(Arc<Mutex<App>>)`
  is the `eframe::App`: the main window and every open dialog are drawn in passes of their own over
  the shared `App` (`App::frame(ui, View)`: `poll()` services → `ui::draw(&AppState, view) ->
  Vec<Action>` → `apply()` → close a dialog's window once it's gone → save config when dirty and no
  pointer button is held). The root pass must not hold the lock while showing dialogs (kittest
  draws embedded dialogs inline). UI code (`app/ui/*`) only reads state and emits `Action`s; it never mutates state or
  calls services. Settings go through `Action::ChangeSetting(Setting)`. `app::targets::Targets`
  decides auto-relinking.
- Hotkeys and MIDI triggers don't go through the frame loop (a hidden Wayland window may get no
  frames): `app::triggers::Dispatcher` runs a `triggers` thread that receives `Trigger::{Hotkey(id),
  Midi(..)}` from the hotkey `EventSink` and the MIDI `on_trigger` callback, resolves them against a
  `TriggerTable` (`Arc<RwLock<…>>`) and calls `AudioBackend::player()` (`Player`: play/stop all)
  directly, then wakes the UI. `AppState::trigger_table()` (pure, unit-tested) maps shortcut ids and
  MIDI bindings to resolved `PlayRequest`s or StopAll; the app rebuilds it after every `apply` and
  `poll`. While a MIDI binding is learned, MIDI triggers go back to the UI (`Dispatcher::learned`);
  while a hotkey is recorded, global shortcut presses do nothing.
- Clips (`app/clips.rs`): a board tile is a `ClipId { path, clip }` (0 = the file's own clip, its
  `SoundConfig`; more clips in `SoundConfig::clips`). Everything that identifies a board sound uses
  it (actions, play requests, progress, editor, hotkey/MIDI targets); things about the file stay
  path-based (decode cache, preload, durations, waveform, folder events). "Duplicate" adds a clip
  with the source's name (numbered), color, volume and trim but no triggers, and opens it in the
  editor; it stays only if the editor is saved (`SoundEditor::unsaved_copy`). "Delete clip" exists
  only for extra clips. A clip's display name is its `name`, else the file stem (extra clips:
  `"<stem> (<id+1>)"`); search matches display names and file stems.
- Look (`ui/style.rs`, applied to both themes in `App::new`): egui's default visuals plus more
  spacing (item spacing 10×8, button padding 10×5, 26 px widgets, 13.5 px body text, 3 px corners),
  text fields a step lighter than egui's near-black (`text_edit_bg_color`, also the waveform
  background) with a faint resting outline (`widgets.inactive.bg_stroke` = the button fill in dark
  mode, so buttons don't change there), brighter labels. Panels use `style::bar_frame` /
  `style::content_frame` (12 px margins). egui 0.37's per-widget `StyleProvider`s need its
  `experimental` feature, so styling stays on `Style`/`Visuals`.
- Board (`ui/board.rs`): hand-painted 180×90 tiles in equal columns (no `egui::Grid`/`Button`):
  the name centered at the largest of 20/17/14/12 px that fits (≤ 3 rows, then "…"), a dim footer
  with `AppState::length_label` left and `trigger_label` right, the progress bar at the bottom.
  The accessible label stays `"<name>\n<subtitle>"`. Colors: `app/colors.rs` + `ui/tile_color.rs`
  (theme-tuned preset fills, black/white text by contrast, swatch pickers). Order: `app/order.rs`
  (`arrange`, move, insert after; entries of missing files are kept); tiles are dragged onto
  each other (before/after by pointer half), not while searching; "Sort by name" clears the order.
  Tile context menu (`ui::board::context_menu`): Edit…, Rename…, Duplicate, Delete clip, color
  swatches + custom color, Sort by name.
- Dialogs are OS windows: `ui::dialog::Dialog { view, title, size, resizable }.show(ctx, |ui| …)`
  opens a deferred viewport (app ID + icon set) drawn in its own pass via `App::frame(ui, view)`,
  so it keeps working while the main window is hidden (Wayland draws no hidden windows). Each
  dialog module has `dialog(&AppState) -> Option<Dialog>` and `body(ui, state, actions)`; the
  window's close button arrives as `dialog::close_requested(ui)` — treat it as Cancel. When a
  dialog's state goes away, `close_if_gone` closes its window from its own pass (needs the eframe
  patch). Non-resizable dialogs (Settings, Rename sound, Add sound from URL) get min = max size,
  which makes Hyprland/sway float them, and scroll content that doesn't fit; "Edit sound" stays
  resizable. Kittest embeds viewports as `egui::Window`s (a dialog's title is its label there).
  Adjust `size` when adding content. Opening a *new* dialog still needs a main-window pass.
- "Rename sound" (`app/rename.rs` state, `app/ui/rename.rs`): opened by `Action::RenameSound(clip)`
  (tile menu); name field focused, Enter saves, Esc/close cancels. A blank name or the clip's
  default name is saved as no name (`clips::saved_name`, shared with the editor).
- "Edit sound" (`app/sound_editor.rs` state/logic, `app/ui/sound_editor.rs` drawing): opened by
  `Action::EditSound(clip)` (tile context menu; rescans the folder if the file isn't listed yet),
  edits a copy (`AppState::editor`) via `Action::Editor(EditorAction)`, Save writes it into the
  clip's `SoundConfig` with `SoundEditor::apply_to`. Its title stays `Edit sound – <file name>`.
  **Extension point:** `ui::sound_editor::sections` calls one function per section
  (`name_section`, `trim_section`, `volume_section`, `color_section`, `hotkey_section`,
  `midi_section`); a new section adds one call there plus its own file, its fields to
  `SoundEditor` (+ `open`/`apply_to`) and may emit its own `Action`s. Tile context menu items go
  into `ui::board::context_menu`.
- New features: add an `Action` variant + `apply` arm, a module under `app/` for logic, and a file
  under `app/ui/` for drawing; keep edits to shared files (`app/mod.rs`, `app/ui/mod.rs`,
  `config.rs`) to small, additive insertions.
- GUI tests: `tests/gui.rs` (`egui_kittest`, fakes, AccessKit queries — every widget needs an
  accessible name or kittest fails). `Setup::start` waits until the MIDI service (its own thread)
  has reported, or its late wake-up would land in the middle of a test and break `harness.run()`'s
  step limit on a busy machine; do the same for any new startup background event. `tests/app_smoke.rs` runs the real binary on the container's
  Xvfb (wgpu on Mesa lavapipe). Icons in labels come from egui's bundled emoji fonts
  (`egui/monochrome_emoji_fonts`); kittest panics on glyphs no bundled font has.

## Hotkeys

- `hotkeys` (library side): `Hotkey` (egui `Key` + Ctrl/Alt/Shift/Super) is stored in the config
  in one canonical format, `Ctrl+Alt+Shift+Super+<egui key name>` (e.g. `Ctrl+Shift+F13`); an
  invalid one is dropped with a warning instead of rejecting the file. Only keys with an X11
  keysym (`hotkey::keysym`) can be hotkeys; it also yields the XDG `preferred_trigger`
  (`CTRL+SHIFT+F13`) and Hyprland key (`CTRL + SHIFT + F13`) formats. egui doesn't report Super on
  Linux, so Super can only be typed into the config.
- `trait HotkeyBackend { register(&[Binding]), capabilities() }`: `register` replaces the whole set.
  Events (`Pressed(id)`, `Released(id)`, `Error`) go to an `EventSink` (a callback, called on the
  backend's thread). Backends: `portal::PortalBackend` (ashpd on a `hotkey-portal` thread with
  `zbus::block_on`; one session per registration, since a session binds only once; registers the
  host app ID first) and `x11::X11Backend` (`global-hotkey`, one
  process-wide event handler, so only one may exist; needs `$DISPLAY`).
  `Capabilities { assign_in_app, bind_hint }`: `portal::capabilities_for($XDG_CURRENT_DESKTOP)`
  gives Hyprland `BindHint::Hyprland`, whose `snippet` is the Lua bind line
  `hl.bind("CTRL + F13", hl.dsp.global("me.maxjoehnk.soundboars:<id>"))` (Hyprland ≥ 0.55; older
  hyprlang configs: `bind = CTRL, F13, global, me.maxjoehnk.soundboars:<id>`).
- Bindings (`app::hotkeys::bindings`): every clip whose `SoundConfig::shortcut_id` is set (the X11
  backend skips those without `hotkey`; the description is "Play <clip's display name>"), plus
  `stop-all` (`Settings::stop_all_hotkey`). Shortcut ids are `play-<slug of the file name>` (extra
  clips too) made unique across all clips with `-2`, `-3`…, assigned when a hotkey is
  recorded or (Hyprland) "Add global shortcut" is clicked, and never changed afterwards (users
  type them into their compositor config); removing and re-adding keeps the saved id.
- `app::hotkeys::Hotkeys` owns the backend: `App::apply` ends with `sync_hotkeys()`, which restarts
  the backend when `AppState::hotkey_backend()` (setting, else portal on Wayland / X11) changed and
  re-registers when the binding set changed. Its sink sends presses to the `triggers` thread and
  errors to the UI.
- UI: `ui::hotkey_section` (editor section; `recorder` and `bind_hint` widgets are reused by the
  settings for "Stop all"). Recording takes the next key press with a hotkey key from the
  window's input (logical key, physical as fallback); Esc cancels.
- Tests: `tests/portal_hotkeys.rs` runs a zbus mock of the portal on the container's session bus;
  `tests/x11_hotkeys.rs` presses keys with `xdotool` on Xvfb (no F13+ in its keymap). Both are
  gated like the PipeWire tests.

## MIDI

- `midi::MidiService` runs a `midi` thread around a `MidiInput` (`MidirInput` = ALSA sequencer via
  `midir`, one client for listing plus one per connection; fakes in tests). It follows
  `Settings::midi_input` (`MidiDevice::{Off, All, Port(name)}`, default `Off`) and polls the port
  list every `HOTPLUG_POLL` (2 s); `midi::hotplug::plan` (pure) says what to connect/drop. Ports are
  matched by their stable name (`midir`'s name without the `client:port` address,
  `stable_port_name`). Triggers go to the `on_trigger` callback on the receiving MIDI thread;
  `MidiEvent::{Ports, Connected, Error}` are polled by the UI (`wake` after each). Errors are
  reported once until the same thing worked again.
- `midi::message::Parser` (one per port): Note On with velocity > 0 triggers; CCs trigger on the
  rising edge across 64, tracked per channel + CC; everything else is ignored. Channels are 1–16.
- `MidiBinding { port: Option<String>, channel, control }` on `SoundConfig::midi` (per clip) and
  `Settings::midi_stop_all`; `port: None` matches any port ("Any device" in the editor). It's the
  one config struct without `#[serde(default)]` (a binding without channel/control means nothing).
- App side (`app/midi.rs`): `MidiState::learning` captures the next trigger instead of acting on
  it (`Action::Midi(MidiAction::Learn(..))`; closing the window or Esc cancels). Otherwise the
  `triggers` thread plays it (see App architecture). UI: `ui/midi_section.rs` (editor section, settings rows, shared learn widget).
- `tests/midi_alsa.rs` needs `/dev/snd/seq` and runs only inside the container; it skips otherwise.
  The kernel sequencer isn't namespaced, so `in-container.sh` passes the host's through only with
  `SOUNDBOARS_HOST_MIDI=1` (the test then only touches its own virtual port). Agents: don't set it.

## URL import

- `import::Downloader` (in `Services`; one download at a time, polled per frame → `DownloadEvent::
  {Progress(Stage), Done(path), Failed(message)}`); impl `import::ytdlp::YtDlp`, fakes in tests.
  `YtDlp::new()` checks `$PATH` for `yt-dlp`, `ffmpeg`, `ffprobe` once; if any is missing, the
  "🔗 Add from URL" button is disabled with the reason as tooltip (`AppState::url_import_unavailable`).
- yt-dlp runs without a shell (`ytdlp::command`, args pinned by a unit test): `--ignore-config`,
  `--no-playlist`, `-x --audio-format mp3`, `--windows-filenames --trim-filenames 120`,
  `--no-overwrites`, output `%(title)s.%(ext)s`. Progress comes from our own `--progress-template`
  / `--print post_process:` / `--print after_move:%(filepath)s` lines on stdout; the error message
  is yt-dlp's `ERROR:` lines (else its last stderr lines).
- It works in a hidden staging folder inside the tab folder (`--paths temp:…/.soundboars-download-*`,
  a `TempDir`), so `.part`/intermediate files never show up as sounds and the finished MP3 arrives
  by rename. Cancel (or dropping `YtDlp`) kills yt-dlp's process group (incl. ffmpeg) and removes
  the staging folder. A file with the same name is kept, not overwritten; yt-dlp reports it as done,
  so the editor opens the existing file.
- Dialog: `app/url_import.rs` (state, URL check: http(s) with a host) + `app/ui/url_import.rs`.
  **Closing it (Cancel or the window's close button) cancels a running download.** Without tabs it
  offers "➕ Add folder". On `Done` it closes, selects the file's tab and runs `Action::EditSound`.
- Tests: `tests/url_import.rs` runs the real yt-dlp against an HTTP server inside the test on
  127.0.0.1 (no network; container only); GUI tests use a fake downloader. The animated progress
  bar never lets kittest settle, so those tests use `run_steps`.

## Tiling window managers

winit 0.30 never sets an `xdg_toplevel` parent on Wayland, so tiling compositors don't know our
dialogs are dialogs. Hyprland (`suggestsFloat = parent || min size == max size`, checked in
0.57.2's source) and sway float fixed-size windows, so the non-resizable dialogs (Settings, Rename
sound, Add sound from URL) float on their own (verified on a headless sway). "Edit sound" is
resizable and needs a rule by app ID (`me.maxjoehnk.soundboars`) and initial title. Hyprland ≥ 0.55
(Lua config, [wiki](https://wiki.hypr.land/0.57.0/configuring/core/rules/window-rules/); RE2
regex):

```lua
hl.window_rule({
  match = {
    class = [[^me\.maxjoehnk\.soundboars$]],
    title = [[^Edit sound.*$]],
  },
  float = true,
})
```

Hyprland 0.53–0.54 (hyprlang): `windowrule = match:class ^(me\.maxjoehnk\.soundboars)$, match:title ^(Edit sound.*)$, float on`

## Milestones

| # | Milestone | Status |
|---|---|---|
| 1 | Docker/CI harness + spike: inject stream, list recording apps, link into one; portal shortcut registration | done |
| 2 | Audio engine: decoding + cache, both streams, voices, retrigger modes, volumes, limiter | done |
| 3 | App UI: folder tabs, `notify`, config, target picker with auto-relink | done |
| 4 | Hotkeys: trait, portal backend (with Hyprland bind hints), X11 backend | done |
| 5 | MIDI: midir, learn mode, hotplug | done |
| 6 | Trim editor: waveform peaks, non-destructive trim, preview | done |
| 7 | URL import: yt-dlp runner, progress, errors → trim editor | done |
| 8 | Polish: tray, multi-target, Opus, export trimmed copy | later |
| 9 | Sound sharing: `soundboars-hub` server (axum + SQLite, k3s), API keys via its CLI, browse/preview/add/upload/delete in the app | planned |
