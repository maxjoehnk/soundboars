//! Target picker, settings and "Add from URL" buttons, tabs and search.

use eframe::egui;

use crate::app::targets::{NO_APPS_HINT, VIRTUAL_MIC_HINT, VIRTUAL_MIC_LABEL, label};
use crate::app::{Action, AppState};
use crate::config::Injection;

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
                url_import_button(ui, state, actions);
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

/// Disabled, with the reason as tooltip, when yt-dlp or ffmpeg are missing.
fn url_import_button(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let unavailable = state.url_import_unavailable.as_deref();
    let button = ui
        .add_enabled(unavailable.is_none(), egui::Button::new("🔗 Add from URL"))
        .on_hover_text("Download a sound from YouTube or another site")
        .on_disabled_hover_text(unavailable.unwrap_or_default());
    if button.clicked() {
        actions.push(Action::OpenUrlImport);
    }
}

/// "Inject into:" the virtual microphone, an app, or nothing.
fn target_picker(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let virtual_mic = state.config.injection == Injection::VirtualMic;
    let saved = state.config.target.as_ref();
    let selected = state.targets.linked_app().map(|app| app.node_id);
    let caption = ui.label("Inject into:");
    ui.add_enabled_ui(state.audio_available, |ui| {
        let selected_text = if virtual_mic {
            VIRTUAL_MIC_LABEL.to_owned()
        } else {
            state.targets.selected_label(saved)
        };
        let combo = egui::ComboBox::from_id_salt("target")
            .width(260.0)
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(virtual_mic, VIRTUAL_MIC_LABEL)
                    .on_hover_text(VIRTUAL_MIC_HINT)
                    .clicked()
                {
                    actions.push(Action::SelectVirtualMic);
                }
                let no_app = !virtual_mic && selected.is_none();
                if ui.selectable_label(no_app, "None").clicked() {
                    actions.push(Action::SelectTarget(None));
                }
                for app in state.targets.apps() {
                    let is_selected = !virtual_mic && selected == Some(app.node_id);
                    if ui.selectable_label(is_selected, label(app)).clicked() {
                        actions.push(Action::SelectTarget(Some(app.node_id)));
                    }
                }
                if state.targets.apps().is_empty() {
                    ui.weak(NO_APPS_HINT);
                }
            });
        let hint = if virtual_mic {
            VIRTUAL_MIC_HINT
        } else {
            NO_APPS_HINT
        };
        combo.response.labelled_by(caption.id).on_hover_text(hint);
        if virtual_mic {
            microphone_hint(ui, state);
        } else if ui
            .button("⟳")
            .on_hover_text("Link to the remembered app again")
            .clicked()
        {
            actions.push(Action::RefreshTargets);
        }
    });
}

/// Which real microphone the virtual microphone carries besides the sounds.
fn microphone_hint(ui: &mut egui::Ui, state: &AppState) {
    let text = match state.targets.microphone() {
        Some(microphone) => format!("with {microphone}"),
        None => "no microphone found".to_owned(),
    };
    ui.weak(text).on_hover_text(format!(
        "{VIRTUAL_MIC_HINT}\nIt carries the sounds and your default microphone while an app \
         records it."
    ));
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
