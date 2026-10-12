//! The scripting API against a host that is just an editor: no window, no real commands.

use std::path::PathBuf;
use std::time::Duration;

use acms_core::color::Rgba8;
use acms_core::command::{Params, Value};
use acms_core::document::Document;
use acms_core::editor::Editor;
use acms_core::geom::{Point, Rect};
use acms_core::image::Image;
use acms_core::selection::Mask;
use acms_core::sprite::{LayerId, Sprite};
use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

use super::*;

const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);

struct TestHost {
    editor: Editor,
    /// The commands scripts ran, with their parameters.
    ran: Vec<(String, Params)>,
    /// What the user "picks" when asked for a folder.
    folder: Option<PathBuf>,
}

impl Host for TestHost {
    fn editor(&mut self) -> &mut Editor {
        &mut self.editor
    }

    fn run_command(&mut self, id: &str, params: &Params) -> Result<(), String> {
        self.ran.push((id.to_owned(), params.clone()));
        match id {
            "test.fail" => Err("it did not work".to_owned()),
            // A command that changes the sprite, as its own undo step.
            "test.paint" => {
                paint(&mut self.editor, Rect::new(7, 7, 1, 1), BLUE);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn choose_folder(&mut self, _title: &str) -> Option<PathBuf> {
        self.folder.clone()
    }
}

fn paint(editor: &mut Editor, rect: Rect, color: Rgba8) {
    let pixels = Image::from_pixels(rect.w, rect.h, vec![color; (rect.w * rect.h) as usize]);
    let document = editor.document_mut().unwrap();
    let layer = document.active_layer();
    document
        .transact("Paint", |tx| {
            tx.set_pixels(layer, 0, rect, &pixels.unwrap())
        })
        .unwrap();
}

/// A host with a blank 8 × 8 sprite open, and an engine.
fn setup() -> (Engine, TestHost) {
    let mut editor = Editor::new();
    editor.add_document(Document::new(Sprite::new(8, 8).unwrap()));
    let host = TestHost {
        editor,
        ran: Vec::new(),
        folder: None,
    };
    (Engine::new().unwrap(), host)
}

fn doc(host: &TestHost) -> &Document {
    host.editor.document().unwrap()
}

fn pixel(host: &TestHost, x: i32, y: i32) -> Rgba8 {
    let document = doc(host);
    document
        .sprite()
        .pixel(document.active_layer(), document.active_frame(), x, y)
}

fn steps(host: &TestHost) -> Vec<String> {
    doc(host).history().map(str::to_owned).collect()
}

/// Runs code as the console would and returns what it printed.
fn eval(engine: &mut Engine, host: &mut TestHost, code: &str) -> Vec<String> {
    let outcome = engine.eval(host, code);
    assert_eq!(outcome, Ok(Outcome::Done), "{code}");
    engine.take_output()
}

fn eval_error(engine: &mut Engine, host: &mut TestHost, code: &str) -> String {
    match engine.eval(host, code) {
        Err(ScriptError(message)) => message,
        other => panic!("`{code}` did not fail: {other:?}"),
    }
}

fn at(x: i32, y: i32) -> PointerEvent {
    PointerEvent {
        position: Point::new(x, y),
        button: PointerButton::Primary,
        modifiers: Modifiers::default(),
    }
}

#[test]
fn the_console_prints_values_and_keeps_variables() {
    let (mut engine, mut host) = setup();
    assert_eq!(eval(&mut engine, &mut host, "1 + 2"), ["3"]);
    assert_eq!(eval(&mut engine, &mut host, "x = 5"), [] as [&str; 0]);
    assert_eq!(eval(&mut engine, &mut host, "x * 2, 'a'"), ["10\ta"]);
    assert_eq!(
        eval(&mut engine, &mut host, "print('hi', nil, true)"),
        ["hi\tnil\ttrue"]
    );
    assert_eq!(eval(&mut engine, &mut host, "local y = 1 print(y)"), ["1"]);
    assert_eq!(
        eval(&mut engine, &mut host, "app.active_sprite"),
        ["Sprite(8 x 8)"]
    );
}

#[test]
fn scripts_cannot_reach_the_machine() {
    let (mut engine, mut host) = setup();
    for name in [
        "os",
        "io",
        "debug",
        "package",
        "require",
        "dofile",
        "loadfile",
        "load",
        "loadstring",
        "collectgarbage",
        "string.dump",
    ] {
        assert_eq!(
            eval(&mut engine, &mut host, name),
            ["nil"],
            "{name} is there"
        );
    }
    // What is harmless stays.
    for name in [
        "string.format",
        "table.insert",
        "math.floor",
        "utf8.char",
        "coroutine.wrap",
        "pcall",
    ] {
        assert_ne!(
            eval(&mut engine, &mut host, name),
            ["nil"],
            "{name} is missing"
        );
    }
    // Bytecode is not accepted in place of source.
    let bytecode = "\u{1b}Lua\u{54}\u{0}";
    assert!(engine.load(&mut host, "evil.lua", bytecode).is_err());
}

#[test]
fn errors_name_the_file_and_line() {
    let (mut engine, mut host) = setup();
    let err = engine
        .load(&mut host, "broken.lua", "local x = 1\nlocal y = x +\n")
        .unwrap_err();
    assert!(err.0.starts_with("broken.lua:"), "{err}");

    let err = engine
        .load(&mut host, "fails.lua", "local t = nil\n\nprint(t.x)\n")
        .unwrap_err();
    assert!(err.0.starts_with("fails.lua:3:"), "{err}");

    // A request the application refuses is reported at the line that made it.
    let code = "local cel = app.active_cel\ncel:set_pixel(0, 0, 'not a colour')\n";
    let err = engine.load(&mut host, "refused.lua", code).unwrap_err();
    assert_eq!(err.0, "refused.lua:2: `not a colour` is not a colour");

    // So is a mistake in a call on an image.
    let code = "local image = app.active_cel:image()\n\n\nimage:set_pixel(0, 0, 5)\n";
    let err = engine.load(&mut host, "image.lua", code).unwrap_err();
    assert!(err.0.starts_with("image.lua:4:"), "{err}");
    assert!(err.0.contains("expected a colour"), "{err}");

    assert_eq!(
        eval_error(&mut engine, &mut host, "error('just this', 0)"),
        "just this"
    );
    assert!(eval_error(&mut engine, &mut host, "nothing()").starts_with("console:1:"));
}

#[test]
fn a_script_that_never_ends_is_stopped() {
    let (mut engine, mut host) = setup();
    engine.set_time_limit(Duration::from_millis(50));
    let message = eval_error(&mut engine, &mut host, "while true do end");
    assert!(message.contains("took too long"), "{message}");
    // Also inside a coroutine the script made itself.
    let code = "coroutine.wrap(function() while true do end end)()";
    let message = eval_error(&mut engine, &mut host, code);
    assert!(message.contains("took too long"), "{message}");
    // The engine is fine afterwards.
    assert_eq!(eval(&mut engine, &mut host, "1"), ["1"]);
}

#[test]
fn colours_are_made_compared_and_set() {
    let (mut engine, mut host) = setup();
    assert_eq!(
        eval(&mut engine, &mut host, "Color(255, 0, 0)"),
        ["#ff0000"]
    );
    assert_eq!(
        eval(&mut engine, &mut host, "Color('#00ff0080')"),
        ["#00ff0080"]
    );
    assert_eq!(eval(&mut engine, &mut host, "Color(1, 2, 3, 4).a"), ["4"]);
    assert_eq!(
        eval(&mut engine, &mut host, "Color{r = 300, g = -5, b = 1.6}"),
        ["#ff0002"]
    );
    assert_eq!(
        eval(
            &mut engine,
            &mut host,
            "Color('ff0000') == Color(255, 0, 0)"
        ),
        ["true"]
    );
    assert_eq!(
        eval(
            &mut engine,
            &mut host,
            "Color('ff0000') == Color(254, 0, 0)"
        ),
        ["false"]
    );
    assert!(eval_error(&mut engine, &mut host, "Color('nope')").contains("not a colour"));
    assert!(eval_error(&mut engine, &mut host, "Color(1, 2)").contains("Color takes"));

    host.editor.primary = RED;
    assert_eq!(eval(&mut engine, &mut host, "app.fg_color"), ["#ff0000"]);
    eval(
        &mut engine,
        &mut host,
        "app.fg_color = '#0000ff' app.bg_color = Color(1, 2, 3)",
    );
    assert_eq!(host.editor.primary, BLUE);
    assert_eq!(host.editor.secondary, Rgba8::rgb(1, 2, 3));
    assert!(eval_error(&mut engine, &mut host, "app.nonsense = 1").contains("cannot be set"));
}

#[test]
fn the_sprite_can_be_read() {
    let (mut engine, mut host) = setup();
    let document = host.editor.document_mut().unwrap();
    document
        .transact("Setup", |tx| {
            tx.add_frame(1, None)?;
            tx.set_frame_duration(1, 250)?;
            tx.add_layer(acms_core::sprite::Slot::new(None, 1), "Ink", false)?;
            tx.add_layer(acms_core::sprite::Slot::new(None, 2), "Folder", true)?;
            tx.add_tag(acms_core::sprite::Tag {
                name: "walk".to_owned(),
                from: 0,
                to: 1,
                direction: acms_core::sprite::Direction::PingPong,
                color: RED,
            })?;
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(2, 3, 4, 2))))
        })
        .unwrap();

    let mut say = |code: &str| eval(&mut engine, &mut host, code).join("\n");
    assert_eq!(
        say("app.active_sprite.width, app.active_sprite.height"),
        "8\t8"
    );
    assert_eq!(
        say("#app.active_sprite.frames, app.active_sprite.frames[2].duration"),
        "2\t250"
    );
    let names = "local n = {} for i, l in ipairs(app.active_sprite.layers) do n[i] = l.name .. ':' .. l.kind end return table.concat(n, ' ')";
    assert_eq!(say(names), "Layer 1:image Ink:image Folder:group");
    assert_eq!(say("app.active_sprite.layers[3]:cel(1)"), "nil");
    assert_eq!(
        say("app.active_sprite.layers[1]:cel(2)"),
        "Cel(layer 1, frame 2)"
    );
    assert_eq!(say("app.active_frame, app.active_cel.frame"), "1\t1");
    let tag =
        "local t = app.active_sprite.tags[1] return t.name, t.from, t.to, t.direction, t.color";
    assert_eq!(say(tag), "walk\t1\t2\tpingpong\t#ff0000");
    let selection = "local s = app.active_sprite.selection return s.x, s.y, s.width, s.height, s:contains(2, 3), s:contains(1, 3)";
    assert_eq!(say(selection), "2\t3\t4\t2\ttrue\tfalse");
    assert_eq!(
        say("#app.active_sprite.palette > 0, app.active_sprite.palette[1].a"),
        "true\t255"
    );
    assert!(
        eval_error(&mut engine, &mut host, "app.active_sprite.width = 3").contains("cannot be set")
    );
}

#[test]
fn without_a_sprite_there_is_nothing_to_read() {
    let mut host = TestHost {
        editor: Editor::new(),
        ran: Vec::new(),
        folder: None,
    };
    let mut engine = Engine::new().unwrap();
    let line = "app.active_sprite, app.active_layer, app.active_cel, app.active_frame";
    assert_eq!(eval(&mut engine, &mut host, line), ["nil\tnil\tnil\tnil"]);
    let message = eval_error(
        &mut engine,
        &mut host,
        "app.transaction('x', function() end)",
    );
    assert!(message.contains("no sprite is open"), "{message}");
}

#[test]
fn pixels_are_read_and_written_through_a_cel() {
    let (mut engine, mut host) = setup();
    eval(
        &mut engine,
        &mut host,
        "app.active_cel:set_pixel(2, 3, '#ff0000')",
    );
    assert_eq!(pixel(&host, 2, 3), RED);
    assert_eq!(
        eval(&mut engine, &mut host, "app.active_cel:get_pixel(2, 3)"),
        ["#ff0000"]
    );
    assert_eq!(
        eval(&mut engine, &mut host, "app.active_cel:get_pixel(0, 0).a"),
        ["0"]
    );
    // Fractions round down; outside the canvas nothing happens.
    eval(
        &mut engine,
        &mut host,
        "app.active_cel:set_pixel(4.9, 0.2, '#0000ff')",
    );
    assert_eq!(pixel(&host, 4, 0), BLUE);
    eval(
        &mut engine,
        &mut host,
        "app.active_cel:set_pixel(-1, 99, '#0000ff')",
    );
    assert_eq!(
        eval(&mut engine, &mut host, "app.active_cel:get_pixel(-1, 99).a"),
        ["0"]
    );
    // Outside a transaction every change is its own undo step.
    assert_eq!(steps(&host), ["Script", "Script"]);

    let message = eval_error(
        &mut engine,
        &mut host,
        "app.active_sprite.layers[1]:cel(9):get_pixel(0, 0)",
    );
    assert!(message.contains("there is no frame"), "{message}");
}

#[test]
fn a_transaction_is_one_undo_step() {
    let (mut engine, mut host) = setup();
    let before = doc(&host).sprite().clone();
    let code = "
        app.transaction('Dots', function()
            local cel = app.active_cel
            for x = 0, 7 do cel:set_pixel(x, x, '#ff0000') end
            app.command('test.paint')
            app.transaction('Inner', function() cel:set_pixel(0, 7, '#ff0000') end)
        end)";
    eval(&mut engine, &mut host, code);
    assert_eq!(steps(&host), ["Dots"]);
    assert_eq!(pixel(&host, 5, 5), RED);
    assert_eq!(pixel(&host, 7, 7), BLUE);
    assert_eq!(pixel(&host, 0, 7), RED);
    let after = doc(&host).sprite().clone();

    let document = host.editor.document_mut().unwrap();
    assert!(document.undo());
    assert_eq!(*document.sprite(), before);
    assert!(document.redo());
    assert_eq!(*document.sprite(), after);
}

#[test]
fn a_failed_transaction_changes_nothing() {
    let (mut engine, mut host) = setup();
    paint(&mut host.editor, Rect::new(0, 0, 1, 1), BLUE);
    let before = doc(&host).sprite().clone();
    let code = "
        app.transaction('Dots', function()
            app.active_cel:set_pixel(3, 3, '#ff0000')
            app.command('test.paint')
            error('stop')
        end)";
    let message = eval_error(&mut engine, &mut host, code);
    assert!(message.ends_with("stop"), "{message}");
    assert_eq!(*doc(&host).sprite(), before);
    assert_eq!(steps(&host), ["Paint"]);
    assert!(!doc(&host).in_group());

    // An error that is caught inside leaves the transaction to go on.
    let code = "
        app.transaction('Kept', function()
            app.active_cel:set_pixel(3, 3, '#ff0000')
            pcall(function() error('caught') end)
        end)";
    eval(&mut engine, &mut host, code);
    assert_eq!(steps(&host), ["Paint", "Kept"]);
    assert_eq!(pixel(&host, 3, 3), RED);
}

#[test]
fn a_transaction_left_open_is_taken_back() {
    let (mut engine, mut host) = setup();
    // A script cannot leave a group open by going around `app.transaction`: whatever is
    // open when it returns is rolled back.
    let code = "
        local co = coroutine.wrap(function()
            app.transaction('Stuck', function()
                app.active_cel:set_pixel(1, 1, '#ff0000')
                coroutine.yield()
            end)
        end)
        co()";
    eval(&mut engine, &mut host, code);
    assert!(!doc(&host).in_group());
    assert_eq!(pixel(&host, 1, 1), Rgba8::TRANSPARENT);
    assert_eq!(steps(&host), [] as [&str; 0]);
}

#[test]
fn images_are_copies_written_back_inside_the_selection() {
    let (mut engine, mut host) = setup();
    paint(&mut host.editor, Rect::new(0, 0, 2, 1), BLUE);
    let code = "
        local cel = app.active_cel
        local image = cel:image()
        assert(image.width == 8 and image.height == 8)
        local copy = image:clone()
        image:set_pixel(1, 0, '#ff0000')
        image:set_pixel(99, 99, '#ff0000')
        assert(copy:get_pixel(1, 0) == Color('#0000ff'))
        assert(cel:get_pixel(1, 0) == Color('#0000ff'), 'the cel changed early')
        cel:set_image(image)";
    eval(&mut engine, &mut host, code);
    assert_eq!(pixel(&host, 1, 0), RED);
    assert_eq!(pixel(&host, 0, 0), BLUE);

    // With a selection, only what is inside it is written.
    let document = host.editor.document_mut().unwrap();
    document
        .transact("Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(8, 8, Rect::new(4, 4, 2, 2))))
        })
        .unwrap();
    let code = "
        local cel = app.active_cel
        local image = cel:image()
        for y = 0, 7 do for x = 0, 7 do image:set_pixel(x, y, '#ff0000') end end
        cel:set_image(image)
        cel:set_pixel(7, 7, '#ff0000')
        cel:set_pixel(5, 5, '#0000ff')";
    eval(&mut engine, &mut host, code);
    assert_eq!(pixel(&host, 4, 4), RED);
    assert_eq!(pixel(&host, 5, 5), BLUE);
    assert_eq!(pixel(&host, 3, 4), Rgba8::TRANSPARENT);
    assert_eq!(pixel(&host, 7, 7), Rgba8::TRANSPARENT);
    assert_eq!(pixel(&host, 0, 0), BLUE);

    let message = eval_error(&mut engine, &mut host, "app.active_cel:set_image(5)");
    assert!(
        message.contains("userdata") || message.contains("image"),
        "{message}"
    );
}

#[test]
fn a_locked_layer_refuses_scripts_too() {
    let (mut engine, mut host) = setup();
    let document = host.editor.document_mut().unwrap();
    let layer = document.active_layer();
    document
        .transact("Lock", |tx| {
            tx.update_layer(layer, |props| {
                if let Some(image) = &mut props.image {
                    image.locked = true;
                }
            })
        })
        .unwrap();
    let message = eval_error(
        &mut engine,
        &mut host,
        "app.active_cel:set_pixel(0, 0, '#ff0000')",
    );
    // A line that is one call has no line of its own to name.
    assert_eq!(message, "the layer is locked");
    let code = "local cel = app.active_cel\ncel:set_pixel(0, 0, '#ff0000')";
    assert_eq!(
        eval_error(&mut engine, &mut host, code),
        "console:2: the layer is locked"
    );
}

#[test]
fn commands_are_registered_and_run() {
    let (mut engine, mut host) = setup();
    let code = "
        local count = 0
        register_command{ id = 'my.dot', name = 'Dot', run = function()
            count = count + 1
            app.transaction('Dot', function() app.active_cel:set_pixel(count, 0, '#ff0000') end)
        end }
        register_command{ id = 'my.twice', run = function()
            app.transaction('Twice', function()
                app.command('my.dot')
                app.command('my.dot')
            end)
        end }";
    engine.load(&mut host, "mine.lua", code).unwrap();
    let dot = Registered {
        id: "my.dot".to_owned(),
        name: "Dot".to_owned(),
    };
    let twice = Registered {
        id: "my.twice".to_owned(),
        name: "my.twice".to_owned(),
    };
    assert_eq!(engine.commands(), [dot, twice]);

    assert_eq!(engine.run_command(&mut host, "my.dot"), Ok(Outcome::Done));
    assert_eq!(pixel(&host, 1, 0), RED);
    // One script command running another one is still one step.
    assert_eq!(engine.run_command(&mut host, "my.twice"), Ok(Outcome::Done));
    assert_eq!(pixel(&host, 3, 0), RED);
    assert_eq!(steps(&host), ["Dot", "Twice"]);
    // Script commands never went through the application.
    assert!(host.ran.is_empty());

    let err = engine.run_command(&mut host, "my.none").unwrap_err();
    assert!(err.0.contains("there is no script command"), "{err}");
}

#[test]
fn a_script_that_fails_to_load_registers_nothing() {
    let (mut engine, mut host) = setup();
    engine
        .load(
            &mut host,
            "good.lua",
            "register_command{ id = 'good', run = function() end }",
        )
        .unwrap();
    let code = "
        register_command{ id = 'half', run = function() end }
        register_tool{ id = 'half_tool' }
        error('gave up')";
    assert!(engine.load(&mut host, "bad.lua", code).is_err());
    let ids: Vec<String> = engine.commands().into_iter().map(|c| c.id).collect();
    assert_eq!(ids, ["good"]);
    assert!(engine.tools().is_empty());
    // The id is free again.
    engine
        .load(
            &mut host,
            "again.lua",
            "register_command{ id = 'half', run = function() end }",
        )
        .unwrap();

    for (code, reason) in [
        (
            "register_command{ id = 'good', run = function() end }",
            "registered already",
        ),
        ("register_command{ run = function() end }", "needs an id"),
        ("register_command{ id = 'x' }", "needs a run function"),
        ("register_command('x')", "takes a table"),
        (
            "register_tool{ id = 't', on_down = 5 }",
            "must be a function",
        ),
        ("register_filter{ id = 'f' }", "needs a run function"),
    ] {
        let err = engine.load(&mut host, "wrong.lua", code).unwrap_err();
        assert!(err.0.starts_with("wrong.lua:1:"), "{err}");
        assert!(err.0.contains(reason), "{err}");
    }
}

#[test]
fn scripts_keep_their_globals_to_themselves() {
    let (mut engine, mut host) = setup();
    engine.load(&mut host, "a.lua", "shared = 1").unwrap();
    engine
        .load(&mut host, "b.lua", "print(shared == nil)")
        .unwrap();
    assert_eq!(engine.take_output(), ["true"]);
    assert_eq!(eval(&mut engine, &mut host, "shared"), ["nil"]);
}

#[test]
fn a_filter_changes_the_active_cel_as_one_step() {
    let (mut engine, mut host) = setup();
    paint(&mut host.editor, Rect::new(2, 2, 1, 1), BLUE);
    let code = "
        register_filter{ id = 'my.red', name = 'Redden', run = function(image)
            for y = 0, image.height - 1 do
                for x = 0, image.width - 1 do
                    if image:get_pixel(x, y).a > 0 then image:set_pixel(x, y, '#ff0000') end
                end
            end
        end }
        register_filter{ id = 'my.new', name = 'Shift', run = function(image)
            local moved = image:clone()
            for y = 0, 7 do for x = 0, 7 do moved:set_pixel(x, y, image:get_pixel(x - 1, y)) end end
            return moved
        end }
        register_filter{ id = 'my.bad', name = 'Bad', run = function(image)
            image:set_pixel(0, 0, '#ff0000')
            error('no')
        end }";
    engine.load(&mut host, "filters.lua", code).unwrap();
    let before = doc(&host).sprite().clone();

    assert_eq!(engine.run_command(&mut host, "my.red"), Ok(Outcome::Done));
    assert_eq!(pixel(&host, 2, 2), RED);
    assert_eq!(steps(&host), ["Paint", "Redden"]);
    host.editor.document_mut().unwrap().undo();
    assert_eq!(*doc(&host).sprite(), before);

    // A filter may hand back another image instead of changing the one it got.
    assert_eq!(engine.run_command(&mut host, "my.new"), Ok(Outcome::Done));
    assert_eq!(pixel(&host, 3, 2), BLUE);
    assert_eq!(pixel(&host, 2, 2), Rgba8::TRANSPARENT);
    assert_eq!(steps(&host), ["Paint", "Shift"]);

    let after = doc(&host).sprite().clone();
    assert!(engine.run_command(&mut host, "my.bad").is_err());
    assert_eq!(*doc(&host).sprite(), after);
    assert_eq!(steps(&host), ["Paint", "Shift"]);
}

#[test]
fn a_tool_stroke_is_one_undo_step() {
    let (mut engine, mut host) = setup();
    host.editor.primary = RED;
    let code = "
        local function dot(event) app.active_cel:set_pixel(event.x, event.y, event.color) end
        register_tool{ id = 'dots', name = 'Dots', on_down = dot, on_move = dot, on_up = function(event)
            print(event.button, event.shift, event.x)
        end }
        register_tool{ id = 'quiet' }";
    engine.load(&mut host, "tools.lua", code).unwrap();
    assert_eq!(engine.tools().len(), 2);
    let before = doc(&host).sprite().clone();

    engine
        .tool_event(&mut host, "dots", ToolPhase::Down, at(1, 1))
        .unwrap();
    engine
        .tool_event(&mut host, "dots", ToolPhase::Move, at(2, 1))
        .unwrap();
    engine
        .tool_event(&mut host, "dots", ToolPhase::Move, at(3, 1))
        .unwrap();
    // The stroke shows while it is drawn, and cannot be undone half way.
    assert_eq!(pixel(&host, 2, 1), RED);
    assert!(!doc(&host).can_undo());
    engine
        .tool_event(&mut host, "dots", ToolPhase::Up, at(3, 1))
        .unwrap();
    assert_eq!(engine.take_output(), ["primary\tfalse\t3"]);
    assert_eq!(steps(&host), ["Dots"]);
    assert_eq!(pixel(&host, 3, 1), RED);
    host.editor.document_mut().unwrap().undo();
    assert_eq!(*doc(&host).sprite(), before);

    // A cancelled stroke leaves nothing.
    engine
        .tool_event(&mut host, "dots", ToolPhase::Down, at(5, 5))
        .unwrap();
    engine.tool_cancel(&mut host);
    assert_eq!(*doc(&host).sprite(), before);
    assert!(!doc(&host).in_group());
    // Stray events without a press are ignored, and a tool without handlers does nothing.
    engine
        .tool_event(&mut host, "dots", ToolPhase::Move, at(5, 5))
        .unwrap();
    engine
        .tool_event(&mut host, "dots", ToolPhase::Up, at(5, 5))
        .unwrap();
    engine
        .tool_event(&mut host, "quiet", ToolPhase::Down, at(5, 5))
        .unwrap();
    engine
        .tool_event(&mut host, "quiet", ToolPhase::Up, at(5, 5))
        .unwrap();
    assert_eq!(*doc(&host).sprite(), before);
    assert!(!doc(&host).can_undo());
}

#[test]
fn a_tool_that_fails_takes_its_stroke_back() {
    let (mut engine, mut host) = setup();
    let code = "
        register_tool{ id = 'fragile', on_down = function(event)
            app.active_cel:set_pixel(event.x, event.y, '#ff0000')
        end, on_move = function(event)
            app.active_cel:set_pixel(event.x, event.y, '#ff0000')
            error('slipped')
        end }";
    engine.load(&mut host, "fragile.lua", code).unwrap();
    engine
        .tool_event(&mut host, "fragile", ToolPhase::Down, at(1, 1))
        .unwrap();
    let err = engine
        .tool_event(&mut host, "fragile", ToolPhase::Move, at(2, 1))
        .unwrap_err();
    assert!(err.0.starts_with("fragile.lua:6:"), "{err}");
    // The rest of the stroke is ignored, quietly.
    engine
        .tool_event(&mut host, "fragile", ToolPhase::Move, at(3, 1))
        .unwrap();
    engine
        .tool_event(&mut host, "fragile", ToolPhase::Up, at(3, 1))
        .unwrap();
    assert_eq!(pixel(&host, 1, 1), Rgba8::TRANSPARENT);
    assert_eq!(steps(&host), [] as [&str; 0]);
    assert!(!doc(&host).in_group());
}

fn dialog_script() -> &'static str {
    "
    register_command{ id = 'my.ask', run = function()
        local answer = Dialog('Settings')
            :number{ id = 'size', label = 'Size', value = 3, min = 1, max = 8 }
            :number{ id = 'ratio', value = 0.5, decimals = 2 }
            :color{ id = 'ink', value = '#ff0000' }
            :text{ id = 'name', value = 'abc' }
            :check{ id = 'on', value = true }
            :dropdown{ id = 'mode', options = { 'one', 'two' }, value = 'two' }
            :show()
        if not answer then print('cancelled') return end
        print(answer.size, math.type(answer.size), answer.ratio, answer.ink, answer.name, answer.on, answer.mode)
        app.transaction('After', function() app.active_cel:set_pixel(answer.size, 0, answer.ink) end)
    end }"
}

#[test]
fn a_dialog_pauses_the_script_until_it_is_answered() {
    let (mut engine, mut host) = setup();
    engine.load(&mut host, "ask.lua", dialog_script()).unwrap();
    let Ok(Outcome::Dialog(mut spec)) = engine.run_command(&mut host, "my.ask") else {
        panic!("no dialog");
    };
    assert!(engine.is_waiting());
    assert_eq!(spec.title, "Settings");
    let ids: Vec<&str> = spec.fields.iter().map(|field| field.id.as_str()).collect();
    assert_eq!(ids, ["size", "ratio", "ink", "name", "on", "mode"]);
    assert_eq!(spec.fields[0].label, "Size");
    assert_eq!(spec.fields[1].label, "ratio");
    assert_eq!(
        spec.fields[0].kind,
        FieldKind::Number {
            value: 3.0,
            min: 1.0,
            max: 8.0,
            decimals: 0
        }
    );
    assert_eq!(spec.fields[2].kind, FieldKind::Color(RED));
    assert_eq!(
        spec.fields[5].kind,
        FieldKind::Dropdown {
            options: vec!["one".to_owned(), "two".to_owned()],
            selected: 1
        }
    );

    // The user changes two fields and confirms.
    if let FieldKind::Number { value, .. } = &mut spec.fields[0].kind {
        *value = 5.0;
    }
    spec.fields[2].kind = FieldKind::Color(BLUE);
    assert_eq!(engine.resume(&mut host, Some(&spec)), Ok(Outcome::Done));
    assert!(!engine.is_waiting());
    assert_eq!(
        engine.take_output(),
        ["5\tinteger\t0.5\t#0000ff\tabc\ttrue\ttwo"]
    );
    assert_eq!(pixel(&host, 5, 0), BLUE);
    assert_eq!(steps(&host), ["After"]);

    // Cancelling gives the script nothing.
    assert!(matches!(
        engine.run_command(&mut host, "my.ask"),
        Ok(Outcome::Dialog(_))
    ));
    assert_eq!(engine.resume(&mut host, None), Ok(Outcome::Done));
    assert_eq!(engine.take_output(), ["cancelled"]);
    // With nothing waiting, an answer goes nowhere.
    assert_eq!(engine.resume(&mut host, None), Ok(Outcome::Done));
    // Starting something else drops a script that still waits.
    assert!(matches!(
        engine.run_command(&mut host, "my.ask"),
        Ok(Outcome::Dialog(_))
    ));
    eval(&mut engine, &mut host, "1");
    assert!(!engine.is_waiting());
}

#[test]
fn a_dialog_cannot_be_shown_everywhere() {
    let (mut engine, mut host) = setup();
    // Not while a script file is loading.
    let err = engine
        .load(&mut host, "early.lua", "Dialog('x'):show()")
        .unwrap_err();
    assert_eq!(err.0, "early.lua:1: a dialog cannot be shown here");
    // Not inside a transaction: the undo step would stay open while the user thinks.
    let code = "app.transaction('x', function() Dialog('x'):show() end)";
    assert!(eval_error(&mut engine, &mut host, code).contains("cannot be shown here"));
    assert!(!doc(&host).in_group());
    // Not from a tool.
    let code = "register_tool{ id = 'asks', on_down = function() Dialog('x'):show() end }";
    engine.load(&mut host, "asks.lua", code).unwrap();
    let err = engine
        .tool_event(&mut host, "asks", ToolPhase::Down, at(0, 0))
        .unwrap_err();
    assert!(err.0.contains("cannot be shown here"), "{err}");
    engine
        .tool_event(&mut host, "asks", ToolPhase::Up, at(0, 0))
        .unwrap();

    // A dialog that makes no sense is refused where it is shown.
    for (code, reason) in [
        (
            "local _ = Dialog('x'):dropdown{ id = 'a' }:show()",
            "needs options",
        ),
        (
            "local _ = Dialog('x'):number{ id = 'a', value = 'b' }:show()",
            "must be a number",
        ),
        (
            "local _ = Dialog('x'):text{ id = 'a' }:text{ id = 'a' }:show()",
            "two fields",
        ),
        (
            "local _ = Dialog('x'):color{ id = 'a', value = 'zz' }:show()",
            "not a colour",
        ),
        ("local _ = Dialog('x'):check{ label = 'a' }", "needs an id"),
    ] {
        let message = eval_error(&mut engine, &mut host, code);
        assert!(message.starts_with("console:1:"), "{message}");
        assert!(message.contains(reason), "{message}");
    }
    // Pausing for anything else is an error, not a hang.
    let message = eval_error(&mut engine, &mut host, "coroutine.yield(1)");
    assert!(message.contains("only pause to show a dialog"), "{message}");
}

#[test]
fn commands_get_their_parameters() {
    let (mut engine, mut host) = setup();
    eval(
        &mut engine,
        &mut host,
        "app.command('layer.new') app.command('x.y', { on = true, n = 3, whole = 6 / 2, f = 0.5, s = 'text', c = Color(255, 0, 0) })",
    );
    assert_eq!(host.ran[0], ("layer.new".to_owned(), Params::new()));
    let (id, params) = &host.ran[1];
    assert_eq!(id, "x.y");
    assert_eq!(params.get("on"), Some(&Value::Bool(true)));
    assert_eq!(params.get("n"), Some(&Value::Int(3)));
    assert_eq!(params.get("whole"), Some(&Value::Int(3)));
    assert_eq!(params.get("f"), Some(&Value::Float(0.5)));
    assert_eq!(params.get("s"), Some(&Value::Text("text".to_owned())));
    assert_eq!(params.get("c"), Some(&Value::Color(RED)));

    assert_eq!(
        eval_error(&mut engine, &mut host, "app.command('test.fail')"),
        "it did not work"
    );
    assert!(
        eval_error(&mut engine, &mut host, "app.command('x', { f = print })")
            .contains("cannot be a function")
    );
    assert!(eval_error(&mut engine, &mut host, "app.command()").contains("needs the id"));
}

#[test]
fn files_can_only_be_named_inside_a_folder_the_user_picked() {
    let (mut engine, mut host) = setup();
    let folder = std::env::temp_dir().join("acms-script-test");
    let inside = folder
        .join("out.gif")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let export = format!("app.command('file.export_gif', {{ path = '{inside}' }})");

    // Nothing was picked: no file may be named, whatever the command.
    let message = eval_error(&mut engine, &mut host, &export);
    assert!(message.contains("app.choose_folder"), "{message}");
    assert!(host.ran.is_empty());
    assert!(
        eval_error(
            &mut engine,
            &mut host,
            "app.command('file.open', { path = 5 })"
        )
        .contains("app.choose_folder")
    );

    // The user declines.
    assert_eq!(
        eval(&mut engine, &mut host, "app.choose_folder('Where?')"),
        ["nil"]
    );
    assert!(engine.eval(&mut host, &export).is_err());

    // The user picks the folder: files in it are fine, others are not.
    host.folder = Some(folder.clone());
    let code = "local folder = app.choose_folder()
         app.command('file.export_gif', { path = app.join_path(folder, 'out.gif') })
         local ok, why = pcall(app.command, 'file.export_gif', { path = app.join_path(folder, '../out.gif') })
         print(ok, why)";
    let printed = eval(&mut engine, &mut host, code);
    assert!(printed[0].starts_with("false\t"), "{printed:?}");
    assert_eq!(host.ran.len(), 1);
    assert_eq!(
        host.ran[0].1.text("path"),
        Some(folder.join("out.gif").to_string_lossy().as_ref())
    );

    // The permission ends with the run it was given in.
    assert!(engine.eval(&mut host, &export).is_err());
    assert_eq!(host.ran.len(), 1);
}

#[test]
fn the_bundled_scripts_load_and_register_what_they_say() {
    let (mut engine, mut host) = setup();
    for (name, code) in BUNDLED {
        engine
            .load(&mut host, name, code)
            .unwrap_or_else(|err| panic!("{name}: {err}"));
    }
    let commands: Vec<String> = engine.commands().into_iter().map(|c| c.id).collect();
    assert_eq!(
        commands,
        [
            "example.outline",
            "example.palette_swap",
            "example.mirror_sprite",
            "example.export_tags"
        ]
    );
    let tools: Vec<String> = engine.tools().into_iter().map(|t| t.id).collect();
    assert_eq!(tools, ["spray"]);
    assert_eq!(LayerId(1), doc(&host).active_layer());
}
