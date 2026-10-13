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
use crate::scripting;
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
    /// See [`adapt_clipboard_keys`].
    paste_seen: bool,
}

/// Makes Ctrl+X / C / V reach the keymap like any other chord.
///
/// The windowing layer does not pass those key presses on. It sends "cut" and "copy" events
/// instead, and for Ctrl+V a "paste" event carrying the clipboard's text — or nothing at all
/// when the clipboard holds no text, as after copying pixels. The release of V does arrive,
/// so that is used when no paste event came first. `paste_seen` remembers that one did.
fn adapt_clipboard_keys(ctx: &egui::Context, paste_seen: &mut bool) {
    use egui::{Event, Key, Modifiers};
    ctx.input_mut(|input| {
        let mut chords = Vec::new();
        for event in &input.events {
            match event {
                Event::Cut => chords.push(Key::X),
                Event::Copy => chords.push(Key::C),
                Event::Paste(_) => {
                    *paste_seen = true;
                    chords.push(Key::V);
                }
                Event::Key {
                    key: Key::V,
                    pressed: false,
                    modifiers,
                    ..
                } => {
                    if modifiers.command && !*paste_seen {
                        chords.push(Key::V);
                    }
                    *paste_seen = false;
                }
                _ => {}
            }
        }
        input
            .events
            .extend(chords.into_iter().map(|key| Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }));
    });
}

impl AcmsApp {
    /// `keymap_file` is the user's keymap file; `None` means built-in shortcuts only.
    /// `scripts_dir` is the folder of the user's scripts; `None` means only the examples that
    /// come with the app.
    pub fn new(
        ctx: &egui::Context,
        keymap_file: Option<&Path>,
        scripts_dir: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let mut registry = CommandRegistry::new();
        commands::register_builtin(&mut registry)?;

        // Scripts first: the keymap may bind keys to their commands.
        let mut state = AppState::default();
        state.scripts.folder = scripts_dir.map(Path::to_owned);
        scripting::reload(&mut registry, &mut state);

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

        state.log.extend(log);
        theme::install(ctx, state.theme);

        Ok(Self {
            registry,
            keymap,
            shown_theme: state.theme,
            state,
            panels: PanelsUi::default(),
            closing: false,
            paste_seen: false,
        })
    }

    /// Lets Copy and Paste reach other programs. Left off in tests.
    pub fn use_system_clipboard(&mut self) {
        self.state.system_clipboard = true;
    }

    fn run_command(&mut self, id: &str, params: &Params) {
        commands::run(&self.registry, &mut self.state, id, params);
    }

    /// One frame of the whole UI. Separate from [`eframe::App::ui`] so tests can drive it
    /// without a window.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        scripting::maintain(&mut self.registry, &mut self.state);

        // Closing the window is quitting, which asks before unsaved changes are lost.
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.closing && self.state.any_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.run_command("app.quit", &Params::new());
        }

        // A playing animation moves on with the clock, and asks to be drawn again when its
        // next frame is due.
        let now = ctx.input(|input| input.time);
        if let Some(wait) = self.state.tick_playback(now) {
            ctx.request_repaint_after(std::time::Duration::from_millis(u64::from(wait)));
        }

        // A dialog is modal: shortcuts wait until it is closed.
        if self.state.dialog.is_none() {
            if !ctx.text_edit_focused() {
                adapt_clipboard_keys(&ctx, &mut self.paste_seen);
            }
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

        // What a script tool saw on the canvas this frame goes to its script now; what it
        // draws is shown next frame.
        if scripting::pump(&self.registry, &mut self.state) {
            ctx.request_repaint();
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
            let app = AcmsApp::new(&ctx, keymap_file, None).unwrap();
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
        assert_eq!(h.app.state.file_request, Some(FileRequest::Open));
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
    fn clipboard_shortcuts_arrive_the_way_the_window_sends_them() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);

        // Ctrl+C and Ctrl+X come as events of their own, not as key presses.
        h.frame(vec![Event::Copy]);
        assert!(h.app.state.editor.clipboard.is_some());
        h.frame(vec![Event::Cut]);
        assert_eq!(h.pixel(4, 2), Rgba8::TRANSPARENT);

        // Ctrl+V with pixels on the clipboard: only the release of V is seen.
        let release = |modifiers| Event::Key {
            key: Key::V,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        };
        h.frame(vec![release(CTRL)]);
        assert_eq!(h.pixel(4, 2), Rgba8::BLACK);
        assert_eq!(h.history(), ["Pencil", "Cut", "Paste"]);

        // With text on the clipboard a paste event comes first; it must not paste twice.
        h.frame(vec![Event::Cut]);
        h.frame(vec![Event::Paste("hello".to_owned())]);
        assert_eq!(h.history().len(), 5);
        h.frame(vec![Event::Cut]);
        h.frame(vec![release(CTRL)]);
        assert_eq!(h.history().len(), 6);
        assert_eq!(h.pixel(4, 2), Rgba8::TRANSPARENT);
        // V released after Ctrl is not a paste.
        h.frame(vec![release(Modifiers::NONE)]);
        assert_eq!(h.history().len(), 6);
    }

    #[test]
    fn dragging_with_a_selection_tool_selects_and_shift_adds() {
        let mut h = Harness::with_sprite();
        h.press(Modifiers::NONE, Key::M);
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (5, 4)]);
        let bounds = |h: &Harness| {
            let sprite = h.app.state.editor.document().unwrap().sprite();
            sprite.selection().map(|mask| mask.bounds())
        };
        assert_eq!(bounds(&h), Some(acms_core::geom::Rect::new(2, 2, 4, 3)));

        // The pencil now only draws inside it.
        h.press(Modifiers::NONE, Key::B);
        h.drag_pixels(PointerButton::Primary, &[(0, 3), (12, 3)]);
        assert_eq!(
            (h.pixel(1, 3), h.pixel(3, 3), h.pixel(8, 3)),
            (Rgba8::TRANSPARENT, Rgba8::BLACK, Rgba8::TRANSPARENT)
        );

        h.press(CTRL, Key::D);
        assert_eq!(bounds(&h), None);
        h.press(CTRL, Key::A);
        assert_eq!(bounds(&h), Some(acms_core::geom::Rect::from_size(16, 16)));
    }

    #[test]
    fn sprite_tabs_switch_and_close() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (6, 2)]);
        let size = Params::new().with("width", 8_u32).with("height", 8_u32);
        h.app.run_command("file.new", &size);
        h.frame(vec![]);
        h.frame(vec![]);
        assert_eq!(h.app.state.editor.documents().len(), 2);
        assert_eq!(h.app.state.editor.document().unwrap().sprite().width(), 8);

        h.press(CTRL, Key::Tab);
        assert_eq!(h.pixel(4, 2), Rgba8::BLACK);
        // Drawing still works after switching: the canvas shows the right sprite.
        h.drag_pixels(PointerButton::Primary, &[(2, 9), (6, 9)]);
        assert_eq!(h.pixel(4, 9), Rgba8::BLACK);

        // Closing the changed sprite asks first.
        h.press(CTRL, Key::W);
        assert!(matches!(
            h.app.state.dialog,
            Some(Dialog::ConfirmDiscard { .. })
        ));
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
    #[test]
    fn drawing_on_the_canvas_is_mirrored_and_wraps_with_the_overlays_shown() {
        use acms_core::image::Image;
        use acms_core::sprite::Slot;

        let mut h = Harness::with_sprite();
        h.app.run_command("symmetry.horizontal", &Params::new());
        h.app.run_command("view.toggle_tiled", &Params::new());
        // A reference picture under the drawing layer and one over it.
        let document = h.app.state.editor.document_mut().unwrap();
        document
            .transact("References", |tx| {
                let picture = || Image::from_pixels(2, 1, vec![Rgba8::WHITE; 2]).unwrap();
                tx.add_reference_layer(Slot::new(None, 0), "Under", picture())?;
                tx.add_reference_layer(Slot::new(None, 2), "Over", picture())
            })
            .unwrap();
        document.set_active_layer(LayerId(1));
        h.frame(vec![]);

        h.app.state.editor.primary = Rgba8::rgb(200, 0, 0);
        h.drag_pixels(PointerButton::Primary, &[(1, 3), (3, 3)]);
        for x in [1, 2, 3, 12, 13, 14] {
            assert_eq!(h.pixel(x, 3), Rgba8::rgb(200, 0, 0), "x = {x}");
        }
        assert_eq!(h.pixel(8, 3), Rgba8::TRANSPARENT);
        assert_eq!(h.history().last().map(String::as_str), Some("Pencil"));

        // Radial symmetry draws its spokes without trouble too.
        h.app.run_command("symmetry.radial", &Params::new());
        h.frame(vec![]);
    }

    #[test]
    fn tool_options_show_for_every_tool_and_ink() {
        let mut h = Harness::with_sprite();
        let editor = &h.app.state.editor;
        let tools: Vec<String> = editor.tools().map(|(id, _)| id.to_owned()).collect();
        for ink in ["simple", "dither", "shade"] {
            h.app
                .run_command("tool.set_ink", &Params::new().with("ink", ink));
            for tool in &tools {
                h.app.run_command(&format!("tool.{tool}"), &Params::new());
                h.frame(vec![]);
            }
        }
        // With a custom brush, a custom pattern and a ramp in the palette.
        h.app.run_command("tool.pencil", &Params::new());
        h.app.run_command("ink.simple", &Params::new());
        h.drag_pixels(PointerButton::Primary, &[(1, 1), (4, 2)]);
        h.app.run_command("brush.from_selection", &Params::new());
        h.app.run_command("pattern.from_selection", &Params::new());
        let steps = Params::new().with("steps", 4_i64);
        h.app.run_command("palette.generate_ramp", &steps);
        h.app.run_command("ink.shade", &Params::new());
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(h.app.state.editor.brush.is_custom());
        assert_eq!(h.app.state.dialog, None);
    }

    #[test]
    fn ctrl_b_makes_a_brush_from_the_selection() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(2, 2), (5, 2)]);
        h.press(CTRL, Key::B);
        let brush = &h.app.state.editor.brush;
        assert_eq!(brush.image().map(|image| image.width()), Some(4));
        // B alone still picks the pencil.
        h.app.run_command("tool.eraser", &Params::new());
        h.press(Modifiers::NONE, Key::B);
        assert_eq!(h.app.state.editor.active_tool(), "pencil");
    }
    /// A sprite with three frames, the pencil having drawn a dot at x = 1, 2, 3 in turn.
    fn three_frames() -> Harness {
        let mut h = Harness::with_sprite();
        for x in 1..=3 {
            if x > 1 {
                h.app.run_command("frame.new", &Params::new());
                h.frame(vec![]);
            }
            h.drag_pixels(PointerButton::Primary, &[(x, 0)]);
        }
        h.app.run_command("frame.first", &Params::new());
        h.frame(vec![]);
        h
    }

    fn dots(h: &Harness) -> Vec<Option<i32>> {
        let sprite = h.app.state.editor.document().unwrap().sprite();
        (0..sprite.frames().len())
            .map(|frame| sprite.cel(LayerId(1), frame).map(|cel| cel.origin().x))
            .collect()
    }

    fn shown_frame(h: &Harness) -> usize {
        h.app.state.editor.document().unwrap().active_frame()
    }

    #[test]
    fn the_timeline_selects_and_moves_cels_and_frames() {
        let mut h = three_frames();
        assert_eq!(dots(&h), [Some(1), Some(2), Some(3)]);

        // A click on a cel shows its frame and selects it.
        let cel = |h: &Harness, frame| h.app.panels.timeline.cel_pos(0, frame);
        let at = cel(&h, 2);
        h.pointer_move(at);
        h.pointer_button(at, PointerButton::Primary, true);
        h.pointer_button(at, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(shown_frame(&h), 2);
        let range = h.app.state.cel_range.unwrap();
        assert_eq!((range.first(), range.last()), (2, 2));

        // Dragging a cel onto another moves its pixels there.
        let (from, to) = (cel(&h, 0), cel(&h, 1));
        h.pointer_move(from);
        h.pointer_button(from, PointerButton::Primary, true);
        h.pointer_move(from + egui::vec2(8.0, 0.0));
        h.pointer_move(to);
        h.pointer_button(to, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(dots(&h), [None, Some(1), Some(3)]);
        assert_eq!(h.history().last().map(String::as_str), Some("Move Cel"));

        // Dragging a frame's number moves the whole frame.
        let number = |h: &Harness, frame| h.app.panels.timeline.frame_pos(frame);
        let (from, to) = (number(&h, 2), number(&h, 0));
        h.pointer_move(from);
        h.pointer_button(from, PointerButton::Primary, true);
        h.pointer_move(from - egui::vec2(8.0, 0.0));
        h.pointer_move(to);
        h.pointer_button(to, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(dots(&h), [Some(3), None, Some(1)]);
        assert_eq!(shown_frame(&h), 0);

        // A click on a number shows that frame.
        let at = number(&h, 1);
        h.pointer_move(at);
        h.pointer_button(at, PointerButton::Primary, true);
        h.pointer_button(at, PointerButton::Primary, false);
        h.frame(vec![]);
        assert_eq!(shown_frame(&h), 1);
    }

    #[test]
    fn frame_keys_step_and_enter_plays_with_the_clock() {
        let mut h = three_frames();
        h.press(Modifiers::NONE, Key::Period);
        assert_eq!(shown_frame(&h), 1);
        h.press(Modifiers::NONE, Key::Comma);
        h.press(Modifiers::NONE, Key::Comma);
        assert_eq!(shown_frame(&h), 2);
        h.press(Modifiers::NONE, Key::Home);
        assert_eq!(shown_frame(&h), 0);

        let at = |h: &mut Harness, seconds: f64| {
            let input = RawInput {
                time: Some(seconds),
                ..h.input()
            };
            h.frame_with(input)
        };
        at(&mut h, 5.0);
        h.app.run_command("playback.toggle", &Params::new());
        assert!(h.app.state.playing.is_some());
        at(&mut h, 5.0);
        // The app asks to be drawn again when the next frame is due.
        let output = at(&mut h, 5.03125);
        let wait = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert!(wait <= std::time::Duration::from_millis(100));
        assert_eq!(shown_frame(&h), 0);
        at(&mut h, 5.125);
        assert_eq!(shown_frame(&h), 1);
        at(&mut h, 5.25);
        assert_eq!(shown_frame(&h), 2);
        at(&mut h, 5.375);
        assert_eq!(shown_frame(&h), 0);

        // Enter stops it, back on the frame it started from.
        at(&mut h, 5.5);
        assert_eq!(shown_frame(&h), 2);
        h.press(Modifiers::NONE, Key::Enter);
        assert!(h.app.state.playing.is_none());
        assert_eq!(shown_frame(&h), 0);
        h.press(Modifiers::NONE, Key::Enter);
        assert!(h.app.state.playing.is_some());
    }

    #[test]
    fn the_canvas_shows_the_frame_with_its_onion_skin() {
        let mut h = three_frames();
        // Each frame shows only its own dot.
        h.app.run_command("frame.next", &Params::new());
        h.frame(vec![]);
        h.pointer_move(h.app.panels.canvas.screen_pos(2, 0));
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover().map(|(_, color)| color.a),
            Some(255)
        );
        h.pointer_move(h.app.panels.canvas.screen_pos(1, 0));
        h.frame(vec![]);
        assert_eq!(
            h.app.panels.canvas.hover().map(|(_, color)| color.a),
            Some(0)
        );

        // With the onion skin on, the neighbours are painted without disturbing the frame.
        h.press(Modifiers::NONE, Key::F3);
        assert!(h.app.state.onion_enabled);
        h.frame(vec![]);
        h.drag_pixels(PointerButton::Primary, &[(5, 5), (8, 5)]);
        let sprite = h.app.state.editor.document().unwrap().sprite();
        assert_ne!(sprite.pixel(LayerId(1), 1, 6, 5), Rgba8::TRANSPARENT);
        assert_eq!(sprite.pixel(LayerId(1), 0, 6, 5), Rgba8::TRANSPARENT);
        assert_eq!(dots(&h), [Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn the_palette_finds_and_runs_a_script_command() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(1, 3), (4, 3)]);
        assert_ne!(h.pixel(1, 3), Rgba8::TRANSPARENT);

        h.press(CTRL.plus(Modifiers::SHIFT), Key::P);
        h.type_text("mirror sprite");
        h.press(Modifiers::NONE, Key::Enter);
        assert!(!h.app.state.palette.is_open());
        // The line from x = 1 to 4 of 16 now runs from 11 to 14.
        assert_eq!(h.pixel(1, 3), Rgba8::TRANSPARENT);
        assert_ne!(h.pixel(11, 3), Rgba8::TRANSPARENT);
        assert_ne!(h.pixel(14, 3), Rgba8::TRANSPARENT);
        assert_eq!(h.history(), ["Pencil", "Mirror Sprite"]);

        // One undo takes the whole script back.
        h.press(CTRL, Key::Z);
        assert_ne!(h.pixel(1, 3), Rgba8::TRANSPARENT);
        assert_eq!(h.pixel(11, 3), Rgba8::TRANSPARENT);
    }

    #[test]
    fn a_key_can_be_bound_to_a_script_command() {
        let file = config::TempFile::new(
            "app-script-key.json",
            r#"{ "bindings": [ { "keys": "Ctrl+J", "command": "example.mirror_sprite" } ] }"#,
        );
        let mut h = Harness::with_keymap_file(Some(file.path()));
        assert!(
            !h.app
                .state
                .log
                .iter()
                .any(|line| line.contains("no command")),
            "{:?}",
            h.app.state.log
        );
        let size = Params::new().with("width", 16_u32).with("height", 16_u32);
        h.app.run_command("file.new", &size);
        h.frame(vec![]);
        h.frame(vec![]);
        h.drag_pixels(PointerButton::Primary, &[(1, 3), (4, 3)]);
        h.press(CTRL, Key::J);
        assert_ne!(h.pixel(14, 3), Rgba8::TRANSPARENT);
        assert_eq!(h.history(), ["Pencil", "Mirror Sprite"]);
    }

    #[test]
    fn a_script_tool_draws_on_the_canvas_as_one_undo_step() {
        let mut h = Harness::with_sprite();
        h.app.run_command("tool.spray", &Params::new());
        h.frame(vec![]);
        h.drag_pixels(PointerButton::Primary, &[(8, 8), (9, 8), (10, 8), (10, 9)]);

        let painted = (0..16)
            .flat_map(|y| (0..16).map(move |x| (x, y)))
            .filter(|&(x, y)| h.pixel(x, y) != Rgba8::TRANSPARENT)
            .count();
        assert!(painted > 0);
        assert_eq!(h.history(), ["Spray"]);
        assert!(!h.app.state.editor.document().unwrap().in_group());
        h.press(CTRL, Key::Z);
        assert!(h.history().len() == 1 && !h.app.state.editor.document().unwrap().can_undo());
        assert_eq!(h.app.state.dialog, None);

        // Escape in the middle of a stroke takes it back.
        let at = h.app.panels.canvas.screen_pos(4, 4);
        h.pointer_move(at);
        h.pointer_button(at, PointerButton::Primary, true);
        h.frame(vec![]);
        h.press(Modifiers::NONE, Key::Escape);
        h.pointer_button(at, PointerButton::Primary, false);
        h.frame(vec![]);
        let document = h.app.state.editor.document().unwrap();
        assert!(!document.in_group());
        assert!(!document.can_undo());
    }

    #[test]
    fn a_script_dialog_is_shown_and_escape_cancels_it() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(1, 3), (4, 3)]);
        h.app.run_command("example.palette_swap", &Params::new());
        assert!(matches!(h.app.state.dialog, Some(Dialog::Script(_))));
        // The dialog stays up from frame to frame, and keeps shortcuts away.
        h.frame(vec![]);
        h.press(Modifiers::NONE, Key::E);
        assert!(matches!(h.app.state.dialog, Some(Dialog::Script(_))));
        assert_eq!(h.app.state.editor.active_tool(), "pencil");

        h.press(Modifiers::NONE, Key::Escape);
        assert_eq!(h.app.state.dialog, None);
        assert_eq!(h.history(), ["Pencil"]);
        // The script was told, and is not left waiting: the next one runs.
        h.app.run_command("example.mirror_sprite", &Params::new());
        assert_eq!(h.history(), ["Pencil", "Mirror Sprite"]);
    }

    #[test]
    fn the_scripts_menu_and_console_are_drawn() {
        let mut h = Harness::with_sprite();
        let console = h
            .app
            .state
            .dock
            .find_tab(&crate::panels::Panel::ScriptConsole)
            .unwrap();
        let _ = h.app.state.dock.set_active_tab(console);
        h.app
            .run_command("script.run", &Params::new().with("code", "1 + 1"));
        h.app
            .run_command("script.run", &Params::new().with("code", "oops("));
        // Drawing the console with output and an error in it must not fail.
        h.frame(vec![]);
        h.frame(vec![]);
        let lines = &h.app.state.scripts.console;
        assert!(lines.iter().any(|line| line.text == "2"));
        assert_eq!(h.app.state.dialog, None);

        // Reload Scripts is carried out at the start of the next frame.
        h.app.state.scripts.console.clear();
        h.app.run_command("script.reload", &Params::new());
        assert!(h.app.state.scripts.console.is_empty());
        h.frame(vec![]);
        assert!(
            h.app.state.scripts.console[0]
                .text
                .starts_with("Loaded 5 of 5 scripts.")
        );
        assert!(h.app.registry.get("example.outline").is_some());
    }

    #[test]
    fn the_scripts_menu_lists_and_runs_script_commands() {
        let mut h = Harness::with_sprite();
        h.drag_pixels(PointerButton::Primary, &[(1, 3), (4, 3)]);
        h.app.state.scripts.console.clear();

        // Where the items are is egui's business: open the menu and try every row under its
        // title until each of the two things looked for has happened.
        let title = Pos2::new(396.0, 16.0);
        let (mut mirrored, mut reloaded, mut opened) = (false, false, false);
        for row in 0..40 {
            h.pointer_move(title);
            h.pointer_button(title, PointerButton::Primary, true);
            h.pointer_button(title, PointerButton::Primary, false);
            h.frame(vec![]);
            opened |= egui::Popup::is_any_open(&h.ctx);
            let item = Pos2::new(title.x + 20.0, 34.0 + 6.0 * row as f32);
            h.pointer_move(item);
            h.pointer_button(item, PointerButton::Primary, true);
            h.pointer_button(item, PointerButton::Primary, false);
            h.frame(vec![]);
            h.frame(vec![]);
            // Whatever was hit, leave no dialog or open menu behind for the next try.
            h.app.state.dialog = None;
            h.press(Modifiers::NONE, Key::Escape);
            mirrored |= h.history().iter().any(|step| step == "Mirror Sprite");
            let console = &h.app.state.scripts.console;
            reloaded |= console.iter().any(|line| line.text.starts_with("Loaded"));
        }
        assert!(opened, "the Scripts menu did not open");
        assert!(mirrored, "Mirror Sprite was not found in the Scripts menu");
        assert!(reloaded, "Reload Scripts was not found in the Scripts menu");
    }

    /// The time one frame may take at 120 frames a second.
    const FRAME_BUDGET: std::time::Duration = std::time::Duration::from_micros(8333);

    /// A 512 × 512 sprite with 20 layers, each with a picture covering the whole canvas.
    fn large_sprite() -> Harness {
        use acms_core::geom::Rect;
        use acms_core::image::Image;
        let mut h = Harness::new();
        let size = Params::new().with("width", 512_u32).with("height", 512_u32);
        h.app.run_command("file.new", &size);
        for _ in 1..20 {
            h.app.run_command("layer.new", &Params::new());
        }
        let document = h.app.state.editor.document_mut().unwrap();
        let layers: Vec<LayerId> = document
            .sprite()
            .walk()
            .iter()
            .map(|entry| entry.layer.id)
            .collect();
        assert_eq!(layers.len(), 20);
        for (index, layer) in layers.into_iter().enumerate() {
            // An opaque background, and above it layers that all let the ones below show.
            let shade = (index * 12) as u8;
            let alpha = if index == 0 { 255 } else { 40 };
            let pixels = vec![Rgba8::new(shade, 255 - shade, 128, alpha); 512 * 512];
            let image = Image::from_pixels(512, 512, pixels).unwrap();
            document
                .transact("Fill", |tx| {
                    tx.set_pixels(layer, 0, Rect::new(0, 0, 512, 512), &image)
                })
                .unwrap();
        }
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    /// One frame as the windowing layer runs it, short of the GPU: the UI, the compositor and
    /// turning the shapes into triangles.
    fn timed_frame(h: &mut Harness, events: Vec<Event>) -> std::time::Duration {
        let started = std::time::Instant::now();
        let output = h.frame(events);
        let _ = h.ctx.tessellate(output.shapes, output.pixels_per_point);
        started.elapsed()
    }

    /// Run with `cargo test --release -p acms-app -- --ignored frame_time --nocapture`.
    /// Timings in a debug build say nothing, so the budget is only enforced in release.
    #[test]
    #[ignore = "a measurement; run it in release"]
    fn frame_time_while_drawing_on_a_large_sprite() {
        let mut h = large_sprite();
        h.app
            .run_command("brush.set_size", &Params::new().with("size", 8_u32));
        let at = |h: &Harness, x: i32, y: i32| h.app.panels.canvas.screen_pos(x, y);

        let start = at(&h, 40, 40);
        h.pointer_move(start);
        h.pointer_button(start, PointerButton::Primary, true);
        let mut times = Vec::new();
        for step in 0..400 {
            // A fast scribble: several pixels a frame, back and forth across the canvas.
            let x = 40 + (step * 7) % 430;
            let y = 40 + (step * 3) % 430;
            let pos = at(&h, x, y);
            times.push(timed_frame(&mut h, vec![Event::PointerMoved(pos)]));
        }
        let end = at(&h, 40, 40);
        h.pointer_button(end, PointerButton::Primary, false);
        assert_eq!(h.history().last().map(String::as_str), Some("Pencil"));

        // Frames with nothing going on.
        let idle: Vec<_> = (0..50).map(|_| timed_frame(&mut h, vec![])).collect();
        // The worst there is: every layer of the whole canvas flattened again.
        h.app.run_command("layer.set_visible", &Params::new());
        let full = timed_frame(&mut h, vec![]);
        h.app.run_command("layer.set_visible", &Params::new());
        // Stepping through an animation: the first time round each frame is flattened, after
        // that its picture is known.
        h.app.run_command("frame.duplicate", &Params::new());
        h.app.run_command("frame.duplicate", &Params::new());
        timed_frame(&mut h, vec![]);
        let mut first_round = Vec::new();
        let mut later_rounds = Vec::new();
        for step in 0..30 {
            h.app.run_command("frame.next", &Params::new());
            let time = timed_frame(&mut h, vec![]);
            if step < 2 {
                first_round.push(time);
            } else if step >= 3 {
                later_rounds.push(time);
            }
        }

        times.sort();
        later_rounds.sort();
        let mean = times.iter().sum::<std::time::Duration>() / times.len() as u32;
        let worst = times[times.len() - 1];
        let p99 = times[times.len() * 99 / 100];
        let idle_mean = idle.iter().sum::<std::time::Duration>() / idle.len() as u32;
        let stepping = later_rounds[later_rounds.len() - 1];
        eprintln!("drawing: mean {mean:?}, 99th percentile {p99:?}, worst {worst:?}");
        eprintln!("idle: mean {idle_mean:?}");
        eprintln!(
            "flattening everything: {full:?}; a frame shown for the first time: {first_round:?}"
        );
        eprintln!("a frame shown again: worst {stepping:?}");
        if !cfg!(debug_assertions) {
            assert!(p99 < FRAME_BUDGET, "99th percentile {p99:?}");
            assert!(mean < FRAME_BUDGET / 2, "mean {mean:?}");
            assert!(
                stepping < FRAME_BUDGET,
                "a frame shown again took {stepping:?}"
            );
            assert!(full < FRAME_BUDGET, "flattening everything took {full:?}");
        }
    }
}
