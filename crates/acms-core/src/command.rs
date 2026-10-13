//! The command registry (see `PLAN.md`, section 4.2).
//!
//! Every action in the app is a [`Command`]. Menus, shortcuts, the command palette and scripts
//! all look commands up in one [`CommandRegistry`] and run them through it.
//!
//! The registry is generic over the context `C` a command reads and changes, so this crate does
//! not need to know anything about the application that hosts it.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use thiserror::Error;

use crate::color::Rgba8;

/// Why a command could not be registered or run.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    #[error("command `{0}` is already registered")]
    Duplicate(String),
    #[error("command `{0}` is not available right now")]
    Disabled(String),
    #[error("command `{command}`: {reason}")]
    BadParams { command: String, reason: String },
    /// The command ran and reported a problem.
    #[error("{0}")]
    Failed(String),
}

impl CommandError {
    pub fn failed(reason: impl fmt::Display) -> Self {
        Self::Failed(reason.to_string())
    }
}

/// The type of a command parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Text,
    Color,
}

impl fmt::Display for ParamKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bool => "true or false",
            Self::Int => "a whole number",
            Self::Float => "a number",
            Self::Text => "text",
            Self::Color => "a colour",
        })
    }
}

/// The value of a command parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Color(Rgba8),
}

impl Value {
    /// This value as the given kind, if it is one or can be read as one: a whole number is
    /// also a number, and text like `#FF8800` is also a colour.
    fn into_kind(self, kind: ParamKind) -> Option<Self> {
        match (kind, self) {
            (ParamKind::Bool, value @ Self::Bool(_))
            | (ParamKind::Int, value @ Self::Int(_))
            | (ParamKind::Float, value @ Self::Float(_))
            | (ParamKind::Text, value @ Self::Text(_))
            | (ParamKind::Color, value @ Self::Color(_)) => Some(value),
            (ParamKind::Float, Self::Int(n)) => Some(Self::Float(n as f64)),
            (ParamKind::Color, Self::Text(text)) => Rgba8::from_hex(&text).map(Self::Color),
            _ => None,
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<Rgba8> for Value {
    fn from(value: Rgba8) -> Self {
        Self::Color(value)
    }
}

/// Named arguments for one run of a command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params(BTreeMap<String, Value>);

impl Params {
    /// No arguments.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an argument, builder style.
    pub fn with(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.0.insert(name.to_owned(), value.into());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every parameter with its value, in order of name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn float(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            Value::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name)? {
            Value::Text(value) => Some(value),
            _ => None,
        }
    }

    pub fn color(&self, name: &str) -> Option<Rgba8> {
        match self.get(name)? {
            Value::Color(value) => Some(*value),
            _ => None,
        }
    }
}

/// One parameter a command accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
}

type EnabledFn<C> = Box<dyn Fn(&C) -> bool>;
type RunFn<C> = Box<dyn Fn(&mut C, &Params) -> Result<(), CommandError>>;

/// One action: a stable id (`"layer.new"`), a display name, typed parameters, an enabled
/// check and a run function.
pub struct Command<C> {
    id: String,
    name: String,
    params: Vec<ParamSpec>,
    enabled: Option<EnabledFn<C>>,
    run: RunFn<C>,
}

impl<C> Command<C> {
    /// Creates a command that is always enabled and takes no parameters.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        run: impl Fn(&mut C, &Params) -> Result<(), CommandError> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            params: Vec::new(),
            enabled: None,
            run: Box::new(run),
        }
    }

    /// Declares a parameter the command cannot run without.
    pub fn param(mut self, name: &'static str, kind: ParamKind) -> Self {
        self.params.push(ParamSpec {
            name,
            kind,
            required: true,
        });
        self
    }

    /// Declares a parameter that may be left out.
    pub fn optional_param(mut self, name: &'static str, kind: ParamKind) -> Self {
        self.params.push(ParamSpec {
            name,
            kind,
            required: false,
        });
        self
    }

    /// Restricts the command to contexts for which `enabled` returns `true`.
    pub fn enabled_when(mut self, enabled: impl Fn(&C) -> bool + 'static) -> Self {
        self.enabled = Some(Box::new(enabled));
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn params(&self) -> &[ParamSpec] {
        &self.params
    }

    /// Whether the command can be run without being given any arguments, as from a menu or
    /// the command palette.
    pub fn runs_without_params(&self) -> bool {
        self.params.iter().all(|spec| !spec.required)
    }

    pub fn is_enabled(&self, ctx: &C) -> bool {
        self.enabled.as_ref().is_none_or(|enabled| enabled(ctx))
    }

    /// Checks `params` against the declared parameters and returns them with every value
    /// converted to its declared type.
    fn check_params(&self, params: &Params) -> Result<Params, CommandError> {
        let bad = |reason: String| CommandError::BadParams {
            command: self.id.clone(),
            reason,
        };

        let mut checked = Params::new();
        for (name, value) in &params.0 {
            let spec = self
                .params
                .iter()
                .find(|spec| spec.name == name)
                .ok_or_else(|| bad(format!("there is no parameter `{name}`")))?;
            let value = value
                .clone()
                .into_kind(spec.kind)
                .ok_or_else(|| bad(format!("`{name}` must be {}", spec.kind)))?;
            checked.0.insert(name.clone(), value);
        }
        for spec in self.params.iter().filter(|spec| spec.required) {
            if checked.get(spec.name).is_none() {
                return Err(bad(format!("`{}` is missing", spec.name)));
            }
        }
        Ok(checked)
    }
}

/// Holds all commands, in registration order.
pub struct CommandRegistry<C> {
    commands: Vec<Command<C>>,
    by_id: HashMap<String, usize>,
}

impl<C> Default for CommandRegistry<C> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            by_id: HashMap::new(),
        }
    }
}

impl<C> CommandRegistry<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a command. Ids must be unique.
    pub fn register(&mut self, command: Command<C>) -> Result<(), CommandError> {
        if self.by_id.contains_key(command.id()) {
            return Err(CommandError::Duplicate(command.id().to_owned()));
        }
        self.by_id
            .insert(command.id().to_owned(), self.commands.len());
        self.commands.push(command);
        Ok(())
    }

    /// Takes a command out again. Returns `false` if there is none with that id.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.by_id.remove(id) else {
            return false;
        };
        self.commands.remove(index);
        for later in self.by_id.values_mut().filter(|at| **at > index) {
            *later -= 1;
        }
        true
    }

    pub fn get(&self, id: &str) -> Option<&Command<C>> {
        self.by_id.get(id).map(|&index| &self.commands[index])
    }

    /// All commands, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Command<C>> {
        self.commands.iter()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Runs the command with the given id, if it exists, accepts `params` and is enabled for
    /// `ctx`.
    pub fn run(&self, id: &str, ctx: &mut C, params: &Params) -> Result<(), CommandError> {
        let command = self
            .get(id)
            .ok_or_else(|| CommandError::Unknown(id.to_owned()))?;
        let params = command.check_params(params)?;
        if !command.is_enabled(ctx) {
            return Err(CommandError::Disabled(id.to_owned()));
        }
        (command.run)(ctx, &params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry<i64> {
        let mut registry = CommandRegistry::new();
        registry
            .register(Command::new("count.increment", "Increment", |n, _| {
                *n += 1;
                Ok(())
            }))
            .unwrap();
        registry
            .register(
                Command::new("count.reset", "Reset", |n, _| {
                    *n = 0;
                    Ok(())
                })
                .enabled_when(|n| *n != 0),
            )
            .unwrap();
        registry
            .register(
                Command::new("count.add", "Add", |n: &mut i64, params: &Params| {
                    let amount = params.int("amount").unwrap_or(0);
                    let times = params.int("times").unwrap_or(1);
                    *n += amount * times;
                    Ok(())
                })
                .param("amount", ParamKind::Int)
                .optional_param("times", ParamKind::Int),
            )
            .unwrap();
        registry
            .register(Command::new("count.fail", "Fail", |_, _| {
                Err(CommandError::failed("it broke"))
            }))
            .unwrap();
        registry
    }

    fn bad_params(command: &str, reason: &str) -> Result<(), CommandError> {
        Err(CommandError::BadParams {
            command: command.to_owned(),
            reason: reason.to_owned(),
        })
    }

    #[test]
    fn runs_a_command_against_the_context() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.increment", &mut n, &Params::new()),
            Ok(())
        );
        assert_eq!(
            registry.run("count.increment", &mut n, &Params::new()),
            Ok(())
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn disabled_command_does_not_run() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.reset", &mut n, &Params::new()),
            Err(CommandError::Disabled("count.reset".to_owned()))
        );
        n = 5;
        assert_eq!(registry.run("count.reset", &mut n, &Params::new()), Ok(()));
        assert_eq!(n, 0);
    }

    #[test]
    fn unknown_command_is_an_error() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.nope", &mut n, &Params::new()),
            Err(CommandError::Unknown("count.nope".to_owned()))
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut registry = registry();
        let result = registry.register(Command::new("count.increment", "Again", |_, _| Ok(())));
        assert_eq!(
            result,
            Err(CommandError::Duplicate("count.increment".to_owned()))
        );
        assert_eq!(registry.len(), 4);
    }

    #[test]
    fn a_command_can_be_removed_and_registered_again() {
        let mut registry = registry();
        let count = registry.len();
        assert!(registry.remove("count.reset"));
        assert!(!registry.remove("count.reset"));
        assert_eq!(registry.len(), count - 1);
        assert!(registry.get("count.reset").is_none());
        // The others are still found under their own ids.
        for command in registry.iter() {
            let found = registry.get(command.id()).map(Command::id);
            assert_eq!(found, Some(command.id()));
        }
        let mut n = 0;
        let two = Params::new().with("amount", 2_i64);
        registry.run("count.add", &mut n, &two).unwrap();
        assert_eq!(n, 2);
        registry
            .register(Command::new("count.reset", "Reset", |n, _| {
                *n = 0;
                Ok(())
            }))
            .unwrap();
        registry.run("count.reset", &mut n, &Params::new()).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn iterates_in_registration_order() {
        let registry = registry();
        let ids: Vec<&str> = registry.iter().map(Command::id).collect();
        assert_eq!(
            ids,
            ["count.increment", "count.reset", "count.add", "count.fail"]
        );
        assert_eq!(
            registry.get("count.reset").map(Command::name),
            Some("Reset")
        );
    }

    #[test]
    fn parameters_reach_the_command() {
        let registry = registry();
        let mut n = 0;
        let params = Params::new().with("amount", 5_i64);
        assert_eq!(registry.run("count.add", &mut n, &params), Ok(()));
        assert_eq!(n, 5);

        let params = Params::new().with("amount", 2_i64).with("times", 10_i64);
        assert_eq!(registry.run("count.add", &mut n, &params), Ok(()));
        assert_eq!(n, 25);
    }

    #[test]
    fn wrong_parameters_are_rejected_before_the_command_runs() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.add", &mut n, &Params::new()),
            bad_params("count.add", "`amount` is missing")
        );
        assert_eq!(
            registry.run("count.add", &mut n, &Params::new().with("amount", "five")),
            bad_params("count.add", "`amount` must be a whole number")
        );
        assert_eq!(
            registry.run(
                "count.add",
                &mut n,
                &Params::new().with("amount", 1_i64).with("extra", true)
            ),
            bad_params("count.add", "there is no parameter `extra`")
        );
        assert_eq!(
            registry.run("count.increment", &mut n, &Params::new().with("x", 1_i64)),
            bad_params("count.increment", "there is no parameter `x`")
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn values_convert_to_the_declared_type_where_that_is_safe() {
        let mut registry = CommandRegistry::<Vec<Value>>::new();
        registry
            .register(
                Command::new("push", "Push", |seen: &mut Vec<Value>, params: &Params| {
                    seen.extend(params.get("ratio").cloned());
                    seen.extend(params.get("tint").cloned());
                    Ok(())
                })
                .optional_param("ratio", ParamKind::Float)
                .optional_param("tint", ParamKind::Color),
            )
            .unwrap();

        let mut seen = Vec::new();
        let params = Params::new().with("ratio", 2_i64).with("tint", "#FF8000");
        registry.run("push", &mut seen, &params).unwrap();
        assert_eq!(
            seen,
            [Value::Float(2.0), Value::Color(Rgba8::rgb(255, 128, 0))]
        );

        // A number is not a whole number, and not every text is a colour.
        assert!(
            registry
                .run("push", &mut seen, &Params::new().with("tint", "reddish"))
                .is_err()
        );
        let registry = registry_with_int();
        assert!(
            registry
                .run("n", &mut 0, &Params::new().with("n", 1.5))
                .is_err()
        );
    }

    fn registry_with_int() -> CommandRegistry<i64> {
        let mut registry = CommandRegistry::new();
        registry
            .register(Command::new("n", "N", |_, _| Ok(())).param("n", ParamKind::Int))
            .unwrap();
        registry
    }

    #[test]
    fn a_failing_command_reports_its_reason() {
        let registry = registry();
        let result = registry.run("count.fail", &mut 0, &Params::new());
        assert_eq!(result, Err(CommandError::Failed("it broke".to_owned())));
        assert_eq!(result.unwrap_err().to_string(), "it broke");
    }

    #[test]
    fn only_commands_without_required_parameters_run_bare() {
        let registry = registry();
        let bare: Vec<&str> = registry
            .iter()
            .filter(|command| command.runs_without_params())
            .map(Command::id)
            .collect();
        assert_eq!(bare, ["count.increment", "count.reset", "count.fail"]);
    }

    #[test]
    fn typed_getters_only_return_their_own_type() {
        let params = Params::new()
            .with("flag", true)
            .with("count", 3_u32)
            .with("ratio", 0.5)
            .with("label", "hi")
            .with("tint", Rgba8::WHITE);
        assert_eq!(params.bool("flag"), Some(true));
        assert_eq!(params.int("count"), Some(3));
        assert_eq!(params.float("ratio"), Some(0.5));
        assert_eq!(params.text("label"), Some("hi"));
        assert_eq!(params.color("tint"), Some(Rgba8::WHITE));
        assert_eq!(params.int("flag"), None);
        assert_eq!(params.text("missing"), None);
        assert!(!params.is_empty());
    }
}
