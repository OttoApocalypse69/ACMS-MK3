//! Scripts in the app: where they are loaded from, how their commands and tools join the
//! app's own, and the Script Console.
//!
//! A script never touches the app directly. Its commands are entries in the registry that ask
//! for the script to be run; [`pump`] then runs it with a [`Host`] made of the app state and
//! the registry, so that everything the script does is a command or a transaction.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use acms_core::command::{Command, CommandRegistry, Params};
use acms_core::editor::Editor;
use acms_core::tool::{PointerEvent, Tool, ToolContext};
use acms_script::{DialogSpec, Engine, Host, Outcome, Registered, ScriptError, ToolPhase};
use egui::{Align, Key, Layout, RichText, ScrollArea, TextEdit, Ui};

use crate::commands;
use crate::state::{AppState, Dialog};

type Registry = CommandRegistry<AppState>;

/// Most lines the console keeps.
const MAX_CONSOLE_LINES: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// A line the user typed.
    Input,
    /// Something a script printed, or a note from the app.
    Output,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    pub kind: LineKind,
    pub text: String,
}

/// Something a command asked the scripts to do; done by [`pump`].
enum Request {
    /// Run the script command with this id.
    Command(String),
    /// Run a line typed into the console.
    Eval(String),
}

/// What a script tool saw on the canvas; passed on to its script by [`pump`].
enum ToolEvent {
    Pointer(String, ToolPhase, PointerEvent),
    Cancel,
}

type ToolEvents = Rc<RefCell<Vec<ToolEvent>>>;

/// A tool registered by a script, as the editor sees it: it only notes what happens, because
/// the script cannot be run from inside the editor.
struct ScriptTool {
    id: String,
    name: String,
    events: ToolEvents,
}

impl ScriptTool {
    fn note(&self, phase: ToolPhase, event: PointerEvent) {
        self.events
            .borrow_mut()
            .push(ToolEvent::Pointer(self.id.clone(), phase, event));
    }
}

impl Tool for ScriptTool {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn pointer_down(&mut self, _: &mut ToolContext<'_>, event: PointerEvent) {
        self.note(ToolPhase::Down, event);
    }

    fn pointer_move(&mut self, _: &mut ToolContext<'_>, event: PointerEvent) {
        self.note(ToolPhase::Move, event);
    }

    fn pointer_up(&mut self, _: &mut ToolContext<'_>, event: PointerEvent) {
        self.note(ToolPhase::Up, event);
    }

    fn cancel(&mut self, _: &mut ToolContext<'_>) {
        self.events.borrow_mut().push(ToolEvent::Cancel);
    }
}

/// The scripting side of the app state.
#[derive(Default)]
pub struct ScriptState {
    /// The Lua state. `None` while a script is running, and if Lua could not be started.
    engine: Option<Engine>,
    /// The user's scripts folder, if there is one.
    pub folder: Option<PathBuf>,
    pub console: Vec<ConsoleLine>,
    requests: Vec<Request>,
    tool_events: ToolEvents,
    /// The script commands that are in the registry now.
    commands: Vec<Registered>,
    /// The script tools that are in the editor now.
    tools: Vec<Registered>,
    /// `script.reload` was run: [`maintain`] loads everything again.
    reload_requested: bool,
    /// The folder a script is given when it asks for one, in place of the native dialog.
    #[cfg(test)]
    pub pick_folder: Option<PathBuf>,
}

impl ScriptState {
    /// The commands scripts registered, in the order they did.
    pub fn commands(&self) -> &[Registered] {
        &self.commands
    }

    fn say(&mut self, kind: LineKind, text: impl Into<String>) {
        // A message of several lines is several lines of the console.
        for line in text.into().lines() {
            self.console.push(ConsoleLine {
                kind,
                text: line.to_owned(),
            });
        }
        let extra = self.console.len().saturating_sub(MAX_CONSOLE_LINES);
        self.console.drain(..extra);
    }
}

/// What a running script reaches the app through.
struct AppHost<'a> {
    state: &'a mut AppState,
    registry: &'a Registry,
}

impl Host for AppHost<'_> {
    fn editor(&mut self) -> &mut Editor {
        &mut self.state.editor
    }

    fn run_command(&mut self, id: &str, params: &Params) -> Result<(), String> {
        let result = self.registry.run(id, self.state, params);
        self.state.sync_active_document();
        result.map_err(|err| err.to_string())
    }

    fn choose_folder(&mut self, title: &str) -> Option<PathBuf> {
        #[cfg(test)]
        {
            let _ = title;
            self.state.scripts.pick_folder.clone()
        }
        #[cfg(not(test))]
        {
            rfd::FileDialog::new().set_title(title).pick_folder()
        }
    }
}

/// Runs `action` with the engine and a host. `None` if there is no engine to run it with:
/// a script is running already, or Lua could not be started.
fn with_engine<R>(
    registry: &Registry,
    state: &mut AppState,
    action: impl FnOnce(&mut Engine, &mut AppHost<'_>) -> R,
) -> Option<R> {
    // The engine is taken out of the state while it runs, so the script can be given the
    // whole state to work on.
    let mut engine = state.scripts.engine.take()?;
    let mut host = AppHost { state, registry };
    let result = action(&mut engine, &mut host);
    for line in engine.take_output() {
        state.scripts.say(LineKind::Output, line);
    }
    state.scripts.engine = Some(engine);
    state.sync_active_document();
    Some(result)
}

/// Shows how a run ended: the dialog it wants, or the error it stopped with.
fn finish(state: &mut AppState, title: &str, result: Result<Outcome, ScriptError>) {
    match result {
        Ok(Outcome::Done) => {}
        Ok(Outcome::Dialog(spec)) => state.dialog = Some(Dialog::Script(spec)),
        Err(err) => fail(state, title, &err),
    }
}

fn fail(state: &mut AppState, title: &str, err: &ScriptError) {
    state.scripts.say(LineKind::Error, err.to_string());
    state.log.push(format!("Script: {err}"));
    state.dialog = Some(Dialog::Message {
        title: title.trim_end_matches('…').to_owned(),
        text: commands::sentence(err),
    });
}

/// Does what commands asked of the scripts, and passes canvas input on to script tools.
/// Returns whether anything was done.
pub fn pump(registry: &Registry, state: &mut AppState) -> bool {
    let mut busy = false;
    while state.scripts.engine.is_some() {
        if !state.scripts.requests.is_empty() {
            busy = true;
            match state.scripts.requests.remove(0) {
                Request::Command(id) => {
                    let title = registry
                        .get(&id)
                        .map_or(id.clone(), |c| c.name().to_owned());
                    let result = with_engine(registry, state, |engine, host| {
                        engine.run_command(host, &id)
                    });
                    if let Some(result) = result {
                        finish(state, &title, result);
                    }
                }
                Request::Eval(code) => {
                    let result =
                        with_engine(registry, state, |engine, host| engine.eval(host, &code));
                    match result {
                        // The console is where its errors belong; no dialog on top.
                        Some(Err(err)) => state.scripts.say(LineKind::Error, err.to_string()),
                        Some(result) => finish(state, "Script", result),
                        None => {}
                    }
                }
            }
            continue;
        }

        let events = std::mem::take(&mut *state.scripts.tool_events.borrow_mut());
        if events.is_empty() {
            break;
        }
        busy = true;
        for event in events {
            match event {
                ToolEvent::Pointer(tool, phase, event) => {
                    let result = with_engine(registry, state, |engine, host| {
                        engine.tool_event(host, &tool, phase, event)
                    });
                    if let Some(Err(err)) = result {
                        let title = state
                            .scripts
                            .tools
                            .iter()
                            .find(|known| known.id == tool)
                            .map_or(tool.clone(), |known| known.name.clone());
                        fail(state, &title, &err);
                    }
                }
                ToolEvent::Cancel => {
                    with_engine(registry, state, |engine, host| engine.tool_cancel(host));
                }
            }
        }
    }
    busy
}

fn registrations_changed(state: &AppState) -> bool {
    state.scripts.engine.as_ref().is_some_and(|engine| {
        engine.commands() != state.scripts.commands || engine.tools() != state.scripts.tools
    })
}

/// Gives a script the answer to the dialog it showed: the dialog as the user left it, or
/// `None` if they cancelled.
pub fn answer_dialog(registry: &Registry, state: &mut AppState, answer: Option<DialogSpec>) {
    let result = with_engine(registry, state, |engine, host| {
        engine.resume(host, answer.as_ref())
    });
    if let Some(result) = result {
        finish(state, "Script", result);
    }
    pump(registry, state);
}

/// The script files of a folder, by name.
fn files_in(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "lua"))
        .collect();
    files.sort();
    Ok(files)
}

/// Takes the commands and tools of scripts out of the app.
fn unregister(registry: &mut Registry, state: &mut AppState) {
    for command in std::mem::take(&mut state.scripts.commands) {
        registry.remove(&command.id);
    }
    for tool in std::mem::take(&mut state.scripts.tools) {
        registry.remove(&format!("tool.{}", tool.id));
        state.editor.remove_tool(&tool.id);
    }
}

/// Makes the registry and the editor's tools match what the scripts have registered.
fn register(registry: &mut Registry, state: &mut AppState) {
    let Some(engine) = &state.scripts.engine else {
        return;
    };
    let (commands, tools) = (engine.commands(), engine.tools());
    unregister(registry, state);

    for command in commands {
        let id = command.id.clone();
        let added = registry.register(Command::new(
            command.id.clone(),
            command.name.clone(),
            move |state: &mut AppState, _| {
                state.scripts.requests.push(Request::Command(id.clone()));
                Ok(())
            },
        ));
        match added {
            Ok(()) => state.scripts.commands.push(command),
            Err(err) => state
                .scripts
                .say(LineKind::Error, format!("Not added: {err}")),
        }
    }
    for tool in tools {
        let command_id = format!("tool.{}", tool.id);
        let script_tool = ScriptTool {
            id: tool.id.clone(),
            name: tool.name.clone(),
            events: state.scripts.tool_events.clone(),
        };
        if registry.get(&command_id).is_some() || !state.editor.add_tool(Box::new(script_tool)) {
            let text = format!("Not added: there is a tool `{}` already", tool.id);
            state.scripts.say(LineKind::Error, text);
            continue;
        }
        let id = tool.id.clone();
        let added = registry.register(Command::new(
            command_id,
            format!("Tool: {}", tool.name),
            move |state: &mut AppState, _| {
                state.editor.set_active_tool(&id);
                Ok(())
            },
        ));
        if added.is_ok() {
            state.scripts.tools.push(tool);
        }
    }
}

/// Throws the Lua state away and loads every script again: the examples that come with the
/// app, then the files in the user's scripts folder. A file there with the name of an example
/// takes its place.
pub fn reload(registry: &mut Registry, state: &mut AppState) {
    unregister(registry, state);
    // A stroke a script tool was in the middle of is taken back before its script goes.
    with_engine(registry, state, |engine, host| engine.tool_cancel(host));
    state.scripts.requests.clear();
    state.scripts.tool_events.borrow_mut().clear();
    if matches!(state.dialog, Some(Dialog::Script(_))) {
        state.dialog = None;
    }
    state.scripts.engine = match Engine::new() {
        Ok(engine) => Some(engine),
        Err(err) => {
            let text = format!("Scripting could not be started: {err}");
            state.scripts.say(LineKind::Error, text);
            None
        }
    };

    let mut sources: Vec<(String, String)> = Vec::new();
    let mut user_files = Vec::new();
    if let Some(folder) = state.scripts.folder.clone() {
        // The folder is made so that there is somewhere to put scripts.
        let listed = std::fs::create_dir_all(&folder).and_then(|()| files_in(&folder));
        match listed {
            Ok(files) => user_files = files,
            Err(err) => {
                let text = format!("Could not read {}: {err}", folder.display());
                state.scripts.say(LineKind::Error, text);
            }
        }
    }
    let name_of = |path: &Path| {
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    };
    for (name, code) in acms_script::BUNDLED {
        if !user_files.iter().any(|path| name_of(path) == name) {
            sources.push((name.to_owned(), code.to_owned()));
        }
    }
    for path in &user_files {
        match std::fs::read_to_string(path) {
            Ok(code) => sources.push((name_of(path), code)),
            Err(err) => {
                let text = format!("Could not read {}: {err}", path.display());
                state.scripts.say(LineKind::Error, text);
            }
        }
    }

    let empty = Registry::new();
    let mut loaded = 0;
    for (name, code) in &sources {
        // While they load, scripts register things; they cannot run commands yet.
        let result = with_engine(&empty, state, |engine, host| engine.load(host, name, code));
        match result {
            Some(Ok(())) => loaded += 1,
            Some(Err(err)) => {
                state.scripts.say(LineKind::Error, err.to_string());
                state.log.push(format!("Script: {err}"));
            }
            None => {}
        }
    }
    register(registry, state);

    let from = match &state.scripts.folder {
        Some(folder) => format!(" Your own go in {}", folder.display()),
        None => String::new(),
    };
    let text = format!("Loaded {loaded} of {} scripts.{from}", sources.len());
    state.scripts.say(LineKind::Output, text);
}

/// The part of scripting that changes the registry, so it cannot happen inside a command:
/// reloading, and taking in what a line in the console registered. Called once per frame.
pub fn maintain(registry: &mut Registry, state: &mut AppState) {
    if std::mem::take(&mut state.scripts.reload_requested) {
        reload(registry, state);
    } else if registrations_changed(state) {
        register(registry, state);
    }
}

pub fn register_commands(registry: &mut Registry) -> Result<(), acms_core::command::CommandError> {
    registry.register(Command::new(
        "script.reload",
        "Reload Scripts",
        |state: &mut AppState, _| {
            state.scripts.reload_requested = true;
            Ok(())
        },
    ))?;
    registry.register(
        Command::new(
            "script.run",
            "Run Script Line",
            |state: &mut AppState, params| {
                let code = params.text("code").unwrap_or_default().to_owned();
                state.scripts.say(LineKind::Input, format!("> {code}"));
                state.scripts.requests.push(Request::Eval(code));
                Ok(())
            },
        )
        .param("code", acms_core::command::ParamKind::Text),
    )?;
    registry.register(
        Command::new(
            "script.clear_console",
            "Clear Script Console",
            |state: &mut AppState, _| {
                state.scripts.console.clear();
                Ok(())
            },
        )
        .enabled_when(|state| !state.scripts.console.is_empty()),
    )?;
    Ok(())
}

/// The Script Console panel's own widget state.
#[derive(Default)]
pub struct ConsoleUi {
    /// The line being typed.
    input: String,
}

/// The Script Console: what scripts printed, and a line to run Lua.
pub fn show_console(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &Registry,
    console: &mut ConsoleUi,
) {
    let tokens = state.theme.tokens();
    let mut run = None;

    // The output scrolls in the space above the input line, which stays at the bottom.
    let row_height = ui.spacing().interact_size.y;
    let below = row_height + 3.0 * ui.spacing().item_spacing.y + 6.0;
    let output_height = (ui.available_height() - below).max(row_height);
    ScrollArea::vertical()
        .max_height(output_height)
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &state.scripts.console {
                let color = match line.kind {
                    LineKind::Input => tokens.text_muted,
                    LineKind::Output => tokens.text,
                    LineKind::Error => tokens.danger,
                };
                ui.label(RichText::new(&line.text).monospace().color(color));
            }
        });
    ui.separator();

    let row = egui::vec2(ui.available_width(), row_height);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        if ui.button("Clear").clicked() {
            run = Some(("script.clear_console", Params::new()));
        }
        if ui
            .button("Reload Scripts")
            .on_hover_text("Load every script again, without restarting")
            .clicked()
        {
            run = Some(("script.reload", Params::new()));
        }
        let run_clicked = ui.button("Run").clicked();
        let edit = ui.add(
            TextEdit::singleline(&mut console.input)
                .desired_width(f32::INFINITY)
                .hint_text("Lua, for example: app.active_sprite.width")
                .font(egui::TextStyle::Monospace),
        );
        let entered = edit.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        if (entered || run_clicked) && !console.input.trim().is_empty() {
            let code = std::mem::take(&mut console.input);
            run = Some(("script.run", Params::new().with("code", code)));
            edit.request_focus();
        }
    });
    if let Some((id, params)) = run {
        commands::run(registry, state, id, &params);
    }
}

#[cfg(test)]
mod tests {
    use acms_core::color::Rgba8;
    use acms_core::geom::{Point, Rect};
    use acms_core::selection::Mask;
    use acms_core::sprite::{LayerId, Sprite};
    use acms_core::tool::{Modifiers, PointerButton};
    use acms_script::FieldKind;

    use super::*;
    use crate::commands::testing::{Fixture, RED};

    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const CLEAR: Rgba8 = Rgba8::TRANSPARENT;

    /// A folder in the OS temp directory that is deleted, with what is in it, when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        /// `name` must be unique among the tests, which run in parallel.
        fn new(name: &str) -> Self {
            let folder = format!("acms-test-{}-{name}", std::process::id());
            let path = std::env::temp_dir().join(folder);
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, contents: &str) {
            std::fs::write(self.0.join(name), contents).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A 16 × 16 sprite open and the bundled scripts loaded.
    fn scripted() -> Fixture {
        let mut f = Fixture::with_sprite();
        reload(&mut f.registry, &mut f.state);
        f
    }

    /// The same, with scripts of the user's own.
    fn with_scripts(folder: &TempDir) -> Fixture {
        let mut f = Fixture::with_sprite();
        f.state.scripts.folder = Some(folder.0.clone());
        reload(&mut f.registry, &mut f.state);
        f
    }

    fn steps(f: &Fixture) -> Vec<String> {
        f.document().history().map(str::to_owned).collect()
    }

    fn sprite(f: &Fixture) -> Sprite {
        f.document().sprite().clone()
    }

    fn undo(f: &mut Fixture) {
        assert!(f.state.editor.document_mut().unwrap().undo());
    }

    fn errors(f: &Fixture) -> Vec<&str> {
        f.state
            .scripts
            .console
            .iter()
            .filter(|line| line.kind == LineKind::Error)
            .map(|line| line.text.as_str())
            .collect()
    }

    fn printed(f: &Fixture) -> Vec<&str> {
        f.state
            .scripts
            .console
            .iter()
            .filter(|line| line.kind == LineKind::Output)
            .map(|line| line.text.as_str())
            .collect()
    }

    fn lua(f: &mut Fixture, code: &str) {
        f.run_with("script.run", Params::new().with("code", code));
    }

    /// Answers the script dialog that is open, after `change` has edited its fields.
    fn answer(f: &mut Fixture, change: impl FnOnce(&mut DialogSpec)) {
        let Some(Dialog::Script(mut spec)) = f.state.dialog.take() else {
            panic!("expected a script dialog, found {:?}", f.state.dialog);
        };
        change(&mut spec);
        answer_dialog(&f.registry, &mut f.state, Some(spec));
    }

    fn set_color(spec: &mut DialogSpec, id: &str, color: Rgba8) {
        let field = spec.fields.iter_mut().find(|field| field.id == id).unwrap();
        field.kind = FieldKind::Color(color);
    }

    /// Makes the sprite two frames long, the second a linked copy of the first, and adds a
    /// second layer with a line of its own.
    fn animate(f: &mut Fixture) {
        f.draw_line(4);
        f.run("frame.duplicate");
        let document = f.state.editor.document_mut().unwrap();
        document
            .transact("Link", |tx| tx.link_cels(LayerId(1), 0, &[1]))
            .unwrap();
        f.run_with("frame.goto", Params::new().with("frame", 0_i64));
        f.run("layer.new");
        f.draw_line(8);
        f.run_with("layer.select", Params::new().with("layer", 1_i64));
    }

    #[test]
    fn every_example_is_a_command_the_palette_lists() {
        let f = scripted();
        assert_eq!(errors(&f), [] as [&str; 0]);
        for (id, name) in [
            ("example.outline", "Outline"),
            ("example.palette_swap", "Palette Swap…"),
            ("example.mirror_sprite", "Mirror Sprite"),
            ("example.export_tags", "Export Every Tag as a GIF…"),
            ("tool.spray", "Tool: Spray"),
        ] {
            let command = f.registry.get(id).unwrap_or_else(|| panic!("no `{id}`"));
            assert_eq!(command.name(), name);
            // What the palette and the menus need of a command.
            assert!(command.runs_without_params(), "{id}");
            assert!(command.is_enabled(&f.state), "{id}");
        }
        assert!(
            f.state
                .editor
                .tools()
                .any(|tool| tool == ("spray", "Spray"))
        );
        assert!(printed(&f)[0].starts_with("Loaded 5 of 5 scripts."));
    }

    #[test]
    fn outline_draws_around_the_picture_as_one_undo_step() {
        let mut f = scripted();
        f.draw_line(4);
        f.state.editor.primary = BLUE;
        let before = sprite(&f);

        f.run("example.outline");
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(f.state.dialog, None);
        for x in 2..=10 {
            assert_eq!(f.pixel(x, 4), RED);
            assert_eq!(f.pixel(x, 3), BLUE);
            assert_eq!(f.pixel(x, 5), BLUE);
        }
        assert_eq!((f.pixel(1, 4), f.pixel(11, 4)), (BLUE, BLUE));
        // Corners are not part of a one-pixel outline, and nothing else was touched.
        assert_eq!(
            (f.pixel(1, 3), f.pixel(11, 5), f.pixel(5, 6)),
            (CLEAR, CLEAR, CLEAR)
        );
        assert_eq!(steps(&f), ["Pencil", "Outline"]);
        let after = sprite(&f);

        undo(&mut f);
        assert_eq!(sprite(&f), before);
        f.run("edit.redo");
        assert_eq!(sprite(&f), after);
    }

    #[test]
    fn a_filter_stays_inside_the_selection() {
        let mut f = scripted();
        f.draw_line(4);
        f.state.editor.primary = BLUE;
        let area = Mask::from_rect(16, 16, Rect::new(0, 0, 6, 4));
        let document = f.state.editor.document_mut().unwrap();
        document
            .transact("Select", |tx| tx.set_selection(Some(area)))
            .unwrap();
        f.run("example.outline");
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!((f.pixel(2, 3), f.pixel(5, 3)), (BLUE, BLUE));
        assert_eq!(
            (f.pixel(6, 3), f.pixel(3, 5), f.pixel(1, 4)),
            (CLEAR, CLEAR, CLEAR)
        );
    }

    #[test]
    fn palette_swap_asks_then_recolours_every_cel_as_one_undo_step() {
        let mut f = scripted();
        animate(&mut f);
        let before = sprite(&f);
        let history = steps(&f).len();

        f.run("example.palette_swap");
        let Some(Dialog::Script(spec)) = &f.state.dialog else {
            panic!("no dialog: {:?}", errors(&f));
        };
        assert_eq!(spec.title, "Palette Swap");
        let labels: Vec<&str> = spec.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["Replace", "With", "Swap both ways", "Frames"]);
        // The dialog starts from the two colours in use.
        assert_eq!(spec.fields[0].kind, FieldKind::Color(RED));
        // Nothing happens before the dialog is answered.
        assert_eq!(sprite(&f), before);

        answer(&mut f, |spec| {
            set_color(spec, "to", BLUE);
            // With linked cels, swapping both ways would undo itself if a picture were
            // changed once per frame it is linked to.
            spec.fields[2].kind = FieldKind::Check(true);
        });
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(f.state.dialog, None);
        let document = f.document();
        for frame in 0..2 {
            assert_eq!(document.sprite().pixel(LayerId(1), frame, 5, 4), BLUE);
        }
        assert_eq!(document.sprite().pixel(LayerId(2), 0, 5, 8), BLUE);
        assert_eq!(steps(&f).len(), history + 1);
        assert_eq!(steps(&f).last().map(String::as_str), Some("Palette Swap"));
        assert_eq!(printed(&f).last(), Some(&"Palette Swap: 27 pixels changed"));

        undo(&mut f);
        assert_eq!(sprite(&f), before);
    }

    #[test]
    fn cancelling_a_script_dialog_changes_nothing() {
        let mut f = scripted();
        f.draw_line(4);
        let before = sprite(&f);
        f.run("example.palette_swap");
        assert!(matches!(f.state.dialog, Some(Dialog::Script(_))));
        f.state.dialog = None;
        answer_dialog(&f.registry, &mut f.state, None);
        assert_eq!(sprite(&f), before);
        assert_eq!(steps(&f), ["Pencil"]);
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(f.state.dialog, None);
    }

    #[test]
    fn palette_swap_can_keep_to_the_frame_being_shown() {
        let mut f = scripted();
        f.draw_line(4);
        f.run("frame.new");
        f.draw_line(6);
        f.run("example.palette_swap");
        answer(&mut f, |spec| {
            set_color(spec, "to", BLUE);
            let FieldKind::Dropdown { selected, .. } = &mut spec.fields[3].kind else {
                panic!("not a dropdown");
            };
            *selected = 1;
        });
        assert_eq!(errors(&f), [] as [&str; 0]);
        let sprite = f.document().sprite();
        assert_eq!(sprite.pixel(LayerId(1), 1, 5, 6), BLUE);
        assert_eq!(sprite.pixel(LayerId(1), 0, 5, 4), RED);
    }

    #[test]
    fn mirror_sprite_flips_every_layer_and_frame_as_one_undo_step() {
        let mut f = scripted();
        animate(&mut f);
        // Something that is not symmetric, on a layer that is locked: it stays.
        f.run("layer.new");
        f.draw_line(12);
        f.run("layer.set_locked");
        let before = sprite(&f);
        let history = steps(&f).len();

        f.run("example.mirror_sprite");
        assert_eq!(errors(&f), [] as [&str; 0]);
        // The line ran from x = 2 to 10 of 16; mirrored it runs from 5 to 13.
        let sprite_now = sprite(&f);
        for frame in 0..2 {
            let row = |x| sprite_now.pixel(LayerId(1), frame, x, 4);
            assert_eq!((row(4), row(5), row(13), row(14)), (CLEAR, RED, RED, CLEAR));
        }
        assert_eq!(sprite_now.pixel(LayerId(2), 0, 13, 8), RED);
        assert_eq!(sprite_now.pixel(LayerId(2), 0, 2, 8), CLEAR);
        assert_eq!(sprite_now.pixel(LayerId(3), 0, 2, 12), RED);
        assert_eq!(steps(&f).len(), history + 1);
        assert_eq!(steps(&f).last().map(String::as_str), Some("Mirror Sprite"));

        undo(&mut f);
        assert_eq!(sprite(&f), before);
        // Mirroring twice is where it started.
        f.run("example.mirror_sprite");
        f.run("example.mirror_sprite");
        assert_eq!(sprite(&f), before);
    }

    #[test]
    fn export_tags_writes_a_gif_per_tag_into_the_chosen_folder() {
        let folder = TempDir::new("export-tags");
        let mut f = scripted();
        f.draw_line(4);
        for _ in 0..3 {
            f.run("frame.duplicate");
        }
        let tag = |name: &str, from: i64, to: i64| {
            Params::new()
                .with("name", name)
                .with("from", from)
                .with("to", to)
        };
        f.run_with("tag.new", tag("walk", 0, 2));
        f.run_with("tag.new", tag("a/b: c", 3, 3));
        f.run_with("tag.new", tag("walk", 1, 2));
        let before = sprite(&f);
        let history = steps(&f);

        // The user is asked for the folder, and may decline.
        f.run("example.export_tags");
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);

        f.state.scripts.pick_folder = Some(folder.0.clone());
        f.run("example.export_tags");
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(f.state.dialog, None);
        let frames_of = |name: &str| {
            let file = std::fs::File::open(folder.0.join(name)).unwrap();
            let mut decoder = gif::DecodeOptions::new().read_info(file).unwrap();
            let mut count = 0;
            while decoder.read_next_frame().unwrap().is_some() {
                count += 1;
            }
            count
        };
        assert_eq!(frames_of("walk.gif"), 3);
        assert_eq!(frames_of("a_b_ c.gif"), 1);
        assert_eq!(frames_of("walk_3.gif"), 2);
        assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 3);
        assert!(
            printed(&f)
                .last()
                .unwrap()
                .starts_with("Exported 3 GIFs to ")
        );
        // Exporting changes nothing in the sprite.
        assert_eq!(sprite(&f), before);
        assert_eq!(steps(&f), history);
    }

    #[test]
    fn a_script_that_stops_with_a_message_shows_it() {
        let mut f = scripted();
        f.run("example.export_tags");
        let Some(Dialog::Message { title, text }) = &f.state.dialog else {
            panic!("no message");
        };
        assert_eq!(title, "Export Every Tag as a GIF");
        assert_eq!(text, "This sprite has no tags.");
        assert_eq!(errors(&f), ["This sprite has no tags."]);

        // Without a sprite the examples say so instead of failing somewhere inside.
        f.state.dialog = None;
        f.run("file.close");
        for id in [
            "example.outline",
            "example.palette_swap",
            "example.mirror_sprite",
        ] {
            f.run(id);
            assert!(
                matches!(&f.state.dialog, Some(Dialog::Message { text, .. }) if text.contains("sprite")),
                "{id}: {:?}",
                f.state.dialog
            );
            f.state.dialog = None;
        }
    }

    fn spray_at(f: &mut Fixture, button: PointerButton, path: &[(i32, i32)]) {
        let event = |&(x, y): &(i32, i32)| PointerEvent {
            position: Point::new(x, y),
            button,
            modifiers: Modifiers::default(),
        };
        f.state.editor.pointer_down(event(&path[0]));
        pump(&f.registry, &mut f.state);
        for point in &path[1..] {
            f.state.editor.pointer_move(event(point));
            pump(&f.registry, &mut f.state);
        }
        f.state.editor.pointer_up(event(&path[path.len() - 1]));
        pump(&f.registry, &mut f.state);
    }

    fn painted(f: &Fixture) -> Vec<(i32, i32, Rgba8)> {
        let mut pixels = Vec::new();
        for y in 0..16 {
            for x in 0..16 {
                if f.pixel(x, y) != CLEAR {
                    pixels.push((x, y, f.pixel(x, y)));
                }
            }
        }
        pixels
    }

    #[test]
    fn the_spray_tool_scatters_dots_as_one_undo_step() {
        let mut f = scripted();
        f.run("tool.spray");
        assert_eq!(f.state.editor.active_tool(), "spray");
        f.state.editor.primary = RED;
        f.state.editor.secondary = BLUE;
        let before = sprite(&f);

        spray_at(
            &mut f,
            PointerButton::Primary,
            &[(8, 8), (8, 8), (8, 8), (8, 8)],
        );
        assert_eq!(errors(&f), [] as [&str; 0]);
        let dots = painted(&f);
        // Four events of six dots each, some landing on the same pixel.
        assert!((2..=24).contains(&dots.len()), "{} dots", dots.len());
        for &(x, y, color) in &dots {
            assert_eq!(color, RED);
            let (dx, dy) = (f64::from(x) - 8.0, f64::from(y) - 8.0);
            assert!(dx.hypot(dy) <= 5.0, "a dot at {x}, {y}");
        }
        assert_eq!(steps(&f), ["Spray"]);
        undo(&mut f);
        assert_eq!(sprite(&f), before);

        // The other button sprays the other colour.
        spray_at(&mut f, PointerButton::Secondary, &[(3, 3), (4, 3)]);
        assert!(painted(&f).iter().all(|&(_, _, color)| color == BLUE));
        assert!(!painted(&f).is_empty());
        assert_eq!(steps(&f), ["Spray"]);
    }

    #[test]
    fn a_cancelled_or_refused_spray_leaves_nothing() {
        let mut f = scripted();
        f.run("tool.spray");
        let before = sprite(&f);
        let at = PointerEvent {
            position: Point::new(8, 8),
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        };
        f.state.editor.pointer_down(at);
        pump(&f.registry, &mut f.state);
        assert!(!painted(&f).is_empty());
        // Switching tools abandons the stroke, like Escape does.
        f.run("tool.pencil");
        assert_eq!(sprite(&f), before);
        assert!(!f.document().in_group());
        assert!(steps(&f).is_empty());

        // On a locked layer the tool is told no, once, and draws nothing.
        f.run("tool.spray");
        f.run("layer.set_locked");
        let locked = sprite(&f);
        spray_at(&mut f, PointerButton::Primary, &[(8, 8), (9, 8), (10, 8)]);
        assert_eq!(sprite(&f), locked);
        assert_eq!(errors(&f), ["spray.lua:23: the layer is locked"]);
        assert!(matches!(&f.state.dialog, Some(Dialog::Message { title, .. }) if title == "Spray"));
        assert!(!f.document().in_group());
    }

    #[test]
    fn the_console_runs_lua_and_shows_what_happened() {
        let mut f = scripted();
        f.state.scripts.console.clear();
        lua(&mut f, "app.active_sprite.width * 2");
        lua(&mut f, "nothing()");
        lua(&mut f, "print('a\\nb')");
        let lines: Vec<(LineKind, &str)> = f
            .state
            .scripts
            .console
            .iter()
            .map(|line| (line.kind, line.text.as_str()))
            .collect();
        assert_eq!(lines[0], (LineKind::Input, "> app.active_sprite.width * 2"));
        assert_eq!(lines[1], (LineKind::Output, "32"));
        assert_eq!(lines[2], (LineKind::Input, "> nothing()"));
        assert_eq!(lines[3].0, LineKind::Error);
        assert!(lines[3].1.contains("nothing"), "{}", lines[3].1);
        assert_eq!(
            lines[5..],
            [(LineKind::Output, "a"), (LineKind::Output, "b")]
        );
        // A mistake in the console is not worth a dialog.
        assert_eq!(f.state.dialog, None);

        assert!(f.is_enabled("script.clear_console"));
        f.run("script.clear_console");
        assert!(f.state.scripts.console.is_empty());
        assert!(!f.is_enabled("script.clear_console"));
    }

    #[test]
    fn scripts_run_the_apps_own_commands() {
        let mut f = scripted();
        f.draw_line(4);
        let before = sprite(&f);
        let code = "app.transaction('Both', function() \
                    app.command('layer.new') \
                    app.command('color.set_primary', { color = '#0000ff' }) \
                    app.active_cel:set_pixel(0, 0, app.fg_color) \
                    app.command('select.all') end)";
        lua(&mut f, code);
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(f.state.editor.primary, BLUE);
        assert_eq!(f.document().sprite().layer_count(), 2);
        assert_eq!(f.pixel_on(LayerId(2), 0, 0), BLUE);
        // A new layer, a pixel and a selection: one step.
        assert_eq!(steps(&f), ["Pencil", "Both"]);
        undo(&mut f);
        assert_eq!(sprite(&f), before);

        // What a command refuses comes back as an error the script can catch.
        lua(&mut f, "print(pcall(app.command, 'no.such'))");
        assert_eq!(
            printed(&f).last(),
            Some(&"false\tconsole:1: unknown command `no.such`")
        );
        lua(
            &mut f,
            "print(pcall(app.command, 'brush.set_size', { size = 'big' }))",
        );
        assert!(
            printed(&f)
                .last()
                .unwrap()
                .contains("`size` must be a whole number")
        );
        // Undo from a script works like the user's.
        f.run("edit.redo");
        lua(&mut f, "app.command('edit.undo')");
        assert_eq!(sprite(&f), before);
    }

    #[test]
    fn a_script_cannot_name_files_it_was_not_given() {
        let folder = TempDir::new("no-access");
        let target = folder.0.join("stolen.acms");
        let path = target.to_string_lossy().replace('\\', "/");
        let mut f = scripted();
        lua(
            &mut f,
            &format!("app.command('file.save_as', {{ path = '{path}' }})"),
        );
        assert!(
            errors(&f)[0].contains("app.choose_folder"),
            "{:?}",
            errors(&f)
        );
        assert!(!target.exists());
        // Without a path the command asks the user, as it always does.
        lua(&mut f, "app.command('file.save_as')");
        assert_eq!(
            f.state.file_request,
            Some(crate::state::FileRequest::SaveAs)
        );

        // Once the user has picked the folder, the same call goes through.
        f.state.scripts.pick_folder = Some(folder.0.clone());
        let code = "local folder = app.choose_folder('Save where?') \
                    app.command('file.save_as', { path = app.join_path(folder, 'mine.acms') })";
        lua(&mut f, code);
        assert_eq!(errors(&f).len(), 1);
        assert!(folder.0.join("mine.acms").exists());
    }

    #[test]
    fn the_users_scripts_are_loaded_and_reloaded() {
        let folder = TempDir::new("user-scripts");
        folder.write(
            "dot.lua",
            "register_command{ id = 'my.dot', name = 'Dot', run = function()
                app.transaction('Dot', function() app.active_cel:set_pixel(1, 1, '#ff0000') end)
            end }",
        );
        folder.write(
            "broken.lua",
            "register_command{ id = 'my.half', run = function() end }\nerror('oops')",
        );
        folder.write("notes.txt", "not a script");
        let mut f = with_scripts(&folder);
        assert_eq!(errors(&f), ["broken.lua:2: oops"]);
        assert!(printed(&f)[0].starts_with("Loaded 6 of 7 scripts."));
        assert!(f.registry.get("my.half").is_none());
        // The examples are still there, next to the user's own.
        assert!(f.registry.get("example.outline").is_some());
        f.run("my.dot");
        assert_eq!(f.pixel(1, 1), RED);

        // The file changes; nothing happens until the scripts are reloaded.
        folder.write(
            "dot.lua",
            "register_command{ id = 'my.dot', name = 'Blue Dot', run = function()
                app.transaction('Dot', function() app.active_cel:set_pixel(2, 2, '#0000ff') end)
            end }
            register_tool{ id = 'mine', name = 'Mine' }",
        );
        std::fs::remove_file(folder.0.join("broken.lua")).unwrap();
        f.state.scripts.console.clear();
        f.run("my.dot");
        assert_eq!(f.pixel(2, 2), CLEAR);
        f.run("script.reload");
        maintain(&mut f.registry, &mut f.state);
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(
            f.registry.get("my.dot").map(Command::name),
            Some("Blue Dot")
        );
        f.run("my.dot");
        assert_eq!(f.pixel(2, 2), BLUE);
        f.run("tool.mine");
        assert_eq!(f.state.editor.active_tool(), "mine");

        // A script that is gone takes its command and its tool with it.
        std::fs::remove_file(folder.0.join("dot.lua")).unwrap();
        f.run("script.reload");
        maintain(&mut f.registry, &mut f.state);
        assert!(f.registry.get("my.dot").is_none());
        assert!(f.registry.get("tool.mine").is_none());
        assert_eq!(f.state.editor.active_tool(), "pencil");
        assert!(f.registry.get("example.outline").is_some());
        // Reloading leaves the sprite and its history alone.
        assert_eq!(steps(&f), ["Dot", "Dot"]);
    }

    #[test]
    fn a_script_of_the_users_replaces_the_example_of_the_same_name() {
        let folder = TempDir::new("override");
        folder.write(
            "outline.lua",
            "register_filter{ id = 'example.outline', name = 'My Outline', run = function() end }",
        );
        let f = with_scripts(&folder);
        assert_eq!(errors(&f), [] as [&str; 0]);
        assert_eq!(
            f.registry.get("example.outline").map(Command::name),
            Some("My Outline")
        );
    }

    #[test]
    fn scripts_cannot_take_the_ids_of_built_in_commands_or_tools() {
        let folder = TempDir::new("clash");
        folder.write(
            "clash.lua",
            "register_command{ id = 'file.new', name = 'Mine', run = function() end }
             register_tool{ id = 'pencil', name = 'Mine' }
             register_command{ id = 'my.fine', run = function() end }",
        );
        let f = with_scripts(&folder);
        assert_eq!(
            errors(&f),
            [
                "Not added: command `file.new` is already registered",
                "Not added: there is a tool `pencil` already"
            ]
        );
        assert_eq!(
            f.registry.get("file.new").map(Command::name),
            Some("New Sprite…")
        );
        assert_eq!(
            f.state
                .editor
                .tools()
                .filter(|(id, _)| *id == "pencil")
                .count(),
            1
        );
        assert!(f.registry.get("my.fine").is_some());
    }

    #[test]
    fn a_command_registered_in_the_console_joins_the_others() {
        let mut f = scripted();
        lua(
            &mut f,
            "register_command{ id = 'my.live', name = 'Live', run = function() print('ran') end }",
        );
        assert!(f.registry.get("my.live").is_none());
        maintain(&mut f.registry, &mut f.state);
        assert_eq!(f.registry.get("my.live").map(Command::name), Some("Live"));
        f.run("my.live");
        assert_eq!(printed(&f).last(), Some(&"ran"));
        // The examples kept their places.
        assert!(f.registry.get("example.outline").is_some());
        assert!(f.registry.get("tool.spray").is_some());
    }

    #[test]
    fn reloading_in_the_middle_of_a_script_stroke_takes_the_stroke_back() {
        let mut f = scripted();
        f.run("tool.spray");
        let before = sprite(&f);
        let at = PointerEvent {
            position: Point::new(8, 8),
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        };
        f.state.editor.pointer_down(at);
        pump(&f.registry, &mut f.state);
        assert!(f.document().in_group());
        f.run("script.reload");
        maintain(&mut f.registry, &mut f.state);
        assert!(!f.document().in_group());
        assert_eq!(sprite(&f), before);
        // The tool was registered again by the reload, and the pencil took over meanwhile.
        assert!(f.registry.get("tool.spray").is_some());
        assert_eq!(f.state.editor.active_tool(), "pencil");
        f.draw_line(2);
        assert_eq!(steps(&f), ["Pencil"]);
    }

    #[test]
    fn a_script_that_never_ends_does_not_hang_the_app() {
        let mut f = scripted();
        if let Some(engine) = &mut f.state.scripts.engine {
            engine.set_time_limit(std::time::Duration::from_millis(50));
        }
        lua(
            &mut f,
            "app.transaction('Stuck', function() app.active_cel:set_pixel(0, 0, '#ff0000') while true do end end)",
        );
        assert!(errors(&f)[0].contains("took too long"), "{:?}", errors(&f));
        // What it had done is taken back, and the app goes on.
        assert_eq!(f.pixel(0, 0), CLEAR);
        assert!(!f.document().in_group());
        f.draw_line(2);
        assert_eq!(steps(&f), ["Pencil"]);
    }

    /// The Lua examples in the API description, in order.
    fn documented_examples() -> Vec<String> {
        let text = include_str!("../../../scripts/API.md").replace("\r\n", "\n");
        let mut blocks = Vec::new();
        let mut rest = text.as_str();
        while let Some((_, after)) = rest.split_once("```lua\n") {
            let (block, after) = after.split_once("```").unwrap();
            blocks.push(block.to_owned());
            rest = after;
        }
        blocks
    }

    #[test]
    fn the_examples_in_the_api_description_run() {
        let folder = TempDir::new("api-examples");
        let mut f = scripted();
        f.state.scripts.pick_folder = Some(folder.0.clone());
        let blocks = documented_examples();
        assert!(blocks.len() >= 9, "{} examples found", blocks.len());
        // The first one is three lines typed into the console, one after the other.
        for line in blocks[0].lines() {
            lua(&mut f, line);
        }
        assert_eq!(printed(&f)[1..], ["16", "1	Layer 1	image"]);
        for block in &blocks[1..] {
            lua(&mut f, block);
            // The example that shows a dialog gets its answer.
            if matches!(f.state.dialog, Some(Dialog::Script(_))) {
                answer(&mut f, |_| {});
            }
            assert_eq!(errors(&f), [] as [&str; 0], "in:\n{block}");
            assert_eq!(f.state.dialog, None, "in:\n{block}");
        }
        // The dialog example printed what was answered.
        assert!(
            printed(&f).contains(&"8\t#ff8000\tgrid\tfalse\tThis frame"),
            "{:?}",
            printed(&f)
        );
        assert!(folder.0.join("sprite.png").exists());

        // What the examples registered works too.
        maintain(&mut f.registry, &mut f.state);
        f.run("tool.pencil");
        f.draw_line(0);
        f.run("my.clear_corner");
        assert_eq!(f.state.dialog, None);
        f.run("my.invert");
        let top = f.document().active_layer();
        assert_eq!(f.pixel_on(top, 5, 0), Rgba8::rgb(0, 255, 255));
        f.run("tool.my_dots");
        f.state.editor.primary = BLUE;
        spray_at(&mut f, PointerButton::Primary, &[(3, 9), (4, 9)]);
        assert_eq!(f.pixel_on(top, 4, 9), BLUE);
        assert_eq!(errors(&f), [] as [&str; 0]);
    }
}
