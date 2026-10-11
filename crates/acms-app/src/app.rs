//! The eframe application: wires the registry, keymap, palette, dialogs and dock together.

use std::collections::BTreeSet;
use std::path::Path;

use acms_core::command::{CommandRegistry, Params};
use egui_dock::{DockArea, DockState};

use crate::commands;
use crate::config;
use crate::dialogs;
use crate::keymap::Keymap;
use crate::menu;
use crate::palette;
use crate::panels::{PanelViewer, PanelsUi};
use crate::state::AppState;
use crate::status;
use crate::theme::{self, ThemeKind};

pub struct AcmsApp {
    registry: CommandRegistry<AppState>,
    keymap: Keymap,
    state: AppState,
    /// Widget state of the panels: textures, text being typed and the like.
    panels: PanelsUi,
    /// The theme egui is currently showing; compared with `state.theme` every frame.
    shown_theme: ThemeKind,
    /// The window has been told to close, so a close request is no longer questioned.
    closing: bool,
}

impl AcmsApp {
    /// `keymap_file` is the user's keymap file; `None` means built-in shortcuts only.
    pub fn new(ctx: &egui::Context, keymap_file: Option<&Path>) -> anyhow::Result<Self> {
        let mut registry = CommandRegistry::new();
        commands::register_builtin(&mut registry)?;

        let (keymap, mut log) = match keymap_file {
            Some(path) => config::load_keymap(path),
            None => (Keymap::with_defaults(), Vec::new()),
        };
        // The binding is kept: the chord is the user's choice, and pressing it says the same.
        let unknown: BTreeSet<&str> = keymap
            .commands()
            .filter(|id| registry.get(id).is_none())
            .collect();
        log.extend(
            unknown
                .into_iter()
                .map(|id| format!("Keymap: there is no command `{id}`")),
        );

        let state = AppState {
            log,
            ..AppState::default()
        };
        theme::install(ctx, state.theme);

        Ok(Self {
            registry,
            keymap,
            shown_theme: state.theme,
            state,
            panels: PanelsUi::default(),
            closing: false,
        })
    }

    fn run_command(&mut self, id: &str, params: &Params) {
        commands::run(&self.registry, &mut self.state, id, params);
    }

    /// One frame of the whole UI. Separate from [`eframe::App::ui`] so tests can drive it
    /// without a window.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // Closing the window is quitting, which asks before unsaved changes are lost.
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.closing && self.state.has_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.run_command("app.quit", &Params::new());
        }

        // A dialog is modal: shortcuts wait until it is closed.
        if self.state.dialog.is_none() {
            let triggered = self
                .keymap
                .triggered(&ctx)
                .map(|(id, params)| (id.to_owned(), params.clone()));
            if let Some((id, params)) = triggered {
                self.run_command(&id, &params);
            }
        }

        egui::Panel::top("menu_bar").show(ui, |ui| {
            menu::show(ui, &mut self.state, &self.registry, &self.keymap);
        });
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            status::show(ui, &self.state, self.panels.canvas.hover());
        });

        // The dock is taken out of the state while it is shown, so that the panels inside it
        // can be given the whole state to read and to run commands on.
        let layout = self.state.layout_generation;
        let mut dock = std::mem::replace(&mut self.state.dock, DockState::new(Vec::new()));
        let surface = self.shown_theme.tokens().surface;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(surface))
            .show(ui, |ui| {
                let style = theme::dock_style(ui.style(), self.shown_theme);
                let mut viewer = PanelViewer {
                    state: &mut self.state,
                    registry: &self.registry,
                    keymap: &self.keymap,
                    ui_state: &mut self.panels,
                };
                DockArea::new(&mut dock)
                    .style(style)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut viewer);
            });
        // If a command replaced the layout meanwhile (Reset Panel Layout), the new one stays.
        if self.state.layout_generation == layout {
            self.state.dock = dock;
        }

        // The palette needs its own state mutably and the rest of the app state read-only.
        let mut palette_state = std::mem::take(&mut self.state.palette);
        let picked = palette::show(
            &ctx,
            &mut palette_state,
            &self.registry,
            &self.keymap,
            &self.state,
        );
        self.state.palette = palette_state;
        if let Some(id) = picked {
            self.run_command(&id, &Params::new());
        }

        dialogs::show(&ctx, &mut self.state, &self.registry);

        if self.state.theme != self.shown_theme {
            self.shown_theme = self.state.theme;
            theme::set_active(&ctx, self.shown_theme);
        }
        if std::mem::take(&mut self.state.quit_requested) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for AcmsApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.frame(ui);

        // Native file dialogs block, so they are shown once the frame's UI is done.
        if self.state.file_request.is_some() {
            dialogs::resolve_file_request(&mut self.state, &self.registry, frame);
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use acms_core::color::Rgba8;
    use acms_core::geom::Point;
    use acms_core::sprite::LayerId;
    use acms_core::stroke;
    use egui::{Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Vec2};

    use super::*;
    use crate::state::{Dialog, FileRequest};

    /// Ctrl as a desktop backend reports it on Windows and Linux.
    const CTRL: Modifiers = Modifiers::CTRL.plus(Modifiers::COMMAND);

    struct Harness {
        ctx: egui::Context,
        app: AcmsApp,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_keymap_file(None)
        }

        fn with_keymap_file(keymap_file: Option<&Path>) -> Self {
            let ctx = egui::Context::default();
            let app = AcmsApp::new(&ctx, keymap_file).unwrap();
            let mut harness = Self { ctx, app };
            harness.frame(vec![]);
            harness
        }

        /// With a blank 16×16 sprite open and laid out on screen.
        fn with_sprite() -> Self {
            let mut harness = Self::new();
            let size = Params::new().with("width", 16_u32).with("height", 16_u32);
            harness.app.run_command("file.new", &size);
            // One frame to learn the canvas size and zoom to fit, one to settle.
            harness.frame(vec![]);
            harness.frame(vec![]);
            harness
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            self.frame_with(RawInput {
                events,
                ..self.input()
            })
        }

        fn input(&self) -> RawInput {
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            }
        }

        fn frame_with(&mut self, input: RawInput) -> egui::FullOutput {
            let mut output = self.ctx.run_ui(input, |ui| self.app.frame(ui));
            // There is no renderer to upload textures to.
            output.textures_delta.clear();
            output
        }

        fn press(&mut self, modifiers: Modifiers, key: Key) {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
            self.frame(vec![]);
        }

        fn type_text(&mut self, text: &str) {
            self.frame(vec![Event::Text(text.to_owned())]);
        }

        fn pointer_move(&mut self, pos: Pos2) {
            self.frame(vec![Event::PointerMoved(pos)]);
        }

        fn pointer_button(&mut self, pos: Pos2, button: PointerButton, pressed: bool) {
            self.frame(vec![Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }

        /// Presses at the first sprite pixel, moves through the others, releases at the last.
        fn drag_pixels(&mut self, button: PointerButton, pixels: &[(i32, i32)]) {
            let positions: Vec<Pos2> = pixels
                .iter()
                .map(|&(x, y)| self.app.panels.canvas.screen_pos(x, y))
                .collect();
            let (first, last) = (positions[0], positions[positions.len() - 1]);
            self.pointer_move(first);
            self.pointer_button(first, button, true);
            for &pos in &positions[1..] {
                self.pointer_move(pos);
            }
            self.pointer_button(last, button, false);
            self.frame(vec![]);
        }

        fn pixel(&self, x: i32, y: i32) -> Rgba8 {
            let document = self.app.state.editor.document().unwrap();
            document.sprite().pixel(LayerId(1), 0, x, y)
        }

        fn history(&self) -> Vec<String> {
            let document = self.app.state.editor.document().unwrap();
            document.history().map(str::to_owned).collect()
        }
    }

    #[test]
    fn a_shortcut_runs_its_command() {
        let mut h = Harness::new();
        h.press(CTRL.plus(Modifiers::SHIFT), Key::T);
        assert_eq!(h.app.state.theme, ThemeKind::Light);
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
    }

    #[test]
    fn the_users_keymap_file_changes_which_keys_run_a_command() {
        let file = config::TempFile::new(
            "app-rebind.json",
            r#"{ "bindings": [
                { "keys": "Ctrl+J", "command": "view.toggle_grid" },
                { "keys": "Ctrl+G", "command": null },
                { "keys": "Ctrl+3", "command": "brush.set_size", "args": { "size": 3 } }
            ] }"#,
        );
        let mut h = Harness::with_keymap_file(Some(file.path()));
        assert!(h.app.state.view.show_grid);

        h.press(CTRL, Key::G);
        assert!(h.app.state.view.show_grid);
        h.press(CTRL, Key::J);
        assert!(!h.app.state.view.show_grid);

        h.press(CTRL, Key::Num3);
        assert_eq!(h.app.state.editor.brush.size(), 3);
    }

    #[test]
    fn a_binding_to_a_command_that_does_not_exist_is_reported() {
        let file = config::TempFile::new(
            "app-unknown.json",
            r#"{ "bindings": [ { "keys": "Ctrl+J", "command": "debug.no_such_thing" } ] }"#,
        );
        let mut h = Harness::with_keymap_file(Some(file.path()));
        assert_eq!(
            h.app.state.log.last().map(String::as_str),
            Some("Keymap: there is no command `debug.no_such_thing`")
        );

        h.press(CTRL, Key::J);
        assert_eq!(
            h.app.state.log.last().map(String::as_str),
            Some("unknown command `debug.no_such_thing`")
        );
    }

    #[test]
    fn ctrl_shift_p_and_ctrl_k_open_the_palette() {
        for (modifiers, key) in [(CTRL.plus(Modifiers::SHIFT), Key::P), (CTRL, Key::K)] {
            let mut h = Harness::new();
            assert!(!h.app.state.palette.is_open());
            h.press(modifiers, key);
            assert!(h.app.state.palette.is_open());
        }
    }

    #[test]
    fn palette_search_then_enter_runs_the_command_and_closes() {
        let mut h = Harness::new();
        h.press(CTRL.plus(Modifiers::SHIFT), Key::P);
        h.type_text("pixel grid");
        h.press(Modifiers::NONE, Key::Enter);
        assert!(!h.app.state.view.show_grid);
        assert!(!h.app.state.palette.is_open());
    }

    #[test]
    fn palette_arrow_keys_move_the_selection() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        // The first two commands in registration order are "New Sprite…" and "Open…".
        h.press(Modifiers::NONE, Key::ArrowDown);
        h.press(Modifiers::NONE, Key::Enter);
        assert_eq!(
            h.app.state.file_request,
            Some(FileRequest::Open { discard: true })
        );
        assert_eq!(h.app.state.dialog, None);
    }

    #[test]
    fn escape_closes_the_palette_without_running_anything() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        h.press(Modifiers::NONE, Key::Escape);
        assert!(!h.app.state.palette.is_open());
        assert_eq!(h.app.state.dialog, None);
        assert!(h.app.state.log.is_empty());
    }

    #[test]
    fn a_disabled_command_cannot_be_run_from_the_palette() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        h.type_text("undo");
        h.press(Modifiers::NONE, Key::Enter);
        // There is nothing to undo, so the command is disabled: the palette stays open.
        assert!(h.app.state.palette.is_open());
        assert!(h.app.state.log.is_empty());
    }

    #[test]
    fn typing_in_the_palette_does_not_trigger_single_key_shortcuts() {
        let mut h = Harness::new();
        h.press(Modifiers::NONE, Key::E);
        assert_eq!(h.app.state.editor.active_tool(), "eraser");

        h.press(CTRL, Key::K);
        h.press(Modifiers::NONE, Key::B);
        assert_eq!(h.app.state.editor.active_tool(), "eraser");
    }

    #[test]
    fn the_quit_shortcut_requests_the_window_to_close() {
        let mut h = Harness::new();
        let output = h.frame(vec![Event::Key {
            key: Key::Q,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: CTRL,
        }]);
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.contains(&egui::ViewportCommand::Close));
    }

    #[test]
    fn dragging_on_the_canvas_draws_one_undoable_stroke() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2), (10, 2)]);

        for x in 2..=10 {
            assert_eq!(h.pixel(x, 2), Rgba8::BLACK, "x = {x}");
        }
        assert_eq!(h.pixel(11, 2), Rgba8::TRANSPARENT);
        assert_eq!(h.pixel(6, 3), Rgba8::TRANSPARENT);
        assert_eq!(h.history(), ["Pencil"]);
        assert!(h.app.state.has_unsaved_changes());

        h.press(CTRL, Key::Z);
        assert_eq!(h.pixel(6, 2), Rgba8::TRANSPARENT);
        h.press(CTRL, Key::Y);
        assert_eq!(h.pixel(6, 2), Rgba8::BLACK);
        h.press(CTRL, Key::Z);
        h.press(CTRL.plus(Modifiers::SHIFT), Key::Z);
        assert_eq!(h.pixel(6, 2), Rgba8::BLACK);
    }

    #[test]
    fn a_fast_stroke_across_the_canvas_leaves_no_gaps() {
        let mut h = Harness::with_sprite();
        // One press, then a single jump most of the way across: as from a fast mouse.
        h.drag_pixels(PointerButton::Primary, &[(1, 1), (14, 9), (3, 14)]);

        for (from, to) in [((1, 1), (14, 9)), ((14, 9), (3, 14))] {
            for p in stroke::line(Point::new(from.0, from.1), Point::new(to.0, to.1)) {
                assert_eq!(h.pixel(p.x, p.y), Rgba8::BLACK, "gap at {p:?}");
            }
        }
        assert_eq!(h.history(), ["Pencil"]);
    }

    #[test]
    fn the_right_button_draws_with_the_secondary_colour() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Secondary, &[(4, 4), (4, 8)]);
        assert_eq!(h.pixel(4, 6), Rgba8::WHITE);
    }

    #[test]
    fn tool_shortcuts_switch_between_pencil_eraser_and_eyedropper() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (10, 2)]);

        h.press(Modifiers::NONE, Key::E);
        h.drag_pixels(PointerButton::Primary, &[(5, 2), (7, 2)]);
        assert_eq!(h.pixel(4, 2), Rgba8::BLACK);
        assert_eq!(h.pixel(6, 2), Rgba8::TRANSPARENT);
        assert_eq!(h.history(), ["Pencil", "Eraser"]);

        // Pick black into the secondary colour, then swap it into primary.
        h.app.state.editor.primary = Rgba8::rgb(1, 2, 3);
        h.press(Modifiers::NONE, Key::I);
        h.drag_pixels(PointerButton::Secondary, &[(3, 2)]);
        assert_eq!(h.app.state.editor.secondary, Rgba8::BLACK);
        h.press(Modifiers::NONE, Key::X);
        assert_eq!(h.app.state.editor.primary, Rgba8::BLACK);
        assert_eq!(h.app.state.editor.secondary, Rgba8::rgb(1, 2, 3));

        h.press(Modifiers::NONE, Key::B);
        assert_eq!(h.app.state.editor.active_tool(), "pencil");
    }

    #[test]
    fn escape_abandons_the_stroke_being_drawn() {
        let mut h = Harness::with_sprite();
        let canvas = &h.app.panels.canvas;
        let (from, to) = (canvas.screen_pos(2, 2), canvas.screen_pos(9, 2));
        h.pointer_move(from);
        h.pointer_button(from, PointerButton::Primary, true);
        h.pointer_move(to);
        assert!(h.app.state.editor.is_interacting());

        h.press(Modifiers::NONE, Key::Escape);
        assert!(!h.app.state.editor.is_interacting());
        h.pointer_button(to, PointerButton::Primary, false);
        assert_eq!(h.pixel(5, 2), Rgba8::TRANSPARENT);
        assert!(h.history().is_empty());
    }

    #[test]
    fn the_wheel_zooms_around_the_pointer() {
        let mut h = Harness::with_sprite();
        let before = h.app.state.view.zoom();
        let under = h.app.panels.canvas.screen_pos(12, 3);
        h.pointer_move(under);

        let wheel = |delta: f32| Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: Vec2::new(0.0, delta),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        };
        h.frame(vec![wheel(1.0)]);
        h.frame(vec![]);
        assert!(h.app.state.view.zoom() > before);
        // The pixel that was under the pointer still is.
        let now = h.app.panels.canvas.screen_pos(12, 3);
        assert!((now - under).length() <= h.app.state.view.zoom() / 2.0 + 1.0);

        h.frame(vec![wheel(-1.0)]);
        h.frame(vec![wheel(-1.0)]);
        assert!(h.app.state.view.zoom() < before);
    }

    #[test]
    fn dragging_with_the_middle_button_pans() {
        let mut h = Harness::with_sprite();
        let start = h.app.panels.canvas.screen_pos(8, 8);
        let corner_before = h.app.panels.canvas.screen_pos(0, 0);

        h.pointer_move(start);
        h.pointer_button(start, PointerButton::Middle, true);
        for step in 1..=4 {
            h.pointer_move(start + Vec2::new(15.0, 10.0) * step as f32);
        }
        h.pointer_button(start + Vec2::new(60.0, 40.0), PointerButton::Middle, false);
        h.frame(vec![]);

        // The drag only counts once the pointer has clearly moved, so allow for a little
        // lost at the start.
        let moved = h.app.panels.canvas.screen_pos(0, 0) - corner_before;
        assert!((40.0..=60.0).contains(&moved.x), "{moved:?}");
        assert!((25.0..=40.0).contains(&moved.y), "{moved:?}");
        // Panning is not drawing.
        assert!(h.history().is_empty());
    }

    #[test]
    fn a_dialog_blocks_shortcuts_and_clicks_on_the_canvas_until_it_is_closed() {
        let mut h = Harness::with_sprite();
        h.press(CTRL, Key::N);
        assert!(matches!(h.app.state.dialog, Some(Dialog::NewSprite { .. })));

        h.press(Modifiers::NONE, Key::E);
        assert_eq!(h.app.state.editor.active_tool(), "pencil");
        h.drag_pixels(PointerButton::Primary, &[(1, 1), (1, 6)]);
        assert!(h.history().is_empty());

        // Escape closes the dialog like Cancel does, and everything works again.
        assert!(h.app.state.dialog.is_some());
        h.press(Modifiers::NONE, Key::Escape);
        assert_eq!(h.app.state.dialog, None);
        h.press(Modifiers::NONE, Key::E);
        assert_eq!(h.app.state.editor.active_tool(), "eraser");
    }

    #[test]
    fn closing_the_window_with_unsaved_changes_asks_first() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);

        let close = |h: &mut Harness| {
            let mut input = h.input();
            let root = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            root.events.push(egui::ViewportEvent::Close);
            let output = h.frame_with(input);
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .clone()
        };

        let commands = close(&mut h);
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        let Some(Dialog::ConfirmDiscard { then }) = h.app.state.dialog.take() else {
            panic!("expected to be asked about unsaved changes");
        };

        // Agreeing closes the window, and the close is not questioned a second time.
        h.app.run_command(&then.id, &then.params);
        let commands = h.frame(vec![]).viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone();
        assert!(commands.contains(&egui::ViewportCommand::Close));
        let commands = close(&mut h);
        assert!(!commands.contains(&egui::ViewportCommand::CancelClose));
    }

    #[test]
    fn closing_the_window_without_changes_just_closes() {
        let mut h = Harness::with_sprite();
        let mut input = h.input();
        let root = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        root.events.push(egui::ViewportEvent::Close);
        let output = h.frame_with(input);
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(!commands.contains(&egui::ViewportCommand::CancelClose));
        assert_eq!(h.app.state.dialog, None);
    }

    impl Harness {
        fn click(&mut self, pos: Pos2) {
            self.pointer_move(pos);
            self.pointer_button(pos, PointerButton::Primary, true);
            self.pointer_button(pos, PointerButton::Primary, false);
            self.frame(vec![]);
        }

        /// Presses at the first screen position, moves through the others, releases.
        fn drag_screen(&mut self, positions: &[Pos2]) {
            let (first, last) = (positions[0], positions[positions.len() - 1]);
            self.pointer_move(first);
            self.pointer_button(first, PointerButton::Primary, true);
            for &pos in &positions[1..] {
                self.pointer_move(pos);
            }
            self.pointer_button(last, PointerButton::Primary, false);
            self.frame(vec![]);
        }

        fn command(&mut self, id: &str) {
            self.app.run_command(id, &Params::new());
            self.frame(vec![]);
            self.frame(vec![]);
        }

        fn active_layer(&self) -> LayerId {
            self.app.state.editor.document().unwrap().active_layer()
        }

        fn layer(&self, id: LayerId) -> acms_core::sprite::Layer {
            let sprite = self.app.state.editor.document().unwrap().sprite();
            sprite.layer(id).unwrap().clone()
        }

        /// Layer ids from the top of the stack to the bottom.
        fn stack(&self) -> Vec<LayerId> {
            let sprite = self.app.state.editor.document().unwrap().sprite();
            let entries = sprite.walk_top_down();
            entries.iter().map(|entry| entry.layer.id).collect()
        }
    }

    #[test]
    fn the_layers_panel_lists_the_stack_and_selects_on_click() {
        let mut h = Harness::with_sprite();
        h.command("layer.new");
        let upper = h.active_layer();
        assert_eq!(h.app.panels.layers.shown_layers(), [upper, LayerId(1)]);

        let below = h.app.panels.layers.name_pos(LayerId(1));
        h.click(below);
        assert_eq!(h.active_layer(), LayerId(1));
        // Selecting is not an edit.
        assert_eq!(h.history(), ["New Layer"]);

        // Drawing now lands on the layer that was clicked.
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);
        assert_eq!(h.pixel(4, 2), Rgba8::BLACK);
    }

    #[test]
    fn clicking_the_eye_hides_a_layer_and_stops_drawing_on_it() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);

        let eye = h.app.panels.layers.eye_pos(LayerId(1));
        h.click(eye);
        assert!(!h.layer(LayerId(1)).visible);
        assert_eq!(h.history(), ["Pencil", "Layer Visibility"]);

        // Hidden: the canvas shows nothing there, and the pencil does nothing.
        let on_line = h.app.panels.canvas.screen_pos(4, 2);
        h.pointer_move(on_line);
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover(),
            Some((Point::new(4, 2), Rgba8::TRANSPARENT))
        );
        h.drag_pixels(PointerButton::Primary, &[(2, 5), (6, 5)]);
        assert_eq!(h.history().len(), 2);

        h.click(eye);
        assert!(h.layer(LayerId(1)).visible);
        h.pointer_move(on_line);
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover(),
            Some((Point::new(4, 2), Rgba8::BLACK))
        );
    }

    #[test]
    fn dragging_a_row_reorders_the_layers() {
        let mut h = Harness::with_sprite();
        h.command("layer.new");
        let upper = h.active_layer();
        assert_eq!(h.stack(), [upper, LayerId(1)]);

        // Drag the top row down past the middle of the bottom row.
        let from = h.app.panels.layers.name_pos(upper);
        let (_, bottom) = h.app.panels.layers.row_span(LayerId(1));
        let to = Pos2::new(from.x, bottom - 4.0);
        h.drag_screen(&[from, from + Vec2::new(0.0, 12.0), to]);
        assert_eq!(h.stack(), [LayerId(1), upper]);
        assert_eq!(h.history(), ["New Layer", "Move Layer"]);

        // Dropping a row where it already is changes nothing.
        let from = h.app.panels.layers.name_pos(upper);
        h.drag_screen(&[from, from + Vec2::new(0.0, 8.0), from]);
        assert_eq!(h.history().len(), 2);

        h.press(CTRL, Key::Z);
        assert_eq!(h.stack(), [upper, LayerId(1)]);
    }

    #[test]
    fn dragging_a_row_onto_a_group_moves_the_layer_into_it() {
        let mut h = Harness::with_sprite();
        h.command("layer.new_group");
        let group = h.active_layer();
        assert_eq!(h.stack(), [group, LayerId(1)]);

        let from = h.app.panels.layers.name_pos(LayerId(1));
        let onto = h.app.panels.layers.name_pos(group);
        h.drag_screen(&[from, from + Vec2::new(0.0, -10.0), onto]);

        let sprite = h.app.state.editor.document().unwrap().sprite();
        assert_eq!(
            sprite.slot_of(LayerId(1)),
            Some(acms_core::sprite::Slot::new(Some(group), 0))
        );
        // The layer is listed under its group, one level in.
        assert_eq!(h.app.panels.layers.shown_layers(), [group, LayerId(1)]);
    }

    #[test]
    fn f2_renames_the_selected_layer_in_place() {
        let mut h = Harness::with_sprite();
        h.press(Modifiers::NONE, Key::F2);
        assert!(h.app.panels.layers.is_renaming());
        // Single-key shortcuts are typing now, not commands.
        h.type_text("Sky");
        h.press(Modifiers::NONE, Key::E);
        assert_eq!(h.app.state.editor.active_tool(), "pencil");

        h.press(Modifiers::NONE, Key::Enter);
        assert!(!h.app.panels.layers.is_renaming());
        assert_eq!(h.layer(LayerId(1)).name, "Sky");
        assert_eq!(h.history(), ["Rename Layer"]);

        // Escape leaves the name alone.
        h.press(Modifiers::NONE, Key::F2);
        h.type_text("Other");
        h.press(Modifiers::NONE, Key::Escape);
        assert!(!h.app.panels.layers.is_renaming());
        assert_eq!(h.layer(LayerId(1)).name, "Sky");
        assert_eq!(h.history().len(), 1);
    }

    #[test]
    fn a_folded_group_hides_its_rows() {
        let mut h = Harness::with_sprite();
        h.command("layer.new_group");
        let group = h.active_layer();
        let into_group = Params::new()
            .with("layer", 1_u32)
            .with("parent", group.0)
            .with("index", 0_i64);
        h.app.run_command("layer.move", &into_group);
        h.frame(vec![]);
        assert_eq!(h.app.panels.layers.shown_layers(), [group, LayerId(1)]);

        let fold = Params::new().with("layer", group.0).with("collapsed", true);
        h.app.run_command("layer.set_collapsed", &fold);
        h.frame(vec![]);
        assert_eq!(h.app.panels.layers.shown_layers(), [group]);
    }

    #[test]
    fn the_status_bar_knows_what_is_under_the_pointer() {
        let mut h = Harness::with_sprite();
        assert_eq!(h.app.panels.canvas.hover(), None);
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);

        let pos = h.app.panels.canvas.screen_pos(5, 2);
        h.pointer_move(pos);
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover(),
            Some((Point::new(5, 2), Rgba8::BLACK))
        );
        let pos = h.app.panels.canvas.screen_pos(5, 9);
        h.pointer_move(pos);
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover(),
            Some((Point::new(5, 9), Rgba8::TRANSPARENT))
        );
    }

    #[test]
    fn reset_layout_from_the_palette_survives_the_frame_it_runs_in() {
        let mut h = Harness::new();
        let all = h.app.state.dock.iter_all_tabs().count();
        h.app
            .state
            .dock
            .retain_tabs(|tab| *tab != crate::panels::Panel::Log);
        h.frame(vec![]);
        assert_eq!(h.app.state.dock.iter_all_tabs().count(), all - 1);

        h.press(CTRL.plus(Modifiers::SHIFT), Key::R);
        assert_eq!(h.app.state.dock.iter_all_tabs().count(), all);
    }
}
