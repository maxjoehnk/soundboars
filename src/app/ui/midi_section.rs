//! MIDI: the "Edit sound" section, the settings rows and the learn widget both use.

use eframe::egui;

use crate::app::sound_editor::{EditorAction, SoundEditor};
use crate::app::{Action, AppState, LearnTarget, MidiAction};
use crate::midi::{MidiBinding, MidiDevice};

/// The sound's binding with Learn and Clear, and which other sounds a trigger would play, too.
pub fn midi_section(
    ui: &mut egui::Ui,
    state: &AppState,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    let binding = editor.midi_binding();
    ui.horizontal(|ui| {
        let learn = LearnWidget {
            caption: ui.label("MIDI:").id,
            binding: binding.as_ref(),
            target: LearnTarget::Sound,
            clear: Action::Editor(EditorAction::ClearMidi),
        };
        learn.show(ui, state, actions);
        if state.midi.learning.is_none() && editor.midi.is_some() {
            any_device_checkbox(ui, editor, actions);
        }
    });
    if let Some(binding) = &binding {
        let mut others = state.sounds_bound_like(binding, Some(&editor.clip));
        let stop_all = state.config.settings.midi_stop_all.as_ref();
        if stop_all.is_some_and(|stop_all| stop_all.overlaps(binding)) {
            others.push("Stop all".to_owned());
        }
        duplicate_warning(ui, &others);
    }
    ui.add_space(6.0);
}

fn any_device_checkbox(ui: &mut egui::Ui, editor: &SoundEditor, actions: &mut Vec<Action>) {
    let mut any = editor.midi_any_device;
    let learned_port = editor.midi.as_ref().is_some_and(|midi| midi.port.is_some());
    let checkbox = egui::Checkbox::new(&mut any, "Any device");
    let response = ui
        .add_enabled(learned_port, checkbox)
        .on_hover_text("React to this note or CC from every MIDI input, not only the learned one");
    if response.changed() {
        actions.push(Action::Editor(EditorAction::MidiAnyDevice(any)));
    }
}

/// Grid rows for the settings: which ports to listen to, which are connected, and the binding of
/// "Stop all".
pub fn settings_rows(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let caption = ui.label("MIDI input:");
    input_picker(ui, state, actions).labelled_by(caption.id);
    ui.end_row();

    let caption = ui.label("Listening to:");
    let connected = match state.midi.connected.as_slice() {
        [] => "nothing".to_owned(),
        ports => ports.join(", "),
    };
    ui.label(connected).labelled_by(caption.id);
    ui.end_row();

    let stop_all = state.config.settings.midi_stop_all.as_ref();
    let learn = LearnWidget {
        caption: ui.label("Stop all (MIDI):").id,
        binding: stop_all,
        target: LearnTarget::StopAll,
        clear: Action::Midi(MidiAction::ClearStopAll),
    };
    ui.vertical(|ui| {
        ui.horizontal(|ui| learn.show(ui, state, actions));
        if let Some(binding) = stop_all {
            duplicate_warning(ui, &state.sounds_bound_like(binding, None));
        }
    });
    ui.end_row();
}

fn input_picker(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) -> egui::Response {
    let current = &state.config.settings.midi_input;
    let combo = egui::ComboBox::from_id_salt("midi input")
        .width(260.0)
        .selected_text(device_label(current, &state.midi.ports))
        .show_ui(ui, |ui| {
            let mut options = vec![MidiDevice::Off, MidiDevice::All];
            options.extend(state.midi.ports.iter().cloned().map(MidiDevice::Port));
            for device in options {
                let label = device_label(&device, &state.midi.ports);
                if ui.selectable_label(*current == device, label).clicked() {
                    actions.push(Action::Midi(MidiAction::SelectInput(device)));
                }
            }
        });
    combo.response
}

fn device_label(device: &MidiDevice, ports: &[String]) -> String {
    match device {
        MidiDevice::Off => "(none)".to_owned(),
        MidiDevice::All => "All devices".to_owned(),
        MidiDevice::Port(name) if ports.contains(name) => name.clone(),
        MidiDevice::Port(name) => format!("{name} (not plugged in)"),
    }
}

/// A binding with Learn and Clear, or, while learning for its target, a prompt with Cancel (Esc
/// cancels, too). Goes after its caption in a horizontal layout.
struct LearnWidget<'a> {
    /// The caption's id, for the accessible name.
    caption: egui::Id,
    binding: Option<&'a MidiBinding>,
    target: LearnTarget,
    /// Emitted by Clear.
    clear: Action,
}

impl LearnWidget<'_> {
    fn show(self, ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
        if state.midi.learning == Some(self.target) {
            ui.label("Press a pad, key or button on your MIDI controller… (Esc cancels)")
                .labelled_by(self.caption);
            let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
            if ui.button("Cancel learning").clicked() || escape {
                actions.push(Action::Midi(MidiAction::CancelLearn));
            }
            return;
        }
        let text = self
            .binding
            .map_or_else(|| "—".to_owned(), ToString::to_string);
        ui.label(text).labelled_by(self.caption);
        let midi_off = state.config.settings.midi_input == MidiDevice::Off;
        let learn = ui
            .add_enabled(!midi_off, egui::Button::new("Learn"))
            .on_hover_text("Bind the next pad, key or button pressed on a MIDI controller")
            .on_disabled_hover_text("Choose a MIDI input in the settings first");
        if learn.clicked() {
            actions.push(Action::Midi(MidiAction::Learn(self.target)));
        }
        let clear = egui::Button::new("Clear");
        if ui.add_enabled(self.binding.is_some(), clear).clicked() {
            actions.push(self.clear);
        }
    }
}

/// Warns (without blocking) that a trigger also does these.
fn duplicate_warning(ui: &mut egui::Ui, others: &[String]) {
    if !others.is_empty() {
        let text = format!("⚠ Also triggers: {}", others.join(", "));
        ui.colored_label(ui.visuals().warn_fg_color, text);
    }
}
