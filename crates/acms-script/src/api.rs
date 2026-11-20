//! What a script may ask of the application. Every request arrives here as an operation name
//! and its arguments, from the Lua half of the API in `prelude.lua`.

use std::cell::Cell;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

use acms_core::color::Rgba8;
use acms_core::command::Params;
use acms_core::document::Document;
use acms_core::editor::DocId;
use acms_core::geom::Rect;
use acms_core::image::Image;
use acms_core::sprite::{Layer, LayerId, LayerKind, SpriteError};
use mlua::{AnyUserData, IntoLuaMulti, Lua, MultiValue, Table, UserData, Value};

use crate::{DialogSpec, Field, FieldKind, Host};

/// Where the metatable of colour values is kept in the Lua registry.
pub(crate) const COLOR_META: &str = "acms.color";

/// Most decimals a number field of a dialog can ask for.
const MAX_DECIMALS: u8 = 6;

/// Most fields a dialog can have, options a dropdown can have, and bytes of any text in it.
const MAX_FIELDS: usize = 64;
const MAX_OPTIONS: usize = 256;
const MAX_DIALOG_TEXT: usize = 4096;

/// Most bytes the pictures scripts hold may take together. They live outside Lua, so the
/// memory limit of the Lua state does not see them.
pub(crate) const MAX_IMAGE_BYTES: usize = 512 * 1024 * 1024;

/// Most text scripts may print before the application collects it, and most of one line.
pub(crate) const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 16 * 1024;

/// How often one run may ask the user for a folder.
const MAX_FOLDER_ASKS: u32 = 4;

/// What a script is told once its time is up, whatever it tries next.
pub(crate) const TOO_LONG: &str = "the script took too long and was stopped";

/// A request that was refused; the text is raised as an error in the script.
pub(crate) struct Fail(pub String);

impl From<mlua::Error> for Fail {
    fn from(err: mlua::Error) -> Self {
        Self(crate::describe(&err))
    }
}

impl From<SpriteError> for Fail {
    fn from(err: SpriteError) -> Self {
        Self(err.to_string())
    }
}

impl From<&str> for Fail {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

impl From<Fail> for mlua::Error {
    fn from(fail: Fail) -> Self {
        mlua::Error::runtime(fail.0)
    }
}

/// An undo group a script has open.
pub(crate) struct Group {
    pub doc: DocId,
    /// How many `app.transaction` calls are inside one another, plus one for a tool stroke.
    pub depth: u32,
    /// The group is the stroke of a script tool: it stays open between pointer events.
    pub stroke: bool,
}

/// What the host functions remember between calls.
pub(crate) struct Session {
    /// Lines printed by scripts, until the application collects them.
    pub output: Vec<String>,
    /// Folders the user picked: the only places a script may name files in.
    pub granted: Vec<PathBuf>,
    pub group: Option<Group>,
    /// Whether the code running now may pause to show a dialog.
    pub may_ask: bool,
    /// When the code running now is stopped for taking too long.
    pub deadline: Rc<Cell<Instant>>,
    /// The time is up. Stays so until the next entry into Lua: nothing the script does after
    /// that, not even catching the error, lets it go on.
    pub expired: Rc<Cell<bool>>,
    /// Bytes the pictures scripts hold take now.
    pub images: Rc<Cell<usize>>,
    /// How often the run going on asked for a folder.
    pub folder_asks: u32,
    /// Bytes of `output`, and whether some was left out for being too much.
    pub output_bytes: usize,
    pub output_cut: bool,
}

impl Session {
    /// Whether the time of the code running now is up; notes it for good if so.
    pub fn out_of_time(&self) -> bool {
        if !self.expired.get() && Instant::now() > self.deadline.get() {
            self.expired.set(true);
        }
        self.expired.get()
    }

    fn say(&mut self, text: String) {
        if self.output_bytes + text.len() > MAX_OUTPUT_BYTES {
            if !self.output_cut {
                self.output_cut = true;
                self.output
                    .push("(the script printed too much; the rest was left out)".to_owned());
            }
            return;
        }
        let text = match text.char_indices().nth(MAX_LINE_BYTES) {
            Some((end, _)) if end < text.len() => format!("{} …", &text[..end]),
            _ => text,
        };
        self.output_bytes += text.len();
        self.output.push(text);
    }
}

/// A picture owned by a script: a copy it can read and change freely. What it takes is
/// counted against [`MAX_IMAGE_BYTES`] for as long as it lives.
pub(crate) struct ScriptImage {
    pub image: Image,
    budget: Rc<Cell<usize>>,
}

impl ScriptImage {
    /// Makes room for a picture, if the pictures scripts hold leave enough.
    pub fn new(lua: &Lua, budget: &Rc<Cell<usize>>, image: Image) -> Result<Self, Fail> {
        let size = image.byte_size();
        if budget.get() + size > MAX_IMAGE_BYTES {
            // Pictures a script let go of are only given back when Lua collects them.
            lua.gc_collect()?;
            lua.gc_collect()?;
            if budget.get() + size > MAX_IMAGE_BYTES {
                return Err(Fail::from(
                    "the script holds too many pictures; let go of some first",
                ));
            }
        }
        budget.set(budget.get() + size);
        Ok(Self {
            image,
            budget: budget.clone(),
        })
    }
}

impl Drop for ScriptImage {
    fn drop(&mut self) {
        let size = self.image.byte_size();
        self.budget.set(self.budget.get().saturating_sub(size));
    }
}

impl UserData for ScriptImage {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("width", |_, this| Ok(this.image.width()));
        fields.add_field_method_get("height", |_, this| Ok(this.image.height()));
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get_pixel", |lua, this, (x, y): (f64, f64)| {
            color_to_lua(lua, this.image.get(coord(x), coord(y)))
        });
        methods.add_method_mut("set_pixel", |_, this, (x, y, color): (f64, f64, Value)| {
            let color = color_from(&color)?;
            this.image.set(coord(x), coord(y), color.normalized());
            Ok(())
        });
        methods.add_method("clone", |lua, this, ()| {
            Ok(ScriptImage::new(lua, &this.budget, this.image.clone())?)
        });
    }
}

/// A coordinate given by a script. Fractions are rounded down, so `width / 2` works.
fn coord(value: f64) -> i32 {
    // `as` saturates, and turns a NaN into 0.
    value.floor() as i32
}

pub(crate) fn color_to_lua(lua: &Lua, color: Rgba8) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, 4)?;
    table.raw_set("r", color.r)?;
    table.raw_set("g", color.g)?;
    table.raw_set("b", color.b)?;
    table.raw_set("a", color.a)?;
    table.set_metatable(Some(lua.named_registry_value::<Table>(COLOR_META)?))?;
    Ok(table)
}

fn channel(table: &Table, name: &str, default: Option<u8>) -> Result<u8, Fail> {
    match table.raw_get::<Option<f64>>(name) {
        Ok(Some(value)) => Ok(value.round().clamp(0.0, 255.0) as u8),
        Ok(None) => default.ok_or_else(|| Fail(format!("a colour needs `{name}`"))),
        Err(_) => Err(Fail(format!("the `{name}` of a colour must be a number"))),
    }
}

/// Reads a colour: a `Color`, any table with `r`, `g`, `b` (and `a`), or hex text.
pub(crate) fn color_from(value: &Value) -> Result<Rgba8, Fail> {
    match value {
        Value::Table(table) => Ok(Rgba8::new(
            channel(table, "r", None)?,
            channel(table, "g", None)?,
            channel(table, "b", None)?,
            channel(table, "a", Some(255))?,
        )),
        Value::String(text) => {
            let text = text.to_string_lossy();
            Rgba8::from_hex(&text).ok_or_else(|| Fail(format!("`{text}` is not a colour")))
        }
        other => Err(Fail(format!(
            "expected a colour, not {}",
            describe_kind(other)
        ))),
    }
}

fn describe_kind(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nothing",
        Value::Boolean(_) => "true or false",
        Value::Integer(_) | Value::Number(_) => "a number",
        Value::String(_) => "text",
        Value::Table(_) => "a table",
        Value::Function(_) => "a function",
        _ => "that",
    }
}

/// The `Color(...)` function: from hex text, a table, or 3 or 4 numbers.
pub(crate) fn color_constructor(lua: &Lua, args: MultiValue) -> mlua::Result<Table> {
    let color = match args.len() {
        1 => color_from(&args[0])?,
        3 | 4 => {
            let mut channels = [255_u8; 4];
            for (slot, value) in channels.iter_mut().zip(args.iter()) {
                let number = match value {
                    Value::Integer(n) => *n as f64,
                    Value::Number(n) => *n,
                    other => {
                        return Err(Fail(format!(
                            "Color takes numbers, not {}",
                            describe_kind(other)
                        ))
                        .into());
                    }
                };
                *slot = number.round().clamp(0.0, 255.0) as u8;
            }
            Rgba8::from_array(channels)
        }
        _ => {
            return Err(Fail::from("Color takes hex text, or red, green, blue (and alpha)").into());
        }
    };
    color_to_lua(lua, color)
}

/// Text of a dialog, of at most [`MAX_DIALOG_TEXT`] bytes.
fn dialog_text(text: &mlua::LuaString, what: &str) -> Result<String, Fail> {
    if text.as_bytes().len() > MAX_DIALOG_TEXT {
        return Err(Fail(format!("{what} is too long")));
    }
    Ok(text.to_string_lossy())
}

fn dialog_string(table: &Table, key: &str, what: &str) -> Result<String, Fail> {
    match table.raw_get::<Value>(key)? {
        Value::String(text) => dialog_text(&text, what),
        _ => Err(Fail(format!("{what} must be text"))),
    }
}

/// Reads the description of a dialog a script built. Only what is in the tables is read
/// (no metamethods run), and only so much of it.
pub(crate) fn parse_dialog(table: &Table) -> Result<DialogSpec, Fail> {
    let title = dialog_string(table, "title", "the title of a dialog")?;
    let mut fields: Vec<Field> = Vec::new();
    let Value::Table(list) = table.raw_get::<Value>("fields")? else {
        return Err(Fail::from("a dialog needs a list of fields"));
    };
    if list.raw_len() > MAX_FIELDS {
        return Err(Fail(format!(
            "a dialog can have at most {MAX_FIELDS} fields"
        )));
    }
    for entry in list.sequence_values::<Table>() {
        let entry = entry?;
        let kind = dialog_string(&entry, "kind", "the kind of a dialog field")?;
        let id = dialog_string(&entry, "id", "the id of a dialog field")?;
        if fields.iter().any(|field| field.id == id) {
            return Err(Fail(format!("the dialog has two fields called `{id}`")));
        }
        let bad = |what: &str| Fail(format!("dialog field `{id}`: {what}"));
        let label = match entry.raw_get::<Value>("label")? {
            Value::Nil => id.clone(),
            Value::String(text) => dialog_text(&text, "the label")?,
            _ => return Err(bad("the label must be text")),
        };
        let value: Value = entry.raw_get("value")?;
        let kind = match kind.as_str() {
            "number" => {
                let number = |name: &str, default: f64| match entry.raw_get::<Value>(name) {
                    Ok(Value::Nil) => Ok(default),
                    Ok(Value::Integer(n)) => Ok(n as f64),
                    Ok(Value::Number(n)) if n.is_finite() => Ok(n),
                    _ => Err(bad(&format!("`{name}` must be a number"))),
                };
                let min = number("min", -1_000_000.0)?;
                let max = number("max", 1_000_000.0)?;
                if min > max {
                    return Err(bad("`min` is larger than `max`"));
                }
                let decimals = number("decimals", 0.0)?.clamp(0.0, f64::from(MAX_DECIMALS)) as u8;
                FieldKind::Number {
                    value: number("value", 0.0)?.clamp(min, max),
                    min,
                    max,
                    decimals,
                }
            }
            "color" => FieldKind::Color(match value {
                Value::Nil => Rgba8::BLACK,
                value => color_from(&value).map_err(|fail| bad(&fail.0))?,
            }),
            "text" => FieldKind::Text(match value {
                Value::Nil => String::new(),
                Value::String(text) => dialog_text(&text, "the value")?,
                _ => return Err(bad("the value must be text")),
            }),
            "check" => FieldKind::Check(match value {
                Value::Nil => false,
                Value::Boolean(on) => on,
                _ => return Err(bad("the value must be true or false")),
            }),
            "dropdown" => {
                let options: Vec<String> = match entry.raw_get::<Value>("options")? {
                    Value::Table(list) if list.raw_len() > MAX_OPTIONS => {
                        return Err(bad(&format!(
                            "a dropdown can have at most {MAX_OPTIONS} options"
                        )));
                    }
                    Value::Table(list) => list
                        .sequence_values::<mlua::LuaString>()
                        .map(|option| match option {
                            Ok(text) => dialog_text(&text, "an option"),
                            Err(_) => Err(bad("the options must be text")),
                        })
                        .collect::<Result<_, Fail>>()?,
                    _ => Vec::new(),
                };
                if options.is_empty() {
                    return Err(bad("a dropdown needs options"));
                }
                let selected = match value {
                    Value::Nil => 0,
                    Value::String(text) => {
                        let text = text.to_string_lossy();
                        options
                            .iter()
                            .position(|option| *option == text)
                            .ok_or_else(|| bad("the value is not one of the options"))?
                    }
                    _ => return Err(bad("the value must be one of the options")),
                };
                FieldKind::Dropdown { options, selected }
            }
            _ => return Err(bad("unknown kind of field")),
        };
        fields.push(Field { id, label, kind });
    }
    Ok(DialogSpec { title, fields })
}

/// What a script gets back from a dialog the user confirmed: a table of values by field id.
pub(crate) fn dialog_values(lua: &Lua, spec: &DialogSpec) -> mlua::Result<Table> {
    let values = lua.create_table()?;
    for field in &spec.fields {
        let id = field.id.as_str();
        match &field.kind {
            FieldKind::Number {
                value, decimals: 0, ..
            } => values.set(id, value.round() as i64)?,
            FieldKind::Number { value, .. } => values.set(id, *value)?,
            FieldKind::Color(color) => values.set(id, color_to_lua(lua, *color)?)?,
            FieldKind::Text(text) => values.set(id, text.as_str())?,
            FieldKind::Check(on) => values.set(id, *on)?,
            FieldKind::Dropdown { options, selected } => {
                values.set(id, options.get(*selected).map(String::as_str))?;
            }
        }
    }
    Ok(values)
}

fn layer_table(lua: &Lua, layer: &Layer) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("id", layer.id.0)?;
    table.set("name", layer.name.as_str())?;
    table.set("visible", layer.visible)?;
    let kind = match &layer.kind {
        LayerKind::Image(_) => "image",
        LayerKind::Group(_) => "group",
        LayerKind::Reference(_) => "reference",
        LayerKind::Tilemap(_) => "tilemap",
    };
    table.set("kind", kind)?;
    if let Some(image) = layer.as_image() {
        table.set("opacity", image.opacity)?;
        table.set("locked", image.locked)?;
    }
    Ok(table)
}

/// The sprite being edited. A stroke in progress is left alone: it may be the stroke of the
/// script tool that is asking.
fn document(host: &mut dyn Host) -> Result<&mut Document, Fail> {
    let editor = host.editor();
    editor
        .active_document()
        .and_then(|id| editor.document_mut_by_id(id))
        .ok_or(Fail::from("no sprite is open"))
}

/// A frame number given by a script, which counts from 1; the frame being shown if left out.
fn frame_index(document: &Document, frame: Option<f64>) -> Result<usize, Fail> {
    match frame {
        None => Ok(document.active_frame()),
        Some(frame) if frame >= 1.0 && frame.fract() == 0.0 => Ok(frame as usize - 1),
        Some(_) => Err(Fail::from("frames are whole numbers counted from 1")),
    }
}

/// Whether a script may name this file: only a file inside a folder the user picked (never
/// the folder itself), with no `..` in the way and no link that leads out of the folder.
/// The folders in `granted` are as [`grant`] made them.
pub(crate) fn allowed(path: &Path, granted: &[PathBuf]) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    // The deepest part of the path that exists, with every link in it followed; the rest
    // does not exist yet, so it cannot be a link.
    let mut existing = path;
    let mut rest = Vec::new();
    let real = loop {
        match std::fs::canonicalize(existing) {
            Ok(real) => break real,
            Err(_) => match (existing.parent(), existing.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_owned());
                    existing = parent;
                }
                _ => return false,
            },
        }
    };
    let mut real = real;
    for name in rest.into_iter().rev() {
        real.push(name);
    }
    granted.iter().any(|folder| {
        real.strip_prefix(folder).is_ok_and(|inside| {
            inside.components().next().is_some()
                && inside
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
        })
    })
}

/// The form a folder the user picked is remembered in: with every link followed, so that
/// [`allowed`] compares like with like. `None` if the folder is not there.
pub(crate) fn grant(folder: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(folder)
        .ok()
        .filter(|real| real.is_dir())
}

/// Refuses to let an undo group a script keeps open grow past what the history may hold:
/// nothing in an open group can be forgotten.
fn check_group(host: &mut dyn Host, session: &Session) -> Result<(), Fail> {
    let Some(group) = &session.group else {
        return Ok(());
    };
    let too_much = host
        .editor()
        .document_mut_by_id(group.doc)
        .is_some_and(|document| document.open_group_bytes() > document.history_limit());
    if too_much {
        return Err(Fail::from(
            "the script changed more in one undo step than the undo history can hold",
        ));
    }
    Ok(())
}

fn command_params(table: Option<Table>) -> Result<Params, Fail> {
    let mut params = Params::new();
    let Some(table) = table else {
        return Ok(params);
    };
    for pair in table.pairs::<Value, Value>() {
        let (name, value) = pair?;
        let Value::String(name) = name else {
            return Err(Fail::from("command parameters are given by name"));
        };
        let name = name.to_string_lossy();
        params = match value {
            Value::Boolean(on) => params.with(&name, on),
            Value::Integer(n) => params.with(&name, n),
            // Arithmetic easily leaves a whole number as a float.
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => params.with(&name, n as i64),
            Value::Number(n) => params.with(&name, n),
            Value::String(text) => params.with(&name, text.to_string_lossy()),
            Value::Table(_) => params.with(&name, color_from(&value)?),
            other => {
                return Err(Fail(format!(
                    "parameter `{name}` cannot be {}",
                    describe_kind(&other)
                )));
            }
        };
    }
    Ok(params)
}

/// Answers one request. What comes back to Lua is `true` and the results, or `false` and
/// the reason the request was refused.
pub(crate) fn dispatch(
    lua: &Lua,
    host: &mut dyn Host,
    session: &mut Session,
    op: &str,
    args: MultiValue,
) -> mlua::Result<MultiValue> {
    // Once the time is up, nothing more is done for the script, whatever it caught.
    if session.out_of_time() {
        return Err(mlua::Error::runtime(TOO_LONG));
    }
    let result = answer(lua, host, session, op, args);
    if session.out_of_time() {
        return Err(mlua::Error::runtime(TOO_LONG));
    }
    match result {
        Ok(mut values) => {
            values.push_front(Value::Boolean(true));
            Ok(values)
        }
        Err(Fail(message)) => {
            let message = match caller_position(lua) {
                Some(position) if crate::position_of(&message).is_none() => {
                    format!("{position}: {message}")
                }
                _ => message,
            };
            (false, message).into_lua_multi(lua)
        }
    }
}

/// The line of the script that is calling into the API right now, as `file:line`. `None` if
/// no script is in sight, as for a line typed into the console that is one call.
fn caller_position(lua: &Lua) -> Option<String> {
    // Walks outwards from the host function, past the Lua half of the API.
    for level in 0..32 {
        let frame = lua.inspect_stack(level, |debug| {
            let source = debug.source();
            let file = source.short_src.map(|file| file.into_owned());
            (source.what, file, debug.current_line())
        })?;
        if let (what, Some(file), Some(line)) = frame
            && what != "C"
            && file != "[acms]"
        {
            return Some(format!("{file}:{line}"));
        }
    }
    None
}

fn answer(
    lua: &Lua,
    host: &mut dyn Host,
    session: &mut Session,
    op: &str,
    args: MultiValue,
) -> Result<MultiValue, Fail> {
    macro_rules! give {
        ($value:expr) => {
            Ok($value.into_lua_multi(lua)?)
        };
    }
    match op {
        "print" => {
            let text: mlua::LuaString = lua.unpack_multi(args)?;
            session.say(text.to_string_lossy());
            give!(())
        }
        "has_sprite" => give!(host.editor().document().is_some()),
        "fg" => give!(color_to_lua(lua, host.editor().primary)?),
        "bg" => give!(color_to_lua(lua, host.editor().secondary)?),
        "set_fg" | "set_bg" => {
            let value: Value = lua.unpack_multi(args)?;
            let color = color_from(&value)?;
            let editor = host.editor();
            match op {
                "set_fg" => editor.primary = color,
                _ => editor.secondary = color,
            }
            give!(())
        }
        "size" => {
            let sprite = document(host)?.sprite();
            give!((sprite.width(), sprite.height()))
        }
        "layers" => {
            let sprite = document(host)?.sprite();
            let list = lua.create_table()?;
            for entry in sprite.walk() {
                list.raw_push(layer_table(lua, entry.layer)?)?;
            }
            give!(list)
        }
        "active_layer" => {
            let Some(document) = host.editor().document() else {
                return give!(Value::Nil);
            };
            match document.sprite().layer(document.active_layer()) {
                Some(layer) => give!(layer_table(lua, layer)?),
                None => give!(Value::Nil),
            }
        }
        "active_frame" => give!(document(host)?.active_frame() + 1),
        "frames" => {
            let sprite = document(host)?.sprite();
            let list = lua.create_table()?;
            for frame in sprite.frames() {
                let table = lua.create_table()?;
                table.set("duration", frame.duration_ms)?;
                list.raw_push(table)?;
            }
            give!(list)
        }
        "palette" => {
            let sprite = document(host)?.sprite();
            let list = lua.create_table()?;
            for color in sprite.palette().colors() {
                list.raw_push(color_to_lua(lua, color)?)?;
            }
            give!(list)
        }
        "tags" => {
            let sprite = document(host)?.sprite();
            let list = lua.create_table()?;
            for tag in sprite.tags() {
                let table = lua.create_table()?;
                table.set("name", tag.name.as_str())?;
                table.set("from", tag.from + 1)?;
                table.set("to", tag.to + 1)?;
                table.set("direction", tag.direction.id())?;
                table.set("color", color_to_lua(lua, tag.color)?)?;
                list.raw_push(table)?;
            }
            give!(list)
        }
        "selection" => {
            let sprite = document(host)?.sprite();
            let bounds = sprite.selection().map(|mask| mask.bounds());
            match bounds.filter(|bounds| !bounds.is_empty()) {
                Some(bounds) => {
                    let table = lua.create_table()?;
                    table.set("x", bounds.x)?;
                    table.set("y", bounds.y)?;
                    table.set("width", bounds.w)?;
                    table.set("height", bounds.h)?;
                    give!(table)
                }
                None => give!(Value::Nil),
            }
        }
        "selected" => {
            let (x, y): (f64, f64) = lua.unpack_multi(args)?;
            give!(document(host)?.sprite().is_selected(coord(x), coord(y)))
        }
        "get_pixel" => {
            let (layer, frame, x, y): (u32, Option<f64>, f64, f64) = lua.unpack_multi(args)?;
            let document = document(host)?;
            let frame = frame_index(document, frame)?;
            let sprite = document.sprite();
            sprite.check_cel(LayerId(layer), frame)?;
            let color = sprite.pixel(LayerId(layer), frame, coord(x), coord(y));
            give!(color_to_lua(lua, color)?)
        }
        "set_pixel" => {
            let (layer, frame, x, y, color): (u32, Option<f64>, f64, f64, Value) =
                lua.unpack_multi(args)?;
            let color = color_from(&color)?;
            let (x, y) = (coord(x), coord(y));
            let document = document(host)?;
            let frame = frame_index(document, frame)?;
            // Like every tool, scripts draw inside the selection only.
            if !document.sprite().is_selected(x, y) {
                document.sprite().check_cel(LayerId(layer), frame)?;
                return give!(());
            }
            let mut pixel = Image::new(1, 1);
            pixel.set(0, 0, color);
            document.transact("Script", |tx| {
                tx.set_pixels(LayerId(layer), frame, Rect::new(x, y, 1, 1), &pixel)
            })?;
            check_group(host, session)?;
            give!(())
        }
        "image" => {
            let (layer, frame): (u32, Option<f64>) = lua.unpack_multi(args)?;
            let document = document(host)?;
            let frame = frame_index(document, frame)?;
            let sprite = document.sprite();
            sprite.check_cel(LayerId(layer), frame)?;
            let image = sprite.read_region(LayerId(layer), frame, sprite.bounds());
            let image = ScriptImage::new(lua, &session.images, image)?;
            give!(lua.create_userdata(image)?)
        }
        "set_image" => {
            let (layer, frame, image): (u32, Option<f64>, AnyUserData) = lua.unpack_multi(args)?;
            let mut image = match image.borrow::<ScriptImage>() {
                Ok(image) => image.image.clone(),
                Err(_) => return Err(Fail::from("expected an image")),
            };
            let document = document(host)?;
            let frame = frame_index(document, frame)?;
            let sprite = document.sprite();
            let bounds = sprite.bounds();
            if (image.width(), image.height()) != (bounds.w, bounds.h) {
                return Err(Fail(format!(
                    "the image is {} x {} but the sprite is {} x {}",
                    image.width(),
                    image.height(),
                    bounds.w,
                    bounds.h
                )));
            }
            sprite.check_cel(LayerId(layer), frame)?;
            // Like every tool, scripts draw inside the selection only.
            if let Some(mask) = sprite.selection() {
                let current = sprite.read_region(LayerId(layer), frame, bounds);
                for y in 0..bounds.h as i32 {
                    for x in 0..bounds.w as i32 {
                        if !mask.contains(x, y) {
                            image.set(x, y, current.get(x, y));
                        }
                    }
                }
            }
            document.transact("Script", |tx| {
                tx.set_pixels(LayerId(layer), frame, bounds, &image)
            })?;
            check_group(host, session)?;
            give!(())
        }
        "begin" => {
            if let Some(group) = &mut session.group {
                group.depth += 1;
                return give!(());
            }
            let editor = host.editor();
            let doc = editor
                .active_document()
                .ok_or(Fail::from("no sprite is open"))?;
            let begun = editor
                .document_mut_by_id(doc)
                .is_some_and(Document::begin_group);
            if !begun {
                return Err(Fail::from("an undo step is being recorded already"));
            }
            session.group = Some(Group {
                doc,
                depth: 1,
                stroke: false,
            });
            give!(())
        }
        "commit" | "rollback" => {
            let name: Option<String> = lua.unpack_multi(args)?;
            let floor = match &session.group {
                Some(group) => u32::from(group.stroke),
                None => return Err(Fail::from("no transaction is open")),
            };
            let Some(group) = &mut session.group else {
                return give!(());
            };
            if group.depth <= floor {
                return Err(Fail::from("no transaction is open"));
            }
            group.depth -= 1;
            if group.depth == 0 {
                let doc = group.doc;
                session.group = None;
                if let Some(document) = host.editor().document_mut_by_id(doc) {
                    match op {
                        "commit" => document.end_group(name.as_deref().unwrap_or("Script")),
                        _ => document.abort_group(),
                    }
                }
            }
            give!(())
        }
        "can_ask" => give!(session.may_ask && session.group.is_none()),
        "check_dialog" => {
            let table: Table = lua.unpack_multi(args)?;
            parse_dialog(&table)?;
            give!(())
        }
        "command" => {
            let (id, params): (String, Option<Table>) = lua.unpack_multi(args)?;
            let params = command_params(params)?;
            if let Some(path) = params.get("path") {
                let named = match path {
                    acms_core::command::Value::Text(path) => Some(Path::new(path)),
                    _ => None,
                };
                if !named.is_some_and(|path| allowed(path, &session.granted)) {
                    return Err(Fail::from(
                        "a script may only name files inside a folder picked with \
                         app.choose_folder",
                    ));
                }
            }
            // The time the command takes is the script's: a script cannot run for ever by
            // having the application work for it.
            host.run_command(&id, &params).map_err(Fail)?;
            check_group(host, session)?;
            give!(())
        }
        "choose_folder" => {
            let title: Option<String> = lua.unpack_multi(args)?;
            // Like a dialog, only when the user ran the script and nothing is half done.
            if !session.may_ask || session.group.is_some() {
                return Err(Fail::from("a folder cannot be asked for here"));
            }
            if session.folder_asks >= MAX_FOLDER_ASKS {
                return Err(Fail::from("the script asked for folders too often"));
            }
            session.folder_asks += 1;
            let started = Instant::now();
            let folder = host.choose_folder(title.as_deref().unwrap_or("Choose a folder"));
            // The time the user takes to choose is not the script's.
            session
                .deadline
                .set(session.deadline.get() + started.elapsed());
            // The script is given the folder as the user picked it; what is remembered is
            // where it really is.
            match folder.and_then(|folder| Some((grant(&folder)?, folder))) {
                Some((real, folder)) => {
                    session.granted.push(real);
                    give!(folder.to_string_lossy().into_owned())
                }
                None => give!(Value::Nil),
            }
        }
        "join_path" => {
            let (folder, name): (String, String) = lua.unpack_multi(args)?;
            let joined = Path::new(&folder).join(name);
            give!(joined.to_string_lossy().into_owned())
        }
        _ => Err(Fail(format!("unknown request `{op}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder made for one test, removed afterwards.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("acms-script-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_granted_folder_itself_is_not_a_file_a_script_may_name() {
        let folder = Folder::new("itself");
        let granted = vec![grant(&folder.0).unwrap()];
        assert!(!allowed(&folder.0, &granted));
        assert!(!allowed(&folder.0.join("."), &granted));
        assert!(allowed(&folder.0.join("out.png"), &granted));
        assert!(allowed(&folder.0.join("new").join("out.png"), &granted));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_granted_folder_is_not_followed() {
        let outside = Folder::new("outside");
        let folder = Folder::new("linked");
        std::os::unix::fs::symlink(&outside.0, folder.0.join("door")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("x"), folder.0.join("file")).unwrap();
        let granted = vec![grant(&folder.0).unwrap()];
        assert!(!allowed(&folder.0.join("door").join("a.png"), &granted));
        assert!(!allowed(&folder.0.join("file"), &granted));
        assert!(allowed(&folder.0.join("a.png"), &granted));
    }

    #[test]
    fn only_files_inside_a_granted_folder_are_allowed() {
        let root = Folder::new("granted");
        let root = grant(&root.0).unwrap();
        let granted = vec![root.clone()];
        assert!(allowed(&root.join("a.gif"), &granted));
        assert!(allowed(&root.join("sub").join("a.gif"), &granted));
        assert!(!allowed(&root.join("..").join("a.gif"), &granted));
        assert!(!allowed(
            &root.join("sub").join("..").join("..").join("a"),
            &granted
        ));
        assert!(!allowed(&std::env::temp_dir().join("a.gif"), &granted));
        assert!(!allowed(Path::new("a.gif"), &granted));
        assert!(!allowed(&root.join("a.gif"), &[]));
        // A folder whose name merely starts the same is another folder.
        let mut sibling = root.clone().into_os_string();
        sibling.push("-too");
        assert!(!allowed(&PathBuf::from(sibling).join("a.gif"), &granted));
    }

    #[test]
    fn coordinates_round_down_and_never_overflow() {
        assert_eq!(coord(2.9), 2);
        assert_eq!(coord(-0.5), -1);
        assert_eq!(coord(f64::NAN), 0);
        assert_eq!(coord(1e300), i32::MAX);
        assert_eq!(coord(-1e300), i32::MIN);
    }
}
