//! The grid of sound tiles: all the same size, in as many columns as fit. Tiles are painted by
//! hand (name centered and as large as fits, a small footer with the length and triggers) and can
//! be dragged onto each other to reorder the tab.

use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{
    self, Align, Color32, CursorIcon, FontId, Galley, Id, LayerId, Order, Pos2, Rect, Sense,
    Stroke, StrokeKind, Vec2, WidgetInfo, accesskit::Role, color_picker, text::LayoutJob,
};

use super::tile_color;
use crate::app::{Action, AppState, ClipId};
use crate::color::TileColor;

const TILE: Vec2 = Vec2::new(180.0, 90.0);
const SPACING: f32 = 8.0;
const ROUNDING: f32 = 6.0;
const PADDING: f32 = 8.0;
/// The name's font sizes, largest first; the first one the name fits in is used.
const NAME_SIZES: [f32; 4] = [20.0, 17.0, 14.0, 12.0];
const MAX_NAME_ROWS: usize = 3;
const FOOTER_SIZE: f32 = 11.0;
const FOOTER_HEIGHT: f32 = 14.0;
/// The progress bar's height and its distance from the tile's edges.
const BAR_HEIGHT: f32 = 3.0;
const BAR_INSET: f32 = 3.0;

pub fn show(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let clips = state.visible_clips();
    if clips.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(empty_message(state)));
        return;
    }
    let now = Instant::now();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(SPACING);
            for row in clips.chunks(columns(ui.available_width())) {
                ui.horizontal(|ui| {
                    for clip in row {
                        tile(ui, state, clip, now, actions);
                    }
                });
            }
        });
}

/// How many tiles fit next to each other; at least one.
fn columns(width: f32) -> usize {
    ((width + SPACING) / (TILE.x + SPACING)).floor().max(1.0) as usize
}

fn empty_message(state: &AppState) -> String {
    if state.is_searching() {
        return "No sounds match the search.".to_owned();
    }
    match state.library.folders().get(state.tab) {
        None => {
            "Add a folder with “➕ Folder”; its audio files become the sounds of a tab.".to_owned()
        }
        Some(folder) => folder.error.clone().unwrap_or_else(|| {
            "No sounds. Drop audio files into this folder or use “Add from URL”.".to_owned()
        }),
    }
}

fn tile(
    ui: &mut egui::Ui,
    state: &AppState,
    clip: &ClipId,
    now: Instant,
    actions: &mut Vec<Action>,
) {
    // Search results span tabs, so they can't be reordered.
    let reorderable = !state.is_searching();
    let sense = if reorderable {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    ui.add_enabled_ui(state.audio_available, |ui| {
        let (rect, response) = ui.allocate_exact_size(TILE, sense);
        let name = state.display_name(clip);
        let label = format!("{name}\n{}", state.subtitle(clip));
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &label));

        let progress = state.progress(clip, now);
        let content = TileContent {
            name: &name,
            length: state.length_label(clip),
            triggers: state.trigger_label(clip),
            progress,
        };
        let look = Look::new(ui, state.tile_color(clip), &response, progress.is_some());
        if ui.is_rect_visible(rect) {
            paint(ui, ui.painter(), rect, &content, &look);
        }
        if reorderable {
            drag_and_drop(ui, clip, &response, &content, &look, actions);
        }
        if response.clicked() {
            actions.push(Action::Play(clip.clone()));
        }
        response.context_menu(|ui| context_menu(ui, state, clip, actions));
        response.on_hover_text(format!("{name}\n{}", clip.path.display()));
    });
}

/// What a tile shows.
struct TileContent<'a> {
    name: &'a str,
    length: String,
    triggers: String,
    progress: Option<f32>,
}

/// The colors of a tile in its current state.
#[derive(Clone, Copy)]
struct Look {
    fill: Color32,
    text: Color32,
    footer: Color32,
    stroke: Stroke,
    bar: Color32,
}

impl Look {
    /// The tile's color (or the theme's button look), hovered, pressed or playing.
    fn new(
        ui: &egui::Ui,
        color: Option<TileColor>,
        response: &egui::Response,
        playing: bool,
    ) -> Self {
        let visuals = ui.visuals();
        let widget = visuals.widgets.style(response);
        let mut look = match color {
            Some(color) => {
                let fill = tile_color::fill(color, visuals);
                let text = tile_color::text_on(fill);
                let fill = if response.hovered() {
                    fill.lerp_to_gamma(text, 0.1)
                } else {
                    fill
                };
                Self {
                    fill,
                    text,
                    footer: text.gamma_multiply(0.7),
                    stroke: widget.bg_stroke,
                    bar: text,
                }
            }
            None => Self {
                fill: widget.weak_bg_fill,
                text: widget.text_color(),
                footer: visuals.weak_text_color(),
                stroke: widget.bg_stroke,
                bar: visuals.selection.stroke.color,
            },
        };
        if playing {
            look.stroke = Stroke::new(2.0, visuals.selection.stroke.color);
            if color.is_none() {
                look.fill = visuals.selection.bg_fill;
                look.text = visuals.selection.stroke.color;
                look.footer = look.text.gamma_multiply(0.75);
            }
        }
        if !ui.is_enabled() {
            look = look.faded(0.5);
        }
        look
    }

    fn faded(self, factor: f32) -> Self {
        Self {
            fill: self.fill.gamma_multiply(factor),
            text: self.text.gamma_multiply(factor),
            footer: self.footer.gamma_multiply(factor),
            stroke: Stroke::new(self.stroke.width, self.stroke.color.gamma_multiply(factor)),
            bar: self.bar.gamma_multiply(factor),
        }
    }
}

fn paint(ui: &egui::Ui, painter: &egui::Painter, rect: Rect, content: &TileContent, look: &Look) {
    painter.rect(rect, ROUNDING, look.fill, look.stroke, StrokeKind::Inside);
    let inner = rect.shrink2(Vec2::new(PADDING, 6.0));
    let footer_top = inner.bottom() - BAR_HEIGHT - FOOTER_HEIGHT;
    let name_area = Rect::from_min_max(inner.min, Pos2::new(inner.right(), footer_top - 2.0));
    let name = name_galley(ui, content.name, name_area, look.text);
    painter.galley(centered(name_area, &name), name, look.text);
    let footer = Rect::from_x_y_ranges(inner.x_range(), footer_top..=footer_top + FOOTER_HEIGHT);
    paint_footer(ui, painter, footer, content, look.footer);
    if let Some(progress) = content.progress {
        progress_bar(painter, rect, progress, look.bar);
    }
}

/// The name in the largest size it fits in without breaking words, up to [`MAX_NAME_ROWS`] rows;
/// cut off with "…" at the smallest size.
fn name_galley(ui: &egui::Ui, name: &str, area: Rect, color: Color32) -> Arc<Galley> {
    let layout = |size: f32| {
        let font = FontId::proportional(size);
        let mut job = LayoutJob::simple(name.to_owned(), font, color, area.width());
        job.wrap.max_rows = MAX_NAME_ROWS;
        job.wrap.overflow_character = Some('…');
        job.halign = Align::Center;
        ui.painter().layout_job(job)
    };
    let fits = |size: f32| {
        let galley = layout(size);
        let words_fit = (name.split_whitespace()).all(|word| {
            ui.painter()
                .layout_no_wrap(word.to_owned(), FontId::proportional(size), color)
                .size()
                .x
                <= area.width()
        });
        !galley.elided && galley.size().y <= area.height() && words_fit
    };
    let smallest = NAME_SIZES[NAME_SIZES.len() - 1];
    let size = NAME_SIZES
        .into_iter()
        .find(|&size| fits(size))
        .unwrap_or(smallest);
    layout(size)
}

/// Where to paint `galley` so it's centered in `area`.
fn centered(area: Rect, galley: &Galley) -> Pos2 {
    area.center() - galley.rect.center().to_vec2()
}

/// The length on the left, the triggers on the right, cut off with "…" so they never overlap.
fn paint_footer(
    ui: &egui::Ui,
    painter: &egui::Painter,
    area: Rect,
    content: &TileContent,
    color: Color32,
) {
    let font = FontId::proportional(FOOTER_SIZE);
    let length = ui
        .painter()
        .layout_no_wrap(content.length.clone(), font.clone(), color);
    let length_width = length.size().x;
    painter.galley(Pos2::new(area.left(), area.top()), length, color);
    if content.triggers.is_empty() {
        return;
    }
    let mut job = LayoutJob::simple(
        content.triggers.clone(),
        font,
        color,
        (area.width() - length_width - 8.0).max(0.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let triggers = ui.painter().layout_job(job);
    let pos = Pos2::new(area.right() - triggers.size().x, area.top());
    painter.galley(pos, triggers, color);
}

fn progress_bar(painter: &egui::Painter, tile: Rect, progress: f32, color: Color32) {
    let width = (tile.width() - 2.0 * BAR_INSET) * progress.clamp(0.0, 1.0);
    let bar = Rect::from_min_size(
        Pos2::new(
            tile.left() + BAR_INSET,
            tile.bottom() - BAR_INSET - BAR_HEIGHT,
        ),
        Vec2::new(width, BAR_HEIGHT),
    );
    painter.rect_filled(bar, BAR_HEIGHT / 2.0, color);
}

/// Dragging a tile carries its clip; dropping it onto another tile moves it before or after that
/// one, depending on which half of it the pointer is over.
fn drag_and_drop(
    ui: &egui::Ui,
    clip: &ClipId,
    response: &egui::Response,
    content: &TileContent,
    look: &Look,
    actions: &mut Vec<Action>,
) {
    response.dnd_set_drag_payload(clip.clone());
    if response.dragged() && egui::DragAndDrop::has_payload_of_type::<ClipId>(ui.ctx()) {
        drag_preview(ui, response.rect, content, look);
    }
    let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
    let after = pointer.is_some_and(|pointer| pointer.x > response.rect.center().x);
    if let Some(dragged) = response.dnd_hover_payload::<ClipId>()
        && *dragged != *clip
    {
        insertion_marker(ui, response.rect, after);
    }
    if let Some(dragged) = response.dnd_release_payload::<ClipId>()
        && *dragged != *clip
    {
        actions.push(Action::MoveClip {
            dragged: (*dragged).clone(),
            target: clip.clone(),
            after,
        });
    }
}

/// Dims the dragged tile in place and paints a see-through copy of it that follows the pointer.
fn drag_preview(ui: &egui::Ui, rect: Rect, content: &TileContent, look: &Look) {
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    ui.painter()
        .rect_filled(rect, ROUNDING, ui.visuals().panel_fill.gamma_multiply(0.6));
    let (pointer, grabbed) = ui.ctx().input(|input| {
        let pointer = &input.pointer;
        (pointer.latest_pos(), pointer.press_origin())
    });
    let (Some(pointer), Some(grabbed)) = (pointer, grabbed) else {
        return;
    };
    let painter = ui.ctx().layer_painter(overlay(Order::Foreground));
    let preview = look.faded(0.8);
    let moved = rect.translate(pointer - grabbed);
    paint(ui, &painter, moved, content, &preview);
}

/// A bar in the gap before or after `tile`, where the dragged tile would go; above the dragged
/// copy.
fn insertion_marker(ui: &egui::Ui, tile: Rect, after: bool) {
    let x = if after {
        tile.right() + SPACING / 2.0
    } else {
        tile.left() - SPACING / 2.0
    };
    let stroke = Stroke::new(3.0, ui.visuals().selection.stroke.color);
    let painter = ui.ctx().layer_painter(overlay(Order::Tooltip));
    painter.vline(x, tile.y_range().expand(2.0), stroke);
}

/// A layer above the board for painting while dragging.
fn overlay(order: Order) -> LayerId {
    LayerId::new(order, Id::unique(("board drag", order)))
}

/// A tile's right-click menu.
fn context_menu(ui: &mut egui::Ui, state: &AppState, clip: &ClipId, actions: &mut Vec<Action>) {
    let mut item = |label: &str, action: fn(ClipId) -> Action| {
        if ui.button(label).clicked() {
            actions.push(action(clip.clone()));
            ui.close();
        }
    };
    item("✏ Edit…", Action::EditSound);
    item("Rename…", Action::RenameSound);
    item("Duplicate", Action::DuplicateClip);
    // The file's own clip goes away with the file.
    if clip.clip != 0 {
        item("Delete clip", Action::DeleteClip);
    }
    ui.separator();
    color_menu(ui, state, clip, actions);
    if !state.is_searching() {
        ui.separator();
        let sorted = (state.config.tabs.get(state.tab)).is_none_or(|tab| tab.order.is_empty());
        if ui
            .add_enabled(!sorted, egui::Button::new("Sort by name"))
            .clicked()
        {
            actions.push(Action::SortByName);
            ui.close();
        }
    }
}

/// The swatches and a custom color picker; applied right away.
fn color_menu(ui: &mut egui::Ui, state: &AppState, clip: &ClipId, actions: &mut Vec<Action>) {
    ui.label("Color");
    let current = state.tile_color(clip);
    if let Some(color) = tile_color::swatches(ui, current) {
        actions.push(Action::SetColor(clip.clone(), color));
        ui.close();
    }
    ui.menu_button("Custom color…", |ui| {
        let [r, g, b] = tile_color::custom_start(current, ui.visuals());
        let mut color = Color32::from_rgb(r, g, b);
        if color_picker::color_picker_color32(ui, &mut color, color_picker::Alpha::Opaque) {
            let custom = TileColor::Custom([color.r(), color.g(), color.b()]);
            actions.push(Action::SetColor(clip.clone(), Some(custom)));
        }
    });
}
