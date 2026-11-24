//! Target picker, settings button, tabs and search.

use eframe::egui;

use crate::app::targets::{NO_APPS_HINT, label};
use crate::app::{Action, AppState};

pub fn show(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    egui::Panel::top("top").show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("SoundBoars");
            ui.separator();
            target_picker(ui, state, actions);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙ Settings").clicked() {
                    actions.push(Action::ShowSettings(true));
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            tabs(ui, state, actions);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                search(ui, state, actions);
            });
        });
        ui.add_space(4.0);
    });
}

fn target_picker(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let saved = state.config.target.as_ref();
    let selected = state.targets.linked_app().map(|app| app.node_id);
    let caption = ui.label("Inject into:");
    ui.add_enabled_ui(state.audio_available, |ui| {
        let combo = egui::ComboBox::from_id_salt("target")
            .width(260.0)
            .selected_text(state.targets.selected_label(saved))
            .show_ui(ui, |ui| {
                if ui.selectable_label(selected.is_none(), "None").clicked() {
                    actions.push(Action::SelectTarget(None));
                }
                for app in state.targets.apps() {
                    let is_selected = selected == Some(app.node_id);
                    if ui.selectable_label(is_selected, label(app)).clicked() {
                        actions.push(Action::SelectTarget(Some(app.node_id)));
                    }
                }
                if state.targets.apps().is_empty() {
                    ui.weak(NO_APPS_HINT);
                }
            });
        combo
            .response
            .labelled_by(caption.id)
            .on_hover_text(NO_APPS_HINT);
        if ui
            .button("⟳")
            .on_hover_text("Link to the remembered app again")
            .clicked()
        {
            actions.push(Action::RefreshTargets);
        }
    });
}

fn tabs(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    for (index, tab) in state.config.tabs.iter().enumerate() {
        let selected = index == state.tab && !state.is_searching();
        let response = ui
            .selectable_label(selected, tab.display_name())
            .on_hover_text(tab.path.display().to_string());
        if response.clicked() {
            actions.push(Action::SelectTab(index));
        }
        response.context_menu(|ui| {
            if ui
                .button("Remove tab")
                .on_hover_text("The folder and its files stay")
                .clicked()
            {
                actions.push(Action::RemoveTab(index));
                ui.close();
            }
        });
    }
    if ui
        .button("➕ Folder")
        .on_hover_text("Add a folder as a tab")
        .clicked()
    {
        actions.push(Action::AddFolder);
    }
}

fn search(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let mut query = state.search.clone();
    let response = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("🔍 Search all sounds…")
            .desired_width(220.0),
    );
    if response.changed() {
        actions.push(Action::Search(query));
    }
}
