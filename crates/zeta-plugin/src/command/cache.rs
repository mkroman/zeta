//! The process-global cache of configured `clap` commands.
//!
//! [`Command`] is `Send`, and [`Command::try_get_matches_from_mut`] is clap's API for parsing
//! with a command that stays alive, so one configured tree per command is reused instead of
//! rebuilding the whole argument tree on every call. The trigger is part of the key because it
//! is rendered as the binary name in usage and help output; the argument type is keyed by
//! [`TypeId`], so parsers must be `'static`.
//!
//! The outer map uses a non-poisoning lock. A fresh command is configured inside `or_insert_with`
//! while the lock is held, but the entry API only inserts after the closure returns: a panic in
//! `configure` unwinds with the map unchanged rather than leaving a half-configured command
//! behind. The per-command `std::sync::Mutex` inside stays poisoning on purpose — a parse that
//! panicked must be detected so its command is evicted, not reused.
//!
//! The cache is process-global state keyed by trigger and argument type rather than data
//! belonging to one [`CommandSpec`](super::CommandSpec), so it lives beside the specification in
//! its own module with a single [`CACHE`] instance.

use std::any::TypeId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, TryLockError};

use clap::{ArgAction, ColorChoice, Command, Parser};

/// The cache key of a configured command: its trigger and the argument type it parses.
type CommandKey = (&'static str, TypeId);

/// The configured commands reused across parses, keyed by trigger and argument type.
pub(super) struct CommandCache {
    commands: parking_lot::Mutex<HashMap<CommandKey, Arc<Mutex<Command>>>>,
}

impl CommandCache {
    /// Creates an empty cache.
    fn new() -> Self {
        Self {
            commands: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Returns the cached command for `T` under `trigger`, configuring it on first use.
    pub(super) fn cached_command<T: Parser + 'static>(
        &self,
        trigger: &'static str,
    ) -> Arc<Mutex<Command>> {
        let mut commands = self.commands.lock();

        Arc::clone(
            commands
                .entry((trigger, TypeId::of::<T>()))
                .or_insert_with(|| Arc::new(Mutex::new(configure(T::command(), trigger)))),
        )
    }

    /// Runs `parse` against the cached command for `T` under `trigger`.
    ///
    /// A busy entry means another thread is parsing the same command (or a value parser recursed
    /// into it); rather than block, that call configures a fresh command. A poisoned entry comes
    /// from a parse that panicked: the per-command lock is a `std::sync::Mutex` on purpose, so the
    /// poison is observed and the entry evicted instead of reusing a command whose parse may have
    /// left it in an unknown state. This call then uses a fresh command directly.
    pub(super) fn with_cached_command<T, R>(
        &self,
        trigger: &'static str,
        parse: impl FnOnce(&mut Command) -> R,
    ) -> R
    where
        T: Parser + 'static,
    {
        match self.cached_command::<T>(trigger).try_lock() {
            Ok(mut command) => parse(&mut command),
            Err(TryLockError::Poisoned(_)) => {
                self.commands.lock().remove(&(trigger, TypeId::of::<T>()));

                parse(&mut configure(T::command(), trigger))
            }
            Err(TryLockError::WouldBlock) => parse(&mut configure(T::command(), trigger)),
        }
    }
}

/// The process-wide cache of configured commands, initialized on first use.
pub(super) static CACHE: LazyLock<CommandCache> = LazyLock::new(CommandCache::new);

/// Applies the settings shared by argument parsing and help introspection.
///
/// The command is named after the trigger so generated usage reads like the user typed it, the
/// implicit `help` subcommand is disabled (plugins provide their own `.help`), and styling is
/// turned off because replies are IRC messages, not terminal output.
pub(super) fn configure(command: Command, trigger: &'static str) -> Command {
    let command = command
        .bin_name(trigger)
        .disable_help_subcommand(true)
        .color(ColorChoice::Never);

    tolerate_repeated_switches(command)
}

/// Allows a switch to appear more than once.
///
/// clap reports an argument conflict for a second `SetTrue`/`SetFalse` occurrence, whereas the
/// parser this API replaced treated repeated switches as idempotent. Only switches are relaxed:
/// repeating an option that takes a value still reports a conflict, as before.
fn tolerate_repeated_switches(command: Command) -> Command {
    let command = command.mut_args(|argument| {
        if matches!(
            *argument.get_action(),
            ArgAction::SetTrue | ArgAction::SetFalse
        ) {
            let id = argument.get_id().clone();

            argument.overrides_with(id)
        } else {
            argument
        }
    });

    command.mut_subcommands(tolerate_repeated_switches)
}
