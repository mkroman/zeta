//! List, inspect, and change per-user settings.
//!
//! The `.settings` command is the user-facing surface of the account subsystem's settings
//! (`zeta_plugin::accounts::settings`): invoked bare it lists every setting the loaded plugins
//! declared with the invoking user's current value, with a `plugin.key` it describes the
//! setting and shows its current value, and with a value it stores (or, with `reset`, removes)
//! that user's setting.
//!
//! The descriptors come from the `UserSettingsCatalog` the runtime publishes from the plugins'
//! `#[plugin(user_settings = …)]` declarations, so the command can only ever offer the settings
//! of the plugins that are actually loaded. The values go through the
//! [`UserSettings`](zeta_plugin::accounts::UserSettings) facet of the shared account service,
//! so every reader sees a write immediately.
//!
//! Users are identified by the IRCv3 `account` tag when the network reports one, else their
//! `user@host` — the framework resolves that into a [`UserIdentity`] on every event. There is
//! nothing to configure; the plugin needs only the database.

use std::sync::Arc;

use clap::Parser;
use serde::{Deserialize, Serialize};
use tracing::warn;
use zeta_utils::text::TruncatableExt;

use zeta_plugin::accounts::UserIdentity;
use zeta_plugin::accounts::UserSettings as Service;
use zeta_plugin::accounts::settings::{UserSettingSpec, UserSettingsCatalog};
use zeta_plugin::database::DbError;
use zeta_plugin::irc::{
    LABELED_PREFIX_BYTES, MAX_LISTING_LENGTH, append_entries_with_more, labeled_reply,
    labeled_reply_prefix, parse_or_usage, plain_reply, send_reply,
};
use zeta_plugin::{ArgsMode, Chain, Client, CommandEvent, Context, ZetaError, plugin};

/// The label every reply from this plugin carries.
const LABEL: &str = "Settings";

/// The value that restores a setting's default.
const RESET_VALUE: &str = "reset";

/// Settings for the user-settings plugin, from its `[plugins.user_settings]` configuration
/// section.
///
/// The plugin has nothing to configure.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {}

/// The `.settings` command arguments.
#[derive(Parser, Debug)]
pub struct Opts {
    /// the setting to view or change, as `plugin.key`
    setting: Option<String>,
    /// the new value, or `reset` to restore the default
    value: Option<String>,
}

/// User settings command plugin.
#[derive(Debug)]
pub struct UserSettings {
    /// The shared per-user settings facet, handed over by the host's account service.
    settings: Arc<Service>,
}

#[plugin(name = "user_settings", settings = Settings)]
impl UserSettings {
    fn new(ctx: &Context, _settings: &Settings) -> Result<Self, ZetaError> {
        Ok(Self {
            settings: Arc::clone(ctx.accounts().settings()),
        })
    }

    /// Lists, describes, or changes one of the invoking user's settings.
    #[command(
        ".settings",
        "List, view, and change your per-user settings",
        event,
        usage_hint = "[<plugin.key> [<value>|reset]]"
    )]
    async fn settings(
        &self,
        ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let Some(identity) = command.identity() else {
            // Without a services account there is nothing to attach settings to.
            send_reply(
                client,
                channel,
                labeled_reply(
                    LABEL,
                    "identify with services (e.g. /msg nickserv identify) to use settings",
                ),
            )?;

            return Ok(());
        };

        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Shell, |line| {
            labeled_reply(LABEL, line)
        })?
        else {
            return Ok(());
        };

        let Some(catalog) = ctx.shared().get::<UserSettingsCatalog>() else {
            send_reply(
                client,
                channel,
                labeled_reply(LABEL, "the settings catalog is unavailable"),
            )?;

            return Ok(());
        };

        match (opts.setting.as_deref(), opts.value.as_deref()) {
            (None, _) => self.overview(client, channel, &identity, &catalog).await?,
            (Some(address), None) => {
                self.describe(client, channel, &identity, &catalog, address)
                    .await?;
            }
            (Some(address), Some(value)) => {
                self.change(client, channel, &identity, &catalog, address, value)
                    .await?;
            }
        }

        Ok(())
    }
}

impl UserSettings {
    /// Replies with the overview listing: every registered setting and the user's value.
    async fn overview(
        &self,
        client: &Client,
        channel: &str,
        identity: &UserIdentity,
        catalog: &UserSettingsCatalog,
    ) -> Result<(), ZetaError> {
        let specs = catalog.all();
        let mut entries = Vec::with_capacity(specs.len());

        for spec in specs {
            let current = match self.settings.get(identity, spec).await {
                Ok(Some(value)) => value,
                Ok(None) => default_value(spec),
                Err(error) => {
                    warn_load_failure(&error);
                    default_value(spec)
                }
            };
            entries.push(overview_entry(spec, &current));
        }

        send_reply(client, channel, overview_listing(&entries))?;

        Ok(())
    }

    /// Replies with the description, accepted values, default, and current value of `address`.
    async fn describe(
        &self,
        client: &Client,
        channel: &str,
        identity: &UserIdentity,
        catalog: &UserSettingsCatalog,
        address: &str,
    ) -> Result<(), ZetaError> {
        let Some(spec) = resolve(client, channel, catalog, address)? else {
            return Ok(());
        };

        let (current, failed) = match self.settings.get(identity, spec).await {
            Ok(Some(value)) => (value, false),
            Ok(None) => (default_value(spec), false),
            Err(error) => {
                warn_load_failure(&error);
                (default_value(spec), true)
            }
        };

        let note = failed.then_some(" (could not load your value)");
        let line = describe_line(spec, &current, note);

        send_reply(client, channel, line)?;

        Ok(())
    }

    /// Stores `value` for `address`, or removes the stored value when `value` is `reset`.
    async fn change(
        &self,
        client: &Client,
        channel: &str,
        identity: &UserIdentity,
        catalog: &UserSettingsCatalog,
        address: &str,
        value: &str,
    ) -> Result<(), ZetaError> {
        let Some(spec) = resolve(client, channel, catalog, address)? else {
            return Ok(());
        };

        if value.eq_ignore_ascii_case(RESET_VALUE) {
            match self.settings.reset(identity, spec).await {
                Ok(true) => {
                    send_reply(
                        client,
                        channel,
                        labeled_reply(LABEL, format_args!("{address} reset")),
                    )?;
                }
                Ok(false) => {
                    send_reply(
                        client,
                        channel,
                        labeled_reply(LABEL, format_args!("{address} was not set")),
                    )?;
                }
                Err(error) => report_store_error(client, channel, &error)?,
            }

            return Ok(());
        }

        let Some(canonical) = spec.canonical(value) else {
            send_reply(
                client,
                channel,
                labeled_reply(
                    LABEL,
                    format_args!("{address} must be one of: {}", spec.kind.values()),
                ),
            )?;

            return Ok(());
        };

        match self.settings.set(identity, spec, canonical).await {
            Ok(()) => {
                send_reply(
                    client,
                    channel,
                    labeled_reply(LABEL, format_args!("{address} set to {canonical}")),
                )?;
            }
            Err(error) => report_store_error(client, channel, &error)?,
        }

        Ok(())
    }
}

/// Why a `plugin.key` address could not be resolved.
#[derive(Debug, Eq, PartialEq)]
enum AddressError {
    /// The address carries no `.` separator.
    Malformed,
    /// The plugin or key is not registered.
    Unknown,
}

/// Resolves a `plugin.key` address to its registered descriptor.
///
/// Pure so the malformed/unknown distinction stays testable without a client.
fn parse_address(
    address: &str,
    catalog: &UserSettingsCatalog,
) -> Result<&'static UserSettingSpec, AddressError> {
    let Some((plugin, key)) = address.split_once('.') else {
        return Err(AddressError::Malformed);
    };

    catalog.find(plugin, key).ok_or(AddressError::Unknown)
}

/// Resolves a `plugin.key` address, replying to the channel when it is malformed or unknown,
/// and returning [`None`] for the caller to stop at.
fn resolve(
    client: &Client,
    channel: &str,
    catalog: &UserSettingsCatalog,
    address: &str,
) -> Result<Option<&'static UserSettingSpec>, ZetaError> {
    match parse_address(address, catalog) {
        Ok(spec) => Ok(Some(spec)),
        Err(AddressError::Malformed) => {
            send_reply(
                client,
                channel,
                labeled_reply(LABEL, "address settings as plugin.key, e.g. tldr.language"),
            )?;

            Ok(None)
        }
        Err(AddressError::Unknown) => {
            send_reply(
                client,
                channel,
                labeled_reply(LABEL, format_args!("unknown setting {address}")),
            )?;

            Ok(None)
        }
    }
}

/// The value shown when the user has not stored one: the descriptor default, or `-` when the
/// plugin's unset behavior is not a stored value.
fn default_value(spec: &UserSettingSpec) -> String {
    spec.default.map_or_else(|| "-".to_owned(), str::to_owned)
}

/// Renders one overview entry: `plugin.key=current`.
fn overview_entry(spec: &UserSettingSpec, current: &str) -> String {
    format!("{}={current}", spec.id())
}

/// Renders the overview listing from `entries`, trailing with a count of the settings that did
/// not fit the line budget.
fn overview_listing(entries: &[String]) -> String {
    let mut listing = labeled_reply_prefix(LABEL);
    // Room for the suffix the budgeted append adds itself, plus the read-more hint.
    let reserved = 48;

    append_entries_with_more(
        &mut listing,
        entries.iter(),
        entries.len(),
        " ",
        MAX_LISTING_LENGTH,
        reserved,
        |dropped| format!(" (+{dropped} more; .settings <plugin.key> shows each)"),
    );

    listing
}

/// Renders the description reply for `spec` showing `current`: description, accepted values,
/// default, and the user's value, within the line budget.
///
/// The whole line — `note` included — is byte-budgeted like the listing replies, since the IRC
/// line limit counts bytes; the note is reserved before the value list is cut, so it is never
/// pushed off the end by the truncation it follows.
fn describe_line(spec: &UserSettingSpec, current: &str, note: Option<&str>) -> String {
    let note = note.unwrap_or_default();
    let mut line = format!(
        "{} — {}; one of: ",
        labeled_reply_prefix(&spec.id()),
        spec.description
    );

    // Budget the value list by what is known before it: the id, description, the note that
    // follows, and the fixed framing around them. Saturating, so an unexpectedly long
    // description just truncates the values harder instead of panicking.
    let framing = LABELED_PREFIX_BYTES + spec.id().len() + spec.description.len() + note.len() + 64;
    let budget = MAX_LISTING_LENGTH.saturating_sub(framing);
    let values = spec.kind.values();
    let values = values.truncate_bytes_within(budget, "…");
    line.push_str(&values);
    line.push_str("; default: ");
    line.push_str(&default_value(spec));
    line.push_str("; your value: ");
    line.push_str(current);
    line.push_str(note);

    line.truncate_bytes_within(MAX_LISTING_LENGTH, "…")
        .into_owned()
}

/// Logs a failed store operation and tells the channel the setting could not be changed.
///
/// [`DbError`]'s `Display` is reply-grade, so it doubles as the reply; the operator detail
/// stays in the `source()` chain this logs.
fn report_store_error(client: &Client, channel: &str, error: &DbError) -> Result<(), ZetaError> {
    warn!(exception.message = %Chain(error), "could not change a user setting");

    send_reply(client, channel, plain_reply(error))?;

    Ok(())
}

/// Logs a failed load; the reply falls back to the default and notes it.
fn warn_load_failure(error: &DbError) {
    warn!(exception.message = %Chain(error), "could not load a user setting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::accounts::settings::{UserSettingKind, UserSettingSpec};

    /// A descriptor under a plugin name unique to this test file's registry uses.
    static LANGUAGE: UserSettingSpec = UserSettingSpec {
        plugin: "settings-command-test",
        key: "language",
        kind: UserSettingKind::Enum(&["DA", "DE", "EN"]),
        description: "the output language",
        default: Some("EN"),
    };
    static LANGUAGE_SPECS: [UserSettingSpec; 1] = [LANGUAGE];

    fn catalog() -> UserSettingsCatalog {
        UserSettingsCatalog::new([("settings-command-test", &LANGUAGE_SPECS[..])])
    }

    #[test]
    fn parse_address_resolves_registered_settings_only() {
        let catalog = catalog();

        assert_eq!(
            parse_address(&LANGUAGE.id(), &catalog).map(UserSettingSpec::id),
            Ok(LANGUAGE.id())
        );
        assert_eq!(
            parse_address("nodot", &catalog).err(),
            Some(AddressError::Malformed)
        );
        assert_eq!(
            parse_address("settings-command-missing.mode", &catalog).err(),
            Some(AddressError::Unknown)
        );
    }

    #[test]
    fn overview_entry_pairs_the_address_with_the_value() {
        assert_eq!(
            overview_entry(&LANGUAGE, "DA"),
            format!("{}=DA", LANGUAGE.id())
        );
        assert_eq!(
            overview_entry(&LANGUAGE, "-"),
            format!("{}=-", LANGUAGE.id())
        );
    }

    #[test]
    fn overview_listing_stays_within_the_line_budget_and_counts_the_rest() {
        let entries: Vec<String> = (0..40)
            .map(|index| format!("plugin{index:02}.setting=VALUE-{index:02}"))
            .collect();
        let listing = overview_listing(&entries);

        assert!(
            listing.len() <= MAX_LISTING_LENGTH,
            "the listing is budgeted: {} bytes",
            listing.len()
        );
        assert!(
            listing.contains("(+"),
            "the dropped count is shown: {listing}"
        );

        let short = overview_listing(&["one.mode=DA".to_owned()]);
        assert!(
            !short.contains("(+"),
            "a fitting listing has no suffix: {short}"
        );
    }

    #[test]
    fn describe_line_names_values_default_and_current() {
        let line = describe_line(&LANGUAGE, "DA", None);

        assert!(
            line.contains("one of: DA, DE, EN"),
            "values are listed: {line}"
        );
        assert!(line.contains("default: EN"), "the default is named: {line}");
        assert!(
            line.contains("your value: DA"),
            "the current value is named: {line}"
        );
        assert!(
            line.len() <= MAX_LISTING_LENGTH,
            "the description is budgeted"
        );
    }

    #[test]
    fn describe_line_carries_the_load_failure_note_within_the_budget() {
        let line = describe_line(&LANGUAGE, "DA", Some(" (could not load your value)"));

        assert!(
            line.contains("(could not load your value)"),
            "the note survives the truncation it follows: {line}"
        );
        assert!(
            line.len() <= MAX_LISTING_LENGTH,
            "the note is reserved inside the budget: {} bytes",
            line.len()
        );
    }

    #[test]
    fn describe_line_truncates_wide_value_lists() {
        let values: Vec<&'static str> = (0..100)
            .map(|index| -> &'static str { Box::leak(format!("LANG{index:03}").into_boxed_str()) })
            .collect();
        let wide = UserSettingSpec {
            plugin: "settings-command-test",
            key: "wide",
            kind: UserSettingKind::Enum(Box::leak(values.into_boxed_slice())),
            description: "a setting with a very wide value list",
            default: None,
        };
        let line = describe_line(&wide, "-", Some(" (could not load your value)"));

        assert!(
            line.len() <= MAX_LISTING_LENGTH,
            "the list is truncated: {}",
            line.len()
        );
        assert!(
            line.contains("(could not load your value)"),
            "the note survives even with a cut value list: {line}"
        );
    }

    #[test]
    fn default_value_renders_the_descriptor_default_or_a_dash() {
        assert_eq!(default_value(&LANGUAGE), "EN");
        assert_eq!(default_value(&LANGUAGE), "EN");
    }
}
