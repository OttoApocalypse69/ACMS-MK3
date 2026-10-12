//! The Timeline panel: layers down, frames across, a cel where they meet.
//!
//! Click a cel to select it (Shift+click for a run of them on one layer), drag it to move it
//! (with Ctrl held, to copy it), drag a frame number to move the frame. Everything is a
//! command.

use acms_core::command::{CommandRegistry, Params};
use acms_core::compositor::MAX_ONION_FRAMES;
use acms_core::sprite::{LayerId, MAX_FRAME_MS};
use egui::{
    Align2, Button, Color32, CursorIcon, DragValue, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, TextStyle, Ui, Vec2,
};

use crate::commands;
use crate::keymap::Keymap;
use crate::panels::with_shortcut;
use crate::state::AppState;

/// Width of the column with the layer names.
const NAMES: f32 = 110.0;
/// Size of one cel.
const CELL: Vec2 = Vec2::new(22.0, 22.0);
/// Rows above the layers: the tags, then the frame numbers.
const HEADER_ROWS: usize = 2;

type Registry = CommandRegistry<AppState>;

/// One layer as the timeline shows it.
struct Row {
    id: LayerId,
    name: String,
    depth: usize,
    /// Whether the layer has cels at all: groups and reference layers do not.
    has_cels: bool,
    shown: bool,
}

/// What is under a point of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    /// A tag's bar, by the tag's position.
    Tag(usize),
    /// The number of a frame.
    Frame(usize),
    /// A layer's name.
    Layer(LayerId),
    Cel(LayerId, usize),
}

/// What the panel keeps between frames. None of it is part of the document.
#[derive(Default)]
pub struct TimelineUi {
    /// Where a drag began.
    dragged: Option<Spot>,
    /// The duration being typed or dragged, and the frame it is for.
    duration: Option<(usize, u32)>,
    /// The top-left corner of the grid, as of the last frame it was shown.
    #[cfg(test)]
    origin: Pos2,
}

fn run(registry: &Registry, state: &mut AppState, id: &str, params: Params) {
    commands::run(registry, state, id, &params);
}

pub fn show(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    keymap: &Keymap,
    me: &mut TimelineUi,
) {
    if state.editor.document().is_none() {
        *me = TimelineUi::default();
        ui.label(RichText::new("No sprite open").weak());
        return;
    }
    controls(ui, state, registry, keymap, me);
    ui.add_space(2.0);
    grid(ui, state, registry, me);
}

/// The row of buttons and settings above the grid.
fn controls(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    keymap: &Keymap,
    me: &mut TimelineUi,
) {
    let Some(document) = state.editor.document() else {
        return;
    };
    let frame = document.active_frame();
    let tags: Vec<String> = document
        .sprite()
        .tags()
        .iter()
        .map(|tag| tag.name.clone())
        .collect();
    let stored_ms = document.sprite().frames()[frame].duration_ms;
    let playing = state.playing.is_some();

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        let play = if playing { "Stop" } else { "Play" };
        let buttons = [
            ("frame.first", "|<"),
            ("frame.previous", "<"),
            ("playback.toggle", play),
            ("frame.next", ">"),
            ("frame.last", ">|"),
        ];
        let edits = [
            ("frame.new", "+ Frame"),
            ("frame.duplicate", "Duplicate"),
            ("frame.delete", "Delete"),
            ("cel.link", "Link"),
            ("cel.unlink", "Unlink"),
            ("tag.new", "Tag…"),
        ];
        let command_button = |ui: &mut Ui, state: &mut AppState, id: &str, label: &str| {
            let Some(command) = registry.get(id) else {
                return;
            };
            let tip = with_shortcut(ui, keymap, command.name().trim_end_matches('…'), id);
            let button = ui.add_enabled(command.is_enabled(state), Button::new(label).small());
            if button.on_hover_text(tip).clicked() {
                run(registry, state, id, Params::new());
            }
        };
        for (id, label) in buttons {
            command_button(ui, state, id, label);
        }

        // Which frames Play goes through.
        let shown = state
            .play_tag
            .and_then(|at| tags.get(at))
            .map_or("All frames", String::as_str);
        egui::ComboBox::from_id_salt("play_tag")
            .selected_text(shown)
            .width(96.0)
            .show_ui(ui, |ui| {
                let all = state.play_tag.is_none();
                if ui.selectable_label(all, "All frames").clicked() {
                    run(registry, state, "playback.set_tag", Params::new());
                }
                for (at, name) in tags.iter().enumerate() {
                    if ui
                        .selectable_label(state.play_tag == Some(at), name)
                        .clicked()
                    {
                        let params = Params::new().with("tag", at as i64);
                        run(registry, state, "playback.set_tag", params);
                    }
                }
            })
            .response
            .on_hover_text("What Play and the Preview panel loop");

        ui.separator();
        for (id, label) in edits {
            command_button(ui, state, id, label);
        }

        ui.separator();
        // The duration is committed when the drag or the typing ends, as one undo step.
        let mut ms = match me.duration {
            Some((for_frame, ms)) if for_frame == frame => ms,
            _ => stored_ms,
        };
        let field = ui
            .add(
                DragValue::new(&mut ms)
                    .range(1..=MAX_FRAME_MS)
                    .suffix(" ms"),
            )
            .on_hover_text("How long this frame is shown");
        if field.changed() {
            me.duration = Some((frame, ms));
        }
        let editing = field.dragged() || field.has_focus();
        if !editing
            && let Some((for_frame, ms)) = me.duration.take()
            && for_frame == frame
            && ms != stored_ms
        {
            let params = Params::new().with("ms", ms).with("frame", frame as i64);
            run(registry, state, "frame.set_duration", params);
        }

        ui.separator();
        let mut onion = state.onion_enabled;
        let tip = with_shortcut(
            ui,
            keymap,
            "Show the frames before and after faintly",
            "onion.toggle",
        );
        if ui
            .checkbox(&mut onion, "Onion skin")
            .on_hover_text(tip)
            .changed()
        {
            run(
                registry,
                state,
                "onion.toggle",
                Params::new().with("on", onion),
            );
        }
        if state.onion_enabled {
            let settings = state.onion;
            let count = |ui: &mut Ui, state: &mut AppState, value: u8, arg: &str, tip: &str| {
                let mut frames = u32::from(value);
                let field = DragValue::new(&mut frames).range(0..=u32::from(MAX_ONION_FRAMES));
                if ui.add(field).on_hover_text(tip).changed() {
                    run(
                        registry,
                        state,
                        "onion.set",
                        Params::new().with(arg, frames),
                    );
                }
            };
            count(
                ui,
                state,
                settings.before,
                "before",
                "Frames shown before this one",
            );
            count(
                ui,
                state,
                settings.after,
                "after",
                "Frames shown after this one",
            );
            let mut percent = (f32::from(settings.opacity) / 255.0 * 100.0).round() as u32;
            let field = DragValue::new(&mut percent).range(5..=100).suffix("%");
            if ui.add(field).on_hover_text("Onion skin opacity").changed() {
                let opacity = (percent as f32 / 100.0 * 255.0).round() as u32;
                run(
                    registry,
                    state,
                    "onion.set",
                    Params::new().with("opacity", opacity),
                );
            }
            let mut tint = settings.tint;
            if ui
                .checkbox(&mut tint, "Tint")
                .on_hover_text("Earlier frames red, later frames blue")
                .changed()
            {
                run(
                    registry,
                    state,
                    "onion.set",
                    Params::new().with("tint", tint),
                );
            }
        }
    });
}

/// The top-left corner of the cel at a row and frame, both counted from 0.
fn cell_rect(origin: Pos2, row: usize, frame: usize) -> Rect {
    let min = origin + Vec2::new(NAMES + frame as f32 * CELL.x, row as f32 * CELL.y);
    Rect::from_min_size(min, CELL)
}

fn grid(ui: &mut Ui, state: &mut AppState, registry: &Registry, me: &mut TimelineUi) {
    let Some(document) = state.editor.document() else {
        return;
    };
    let sprite = document.sprite();
    let (active_layer, active_frame) = (document.active_layer(), document.active_frame());
    let frames = sprite.frames().len();
    let rows: Vec<Row> = sprite
        .walk_top_down()
        .iter()
        .map(|entry| Row {
            id: entry.layer.id,
            name: entry.layer.name.clone(),
            depth: entry.depth,
            has_cels: entry.layer.as_image().is_some(),
            shown: entry.shown,
        })
        .collect();
    // What each cel holds: nothing, pixels, or pixels shared with other frames.
    let cel = |layer: LayerId, frame: usize| {
        let linked = !sprite.linked_frames(layer, frame).is_empty();
        (sprite.cel(layer, frame).is_some(), linked)
    };
    let cels: Vec<Vec<(bool, bool)>> = rows
        .iter()
        .map(|row| (0..frames).map(|frame| cel(row.id, frame)).collect())
        .collect();
    let tags: Vec<(String, usize, usize, Color32)> = sprite
        .tags()
        .iter()
        .map(|tag| {
            let c = tag.color;
            let color = Color32::from_rgba_unmultiplied(c.r, c.g, c.b, 255);
            (tag.name.clone(), tag.from, tag.to, color)
        })
        .collect();
    let range = state.cel_range;

    let size = Vec2::new(
        NAMES + frames as f32 * CELL.x,
        (rows.len() + HEADER_ROWS) as f32 * CELL.y,
    );
    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let origin = area.min;
    #[cfg(test)]
    {
        me.origin = origin;
    }

    // What is under a point.
    let spot_at = |pos: Pos2| -> Option<Spot> {
        if !area.contains(pos) {
            return None;
        }
        let row = ((pos.y - origin.y) / CELL.y) as usize;
        let in_names = pos.x < origin.x + NAMES;
        let frame = (((pos.x - origin.x - NAMES) / CELL.x).max(0.0) as usize).min(frames - 1);
        match row {
            0 if in_names => None,
            // The tag made last is drawn on top, so it is the one a click hits.
            0 => tags
                .iter()
                .rposition(|(_, from, to, _)| (*from..=*to).contains(&frame))
                .map(Spot::Tag),
            1 if in_names => None,
            1 => Some(Spot::Frame(frame)),
            row => {
                let row = rows.get(row - HEADER_ROWS)?;
                Some(if in_names || !row.has_cels {
                    Spot::Layer(row.id)
                } else {
                    Spot::Cel(row.id, frame)
                })
            }
        }
    };

    // Painting.
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(area);
    let font = TextStyle::Small.resolve(ui.style());
    let body = TextStyle::Body.resolve(ui.style());
    let line = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let accent = visuals.selection.stroke.color;
    let (text, weak) = (visuals.text_color(), visuals.weak_text_color());

    for (name, from, to, color) in &tags {
        let bar = cell_rect(origin, 0, *from)
            .union(cell_rect(origin, 0, *to))
            .shrink2(Vec2::new(1.0, 3.0));
        painter.rect_filled(bar, 3.0, *color);
        let label = painter.layout_no_wrap(name.clone(), font.clone(), Color32::WHITE);
        let clipped = painter.with_clip_rect(bar.intersect(area));
        clipped.galley(
            Pos2::new(bar.left() + 4.0, bar.center().y - label.size().y / 2.0),
            label,
            Color32::WHITE,
        );
    }
    for frame in 0..frames {
        let cell = cell_rect(origin, 1, frame);
        if frame == active_frame {
            painter.rect_filled(cell.shrink(1.0), 3.0, visuals.selection.bg_fill);
        }
        painter.text(
            cell.center(),
            Align2::CENTER_CENTER,
            (frame + 1).to_string(),
            font.clone(),
            if frame == active_frame { text } else { weak },
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let y = index + HEADER_ROWS;
        let names = Rect::from_min_size(
            origin + Vec2::new(0.0, y as f32 * CELL.y),
            Vec2::new(NAMES, CELL.y),
        );
        if row.id == active_layer {
            painter.rect_filled(names.shrink(1.0), 3.0, visuals.selection.bg_fill);
        }
        let clipped = painter.with_clip_rect(names.intersect(area));
        clipped.text(
            names.left_center() + Vec2::new(6.0 + row.depth as f32 * 10.0, 0.0),
            Align2::LEFT_CENTER,
            &row.name,
            body.clone(),
            if row.shown { text } else { weak },
        );
        if !row.has_cels {
            continue;
        }
        for frame in 0..frames {
            let cell = cell_rect(origin, y, frame);
            let selected = range.is_some_and(|r| r.layer == row.id && r.contains(frame));
            if selected {
                painter.rect_filled(cell, 0.0, visuals.selection.bg_fill);
            }
            painter.rect_stroke(cell, 0.0, line, StrokeKind::Inside);
            let (filled, linked) = cels[index][frame];
            let color = if row.shown { text } else { weak };
            // Linked cels are joined by a line through them.
            if linked {
                let next_linked = cels[index].get(frame + 1).is_some_and(|next| next.1);
                let previous_linked = frame > 0 && cels[index][frame - 1].1;
                let left = if previous_linked {
                    cell.left()
                } else {
                    cell.center().x
                };
                let right = if next_linked {
                    cell.right()
                } else {
                    cell.center().x
                };
                painter.hline(left..=right, cell.center().y, Stroke::new(2.0, color));
            }
            if filled {
                painter.circle_filled(cell.center(), 4.0, color);
            } else {
                painter.circle_stroke(cell.center(), 3.0, Stroke::new(1.0, weak));
            }
            if row.id == active_layer && frame == active_frame {
                painter.rect_stroke(
                    cell.shrink(1.0),
                    2.0,
                    Stroke::new(2.0, accent),
                    StrokeKind::Inside,
                );
            }
        }
    }

    // Input.
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let (shift, ctrl) = ui.input(|input| (input.modifiers.shift, input.modifiers.command));
    if response.drag_started() {
        let pressed = ui.input(|input| input.pointer.press_origin());
        me.dragged = pressed.and_then(spot_at);
    }
    let target = pointer.and_then(spot_at);

    if response.clicked() {
        match target {
            Some(Spot::Cel(layer, frame)) => {
                let params = Params::new()
                    .with("layer", layer.0)
                    .with("frame", frame as i64)
                    .with("extend", shift);
                run(registry, state, "timeline.select", params);
            }
            Some(Spot::Frame(frame)) => {
                // Shift extends the selection along the layer it is on.
                let params = Params::new()
                    .with("frame", frame as i64)
                    .with("extend", shift);
                run(registry, state, "timeline.select", params);
            }
            Some(Spot::Layer(layer)) => {
                run(
                    registry,
                    state,
                    "layer.select",
                    Params::new().with("layer", layer.0),
                );
            }
            Some(Spot::Tag(tag)) => {
                run(
                    registry,
                    state,
                    "tag.edit",
                    Params::new().with("tag", tag as i64),
                );
            }
            None => {}
        }
    }

    // A drag in progress shows where it would land.
    if response.dragged() {
        let landing = match (me.dragged, target) {
            (Some(Spot::Cel(..)), Some(Spot::Cel(layer, frame))) => rows
                .iter()
                .position(|row| row.id == layer)
                .map(|row| cell_rect(origin, row + HEADER_ROWS, frame)),
            (Some(Spot::Frame(_)), Some(Spot::Frame(frame) | Spot::Cel(_, frame))) => {
                Some(cell_rect(origin, 1, frame))
            }
            _ => None,
        };
        if let Some(cell) = landing {
            painter.rect_stroke(cell, 2.0, Stroke::new(2.0, accent), StrokeKind::Inside);
            ui.ctx().set_cursor_icon(if ctrl {
                CursorIcon::Copy
            } else {
                CursorIcon::Grabbing
            });
        }
    }
    if response.drag_stopped() {
        match (me.dragged.take(), target) {
            (Some(Spot::Cel(layer, frame)), Some(Spot::Cel(to_layer, to_frame)))
                if (layer, frame) != (to_layer, to_frame) =>
            {
                let params = Params::new()
                    .with("layer", layer.0)
                    .with("frame", frame as i64)
                    .with("to_layer", to_layer.0)
                    .with("to_frame", to_frame as i64)
                    .with("copy", ctrl);
                run(registry, state, "cel.move", params);
            }
            (Some(Spot::Frame(from)), Some(Spot::Frame(to) | Spot::Cel(_, to))) if from != to => {
                let params = Params::new()
                    .with("from", from as i64)
                    .with("to", to as i64);
                run(registry, state, "frame.move", params);
            }
            _ => {}
        }
    }
    if !response.dragged() && !response.drag_stopped() {
        me.dragged = None;
    }
}

#[cfg(test)]
impl TimelineUi {
    /// The middle of the cel at a layer row (0 is the top layer) and frame, as of the last
    /// frame the panel was shown.
    pub fn cel_pos(&self, row: usize, frame: usize) -> Pos2 {
        cell_rect(self.origin, row + HEADER_ROWS, frame).center()
    }

    /// The middle of a frame's number.
    pub fn frame_pos(&self, frame: usize) -> Pos2 {
        cell_rect(self.origin, 1, frame).center()
    }
}
