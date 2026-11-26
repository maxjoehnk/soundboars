# Vendored crates

## eframe 0.37.0 (patched)

`vendor/eframe` is eframe 0.37.0 from crates.io (MIT OR Apache-2.0, © Emil Ernerfeldt and the
egui contributors, <https://github.com/emilk/egui>), used via `[patch.crates-io]` in
`Cargo.toml`. The only change is `eframe-close-deferred-viewports.patch`
(`src/native/wgpu_integration.rs`).

**Why:** SoundBoars' dialogs are deferred viewports, so they keep working while the main window
is hidden (Wayland compositors don't let hidden windows draw). Unpatched eframe only removes a
deferred viewport's window in its *parent's* pass, so a dialog closed with Save/Cancel or the
window's close button stayed on screen, blank, until the main window was shown again.

**What:** when a deferred viewport's own pass ends with a close request — a
`ViewportCommand::Close` for itself, or the user closing the window without `CancelClose` — its
window is dropped right away. It stays closed until the parent's next pass, which decides again
whether to show it. The app side is `app::close_if_gone`.

**Upgrading egui/eframe:** copy the new eframe release from `~/.cargo/registry/src/*/eframe-<ver>`
here (without `Cargo.lock`, `.cargo_vcs_info.json`, `.cargo-ok`), re-apply the patch
(`patch -p3 -d vendor/eframe < vendor/eframe-close-deferred-viewports.patch`, fix conflicts by
hand), and run the dialog tests. Drop the vendored copy once eframe supports this upstream.
