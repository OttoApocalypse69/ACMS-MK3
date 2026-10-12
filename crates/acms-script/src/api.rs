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
}

/// A picture owned by a script: a copy it can read and change freely.
pub(crate) struct ScriptImage(pub Image);

impl UserData for ScriptImage {
    fn add_fields<F: mlua::UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("width", |_, this| Ok(this.0.width()));
        fields.add_field_method_get("height", |_, this| Ok(this.0.height()));
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get_pixel", |lua, this, (x, y): (f64, f64)| {
            color_to_lua(lua, this.0.get(coord(x), coord(y)))
        });
        methods.add_method_mut("set_pixel", |_, this, (x, y, color): (f64, f64, Value)| {
            let color = color_from(&color)?;
            this.0.set(coord(x), coord(y), color.normalized());
            Ok(())
        });
        methods.add_method("clone", |_, this, ()| Ok(ScriptImage(this.0.clone())));
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

/// Reads the description of a dialog a script built.
pub(crate) fn parse_dialog(table: &Table) -> Result<DialogSpec, Fail> {
    let title: String = table.get("title")?;
    let mut fields: Vec<Field> = Vec::new();
    for entry in table.get::<Table>("fields")?.sequence_values::<Table>() {
        let entry = entry?;
        let kind: String = entry.get("kind")?;
        let id: String = entry.get("id")?;
        if fields.iter().any(|field| field.id == id) {
            return Err(Fail(format!("the dialog has two fields called `{id}`")));
        }
        let bad = |what: &str| Fail(format!("dialog field `{id}`: {what}"));
        let label = match entry.get::<Value>("label")? {
            Value::Nil => id.clone(),
            Value::String(text) => text.to_string_lossy(),
            _ => return Err(bad("the label must be text")),
        };
        let value: Value = entry.get("value")?;
        let kind = match kind.as_str() {
            "number" => {
                let number = |name: &str, default: f64| match entry.get::<Value>(name) {
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
                Value::String(text) => text.to_string_lossy(),
                _ => return Err(bad("the value must be text")),
            }),
            "check" => FieldKind::Check(match value {
                Value::Nil => false,
                Value::Boolean(on) => on,
                _ => return Err(bad("the value must be true or false")),
            }),
            "dropdown" => {
                let options: Vec<String> = match entry.get::<Value>("options")? {
                    Value::Table(list) => list
                        .sequence_values::<String>()
                        .collect::<mlua::Result<_>>()
                        .map_err(|_| bad("the options must be text"))?,
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

fn document(host: &mut dyn Host) -> Result<&mut Document, Fail> {
    host.editor()
        .document_mut()
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

/// Whether a script may name this file: only inside a folder the user picked.
fn allowed(path: &Path, granted: &[PathBuf]) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && granted.iter().any(|folder| path.starts_with(folder))
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
    match answer(lua, host, session, op, args) {
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
            session.output.push(text.to_string_lossy());
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
            give!(())
        }
        "image" => {
            let (layer, frame): (u32, Option<f64>) = lua.unpack_multi(args)?;
            let document = document(host)?;
            let frame = frame_index(document, frame)?;
            let sprite = document.sprite();
            sprite.check_cel(LayerId(layer), frame)?;
            let image = sprite.read_region(LayerId(layer), frame, sprite.bounds());
            give!(lua.create_userdata(ScriptImage(image))?)
        }
        "set_image" => {
            let (layer, frame, image): (u32, Option<f64>, AnyUserData) = lua.unpack_multi(args)?;
            let mut image = match image.borrow::<ScriptImage>() {
                Ok(image) => image.0.clone(),
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
            let begun = editor.document_mut().is_some_and(Document::begin_group);
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
            let started = Instant::now();
            let result = host.run_command(&id, &params);
            // Time the application spent is not the script's.
            session
                .deadline
                .set(session.deadline.get() + started.elapsed());
            result.map_err(Fail)?;
            give!(())
        }
        "choose_folder" => {
            let title: Option<String> = lua.unpack_multi(args)?;
            let started = Instant::now();
            let folder = host.choose_folder(title.as_deref().unwrap_or("Choose a folder"));
            session
                .deadline
                .set(session.deadline.get() + started.elapsed());
            match folder {
                Some(folder) => {
                    let text = folder.to_string_lossy().into_owned();
                    session.granted.push(folder);
                    give!(text)
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

    #[test]
    fn only_files_inside_a_granted_folder_are_allowed() {
        let root = std::env::temp_dir().join("acms-granted");
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
        let sibling = std::env::temp_dir().join("acms-granted-too").join("a.gif");
        assert!(!allowed(&sibling, &granted));
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
