//! The "Edit sound" window's hotkey section, plus the widgets it shares with the settings: the
//! hotkey recorder and the bind hint for desktops that bind keys themselves (Hyprland).

use eframe::egui::{self, Event, Key};

use crate::app::sound_editor::{EditorAction, SoundEditor};
use crate::app::{Action, AppState};
use crate::hotkeys::{BindHint, Hotkey};

pub fn hotkey_section(
    ui: &mut egui::Ui,
    state: &AppState,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    let capabilities = state.hotkey_capabilities;
    match capabilities
        .bind_hint
        .filter(|_| !capabilities.assign_in_app)
    {
        Some(hint) => external_shortcut(ui, hint, editor, actions),
        None => {
            let event = recorder(ui, "Hotkey:", editor.hotkey, editor.recording);
            actions.extend(event.map(|event| {
                Action::Editor(match event {
                    RecorderEvent::Start => EditorAction::RecordHotkey,
                    RecorderEvent::Cancel => EditorAction::CancelRecording,
                    RecorderEvent::Recorded(hotkey) => EditorAction::Hotkey(hotkey),
                    RecorderEvent::Clear => EditorAction::ClearHotkey,
                })
            }));
        }
    }
    ui.add_space(6.0);
}

/// The sound's global shortcut, bound in the desktop's config.
fn external_shortcut(
    ui: &mut egui::Ui,
    hint: BindHint,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    let Some(id) = &editor.shortcut_id else {
        ui.horizontal(|ui| {
            ui.label("Global shortcut:");
            if ui.button("Add global shortcut").clicked() {
                actions.push(Action::Editor(EditorAction::AddShortcut));
            }
        });
        return;
    };
    let remove = bind_hint(ui, "Global shortcut:", hint, id, editor.hotkey, true);
    if remove {
        actions.push(Action::Editor(EditorAction::ClearHotkey));
    }
}

/// The shortcut id and the line binding it in the desktop's config, with a copy button. Returns
/// `true` when "Remove" (shown if `removable`) was clicked.
pub fn bind_hint(
    ui: &mut egui::Ui,
    caption: &str,
    hint: BindHint,
    id: &str,
    hotkey: Option<Hotkey>,
    removable: bool,
) -> bool {
    let mut remove = false;
    ui.horizontal(|ui| {
        let caption = ui.label(caption);
        ui.monospace(id).labelled_by(caption.id);
        if removable && ui.button("Remove").clicked() {
            remove = true;
        }
    });
    let snippet = hint.snippet(id, hotkey);
    ui.weak(hint.instructions());
    ui.horizontal(|ui| {
        if ui
            .button("Copy")
            .on_hover_text("Copy to the clipboard")
            .clicked()
        {
            ui.ctx().copy_text(snippet.clone());
        }
        ui.monospace(&snippet);
    });
    remove
}

/// What the user did with a [`recorder`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecorderEvent {
    Start,
    Cancel,
    Recorded(Hotkey),
    Clear,
}

/// Shows `hotkey` with "Record" and "Clear". While `recording`, the next key combination pressed
/// (in this window) is recorded; Escape cancels.
pub fn recorder(
    ui: &mut egui::Ui,
    caption: &str,
    hotkey: Option<Hotkey>,
    recording: bool,
) -> Option<RecorderEvent> {
    // Before drawing, so the keys don't also reach the buttons.
    let mut event = if recording { take_key_press(ui) } else { None };
    ui.horizontal(|ui| {
        let caption = ui.label(caption);
        if recording {
            ui.strong("Press the keys… (Esc cancels)")
                .labelled_by(caption.id);
            return;
        }
        let shown = hotkey.map_or_else(|| "None".to_owned(), |hotkey| hotkey.to_string());
        ui.monospace(shown).labelled_by(caption.id);
        if ui.button("Record").clicked() {
            event = Some(RecorderEvent::Start);
        }
        if ui
            .add_enabled(hotkey.is_some(), egui::Button::new("Clear"))
            // Unique, next to the MIDI section's "Clear".
            .accessible_name("Clear hotkey")
            .clicked()
        {
            event = Some(RecorderEvent::Clear);
        }
    });
    event
}

/// Removes the first key press that makes a hotkey (or Escape) from this frame's input.
fn take_key_press(ui: &egui::Ui) -> Option<RecorderEvent> {
    ui.input_mut(|input| {
        let index = input.events.iter().position(|event| {
            matches!(event, Event::Key { key, physical_key, pressed: true, repeat: false, modifiers }
                if *key == Key::Escape || Hotkey::from_key_press(*key, *physical_key, *modifiers).is_some())
        })?;
        match input.events.remove(index) {
            Event::Key { key: Key::Escape, .. } => Some(RecorderEvent::Cancel),
            Event::Key {
                key,
                physical_key,
                modifiers,
                ..
            } => Hotkey::from_key_press(key, physical_key, modifiers).map(RecorderEvent::Recorded),
            _ => None,
        }
    })
}
