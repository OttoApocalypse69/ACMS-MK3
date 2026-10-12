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
    // The input line stays at the bottom; the output above it scrolls.
    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
        ui.separator();
        ScrollArea::vertical()
            .auto_shrink(false)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                    for line in &state.scripts.console {
                        let color = match line.kind {
                            LineKind::Input => tokens.text_muted,
                            LineKind::Output => tokens.text,
                            LineKind::Error => tokens.danger,
                        };
                        ui.label(RichText::new(&line.text).monospace().color(color));
                    }
                });
            });
    });
    if let Some((id, params)) = run {
        commands::run(registry, state, id, &params);
    }
}
