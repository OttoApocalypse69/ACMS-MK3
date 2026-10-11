//! The command registry (see `PLAN.md`, section 4.2).
//!
//! Every action in the app is a [`Command`]. Menus, shortcuts, the command palette and scripts
//! all look commands up in one [`CommandRegistry`] and run them through it.
//!
//! The registry is generic over the context `C` a command reads and changes, so this crate does
//! not need to know anything about the application that hosts it.

use std::collections::HashMap;

use thiserror::Error;

/// Why a command could not be registered or run.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("unknown command `{0}`")]
    Unknown(String),
    #[error("command `{0}` is already registered")]
    Duplicate(String),
    #[error("command `{0}` is not available right now")]
    Disabled(String),
}

type EnabledFn<C> = Box<dyn Fn(&C) -> bool>;
type RunFn<C> = Box<dyn Fn(&mut C)>;

/// One action: a stable id (`"layer.new"`), a display name, an enabled check and a run function.
pub struct Command<C> {
    id: String,
    name: String,
    enabled: Option<EnabledFn<C>>,
    run: RunFn<C>,
}

impl<C> Command<C> {
    /// Creates a command that is always enabled.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        run: impl Fn(&mut C) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: None,
            run: Box::new(run),
        }
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

    pub fn is_enabled(&self, ctx: &C) -> bool {
        self.enabled.as_ref().is_none_or(|enabled| enabled(ctx))
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

    /// Runs the command with the given id, if it exists and is enabled for `ctx`.
    pub fn run(&self, id: &str, ctx: &mut C) -> Result<(), CommandError> {
        let command = self
            .get(id)
            .ok_or_else(|| CommandError::Unknown(id.to_owned()))?;
        if !command.is_enabled(ctx) {
            return Err(CommandError::Disabled(id.to_owned()));
        }
        (command.run)(ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry<i32> {
        let mut registry = CommandRegistry::new();
        registry
            .register(Command::new("count.increment", "Increment", |n| *n += 1))
            .unwrap();
        registry
            .register(Command::new("count.reset", "Reset", |n| *n = 0).enabled_when(|n| *n != 0))
            .unwrap();
        registry
    }

    #[test]
    fn runs_a_command_against_the_context() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(registry.run("count.increment", &mut n), Ok(()));
        assert_eq!(registry.run("count.increment", &mut n), Ok(()));
        assert_eq!(n, 2);
    }

    #[test]
    fn disabled_command_does_not_run() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.reset", &mut n),
            Err(CommandError::Disabled("count.reset".to_owned()))
        );
        n = 5;
        assert_eq!(registry.run("count.reset", &mut n), Ok(()));
        assert_eq!(n, 0);
    }

    #[test]
    fn unknown_command_is_an_error() {
        let registry = registry();
        let mut n = 0;
        assert_eq!(
            registry.run("count.nope", &mut n),
            Err(CommandError::Unknown("count.nope".to_owned()))
        );
        assert_eq!(n, 0);
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut registry = registry();
        let result = registry.register(Command::new("count.increment", "Again", |_| {}));
        assert_eq!(
            result,
            Err(CommandError::Duplicate("count.increment".to_owned()))
        );
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn iterates_in_registration_order() {
        let registry = registry();
        let ids: Vec<&str> = registry.iter().map(Command::id).collect();
        assert_eq!(ids, ["count.increment", "count.reset"]);
        assert_eq!(
            registry.get("count.reset").map(Command::name),
            Some("Reset")
        );
    }
}
