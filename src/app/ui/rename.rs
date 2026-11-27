//! The "Rename sound" window: a name field, Save and Cancel. Enter saves; Esc or closing the window
//! cancels.

use eframe::egui;

use super::View;
use super::dialog::{Dialog, close_requested};
use crate::app::{Action, AppState, Rename, RenameAction};

pub fn dialog(state: &AppState) -> Option<Dialog> {
    state.rename.as_ref().map(|_| Dialog {
        view: View::Rename,
        title: "Rename sound".to_owned(),
        size: [360.0, 110.0],
        resizable: false,
    })
}

pub fn body(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let Some(rename) = &state.rename else {
        return;
    };
    name_field(ui, rename, actions);
    ui.weak(format!("File: {}", rename.file_name));
    ui.separator();
    buttons(ui, actions);
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if escape || close_requested(ui) {
        actions.push(Action::Rename(RenameAction::Close));
    }
}

fn name_field(ui: &mut egui::Ui, rename: &Rename, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let caption = ui.label("Name:");
        let mut name = rename.name.clone();
        let field = egui::TextEdit::singleline(&mut name)
            .hint_text(rename.default_name())
            .desired_width(f32::INFINITY);
        let response = ui.add(field).labelled_by(caption.id);
        if response.changed() {
            actions.push(Action::Rename(RenameAction::Name(name)));
        }
        if rename.focus {
            response.request_focus();
            actions.push(Action::Rename(RenameAction::Focused));
        }
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            actions.push(Action::Rename(RenameAction::Save));
        }
    });
}

fn buttons(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            actions.push(Action::Rename(RenameAction::Save));
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::Rename(RenameAction::Close));
        }
    });
}
