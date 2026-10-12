//! ACMS scripting: the Lua host and API bindings.
//!
//! An [`Engine`] is one Lua state with the scripting API in it. It knows nothing about the
//! application: whatever a script asks for goes through the [`Host`] it is given for the
//! duration of each call. The API itself is described in `scripts/API.md`.
//!
//! Scripts are sandboxed. They have Lua's `string`, `table`, `math`, `utf8` and `coroutine`
//! libraries and nothing that reaches the machine: no `os`, `io`, `debug` or `package`, no way
//! to load files or bytecode. A script that runs too long is stopped, and so is one that
//! takes too much memory. Files can only be named inside a folder the user picked.

mod api;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use acms_core::color::Rgba8;
use acms_core::command::Params;
use acms_core::editor::Editor;
use acms_core::tool::{PointerButton, PointerEvent};
use mlua::chunk::ChunkMode;
use mlua::thread::ThreadStatus;
use mlua::{
    Function, HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Table, Thread, Value, VmState,
};

use api::{Group, Session};

/// The example scripts that come with the application, as file name and source.
pub const BUNDLED: [(&str, &str); 5] = [
    ("outline.lua", include_str!("../../../scripts/outline.lua")),
    (
        "palette_swap.lua",
        include_str!("../../../scripts/palette_swap.lua"),
    ),
    (
        "mirror_sprite.lua",
        include_str!("../../../scripts/mirror_sprite.lua"),
    ),
    (
        "export_tags.lua",
        include_str!("../../../scripts/export_tags.lua"),
    ),
    ("spray.lua", include_str!("../../../scripts/spray.lua")),
];

/// How long a script may run before it is stopped, unless changed.
const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(10);

/// Most memory the Lua state may use.
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// How many Lua instructions run between two looks at the clock.
const INSTRUCTIONS_PER_CHECK: u32 = 10_000;

/// What scripts reach the application through.
pub trait Host {
    /// The editor: the open sprites, the colours, the tools.
    fn editor(&mut self) -> &mut Editor;

    /// Runs one of the application's commands. The error is the reason it could not run.
    fn run_command(&mut self, id: &str, params: &Params) -> Result<(), String>;

    /// Asks the user for a folder the script may then name files in. `None` if they decline.
    fn choose_folder(&mut self, title: &str) -> Option<PathBuf>;
}

/// Why a script stopped. The text names the file and line where it can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError(pub String);

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ScriptError {}

impl From<mlua::Error> for ScriptError {
    fn from(err: mlua::Error) -> Self {
        Self(describe(&err))
    }
}

/// A command or a tool a script registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub id: String,
    pub name: String,
}

/// A dialog a script wants shown.
#[derive(Clone, Debug, PartialEq)]
pub struct DialogSpec {
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The name the script reads the value by.
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
}

/// The kind of a dialog field, with its value.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Number {
        value: f64,
        min: f64,
        max: f64,
        /// Digits after the point; 0 for whole numbers.
        decimals: u8,
    },
    Color(Rgba8),
    Text(String),
    Check(bool),
    Dropdown {
        options: Vec<String>,
        selected: usize,
    },
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done,
    /// The script waits for this dialog; give the answer to [`Engine::resume`].
    Dialog(DialogSpec),
}

/// The part of a pointer interaction a script tool is told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPhase {
    Down,
    Move,
    Up,
}

/// The stroke of a script tool that is going on.
struct Stroke {
    tool: String,
    /// The tool raised an error: the rest of the stroke is ignored.
    failed: bool,
}

pub struct Engine {
    lua: Lua,
    /// What `prelude.lua` returned: the functions and tables of the Lua half.
    internal: Table,
    /// Where lines typed into the console keep their variables.
    console: Table,
    session: Session,
    /// A script that showed a dialog and waits for the answer.
    pending: Option<Thread>,
    stroke: Option<Stroke>,
    time_limit: Duration,
}

impl Engine {
    /// A fresh Lua state with the API in it and no scripts loaded.
    pub fn new() -> Result<Self, ScriptError> {
        let libs = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::new())?;
        lua.set_memory_limit(MEMORY_LIMIT)?;

        let internal: Table = lua
            .load(include_str!("prelude.lua"))
            .set_name("=[acms]")
            .set_mode(ChunkMode::Text)
            .call(())?;
        lua.set_named_registry_value(api::COLOR_META, internal.get::<Table>("color")?)?;
        lua.globals()
            .set("Color", lua.create_function(api::color_constructor)?)?;
        let console: Table = internal.get::<Function>("environment")?.call(())?;

        // The clock is looked at every so often while Lua code runs; coroutines a script
        // makes inherit the hook.
        let deadline = Rc::new(Cell::new(Instant::now() + DEFAULT_TIME_LIMIT));
        let limit = deadline.clone();
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(INSTRUCTIONS_PER_CHECK),
            move |_, _| {
                if Instant::now() > limit.get() {
                    Err(mlua::Error::runtime(
                        "the script took too long and was stopped",
                    ))
                } else {
                    Ok(VmState::Continue)
                }
            },
        )?;

        Ok(Self {
            lua,
            internal,
            console,
            session: Session {
                output: Vec::new(),
                granted: Vec::new(),
                group: None,
                may_ask: false,
                deadline,
            },
            pending: None,
            stroke: None,
            time_limit: DEFAULT_TIME_LIMIT,
        })
    }

    /// Changes how long a script may run before it is stopped.
    pub fn set_time_limit(&mut self, limit: Duration) {
        self.time_limit = limit;
    }

    /// The lines scripts printed since the last call.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.session.output)
    }

    fn registered(&self, list: &str) -> Vec<Registered> {
        let Ok(list) = self.internal.get::<Table>(list) else {
            return Vec::new();
        };
        list.sequence_values::<Table>()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                Some(Registered {
                    id: entry.get("id").ok()?,
                    name: entry.get("name").ok()?,
                })
            })
            .collect()
    }

    /// The commands (and filters) scripts registered, in the order they did.
    pub fn commands(&self) -> Vec<Registered> {
        self.registered("commands")
    }

    /// The tools scripts registered, in the order they did.
    pub fn tools(&self) -> Vec<Registered> {
        self.registered("tools")
    }

    /// Whether a script waits for the answer to a dialog.
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Runs Lua code with the host connected, and connects nothing to it afterwards.
    fn enter<R>(
        &mut self,
        host: &mut dyn Host,
        may_ask: bool,
        run: impl FnOnce() -> mlua::Result<R>,
    ) -> Result<R, ScriptError> {
        self.session.may_ask = may_ask;
        self.session.deadline.set(Instant::now() + self.time_limit);
        let set_host: Function = self.internal.get("set_host")?;
        let session = &mut self.session;
        let result = self.lua.scope(|scope| {
            let call =
                scope.create_function_mut(|lua, (op, args): (mlua::LuaString, MultiValue)| {
                    let op = op.to_str()?;
                    api::dispatch(lua, &mut *host, &mut *session, &op, args)
                })?;
            set_host.call::<()>(call)?;
            let result = run();
            set_host.call::<()>(Value::Nil)?;
            result
        });
        result.map_err(ScriptError::from)
    }

    /// Takes back whatever a transaction left open; the stroke of a tool stays open.
    fn settle(&mut self, host: &mut dyn Host) {
        match &mut self.session.group {
            Some(group) if group.stroke => group.depth = 1,
            Some(group) => {
                if let Some(document) = host.editor().document_mut_by_id(group.doc) {
                    document.abort_group();
                }
                self.session.group = None;
            }
            None => {}
        }
    }

    /// Starts something new from the top: an earlier script still waiting for a dialog is
    /// dropped, and so are the folders it was allowed to use.
    fn begin_run(&mut self) {
        self.pending = None;
        self.session.granted.clear();
    }

    /// Runs or continues a script that may pause to show a dialog.
    fn drive(
        &mut self,
        host: &mut dyn Host,
        thread: Thread,
        args: impl mlua::IntoLuaMulti,
    ) -> Result<Outcome, ScriptError> {
        let result = self.enter(host, true, || thread.resume::<MultiValue>(args));
        self.settle(host);
        let values = result?;
        if thread.status() != ThreadStatus::Resumable {
            return Ok(Outcome::Done);
        }
        let marker: Table = self.internal.get("dialog")?;
        let mut values = values.into_iter();
        match (values.next(), values.next()) {
            (Some(Value::Table(first)), Some(Value::Table(spec))) if first == marker => {
                let spec = api::parse_dialog(&spec).map_err(|fail| ScriptError(fail.0))?;
                self.pending = Some(thread);
                Ok(Outcome::Dialog(spec))
            }
            _ => Err(ScriptError(
                "a script can only pause to show a dialog".to_owned(),
            )),
        }
    }

    /// Loads one script file: runs it, so that it registers what it has. `name` is the file
    /// name shown in errors. A script that fails leaves nothing registered.
    pub fn load(&mut self, host: &mut dyn Host, name: &str, code: &str) -> Result<(), ScriptError> {
        self.begin_run();
        let count = |list: &str| {
            self.internal
                .get::<Table>(list)
                .map_or(0, |list| list.raw_len())
        };
        let (commands, tools) = (count("commands"), count("tools"));
        let environment: Table = self.internal.get::<Function>("environment")?.call(())?;
        let result = self
            .lua
            .load(code)
            .set_name(format!("@{name}"))
            .set_mode(ChunkMode::Text)
            .set_environment(environment)
            .into_function()
            .map_err(ScriptError::from)
            .and_then(|chunk| self.enter(host, false, || chunk.call::<()>(())));
        self.settle(host);
        if result.is_err() {
            self.internal
                .get::<Function>("forget_after")?
                .call::<()>((commands, tools))?;
        }
        result
    }

    /// Runs a command a script registered.
    pub fn run_command(&mut self, host: &mut dyn Host, id: &str) -> Result<Outcome, ScriptError> {
        self.begin_run();
        let run: Function = self.internal.get("run_command")?;
        let thread = self.lua.create_thread(run)?;
        self.drive(host, thread, id)
    }

    /// Runs a line typed into the console. If it is an expression, its value is printed.
    pub fn eval(&mut self, host: &mut dyn Host, code: &str) -> Result<Outcome, ScriptError> {
        self.begin_run();
        let compile = |code: &str| {
            self.lua
                .load(code)
                .set_name("=console")
                .set_mode(ChunkMode::Text)
                .set_environment(self.console.clone())
                .into_function()
        };
        let chunk = match compile(&format!("return {code}")) {
            Ok(chunk) => chunk,
            Err(_) => compile(code)?,
        };
        let eval: Function = self.internal.get("eval")?;
        let thread = self.lua.create_thread(eval)?;
        self.drive(host, thread, chunk)
    }

    /// Continues the script that waits for a dialog: with the dialog as the user left it, or
    /// with `None` if they cancelled.
    pub fn resume(
        &mut self,
        host: &mut dyn Host,
        answer: Option<&DialogSpec>,
    ) -> Result<Outcome, ScriptError> {
        let Some(thread) = self.pending.take() else {
            return Ok(Outcome::Done);
        };
        let values = match answer {
            Some(spec) => Value::Table(api::dialog_values(&self.lua, spec)?),
            None => Value::Nil,
        };
        self.drive(host, thread, values)
    }

    /// Passes a pointer event on the canvas to a script tool. A whole press, drag and
    /// release is one undo step; an error takes the stroke back and ends it.
    pub fn tool_event(
        &mut self,
        host: &mut dyn Host,
        tool: &str,
        phase: ToolPhase,
        event: PointerEvent,
    ) -> Result<(), ScriptError> {
        if phase == ToolPhase::Down {
            self.tool_cancel(host);
            self.pending = None;
            let editor = host.editor();
            let Some(doc) = editor.active_document() else {
                return Ok(());
            };
            if !editor.document_mut().is_some_and(|d| d.begin_group()) {
                return Ok(());
            }
            self.session.group = Some(Group {
                doc,
                depth: 1,
                stroke: true,
            });
            self.stroke = Some(Stroke {
                tool: tool.to_owned(),
                failed: false,
            });
        }
        let Some(stroke) = &self.stroke else {
            return Ok(());
        };
        if stroke.tool != tool {
            return Ok(());
        }

        let mut result = Ok(());
        if !stroke.failed {
            let color = match event.button {
                PointerButton::Primary => host.editor().primary,
                PointerButton::Secondary => host.editor().secondary,
            };
            let handler: Function = self.internal.get("tool_event")?;
            let table = self.lua.create_table()?;
            table.set("x", event.position.x)?;
            table.set("y", event.position.y)?;
            let button = match event.button {
                PointerButton::Primary => "primary",
                PointerButton::Secondary => "secondary",
            };
            table.set("button", button)?;
            table.set("color", api::color_to_lua(&self.lua, color)?)?;
            table.set("shift", event.modifiers.shift)?;
            table.set("ctrl", event.modifiers.ctrl)?;
            table.set("alt", event.modifiers.alt)?;
            let phase_name = match phase {
                ToolPhase::Down => "down",
                ToolPhase::Move => "move",
                ToolPhase::Up => "up",
            };
            result = self.enter(host, false, || {
                handler.call::<()>((tool, phase_name, table))
            });
            self.settle(host);
        }

        if result.is_err() {
            self.abort_stroke(host);
            if let Some(stroke) = &mut self.stroke {
                stroke.failed = true;
            }
        }
        if phase == ToolPhase::Up {
            let name = self
                .tools()
                .into_iter()
                .find(|known| known.id == tool)
                .map_or_else(|| tool.to_owned(), |known| known.name);
            if let Some(group) = self.session.group.take()
                && let Some(document) = host.editor().document_mut_by_id(group.doc)
            {
                document.end_group(&name);
            }
            self.stroke = None;
        }
        result
    }

    fn abort_stroke(&mut self, host: &mut dyn Host) {
        if let Some(group) = self.session.group.take()
            && let Some(document) = host.editor().document_mut_by_id(group.doc)
        {
            document.abort_group();
        }
    }

    /// Abandons the stroke of a script tool, taking back what it drew.
    pub fn tool_cancel(&mut self, host: &mut dyn Host) {
        if self.stroke.take().is_some() {
            self.abort_stroke(host);
        }
    }
}

/// Whether a message starts with the place it is about, as in `outline.lua:12: ...`.
pub(crate) fn position_of(text: &str) -> Option<&str> {
    let (file, rest) = text.split_once(':')?;
    let (line, _) = rest.split_once(':')?;
    let named = !file.is_empty() && !file.contains(['\n', '\t', ' ']);
    let numbered = !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit());
    (named && numbered).then(|| &text[..file.len() + 1 + line.len()])
}

/// The first place in a traceback that is in a script, not in the host.
fn position_in(traceback: &str) -> Option<&str> {
    traceback
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("[C]") && !line.starts_with("[acms]"))
        .find_map(position_of)
}

/// An error as one message, starting with the file and line where that is known.
pub(crate) fn describe(err: &mlua::Error) -> String {
    use mlua::Error;
    let without_traceback = |message: &str| match message.split_once("\nstack traceback:") {
        Some((message, _)) => message.to_owned(),
        None => message.to_owned(),
    };
    match err {
        Error::CallbackError { traceback, cause } => {
            let message = describe(cause);
            match position_in(traceback) {
                Some(position) if position_of(&message).is_none() => {
                    format!("{position}: {message}")
                }
                _ => message,
            }
        }
        Error::RuntimeError(message) | Error::MemoryError(message) => without_traceback(message),
        Error::SyntaxError { message, .. } => without_traceback(message),
        Error::WithContext { cause, .. } => describe(cause),
        other => without_traceback(&other.to_string()),
    }
}
