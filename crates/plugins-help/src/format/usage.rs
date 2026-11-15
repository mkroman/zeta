//! Deriving usage synopses and argument labels from a command's `clap` metadata.

use clap::{Arg, ArgAction, Command};

/// Builds the usage synopsis of a command from its argument information.
///
/// Optional flags and arguments are bracketed and repeating ones are suffixed with `...`;
/// subcommands are summarized rather than listed.
pub(super) fn usage(prefix: &str, info: &Command) -> String {
    let mut usage = prefix.to_string();

    if info.has_subcommands() {
        usage.push_str(" <command> [<args>]");

        return usage;
    }

    for flag in visible_flags(info) {
        usage.push(' ');
        usage.push_str(&flag_usage(flag));
    }

    for positional in visible_positionals(info) {
        usage.push(' ');
        usage.push_str(&positional_label(positional));
    }

    usage
}

/// Returns the flags that should appear in help output.
pub(super) fn visible_flags(info: &Command) -> impl Iterator<Item = &Arg> {
    info.get_opts().filter(|flag| !flag.is_hide_set())
}

/// Returns the positional arguments that should appear in help output.
pub(super) fn visible_positionals(info: &Command) -> impl Iterator<Item = &Arg> {
    info.get_positionals()
        .filter(|positional| !positional.is_hide_set())
}

/// Returns the subcommands that should appear in help output.
pub(super) fn visible_subcommands(info: &Command) -> impl Iterator<Item = &Command> {
    info.get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
}

/// Whether the argument accepts more than one value.
fn is_repeating(argument: &Arg) -> bool {
    matches!(*argument.get_action(), ArgAction::Append)
        || argument
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1)
}

/// Formats an argument's long form, e.g. `--short`, when it has one.
fn long_flag(argument: &Arg) -> Option<String> {
    argument.get_long().map(|long| format!("--{long}"))
}

/// Returns the name clap uses for an argument's value, e.g. `HOST`.
///
/// clap derives a value name from the field name and lets the plugin type override it, so prefer
/// it over the internal id, which only carries the Rust field name.
fn value_name_text(argument: &Arg) -> String {
    argument
        .get_value_names()
        .and_then(|names| names.first())
        .map_or_else(|| argument.get_id().to_string(), ToString::to_string)
}

/// Formats an argument's value placeholder, e.g. `<HOST>`.
fn value_name(argument: &Arg) -> String {
    format!("<{}>", value_name_text(argument))
}

/// Formats an argument's value placeholder, marking a repeating argument with `...`.
fn value_label(argument: &Arg) -> String {
    if is_repeating(argument) {
        format!("<{}...>", value_name_text(argument))
    } else {
        value_name(argument)
    }
}

/// Formats a flag for the usage synopsis, using its short form when it has one.
fn flag_usage(flag: &Arg) -> String {
    let mut label = flag.get_short().map_or_else(
        || long_flag(flag).unwrap_or_else(|| format!("--{}", flag.get_id())),
        |short| format!("-{short}"),
    );

    if flag.get_action().takes_values() {
        label.push(' ');
        label.push_str(&value_label(flag));
    }

    if flag.is_required_set() {
        label
    } else {
        format!("[{label}]")
    }
}

/// Formats a flag for the argument list, showing both its short and long forms.
///
/// A short-only argument is rendered as just `-x`; clap does not accept the long form that the
/// field id would suggest. Arguments with neither form fall back to the id so the list stays
/// readable.
pub(super) fn flag_label(flag: &Arg) -> String {
    let mut label = String::new();

    if let Some(short) = flag.get_short() {
        label.push('-');
        label.push(short);
    }

    if let Some(long) = long_flag(flag) {
        if !label.is_empty() {
            label.push_str(", ");
        }

        label.push_str(&long);
    }

    if label.is_empty() {
        label.push_str("--");
        label.push_str(flag.get_id().as_str());
    }

    if flag.get_action().takes_values() {
        label.push(' ');
        label.push_str(&value_name(flag));
    }

    label
}

/// Formats a positional argument, marking its optionality and repetition.
pub(super) fn positional_label(positional: &Arg) -> String {
    let label = value_label(positional);

    if positional.is_required_set() {
        label
    } else {
        format!("[{label}]")
    }
}
