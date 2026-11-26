//! The settings dialog.

use eframe::egui;

use super::View;
use super::dialog::{Dialog, close_requested};
use super::hotkey_section::{RecorderEvent, bind_hint, recorder};
use super::midi_section;
use crate::app::{Action, AppState, STOP_ALL_ID, Setting};
use crate::audio::Retrigger;
use crate::config::Theme;
use crate::hotkeys::BackendKind;

pub fn dialog(state: &AppState) -> Option<Dialog> {
    state.settings_open.then(|| Dialog {
        view: View::Settings,
        title: "Settings".to_owned(),
        size: [560.0, 340.0],
    })
}

pub fn body(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    if !state.settings_open {
        return;
    }
    egui::Grid::new("settings")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            rows(ui, state, actions);
            midi_section::settings_rows(ui, state, actions);
        });
    ui.separator();
    hotkey_settings(ui, state, actions);
    if close_requested(ui) {
        actions.push(Action::ShowSettings(false));
    }
}

fn rows(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let settings = &state.config.settings;
    let retrigger_options = [
        (Retrigger::Overlap, "Overlap"),
        (Retrigger::Restart, "Restart"),
        (Retrigger::Stop, "Stop"),
    ];
    let caption = "Re-trigger playing sound:";
    if let Some(retrigger) = choice_row(ui, caption, settings.retrigger, retrigger_options) {
        actions.push(Action::ChangeSetting(Setting::Retrigger(retrigger)));
    }
    let theme_options = [(Theme::Dark, "Dark"), (Theme::Light, "Light")];
    if let Some(theme) = choice_row(ui, "Theme:", settings.theme, theme_options) {
        actions.push(Action::ChangeSetting(Setting::Theme(theme)));
    }
    let backend_options = [
        (BackendKind::Portal, "XDG portal"),
        (BackendKind::X11, "X11 (global-hotkey)"),
        (BackendKind::Off, "Off"),
    ];
    let backend = state.hotkey_backend();
    if let Some(kind) = choice_row(ui, "Hotkey backend:", backend, backend_options) {
        actions.push(Action::ChangeSetting(Setting::HotkeyBackend(kind)));
    }
}

/// A hint on the chosen backend, and the "Stop all" hotkey.
fn hotkey_settings(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let capabilities = state.hotkey_capabilities;
    if let Some(hint) = backend_hint(state.hotkey_backend(), capabilities.assign_in_app) {
        ui.weak(hint);
    }
    let settings = &state.config.settings;
    let caption = "Stop all hotkey:";
    if let Some(hint) = capabilities
        .bind_hint
        .filter(|_| !capabilities.assign_in_app)
    {
        bind_hint(
            ui,
            caption,
            hint,
            STOP_ALL_ID,
            settings.stop_all_hotkey,
            false,
        );
        return;
    }
    let event = recorder(
        ui,
        caption,
        settings.stop_all_hotkey,
        state.recording_stop_all,
    );
    actions.extend(event.map(|event| match event {
        RecorderEvent::Start => Action::RecordStopAllHotkey(true),
        RecorderEvent::Cancel => Action::RecordStopAllHotkey(false),
        RecorderEvent::Recorded(hotkey) => {
            Action::ChangeSetting(Setting::StopAllHotkey(Some(hotkey)))
        }
        RecorderEvent::Clear => Action::ChangeSetting(Setting::StopAllHotkey(None)),
    }));
}

fn backend_hint(kind: BackendKind, assign_in_app: bool) -> Option<&'static str> {
    match kind {
        BackendKind::X11 => Some(
            "X11 hotkeys only work in X11 sessions. On Wayland they only fire while a window of \
             an X11 app (XWayland) has focus.",
        ),
        BackendKind::Portal if !assign_in_app => Some(
            "Your desktop ignores the keys chosen here: bind the global shortcuts in its config \
             (see “Edit sound” of each sound).",
        ),
        BackendKind::Portal => {
            Some("Your desktop may ask you to confirm or change the keys when they are registered.")
        }
        BackendKind::Off => None,
    }
}

/// A grid row of radio buttons; returns the option the user clicked.
fn choice_row<T: PartialEq + Copy, const N: usize>(
    ui: &mut egui::Ui,
    caption: &str,
    current: T,
    options: [(T, &str); N],
) -> Option<T> {
    ui.label(caption);
    let mut chosen = None;
    ui.horizontal(|ui| {
        for (option, name) in options {
            if ui.radio(current == option, name).clicked() {
                chosen = Some(option);
            }
        }
    });
    ui.end_row();
    chosen
}
