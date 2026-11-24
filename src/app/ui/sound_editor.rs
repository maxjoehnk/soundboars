//! The "Edit sound" window. Every section is one function, called in order from [`body`].

use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use super::dialog::Dialog;
use super::hotkey_section::hotkey_section;
use crate::app::sound_editor::{self, EditorAction, SoundEditor, WaveformState};
use crate::app::{Action, AppState, format_duration};
use crate::audio::Waveform;

const WAVEFORM_HEIGHT: f32 = 120.0;

pub fn show(ctx: &egui::Context, state: &AppState, actions: &mut Vec<Action>) {
    let Some(editor) = &state.editor else {
        return;
    };
    let title = format!("Edit sound – {}", editor.name);
    let dialog = Dialog {
        id: "sound editor",
        title: &title,
        size: [680.0, 340.0],
    };
    if dialog.show(ctx, |ui| body(ui, state, editor, actions)) {
        actions.push(Action::Editor(EditorAction::Close));
    }
}

fn body(ui: &mut egui::Ui, state: &AppState, editor: &SoundEditor, actions: &mut Vec<Action>) {
    name_section(ui, editor);
    trim_section(ui, state, editor, actions);
    volume_section(ui, editor, actions);
    hotkey_section(ui, state, editor, actions);
    ui.separator();
    buttons(ui, actions);
}

fn name_section(ui: &mut egui::Ui, editor: &SoundEditor) {
    ui.horizontal(|ui| {
        let caption = ui.label("Sound:");
        ui.strong(&editor.name).labelled_by(caption.id);
    });
    ui.add_space(6.0);
}

fn trim_section(
    ui: &mut egui::Ui,
    state: &AppState,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    ui.label("Trim (drag the handles):");
    let size = Vec2::new(ui.available_width(), WAVEFORM_HEIGHT);
    match &editor.waveform {
        WaveformState::Loading => placeholder(ui, size, "Loading waveform…", None),
        WaveformState::Failed(error) => {
            let message = format!("No waveform: {error}");
            placeholder(ui, size, &message, Some(ui.visuals().error_fg_color));
        }
        WaveformState::Ready(waveform) => {
            let playhead = playhead_ms(state, editor);
            waveform_view(ui, size, editor, waveform, playhead, actions);
        }
    }
    trim_controls(ui, state, editor, actions);
    ui.add_space(6.0);
}

fn placeholder(ui: &mut egui::Ui, size: Vec2, message: &str, color: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter()
        .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    let mut text = egui::RichText::new(message);
    if let Some(color) = color {
        text = text.color(color);
    }
    ui.put(rect, egui::Label::new(text));
}

/// Where the preview is now, while it plays.
fn playhead_ms(state: &AppState, editor: &SoundEditor) -> Option<u64> {
    let preview = state.preview(&editor.path)?;
    Some(editor.playhead_ms(preview, Instant::now()))
}

/// The waveform with the trim handles; dragging moves the handle nearer to where it started.
fn waveform_view(
    ui: &mut egui::Ui,
    size: Vec2,
    editor: &SoundEditor,
    waveform: &Waveform,
    playhead_ms: Option<u64>,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::drag());
    let response = response.accessible_name("Waveform");
    let duration = sound_editor::duration_ms(waveform.duration);
    let x_of = |ms| sound_editor::x_of(ms, rect.left(), rect.width(), duration);
    let start = x_of(editor.start_ms);
    let end = x_of(editor.end_ms.unwrap_or(duration));

    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    draw_peaks(&painter, rect, waveform, start..=end, visuals);
    let shade = Color32::from_black_alpha(90);
    painter.rect_filled(
        Rect::from_x_y_ranges(rect.left()..=start, rect.y_range()),
        0.0,
        shade,
    );
    painter.rect_filled(
        Rect::from_x_y_ranges(end..=rect.right(), rect.y_range()),
        0.0,
        shade,
    );
    for x in [start, end] {
        painter.vline(
            x,
            rect.y_range(),
            Stroke::new(2.0, visuals.strong_text_color()),
        );
    }
    if let Some(ms) = playhead_ms {
        let red = Color32::from_rgb(230, 80, 80);
        painter.vline(x_of(ms), rect.y_range(), Stroke::new(1.5, red));
    }

    if let Some(pos) = response.interact_pointer_pos() {
        let ms = sound_editor::ms_at(pos.x, rect.left(), rect.width(), duration);
        if response.drag_started() {
            actions.push(Action::Editor(EditorAction::Grab(ms)));
        } else if response.dragged() && response.drag_delta() != Vec2::ZERO {
            actions.push(Action::Editor(EditorAction::Drag(ms)));
        }
    }
    if response.drag_stopped() {
        actions.push(Action::Editor(EditorAction::Release));
    }
}

/// One vertical line per peak, highlighted inside the trim.
fn draw_peaks(
    painter: &egui::Painter,
    rect: Rect,
    waveform: &Waveform,
    trimmed: std::ops::RangeInclusive<f32>,
    visuals: &egui::Visuals,
) {
    let count = waveform.peaks.len() as f32;
    let half_height = rect.height() / 2.0;
    for (index, peak) in waveform.peaks.iter().enumerate() {
        let x = rect.left() + rect.width() * (index as f32 + 0.5) / count;
        let color = if trimmed.contains(&x) {
            visuals.selection.stroke.color
        } else {
            visuals.weak_text_color()
        };
        let top = rect.center().y - peak.max.clamp(-1.0, 1.0) * half_height;
        let bottom = rect.center().y - peak.min.clamp(-1.0, 1.0) * half_height;
        painter.line_segment(
            [Pos2::new(x, top), Pos2::new(x, bottom.max(top + 1.0))],
            Stroke::new(1.0, color),
        );
    }
}

/// Start and end as numbers, the length, Reset and Preview; once the sound's length is known.
fn trim_controls(
    ui: &mut egui::Ui,
    state: &AppState,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    let Some(duration) = editor.duration_ms() else {
        return;
    };
    ui.horizontal(|ui| {
        let end = editor.end_ms.unwrap_or(duration);
        if let Some(start) = seconds_field(ui, "Start", editor.start_ms, 0..=end) {
            actions.push(Action::Editor(EditorAction::Start(start)));
        }
        if let Some(end) = seconds_field(ui, "End", end, editor.start_ms..=duration) {
            actions.push(Action::Editor(EditorAction::End(end)));
        }
        if let Some(length) = editor.trimmed_length() {
            ui.weak(format!("Length {}", format_duration(length)));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("↺ Reset").clicked() {
                actions.push(Action::Editor(EditorAction::Reset));
            }
            preview_button(ui, state, editor, actions);
        });
    });
}

/// A time in seconds; returns the new time in milliseconds when it changed.
fn seconds_field(
    ui: &mut egui::Ui,
    name: &str,
    ms: u64,
    range_ms: std::ops::RangeInclusive<u64>,
) -> Option<u64> {
    let to_seconds = |ms: u64| ms as f64 / 1000.0;
    let caption = ui.label(name);
    let mut seconds = to_seconds(ms);
    let field = egui::DragValue::new(&mut seconds)
        .range(to_seconds(*range_ms.start())..=to_seconds(*range_ms.end()))
        .speed(0.01)
        .max_decimals(3)
        .suffix(" s");
    let response = ui.add(field).labelled_by(caption.id);
    response
        .changed()
        .then(|| (seconds * 1000.0).round().max(0.0) as u64)
}

fn preview_button(
    ui: &mut egui::Ui,
    state: &AppState,
    editor: &SoundEditor,
    actions: &mut Vec<Action>,
) {
    let playing = state.preview(&editor.path).is_some();
    let (label, edit) = if playing {
        ("⏹ Stop", EditorAction::StopPreview)
    } else {
        ("▶ Preview", EditorAction::Preview)
    };
    let enabled = state.audio_available;
    let hint = "Plays only here, never into the call";
    if ui
        .add_enabled(enabled, egui::Button::new(label))
        .on_hover_text(hint)
        .clicked()
    {
        actions.push(Action::Editor(edit));
    }
}

fn volume_section(ui: &mut egui::Ui, editor: &SoundEditor, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let caption = ui.label("Volume:");
        let mut percent = editor.volume * 100.0;
        let slider = egui::Slider::new(&mut percent, 0.0..=200.0)
            .suffix(" %")
            .fixed_decimals(0);
        if ui.add(slider).labelled_by(caption.id).changed() {
            actions.push(Action::Editor(EditorAction::Volume(percent / 100.0)));
        }
    });
}

fn buttons(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            actions.push(Action::Editor(EditorAction::Save));
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::Editor(EditorAction::Close));
        }
    });
}
