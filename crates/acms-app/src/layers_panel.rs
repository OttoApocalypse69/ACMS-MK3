//! The Layers panel: the layer tree from top to bottom, with the selected layer's blend mode
//! and opacity above it. Everything it does is a `layer.*` command.

use acms_core::command::{CommandRegistry, Params};
use acms_core::sprite::{BlendMode, ImageLayer, LayerId, Slot};
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Button, Color32, ComboBox, CursorIcon, Key, Pos2, Rect, RichText, Sense, Shape, Slider, Stroke,
    StrokeKind, TextEdit, TextStyle, Ui, Vec2,
};

use crate::commands;
use crate::keymap::Keymap;
use crate::panels::with_shortcut;
use crate::state::AppState;

const ROW_HEIGHT: f32 = 26.0;
/// How far the contents of a group are shifted to the right.
const INDENT: f32 = 14.0;
const FOLD_WIDTH: f32 = 16.0;
const ICON_WIDTH: f32 = 22.0;

type Registry = CommandRegistry<AppState>;

/// One layer as the panel shows it, copied out of the sprite so commands can run while the
/// rows are laid out.
struct Item {
    id: LayerId,
    name: String,
    depth: usize,
    slot: Slot,
    /// Drawn on the canvas: visible, and so is every group around it.
    shown: bool,
    visible: bool,
    /// For a group: whether it is folded away, and how many layers it holds.
    group: Option<(bool, usize)>,
    /// For an image layer: whether it is locked, and whether its alpha is.
    locks: Option<(bool, bool)>,
    reference: bool,
}

/// Where a row ended up on screen.
#[derive(Clone, Copy, Debug)]
struct Row {
    id: LayerId,
    rect: Rect,
    slot: Slot,
    group: Option<(bool, usize)>,
    /// Where to click in tests.
    #[cfg(test)]
    eye: Rect,
    #[cfg(test)]
    name: Rect,
}

struct Rename {
    id: LayerId,
    text: String,
    /// The text field has been given the keyboard focus.
    focused: bool,
}

/// What the panel keeps between frames. None of it is part of the document.
#[derive(Default)]
pub struct LayersUi {
    rename: Option<Rename>,
    /// The layer whose row is being dragged to a new place.
    dragged: Option<LayerId>,
    /// Counts opacity adjustments, so each drag of the slider is one undo step.
    opacity_drag: u64,
    rows: Vec<Row>,
}

/// Where a dragged row would land.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DropTarget {
    /// The position in the tree as it is now, the dragged layer still in its old place.
    insert: Slot,
    /// Draw a line at this height, or…
    line: Option<f32>,
    /// …highlight this group, which the layer would go into.
    into: Option<LayerId>,
}

/// Works out where a row dropped at height `y` goes. Rows are in display order, top first.
fn drop_target(rows: &[Row], y: f32) -> Option<DropTarget> {
    let (first, last) = (rows.first()?, rows.last()?);
    let line = |insert: Slot, y: f32| DropTarget {
        insert,
        line: Some(y),
        into: None,
    };
    let above = |row: &Row| {
        line(
            Slot::new(row.slot.parent, row.slot.index + 1),
            row.rect.top(),
        )
    };
    let below = |row: &Row| line(row.slot, row.rect.bottom());

    if y < first.rect.top() {
        return Some(above(first));
    }
    if y >= last.rect.bottom() {
        // Past the end of the list: the very bottom of the sprite.
        return Some(line(Slot::new(None, 0), last.rect.bottom()));
    }
    let row = rows.iter().find(|row| row.rect.y_range().contains(y))?;
    let part = (y - row.rect.top()) / row.rect.height();

    if let Some((collapsed, count)) = row.group {
        // The middle of a group's row means "into the group"; so does its lower edge when the
        // group is open, because the next row down is already inside it.
        let open = !collapsed && count > 0;
        if (0.25..=0.75).contains(&part) || (open && part > 0.75) {
            return Some(DropTarget {
                insert: Slot::new(Some(row.id), count),
                line: None,
                into: Some(row.id),
            });
        }
    }
    Some(if part < 0.5 { above(row) } else { below(row) })
}

/// Turns an insert position in the current tree into where the layer ends up once it has
/// left its old place, which is what `layer.move` wants.
fn destination(from: Slot, insert: Slot) -> Slot {
    if from.parent == insert.parent && from.index < insert.index {
        Slot::new(insert.parent, insert.index - 1)
    } else {
        insert
    }
}

fn run(registry: &Registry, state: &mut AppState, id: &str, params: Params) {
    commands::run(registry, state, id, &params);
}

fn on(layer: LayerId) -> Params {
    Params::new().with("layer", layer.0)
}

pub fn show(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    keymap: &Keymap,
    me: &mut LayersUi,
) {
    let Some(document) = state.editor.document() else {
        *me = LayersUi::default();
        ui.label(RichText::new("No sprite open").weak());
        return;
    };
    let sprite = document.sprite();
    let active = document.active_layer();
    let active_layer = sprite.layer(active);
    let active_image = active_layer.and_then(|layer| layer.as_image().copied());
    // A reference layer has an opacity too, but no blend mode.
    let reference_opacity = active_layer
        .and_then(|layer| layer.as_reference())
        .map(|reference| reference.opacity);

    let mut items = Vec::new();
    let mut folded_at: Option<usize> = None;
    for entry in sprite.walk_top_down() {
        // Skip what is inside a folded group.
        if folded_at.is_some_and(|depth| entry.depth > depth) {
            continue;
        }
        let group = entry
            .layer
            .as_group()
            .map(|group| (group.collapsed, group.children.len()));
        folded_at = group
            .is_some_and(|(collapsed, _)| collapsed)
            .then_some(entry.depth);
        items.push(Item {
            id: entry.layer.id,
            name: entry.layer.name.clone(),
            depth: entry.depth,
            slot: entry.slot,
            shown: entry.shown,
            visible: entry.layer.visible,
            group,
            locks: entry
                .layer
                .as_image()
                .map(|image| (image.locked, image.alpha_lock)),
            reference: entry.layer.is_reference(),
        });
    }

    if let Some(id) = state.rename_layer_request.take()
        && let Some(item) = items.iter().find(|item| item.id == id)
    {
        me.rename = Some(Rename {
            id,
            text: item.name.clone(),
            focused: false,
        });
    }

    toolbar(ui, state, registry, keymap);
    // Shown greyed out for a group rather than hidden, so the rows below never jump.
    let shown = active_image.unwrap_or(ImageLayer {
        opacity: reference_opacity.unwrap_or(255),
        ..ImageLayer::default()
    });
    ui.add_enabled_ui(
        active_image.is_some() || reference_opacity.is_some(),
        |ui| {
            properties(ui, state, registry, me, shown, active_image.is_some());
        },
    );
    ui.separator();
    rows(ui, state, registry, me, &items, active);
}

fn toolbar(ui: &mut Ui, state: &mut AppState, registry: &Registry, keymap: &Keymap) {
    let buttons = [
        ("layer.new", "New"),
        ("layer.new_group", "Group"),
        ("layer.duplicate", "Copy"),
        ("layer.delete", "Delete"),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for (id, label) in buttons {
            let Some(command) = registry.get(id) else {
                continue;
            };
            let tip = with_shortcut(ui, keymap, command.name().trim_end_matches('…'), id);
            let button = ui.add_enabled(command.is_enabled(state), Button::new(label));
            if button.on_hover_text(tip).clicked() {
                run(registry, state, id, Params::new());
            }
        }
    });
}

/// Blend mode and opacity of the selected layer. `blends` is false for a layer that has an
/// opacity but no blend mode.
fn properties(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    me: &mut LayersUi,
    image: ImageLayer,
    blends: bool,
) {
    ui.horizontal(|ui| {
        ui.add_enabled_ui(blends, |ui| blend_mode(ui, state, registry, image));

        let mut percent = (f32::from(image.opacity) / 255.0 * 100.0).round() as u32;
        ui.spacing_mut().slider_width = (ui.available_width() - 62.0).max(40.0);
        let slider = ui
            .add(Slider::new(&mut percent, 0..=100).suffix("%"))
            .on_hover_text("Layer opacity");
        if slider.drag_started() {
            me.opacity_drag += 1;
        }
        if slider.changed() {
            if !slider.dragged() {
                // A typed value or a key press is a step of its own.
                me.opacity_drag += 1;
            }
            let opacity = (percent as f32 / 100.0 * 255.0).round() as u32;
            let params = Params::new()
                .with("opacity", opacity)
                .with("merge", me.opacity_drag as i64);
            run(registry, state, "layer.set_opacity", params);
        }
    });
}

fn blend_mode(ui: &mut Ui, state: &mut AppState, registry: &Registry, image: ImageLayer) {
    ComboBox::from_id_salt("layer_blend")
        .selected_text(image.blend.name())
        .width(92.0)
        .show_ui(ui, |ui| {
            for mode in BlendMode::ALL {
                if ui
                    .selectable_label(mode == image.blend, mode.name())
                    .clicked()
                {
                    let params = Params::new().with("blend", mode.id());
                    run(registry, state, "layer.set_blend", params);
                }
            }
        })
        .response
        .on_hover_text("Blend mode");
}

fn rows(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    me: &mut LayersUi,
    items: &[Item],
    active: LayerId,
) {
    let visuals = ui.visuals().clone();
    let text_color = visuals.text_color();
    let weak_color = visuals.weak_text_color();
    let font = TextStyle::Body.resolve(ui.style());

    // Rows sit directly under one another.
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut rows = Vec::with_capacity(items.len());
    // Commands wait until every row is laid out, so the rows all describe the same tree.
    let mut pending: Vec<(&str, Params)> = Vec::new();
    let mut rename_done: Option<bool> = None;

    for item in items {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::hover());
        let mut x = rect.left() + item.depth as f32 * INDENT;
        let mut cell = |width: f32| {
            let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(width, ROW_HEIGHT));
            x += width;
            cell
        };
        let fold_rect = cell(FOLD_WIDTH);
        let (eye_rect, lock_rect, alpha_rect) =
            (cell(ICON_WIDTH), cell(ICON_WIDTH), cell(ICON_WIDTH));
        let name_rect = Rect::from_min_max(Pos2::new(x + 2.0, rect.top()), rect.right_bottom());
        let id = ui.id().with(("layer", item.id.0));
        let painter = ui.painter().clone();

        if item.id == active {
            painter.rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if ui.rect_contains_pointer(rect) && me.dragged.is_none() {
            painter.rect_filled(rect, 4.0, visuals.widgets.inactive.weak_bg_fill);
        }
        let color = if item.shown { text_color } else { weak_color };

        if let Some((collapsed, _)) = item.group {
            let fold = ui.interact(fold_rect, id.with("fold"), Sense::click());
            paint_fold(&painter, fold_rect.center(), collapsed, color);
            if fold.clicked() {
                let params = on(item.id).with("collapsed", !collapsed);
                pending.push(("layer.set_collapsed", params));
            }
        }

        let eye = ui
            .interact(eye_rect, id.with("eye"), Sense::click())
            .on_hover_text(if item.visible { "Hide" } else { "Show" });
        paint_eye(&painter, eye_rect.center(), item.visible, color, weak_color);
        if eye.clicked() {
            pending.push(("layer.set_visible", on(item.id)));
        }

        if let Some((locked, alpha_locked)) = item.locks {
            let lock = ui
                .interact(lock_rect, id.with("lock"), Sense::click())
                .on_hover_text(if locked { "Unlock" } else { "Lock" });
            paint_lock(&painter, lock_rect.center(), locked, color, weak_color);
            if lock.clicked() {
                pending.push(("layer.set_locked", on(item.id)));
            }

            let alpha = ui
                .interact(alpha_rect, id.with("alpha"), Sense::click())
                .on_hover_text(
                    "Alpha lock: drawing recolours the pixels that are there and leaves \
                     transparency alone",
                );
            paint_alpha_lock(
                &painter,
                alpha_rect.center(),
                alpha_locked,
                color,
                weak_color,
            );
            if alpha.clicked() {
                pending.push(("layer.set_alpha_lock", on(item.id)));
            }
        } else if item.reference {
            ui.interact(lock_rect, id.with("reference"), Sense::hover())
                .on_hover_text("Reference layer: shown, never drawn on or exported");
            paint_reference(&painter, lock_rect.center(), color);
        }

        match me.rename.as_mut().filter(|rename| rename.id == item.id) {
            Some(rename) => {
                let field = TextEdit::singleline(&mut rename.text).font(TextStyle::Body);
                let response = ui.put(name_rect.shrink2(Vec2::new(0.0, 2.0)), field);
                if !rename.focused {
                    response.request_focus();
                    rename.focused = true;
                    // Start with the whole name selected, so typing replaces it.
                    let mut edit = TextEditState::load(ui.ctx(), response.id).unwrap_or_default();
                    let end = CCursor::new(rename.text.chars().count());
                    let all = CCursorRange::two(CCursor::new(0), end);
                    edit.cursor.set_char_range(Some(all));
                    edit.store(ui.ctx(), response.id);
                } else if response.lost_focus() {
                    let cancelled = ui.input(|input| input.key_pressed(Key::Escape));
                    rename_done = Some(!cancelled);
                }
            }
            None => {
                let name = ui.interact(name_rect, id.with("name"), Sense::click_and_drag());
                let at = Pos2::new(name_rect.left() + 2.0, name_rect.center().y);
                painter.text(
                    at,
                    egui::Align2::LEFT_CENTER,
                    &item.name,
                    font.clone(),
                    color,
                );
                if name.double_clicked() {
                    me.rename = Some(Rename {
                        id: item.id,
                        text: item.name.clone(),
                        focused: false,
                    });
                } else if name.clicked() || name.drag_started() {
                    pending.push(("layer.select", on(item.id)));
                }
                if name.drag_started() {
                    me.dragged = Some(item.id);
                }
            }
        }

        rows.push(Row {
            id: item.id,
            rect,
            slot: item.slot,
            group: item.group,
            #[cfg(test)]
            eye: eye_rect,
            #[cfg(test)]
            name: name_rect,
        });
    }

    if let Some(commit) = rename_done
        && let Some(rename) = me.rename.take()
    {
        let unchanged = items
            .iter()
            .any(|item| item.id == rename.id && item.name == rename.text.trim());
        if commit && !unchanged && !rename.text.trim().is_empty() {
            pending.push(("layer.rename", on(rename.id).with("name", rename.text)));
        }
    }

    if let Some(dragged) = me.dragged {
        let (pointer, down) =
            ui.input(|input| (input.pointer.interact_pos(), input.pointer.any_down()));
        let target = rows
            .iter()
            .find(|row| row.id == dragged)
            .zip(pointer)
            .and_then(|(source, pointer)| {
                let target = drop_target(&rows, pointer.y)?;
                let to = destination(source.slot, target.insert);
                // Not onto itself, and not into its own inside.
                let sprite = state.editor.document()?.sprite();
                let into_itself = to
                    .parent
                    .is_some_and(|parent| sprite.is_within(parent, dragged));
                (to != source.slot && !into_itself).then_some((target, to))
            });

        if let Some((target, _)) = target {
            let accent = visuals.selection.stroke.color;
            let painter = ui.painter();
            if let (Some(y), Some(first)) = (target.line, rows.first()) {
                painter.hline(first.rect.x_range(), y, Stroke::new(2.0, accent));
            }
            if let Some(row) = rows.iter().find(|row| Some(row.id) == target.into) {
                painter.rect_stroke(row.rect, 4.0, Stroke::new(2.0, accent), StrokeKind::Inside);
            }
        }

        if down {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else {
            me.dragged = None;
            if let Some((_, to)) = target {
                let mut params = on(dragged).with("index", to.index as i64);
                if let Some(parent) = to.parent {
                    params = params.with("parent", parent.0);
                }
                pending.push(("layer.move", params));
            }
        }
    }

    me.rows = rows;
    for (id, params) in pending {
        run(registry, state, id, params);
    }
}

/// A small triangle: pointing right when the group is folded, down when it is open.
fn paint_fold(painter: &egui::Painter, center: Pos2, collapsed: bool, color: Color32) {
    let points = if collapsed {
        [
            Vec2::new(-2.5, -4.0),
            Vec2::new(-2.5, 4.0),
            Vec2::new(3.5, 0.0),
        ]
    } else {
        [
            Vec2::new(-4.0, -2.5),
            Vec2::new(4.0, -2.5),
            Vec2::new(0.0, 3.5),
        ]
    };
    let points = points.map(|offset| center + offset).to_vec();
    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// An eye when the layer is visible, a short dash when it is not.
fn paint_eye(painter: &egui::Painter, center: Pos2, visible: bool, color: Color32, weak: Color32) {
    if visible {
        painter.circle_stroke(center, 5.0, Stroke::new(1.5, color));
        painter.circle_filled(center, 2.0, color);
    } else {
        let half = Vec2::new(4.0, 0.0);
        painter.line_segment([center - half, center + half], Stroke::new(1.5, weak));
    }
}

/// A padlock: solid when locked, a faint outline when not.
/// A small framed picture: a sun over a hill.
fn paint_reference(painter: &egui::Painter, center: Pos2, color: Color32) {
    let frame = Rect::from_center_size(center, Vec2::new(12.0, 10.0));
    painter.rect_stroke(frame, 1.0, Stroke::new(1.0, color), StrokeKind::Inside);
    painter.circle_filled(center + Vec2::new(2.5, -1.5), 1.2, color);
    let hill = [
        center + Vec2::new(-4.0, 3.0),
        center + Vec2::new(-1.0, -0.5),
        center + Vec2::new(2.0, 3.0),
    ];
    painter.add(Shape::line(hill.to_vec(), Stroke::new(1.0, color)));
}

fn paint_lock(painter: &egui::Painter, center: Pos2, locked: bool, color: Color32, weak: Color32) {
    let body = Rect::from_center_size(center + Vec2::new(0.0, 2.5), Vec2::new(10.0, 7.0));
    let shackle = center + Vec2::new(0.0, -2.0);
    if locked {
        painter.circle_stroke(shackle, 3.0, Stroke::new(1.5, color));
        painter.rect_filled(body, 2.0, color);
    } else {
        let faint = weak.gamma_multiply(0.6);
        painter.rect_stroke(body, 2.0, Stroke::new(1.0, faint), StrokeKind::Inside);
    }
}

/// A little checkerboard, the usual picture for transparency: solid when the alpha is locked.
fn paint_alpha_lock(
    painter: &egui::Painter,
    center: Pos2,
    locked: bool,
    color: Color32,
    weak: Color32,
) {
    let frame = Rect::from_center_size(center, Vec2::splat(10.0));
    let (ink, outline) = if locked {
        (color, color)
    } else {
        (Color32::TRANSPARENT, weak.gamma_multiply(0.6))
    };
    let square = Vec2::splat(5.0);
    painter.rect_filled(Rect::from_min_size(frame.min, square), 0.0, ink);
    painter.rect_filled(Rect::from_min_size(frame.center(), square), 0.0, ink);
    painter.rect_stroke(frame, 1.0, Stroke::new(1.0, outline), StrokeKind::Inside);
}

#[cfg(test)]
impl LayersUi {
    fn row(&self, layer: LayerId) -> Row {
        let row = self.rows.iter().find(|row| row.id == layer);
        *row.expect("the layer has a row in the panel")
    }

    /// Where to click to select or drag a layer.
    pub fn name_pos(&self, layer: LayerId) -> Pos2 {
        self.row(layer).name.center()
    }

    /// Where to click to show or hide a layer.
    pub fn eye_pos(&self, layer: LayerId) -> Pos2 {
        self.row(layer).eye.center()
    }

    /// The top and bottom edge of a layer's row.
    pub fn row_span(&self, layer: LayerId) -> (f32, f32) {
        let rect = self.row(layer).rect;
        (rect.top(), rect.bottom())
    }

    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    /// The layers with a row, top to bottom.
    pub fn shown_layers(&self) -> Vec<LayerId> {
        self.rows.iter().map(|row| row.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows 26 high, starting at y = 100:
    ///
    /// ```text
    /// 100  5 Top
    /// 126  4 Group (open, two children)
    /// 152      3 Inner top
    /// 178      2 Inner bottom
    /// 204  1 Bottom
    /// ```
    fn rows() -> Vec<Row> {
        let group = Some(LayerId(4));
        let layout = [
            (5, Slot::new(None, 2), None),
            (4, Slot::new(None, 1), Some((false, 2))),
            (3, Slot::new(group, 1), None),
            (2, Slot::new(group, 0), None),
            (1, Slot::new(None, 0), None),
        ];
        layout
            .into_iter()
            .enumerate()
            .map(|(index, (id, slot, group))| {
                let top = 100.0 + index as f32 * ROW_HEIGHT;
                let rect = Rect::from_min_size(Pos2::new(0.0, top), Vec2::new(200.0, ROW_HEIGHT));
                Row {
                    id: LayerId(id),
                    rect,
                    slot,
                    group,
                    eye: rect,
                    name: rect,
                }
            })
            .collect()
    }

    fn insert_at(y: f32) -> Slot {
        drop_target(&rows(), y).unwrap().insert
    }

    #[test]
    fn dropping_on_the_upper_or_lower_half_of_a_row() {
        // Upper half of "Top": above it, i.e. the very top.
        assert_eq!(insert_at(104.0), Slot::new(None, 3));
        // Lower half of "Top": between it and the group.
        assert_eq!(insert_at(122.0), Slot::new(None, 2));
        // Inside the group: above and below "Inner bottom".
        assert_eq!(insert_at(180.0), Slot::new(Some(LayerId(4)), 1));
        assert_eq!(insert_at(200.0), Slot::new(Some(LayerId(4)), 0));
        // Around "Bottom".
        assert_eq!(insert_at(206.0), Slot::new(None, 1));
        assert_eq!(insert_at(228.0), Slot::new(None, 0));
    }

    #[test]
    fn dropping_outside_the_list() {
        assert_eq!(insert_at(20.0), Slot::new(None, 3));
        assert_eq!(insert_at(900.0), Slot::new(None, 0));
        assert_eq!(drop_target(&[], 50.0), None);
    }

    #[test]
    fn dropping_on_a_group_goes_into_it() {
        let into = Slot::new(Some(LayerId(4)), 2);
        // The top edge of the group's row is still "above the group".
        assert_eq!(insert_at(128.0), Slot::new(None, 2));
        // The middle and, as the group is open, the lower edge go inside, on top.
        let middle = drop_target(&rows(), 139.0).unwrap();
        assert_eq!(
            (middle.insert, middle.into, middle.line),
            (into, Some(LayerId(4)), None)
        );
        assert_eq!(insert_at(150.0), into);

        // A folded group takes drops on its lower edge as "below the group".
        let mut folded = rows();
        folded[1].group = Some((true, 2));
        folded.drain(2..4);
        for (index, row) in folded.iter_mut().enumerate() {
            row.rect = Rect::from_min_size(
                Pos2::new(0.0, 100.0 + index as f32 * ROW_HEIGHT),
                Vec2::new(200.0, ROW_HEIGHT),
            );
        }
        assert_eq!(
            drop_target(&folded, 150.0).unwrap().insert,
            Slot::new(None, 1)
        );
        assert_eq!(drop_target(&folded, 139.0).unwrap().insert, into);
    }

    #[test]
    fn the_destination_allows_for_the_layer_leaving_its_old_place() {
        let top = |index| Slot::new(None, index);
        // Moving up within the same list: the slots above shift down by one.
        assert_eq!(destination(top(0), top(3)), top(2));
        assert_eq!(destination(top(0), top(1)), top(0));
        // Moving down, or to another list, needs no correction.
        assert_eq!(destination(top(2), top(0)), top(0));
        let inside = Slot::new(Some(LayerId(4)), 2);
        assert_eq!(destination(top(0), inside), inside);
    }
}
