//! The dockable panels and their default layout (see `PLAN.md`, section 5).
//!
//! Panels show the state and run commands; they never change the document themselves.

use acms_core::color::Rgba8;
use acms_core::command::{CommandRegistry, Params};
use acms_core::pattern::{MAX_DENSITY, Pattern};
use acms_core::sprite::{MAX_RADIAL, Symmetry};
use acms_core::stroke::{BrushShape, MAX_BRUSH_SIZE};
use acms_core::tool::{InkKind, OptionKind, ToolOptions};
use egui::color_picker::{self, Alpha};
use egui::ecolor::Hsva;
use egui::{
    Align, Color32, Id, Layout, Margin, RichText, Sense, Slider, Stroke, StrokeKind, TextEdit,
    TextStyle, Ui, Vec2, WidgetText,
};
use egui_dock::{DockState, NodeIndex, TabStyle, TabViewer};

use crate::canvas::CanvasView;
use crate::commands;
use crate::keymap::Keymap;
use crate::layers_panel::{self, LayersUi};
use crate::state::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The central area where the open sprite is shown. Cannot be closed.
    Canvas,
    Tools,
    ToolOptions,
    Colour,
    Palette,
    Layers,
    Timeline,
    Tileset,
    History,
    Preview,
    ScriptConsole,
    Log,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas",
            Self::Tools => "Tools",
            Self::ToolOptions => "Tool Options",
            Self::Colour => "Colour",
            Self::Palette => "Palette",
            Self::Layers => "Layers",
            Self::Timeline => "Timeline",
            Self::Tileset => "Tileset",
            Self::History => "History",
            Self::Preview => "Preview",
            Self::ScriptConsole => "Script Console",
            Self::Log => "Log",
        }
    }
}

/// The layout the app starts with, and the one "Reset Panel Layout" restores.
///
/// ```text
/// +---------+------------------------+----------+
/// | Tools   |                        | Colour   |
/// +---------+                        |          |
/// | Tool    |         Canvas         +----------+
/// | Options |                        | Palette  |
/// +---------+                        +----------+
/// | Preview |                        | Layers | |
/// +---------+------------------------+ History  |
/// | Timeline | Tileset | Script Console | Log   |
/// +----------------------------------+----------+
/// ```
pub fn default_layout() -> DockState<Panel> {
    use Panel::*;

    let mut dock = DockState::new(vec![Canvas]);
    let tree = dock.main_surface_mut();

    // A fraction is the share of the left / top side of the split, whichever node ends up
    // there (checked against egui_dock 0.21; its `split_left` docs say otherwise).
    let [main, colour] = tree.split_right(NodeIndex::root(), 0.77, vec![Colour]);
    let [_colour, palette] = tree.split_below(colour, 0.5, vec![Palette]);
    tree.split_below(palette, 0.37, vec![Layers, History]);

    let [main, _bottom] = tree.split_below(main, 0.78, vec![Timeline, Tileset, ScriptConsole, Log]);
    let [_canvas, tools] = tree.split_left(main, 0.17, vec![Tools]);
    tree.split_below(tools, 0.6, vec![ToolOptions, Preview]);

    dock
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ColourSlot {
    #[default]
    Primary,
    Secondary,
}

/// The Colour panel's own widget state.
#[derive(Default)]
struct ColourUi {
    /// Which colour the picker edits.
    slot: ColourSlot,
    /// The picker works in HSV so that hue and saturation survive a trip through black.
    hsva: Hsva,
    /// The colour `hsva` currently stands for.
    hsva_for: Option<Rgba8>,
    hex: String,
    /// The hex field holds text the user typed and has not committed yet.
    hex_edited: bool,
}

/// What the panels keep between frames. None of it is part of the document.
#[derive(Default)]
pub struct PanelsUi {
    pub canvas: CanvasView,
    pub layers: LayersUi,
    colour: ColourUi,
}

/// Draws panel contents for egui_dock.
pub struct PanelViewer<'a> {
    pub state: &'a mut AppState,
    pub registry: &'a CommandRegistry<AppState>,
    pub keymap: &'a Keymap,
    pub ui_state: &'a mut PanelsUi,
}

impl TabViewer for PanelViewer<'_> {
    type Tab = Panel;

    fn id(&mut self, tab: &mut Panel) -> Id {
        Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Panel) -> WidgetText {
        match tab {
            Panel::Canvas => self.state.document_title().into(),
            other => other.title().into(),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Panel) {
        let Self {
            state,
            registry,
            keymap,
            ui_state,
        } = self;
        match tab {
            Panel::Canvas => ui_state.canvas.show(ui, state, registry, keymap),
            Panel::Tools => tools(ui, state, registry, keymap),
            Panel::ToolOptions => tool_options(ui, state, registry),
            Panel::Colour => colour(ui, state, registry, keymap, &mut ui_state.colour),
            Panel::Palette => palette(ui, state, registry),
            Panel::Layers => layers_panel::show(ui, state, registry, keymap, &mut ui_state.layers),
            Panel::History => history(ui, state, registry),
            Panel::Log => log(ui, state),
            other => {
                ui.label(RichText::new(format!("{} — placeholder", other.title())).weak());
            }
        }
    }

    fn is_closeable(&self, tab: &Panel) -> bool {
        *tab != Panel::Canvas
    }

    fn allowed_in_windows(&self, tab: &mut Panel) -> bool {
        *tab != Panel::Canvas
    }

    /// The canvas fills its tab edge to edge, on the darker surface colour.
    fn tab_style_override(&self, tab: &Panel, global: &TabStyle) -> Option<TabStyle> {
        (*tab == Panel::Canvas).then(|| {
            let mut style = global.clone();
            style.tab_body.inner_margin = Margin::ZERO;
            style.tab_body.bg_fill = self.state.theme.tokens().surface;
            style
        })
    }

    /// The canvas pans by itself; the other panels scroll when their content does not fit.
    fn scroll_bars(&self, tab: &Panel) -> [bool; 2] {
        [*tab != Panel::Canvas; 2]
    }
}

type Registry = CommandRegistry<AppState>;

/// `"Pencil (B)"`: a label with the shortcut of the command behind it, if it has one.
pub(crate) fn with_shortcut(ui: &Ui, keymap: &Keymap, label: &str, command: &str) -> String {
    match keymap.shortcuts_for(command).next() {
        Some(shortcut) => format!("{label} ({})", ui.ctx().format_shortcut(shortcut)),
        None => label.to_owned(),
    }
}

fn color32(color: Rgba8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn no_sprite(ui: &mut Ui) {
    ui.label(RichText::new("No sprite open").weak());
}

fn tools(ui: &mut Ui, state: &mut AppState, registry: &Registry, keymap: &Keymap) {
    let active = state.editor.active_tool();
    let tools: Vec<_> = state.editor.tools().collect();
    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
        // Compact rows: the whole list should fit without scrolling in the default layout.
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.spacing_mut().interact_size.y = 22.0;
        ui.spacing_mut().button_padding.y = 2.0;
        for (id, name) in tools {
            let command = format!("tool.{id}");
            let tip = with_shortcut(ui, keymap, name, &command);
            if ui
                .selectable_label(active == id, name)
                .on_hover_text(tip)
                .clicked()
            {
                commands::run(registry, state, &command, &Params::new());
            }
        }
    });
}

fn tool_options(ui: &mut Ui, state: &mut AppState, registry: &Registry) {
    let kinds = state.editor.tool_options();
    if kinds.is_empty() {
        ui.label(RichText::new("This tool has no options.").weak());
        return;
    }
    let brush = state.editor.brush.clone();
    let options = state.editor.options.clone();
    // A checkbox that runs a command with its new value.
    let toggle = |ui: &mut Ui, state: &mut AppState, on: bool, label: &str, id: &str, arg: &str| {
        let mut value = on;
        if ui.checkbox(&mut value, label).changed() {
            commands::run(registry, state, id, &Params::new().with(arg, value));
        }
    };
    // The slider takes the width the panel has, leaving room for the number.
    let slider_width = |ui: &Ui| (ui.available_width() - 56.0).max(40.0);

    for kind in kinds {
        match kind {
            OptionKind::Brush => {
                match brush.image() {
                    Some(image) => {
                        let size = format!("{} × {}", image.width(), image.height());
                        ui.label(format!("Custom brush, {size}"));
                    }
                    None => {
                        ui.label("Brush size");
                        let mut size = brush.size();
                        ui.spacing_mut().slider_width = slider_width(ui);
                        if ui.add(Slider::new(&mut size, 1..=MAX_BRUSH_SIZE)).changed() {
                            let params = Params::new().with("size", size);
                            commands::run(registry, state, "brush.set_size", &params);
                        }
                        ui.label("Brush shape");
                    }
                }
                ui.horizontal(|ui| {
                    let shapes = [
                        (BrushShape::Round, "Round", "round"),
                        (BrushShape::Square, "Square", "square"),
                    ];
                    for (shape, label, value) in shapes {
                        let selected = !brush.is_custom() && brush.shape() == shape;
                        if ui.selectable_label(selected, label).clicked() {
                            let params = Params::new().with("shape", value);
                            commands::run(registry, state, "brush.set_shape", &params);
                        }
                    }
                });
            }
            OptionKind::Ink => {
                ui.label("Ink");
                ui.horizontal_wrapped(|ui| {
                    for ink in InkKind::ALL {
                        if ui
                            .selectable_label(options.ink == ink, ink.name())
                            .clicked()
                        {
                            let params = Params::new().with("ink", ink.id());
                            commands::run(registry, state, "tool.set_ink", &params);
                        }
                    }
                });
                match options.ink {
                    InkKind::Simple => {}
                    InkKind::Dither => pattern_picker(ui, state, registry, &options),
                    InkKind::Shade => {
                        let has_ramps = state.editor.document().is_some_and(|document| {
                            !document.sprite().palette().ramps().is_empty()
                        });
                        let hint = if has_ramps {
                            "Left button: one step along the ramp. Right button: one step back."
                        } else {
                            "The palette has no ramps yet. Make one in the Palette panel."
                        };
                        ui.label(RichText::new(hint).weak());
                    }
                }
            }
            OptionKind::PixelPerfect => {
                let on = options.pixel_perfect;
                toggle(
                    ui,
                    state,
                    on,
                    "Pixel-perfect",
                    "tool.set_pixel_perfect",
                    "on",
                );
            }
            OptionKind::Symmetry => symmetry_options(ui, state, registry, options.tiled),
            OptionKind::ShapeFill => {
                let on = options.shape_filled;
                toggle(ui, state, on, "Filled", "tool.set_filled", "filled");
                ui.label(RichText::new("Hold Shift for a square or circle.").weak());
            }
            OptionKind::Similarity => {
                let on = options.fill_contiguous;
                toggle(
                    ui,
                    state,
                    on,
                    "Contiguous",
                    "tool.set_contiguous",
                    "contiguous",
                );
                ui.label("Tolerance");
                let mut tolerance = u32::from(options.tolerance);
                ui.spacing_mut().slider_width = slider_width(ui);
                if ui.add(Slider::new(&mut tolerance, 0..=255)).changed() {
                    let params = Params::new().with("tolerance", tolerance);
                    commands::run(registry, state, "tool.set_tolerance", &params);
                }
            }
            OptionKind::Gradient => {
                let on = options.gradient_dither;
                toggle(ui, state, on, "Dithered", "tool.set_dither", "dither");
                ui.label(RichText::new("From the primary to the secondary colour.").weak());
            }
            OptionKind::SelectionMode => {
                ui.label(RichText::new("Shift adds, Alt subtracts, both intersect.").weak());
            }
        }
    }
}

/// Side of one pattern preview, and of one of the pattern's pixels in it.
const PATTERN_PREVIEW: f32 = 32.0;
const PATTERN_PIXEL: f32 = 4.0;

/// The dither patterns as pictures to pick from, and the density of the one picked.
fn pattern_picker(ui: &mut Ui, state: &mut AppState, registry: &Registry, options: &ToolOptions) {
    let (first, second) = (state.editor.primary, state.editor.secondary);
    let mut patterns = Pattern::BUILT_IN.to_vec();
    patterns.extend(state.editor.custom_pattern.clone());

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for pattern in patterns {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::splat(PATTERN_PREVIEW), Sense::click());
            let painter = ui.painter_at(rect);
            let cells = (PATTERN_PREVIEW / PATTERN_PIXEL) as i32;
            for y in 0..cells {
                for x in 0..cells {
                    let color = if pattern.picks_first(x, y, options.dither_density) {
                        first
                    } else {
                        second
                    };
                    let min = rect.min + Vec2::new(x as f32, y as f32) * PATTERN_PIXEL;
                    let cell = egui::Rect::from_min_size(min, Vec2::splat(PATTERN_PIXEL));
                    color_picker::show_color_at(&painter, color32(color), cell);
                }
            }
            let visuals = ui.visuals();
            let stroke = if pattern == options.pattern {
                Stroke::new(2.0, visuals.selection.stroke.color)
            } else {
                Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color)
            };
            painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
            if response.on_hover_text(pattern.name()).clicked() {
                let params = Params::new().with("pattern", pattern.id());
                commands::run(registry, state, "tool.set_pattern", &params);
            }
        }
    });

    if options.pattern.uses_density() {
        ui.label("Density");
        let mut density = u32::from(options.dither_density);
        ui.spacing_mut().slider_width = (ui.available_width() - 56.0).max(40.0);
        let slider = Slider::new(&mut density, 0..=u32::from(MAX_DENSITY)).suffix("%");
        if ui.add(slider).changed() {
            let params = Params::new().with("density", density);
            commands::run(registry, state, "tool.set_density", &params);
        }
    }
    ui.label(RichText::new("The right button swaps the two colours.").weak());
}

/// The sprite's symmetry, and whether strokes wrap around the canvas.
fn symmetry_options(ui: &mut Ui, state: &mut AppState, registry: &Registry, tiled: bool) {
    let Some(symmetry) = state
        .editor
        .document()
        .map(|document| document.sprite().symmetry())
    else {
        return;
    };
    let radial = Symmetry::Radial(state.radial_count);
    ui.label("Symmetry");
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("symmetry")
            .selected_text(symmetry.name())
            .width(96.0)
            .show_ui(ui, |ui| {
                let modes = [
                    Symmetry::Off,
                    Symmetry::Horizontal,
                    Symmetry::Vertical,
                    Symmetry::Both,
                    radial,
                ];
                for mode in modes {
                    let selected = mode.id() == symmetry.id();
                    if ui.selectable_label(selected, mode.name()).clicked() {
                        let params = Params::new().with("mode", mode.id());
                        commands::run(registry, state, "symmetry.set", &params);
                    }
                }
            });
        if let Symmetry::Radial(count) = symmetry {
            let mut copies = u32::from(count);
            let field = egui::DragValue::new(&mut copies).range(2..=u32::from(MAX_RADIAL));
            if ui.add(field).on_hover_text("Copies").changed() {
                let params = Params::new().with("mode", "radial").with("count", copies);
                commands::run(registry, state, "symmetry.set", &params);
            }
        }
    });

    let mut wrap = tiled;
    if ui
        .checkbox(&mut wrap, "Tiled")
        .on_hover_text("Strokes that leave the canvas come back in on the other side")
        .changed()
    {
        let params = Params::new().with("on", wrap);
        commands::run(registry, state, "view.toggle_tiled", &params);
    }
}

/// A clickable colour swatch. `ring` draws the accent outline that marks a selection.
fn swatch(ui: &mut Ui, color: Rgba8, size: Vec2, ring: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    color_picker::show_color_at(ui.painter(), color32(color), rect);
    let visuals = ui.visuals();
    let stroke = if ring {
        Stroke::new(2.0, visuals.selection.stroke.color)
    } else {
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color)
    };
    ui.painter()
        .rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
    response
}

fn colour(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    keymap: &Keymap,
    panel: &mut ColourUi,
) {
    let color_of = |state: &AppState, slot: ColourSlot| match slot {
        ColourSlot::Primary => state.editor.primary,
        ColourSlot::Secondary => state.editor.secondary,
    };
    let set = |state: &mut AppState, slot: ColourSlot, color: Rgba8| {
        let command = match slot {
            ColourSlot::Primary => "color.set_primary",
            ColourSlot::Secondary => "color.set_secondary",
        };
        let params = Params::new().with("color", color);
        commands::run(registry, state, command, &params);
    };

    ui.horizontal(|ui| {
        let slots = [
            (ColourSlot::Primary, "Primary colour"),
            (ColourSlot::Secondary, "Secondary colour"),
        ];
        for (slot, label) in slots {
            let size = Vec2::new(36.0, 28.0);
            let color = color_of(state, slot);
            let response = swatch(ui, color, size, panel.slot == slot).on_hover_text(label);
            if response.clicked() && panel.slot != slot {
                panel.slot = slot;
                panel.hex_edited = false;
            }
        }
        let tip = with_shortcut(ui, keymap, "Swap primary and secondary", "color.swap");
        if ui.button("Swap").on_hover_text(tip).clicked() {
            commands::run(registry, state, "color.swap", &Params::new());
        }

        // The hex field shows the selected colour unless the user is typing in it.
        if !panel.hex_edited {
            panel.hex = color_of(state, panel.slot).to_hex();
        }
        let field = TextEdit::singleline(&mut panel.hex)
            .font(TextStyle::Monospace)
            .desired_width(84.0);
        let response = ui
            .add(field)
            .on_hover_text("Hex colour: RRGGBB or RRGGBBAA");
        panel.hex_edited |= response.changed();
        if response.lost_focus() && std::mem::take(&mut panel.hex_edited) {
            // Text that is not a colour is simply replaced by the current one next frame.
            if let Some(color) = Rgba8::from_hex(&panel.hex) {
                set(state, panel.slot, color);
            }
        }
    });

    // Follow the colour when something else changed it (palette, eyedropper, swap, hex).
    let current = color_of(state, panel.slot);
    if panel.hsva_for != Some(current) {
        panel.hsva = Hsva::from_srgba_unmultiplied(current.to_array());
        panel.hsva_for = Some(current);
    }
    ui.spacing_mut().slider_width = ui.available_width().clamp(100.0, 130.0);
    if color_picker::color_picker_hsva_2d(ui, &mut panel.hsva, Alpha::OnlyBlend) {
        let picked = Rgba8::from_array(panel.hsva.to_srgba_unmultiplied());
        panel.hsva_for = Some(picked);
        set(state, panel.slot, picked);
    }
}
/// Changes which swatches are selected after a click on the one at `index`: a plain click
/// selects only that one, Ctrl adds or removes it, Shift extends from the last one picked.
fn pick_swatch(selection: &mut Vec<usize>, index: usize, ctrl: bool, shift: bool) {
    match (selection.last().copied(), ctrl, shift) {
        (Some(last), _, true) => {
            let range: Vec<usize> = if last <= index {
                (last..=index).collect()
            } else {
                (index..=last).rev().collect()
            };
            for picked in range {
                if !selection.contains(&picked) {
                    selection.push(picked);
                }
            }
        }
        (_, true, _) => match selection.iter().position(|picked| *picked == index) {
            Some(at) => {
                selection.remove(at);
            }
            None => selection.push(index),
        },
        _ => {
            selection.clear();
            selection.push(index);
        }
    }
}

fn palette(ui: &mut Ui, state: &mut AppState, registry: &Registry) {
    let Some(document) = state.editor.document() else {
        no_sprite(ui);
        return;
    };
    let palette = document.sprite().palette();
    let entries = palette.entries().to_vec();
    let ramps = palette.ramps().to_vec();
    let (primary, secondary) = (state.editor.primary, state.editor.secondary);
    let set_color = |state: &mut AppState, command: &str, color: Rgba8| {
        let params = Params::new().with("color", color);
        commands::run(registry, state, command, &params);
    };

    // Small, so the swatches keep most of the panel.
    let buttons = [
        ("palette.add_color", "+"),
        ("palette.remove_colors", "−"),
        ("palette.new_ramp", "Ramp"),
        ("palette.generate_ramp", "Generate…"),
    ];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for (id, label) in buttons {
            let Some(command) = registry.get(id) else {
                continue;
            };
            let tip = command.name().trim_end_matches('…').to_owned();
            let button = egui::Button::new(label).small();
            let button = ui.add_enabled(command.is_enabled(state), button);
            if button.on_hover_text(tip).clicked() {
                commands::run(registry, state, id, &Params::new());
            }
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(3.0);
        for (index, entry) in entries.iter().enumerate() {
            let name = entry.name.clone().unwrap_or_else(|| entry.color.to_hex());
            let tip = format!(
                "{name}\nClick: primary colour\nRight click: secondary colour\nCtrl or Shift \
                 click: select several"
            );
            let selected = state.swatch_selection.contains(&index);
            let response = swatch(ui, entry.color, Vec2::splat(20.0), selected);
            // A corner mark on the two colours in use.
            let corner = |at: egui::Pos2, toward: Vec2| {
                let points = vec![
                    at,
                    at + Vec2::new(toward.x, 0.0),
                    at + Vec2::new(0.0, toward.y),
                ];
                let fill = ui.visuals().strong_text_color();
                ui.painter()
                    .add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
            };
            if entry.color == primary {
                corner(response.rect.left_top(), Vec2::splat(7.0));
            }
            if entry.color == secondary {
                corner(response.rect.right_bottom(), Vec2::splat(-7.0));
            }

            let response = response.on_hover_text(tip);
            if response.clicked() {
                let modifiers = ui.input(|input| input.modifiers);
                let (ctrl, shift) = (modifiers.command, modifiers.shift);
                pick_swatch(&mut state.swatch_selection, index, ctrl, shift);
                if !ctrl && !shift {
                    set_color(state, "color.set_primary", entry.color);
                }
            } else if response.secondary_clicked() {
                set_color(state, "color.set_secondary", entry.color);
            }
        }
    });

    if ramps.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("Ramps").weak());
    for (index, ramp) in ramps.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(1.0, 0.0);
            for &color in ramp {
                let response =
                    swatch(ui, color, Vec2::new(14.0, 16.0), false).on_hover_text(color.to_hex());
                if response.clicked() {
                    set_color(state, "color.set_primary", color);
                } else if response.secondary_clicked() {
                    set_color(state, "color.set_secondary", color);
                }
            }
            ui.add_space(4.0);
            if ui.small_button("×").on_hover_text("Delete ramp").clicked() {
                let params = Params::new().with("ramp", index as i64);
                commands::run(registry, state, "palette.delete_ramp", &params);
            }
        });
    }
}

fn history(ui: &mut Ui, state: &mut AppState, registry: &Registry) {
    let Some(document) = state.editor.document() else {
        no_sprite(ui);
        return;
    };
    let cursor = document.history_cursor();
    // Row 0 is the sprite before any of the listed steps.
    let rows: Vec<String> = std::iter::once("Start")
        .chain(document.history())
        .map(str::to_owned)
        .collect();

    ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
        for (steps, name) in rows.into_iter().enumerate() {
            // Steps after the cursor have been undone and can still be redone.
            let label = if steps > cursor {
                RichText::new(name).weak()
            } else {
                RichText::new(name)
            };
            if ui.selectable_label(steps == cursor, label).clicked() && steps != cursor {
                let params = Params::new().with("steps", steps as i64);
                commands::run(registry, state, "edit.history_jump", &params);
            }
        }
    });
}

fn log(ui: &mut Ui, state: &AppState) {
    if state.log.is_empty() {
        ui.label(RichText::new("Nothing logged yet.").weak());
    }
    for line in &state.log {
        ui.monospace(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_contains_every_panel_once() {
        use Panel::*;
        let all = [
            Canvas,
            Tools,
            ToolOptions,
            Colour,
            Palette,
            Layers,
            Timeline,
            Tileset,
            History,
            Preview,
            ScriptConsole,
            Log,
        ];

        let dock = default_layout();
        let placed: Vec<Panel> = dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
        assert_eq!(placed.len(), all.len());
        for panel in all {
            assert!(placed.contains(&panel), "{panel:?} is missing");
        }
    }
    #[test]
    fn swatches_are_picked_one_several_or_in_a_run() {
        let mut selection = Vec::new();
        pick_swatch(&mut selection, 4, false, false);
        assert_eq!(selection, [4]);
        pick_swatch(&mut selection, 7, false, false);
        assert_eq!(selection, [7]);

        // Ctrl adds and takes away, keeping the order of picking.
        pick_swatch(&mut selection, 2, true, false);
        pick_swatch(&mut selection, 9, true, false);
        assert_eq!(selection, [7, 2, 9]);
        pick_swatch(&mut selection, 2, true, false);
        assert_eq!(selection, [7, 9]);

        // Shift runs from the last one picked, in the direction of the click.
        pick_swatch(&mut selection, 11, false, true);
        assert_eq!(selection, [7, 9, 10, 11]);
        let mut selection = vec![5];
        pick_swatch(&mut selection, 2, false, true);
        assert_eq!(selection, [5, 4, 3, 2]);
        // With nothing picked yet, Shift is a plain click.
        let mut selection = Vec::new();
        pick_swatch(&mut selection, 3, false, true);
        assert_eq!(selection, [3]);
    }
}
