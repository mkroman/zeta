//! Schedule alerts to be delivered in a channel at a given time.
//!
//! Alerts are created with the `.alert <message> <in|at> <datetime>` command, e.g.
//! `.alert hello world at 4:20` or `.alert another message in 10 minutes`, and support human
//! datetime expressions ("tomorrow 8pm", "next friday 10:30", "10 minutes", ..). The pending
//! alerts of a user in the current channel are listed with `.alert -l`.
//!
//! Alerts are persisted in the database, and a window of upcoming alerts is cached in memory by
//! the alert service, syncing the window from the database every few minutes. A scheduler task
//! delivers each alert to a delivery task as it becomes due, which sends it in the channel it
//! was created in; alerts are deleted from the database once sent.

mod error;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use {
    error::Error,
    model::{Alert, NewAlert},
    service::AlertService,
};

use std::sync::Arc;
use std::time::Duration;

use argh::{ArgsInfo, FromArgs};
use chrono::{Datelike, Days};
use interim::{Dialect, parse_date_string};
use irc::proto::Prefix as IrcPrefix;
use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Local, Utc};
use tokio::sync::mpsc;
use tracing::{debug, error, trace};

use crate::{plugin::prelude::*, utils::Truncatable};

/// The `.alert` command.
const ALERT: PluginCommand = PluginCommand::with_args::<Opts>(
    Prefix::new(".alert"),
    "Schedule an alert to be posted later, or list pending alerts",
);

/// The usage hint for the `.alert` command.
const USAGE: &str = "Usage: .alert\x0f [-l] <message> <in|at> <datetime>";

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[ALERT];

/// Schedule an alert to be posted later, or list pending alerts.
#[derive(FromArgs, ArgsInfo, Debug)]
#[argh(help_triggers("--help"))]
struct Opts {
    /// list your pending alerts in this channel
    #[argh(switch, short = 'l')]
    list: bool,
    /// the alert message, followed by `in` or `at` and a datetime
    #[argh(positional, greedy)]
    args: Vec<String>,
}

impl Opts {
    /// Returns the joined message and datetime expression.
    fn input(&self) -> String {
        self.args.join(" ")
    }
}

/// Settings for the alert plugin, from its `[plugins.alert]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// How long the scheduler waits before retrying after a failed tick.
    #[serde(default = "default_retry_delay", with = "humantime_serde")]
    pub retry_delay: Duration,
    /// How often the window of upcoming alerts is synced from the database.
    ///
    /// Alerts are only delivered on time if the window is at least as large as this interval,
    /// as the last sync before an alert is due can be a full interval earlier.
    #[serde(default = "default_sync_interval", with = "humantime_serde")]
    pub sync_interval: Duration,
    /// How far ahead of their due time alerts are kept in memory.
    ///
    /// Should be at least [`Settings::sync_interval`], so every alert is cached before it is
    /// due.
    #[serde(default = "default_window", with = "humantime_serde")]
    pub window: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            retry_delay: default_retry_delay(),
            sync_interval: default_sync_interval(),
            window: default_window(),
        }
    }
}

/// Returns the default scheduler retry delay.
const fn default_retry_delay() -> Duration {
    Duration::from_secs(30)
}

/// Returns the default alert window sync interval.
const fn default_sync_interval() -> Duration {
    Duration::from_mins(5)
}

/// Returns the default alert window.
const fn default_window() -> Duration {
    Duration::from_mins(15)
}

/// Reply messages used when an alert has been stored.
const SUCCESS_MESSAGES: &[&str] = &[
    "Got it, boss!",
    "Will do.",
    "At your service.",
    "Gotcha.",
    "Done and done.",
    "Roger.",
    "As you wish.",
    "Your wish is my command.",
    "Very well, then.",
];

/// Alert plugin.
///
/// Lets users schedule alerts for themselves with the `.alert <message> <in|at> <datetime>`
/// command; a scheduler task delivers due alerts to a delivery task, started in [`loaded`],
/// which sends them in the channel the alert was created in.
///
/// The alert service is published to [`Context::shared`], so other plugins can schedule alerts
/// through [`AlertService::create`].
///
/// [`loaded`]: Plugin::loaded
pub struct AlertPlugin {
    /// The alert service, also published for other plugins to use.
    service: Arc<AlertService>,
    /// The receiver of due alerts, moved into the delivery task on load.
    receiver: Option<mpsc::UnboundedReceiver<Alert>>,
}

impl AlertPlugin {
    /// Spawns the delivery task that sends due alerts over IRC as they are delivered by the
    /// scheduler.
    fn start_delivery(client: &Client, mut receiver: mpsc::UnboundedReceiver<Alert>) {
        let sender = client.sender();

        tokio::spawn(async move {
            debug!("starting alert delivery task");

            while let Some(alert) = receiver.recv().await {
                trace!(?alert, "delivering alert");

                if let Err(err) = sender.send_privmsg(
                    &alert.channel,
                    format!("{}: {}", alert.nickname, alert.message),
                ) {
                    error!(?err, alert_id = alert.id, "could not deliver alert");
                }
            }

            debug!("alert delivery channel is closed");
        });
    }
}

#[async_trait]
impl Plugin<Context> for AlertPlugin {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let mut service = AlertService::new(ctx.db.clone(), settings);
        let receiver = service.take_receiver();
        let service = Arc::new(service);

        ctx.shared.publish(Arc::clone(&service));

        Ok(AlertPlugin { service, receiver })
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    async fn loaded(&mut self, _ctx: &Context, client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;
        self.service.start_scheduler();

        if let Some(receiver) = self.receiver.take() {
            Self::start_delivery(client, receiver);
        }

        Ok(())
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, msg) = &message.command else {
            return Ok(());
        };

        let Some(IrcPrefix::Nickname(nickname, username, hostname)) = &message.prefix else {
            return Ok(());
        };

        if let Some(args) = ALERT.parse(msg) {
            let opts = match ALERT.parse_words::<Opts>(args) {
                Ok(opts) => opts,
                Err(err) => {
                    for line in err.to_string().lines().filter(|line| !line.is_empty()) {
                        client.send_privmsg(channel, reply("Alert", line))?;
                    }

                    return Ok(());
                }
            };

            if opts.list {
                if !opts.args.is_empty() {
                    client.send_privmsg(channel, reply("Alert", USAGE))?;

                    return Ok(());
                }

                let pending = match self.service.pending_for(channel, nickname).await {
                    Ok(pending) => pending,
                    Err(err) => {
                        error!(?err, "could not list pending alerts");

                        client.send_privmsg(
                            channel,
                            reply("Alert", "could not list your pending alerts"),
                        )?;

                        return Ok(());
                    }
                };

                client.send_privmsg(channel, format_pending(&pending))?;

                return Ok(());
            }

            let input = opts.input();

            let Some((message, time_spec)) = split_args(&input) else {
                client.send_privmsg(channel, reply("Alert", USAGE))?;

                return Ok(());
            };

            let time = match parse_time(time_spec, Local::now()) {
                Ok(time) => time,
                Err(err) => {
                    client.send_privmsg(channel, reply("Alert", err.to_string()))?;

                    return Ok(());
                }
            };

            let alert = NewAlert {
                nickname: nickname.to_owned(),
                username: username.to_owned(),
                hostname: hostname.to_owned(),
                channel: channel.to_owned(),
                message: message.to_owned(),
                time,
            };

            match self.service.create(alert).await {
                Ok(_) => {
                    let local = time.with_timezone(&Local).format("%d/%m/%Y %H:%M:%S");

                    client.send_privmsg(
                        channel,
                        reply(
                            "Alert",
                            format!("{} Alert stored for\x0f {local}.", success_message()),
                        ),
                    )?;
                }
                Err(err) => {
                    error!(?err, "could not store alert");
                    client.send_privmsg(channel, reply("Alert", "could not store the alert"))?;
                }
            }
        }

        Ok(())
    }
}

/// Errors that can occur while parsing the datetime of an alert.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseTimeError {
    /// The datetime could not be understood.
    #[error("ambiguous or unsupported datetime")]
    Ambiguous,
    /// The datetime occurs in the past.
    #[error("specified time occurs in the past")]
    Past,
}

/// Splits the command arguments into the alert message and its datetime expression.
///
/// The message and datetime are separated by the last occurrence of `at` or `in`, so messages
/// containing either word are handled correctly. Returns `None` if either part is missing.
#[must_use]
fn split_args(args: &str) -> Option<(&str, &str)> {
    let index = rfind_ignore_case(args, " at ").max(rfind_ignore_case(args, " in "))?;

    let message = &args[..index];
    let time_spec = args[index + " at ".len()..].trim();

    (!message.trim().is_empty() && !time_spec.is_empty()).then_some((message, time_spec))
}

/// Returns the byte index of the last occurrence of `needle` in `s`, ignoring ASCII case.
fn rfind_ignore_case(s: &str, needle: &str) -> Option<usize> {
    s.as_bytes()
        .windows(needle.len())
        .rposition(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Parses the datetime expression of an alert.
///
/// The expression is parsed as a human datetime relative to `now`; if a bare time ("4:20")
/// resolves to the past it is rolled forward to the next day, while expressions containing a
/// date that occurs in the past are rejected.
///
/// # Errors
///
/// Returns [`ParseTimeError::Ambiguous`] if the expression could not be parsed, or
/// [`ParseTimeError::Past`] if the expression contains a date that occurs in the past.
fn parse_time(spec: &str, now: DateTime<Local>) -> Result<DateTime<Utc>, ParseTimeError> {
    // Tolerate a leading article in the datetime expression, e.g. "at the weekend".
    let spec = strip_article(spec.trim());

    let time: DateTime<Local> = parse_date_string(spec, now, Dialect::Uk)
        .ok()
        .or_else(|| parse_time_first(spec, now))
        .ok_or(ParseTimeError::Ambiguous)?;

    if time > now {
        return Ok(time.with_timezone(&Utc));
    }

    if is_time_only(spec) {
        // Bare times such as "4:20" roll forward to their next occurrence.
        let days = u64::try_from((now - time).num_days()).unwrap_or_default() + 1;
        let time = time
            .checked_add_days(Days::new(days))
            .ok_or(ParseTimeError::Ambiguous)?;

        return Ok(time.with_timezone(&Utc));
    }

    Err(ParseTimeError::Past)
}

/// Strips leading `at` or `on` article from the datetime expression.
fn strip_article(spec: &str) -> &str {
    for article in ["at ", "on "] {
        if spec.len() > article.len()
            && spec
                .get(..article.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(article))
        {
            // The article is ASCII, so the prefix ends at a character boundary.
            return &spec[article.len()..];
        }
    }

    spec
}

/// Parses datetime expressions written time-first, e.g. `10:30 12/12/2032`, by swapping the two
/// tokens and letting the date lead as the parser expects.
///
/// Only applies when the first token is a clock time and the second looks like a calendar date;
/// otherwise returns `None`.
fn parse_time_first(spec: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let (time, date) = spec.split_once(' ')?;

    if !is_clock_time(time) || !(date.contains('/') || date.contains('-')) {
        return None;
    }

    parse_date_string(&format!("{date} {time}"), now, Dialect::Uk).ok()
}

/// Returns `true` if the datetime expression consists solely of a clock time, e.g. `4:20`,
/// `16:34:10` or `4:20pm`.
fn is_time_only(spec: &str) -> bool {
    // Split off a separate meridiem token, e.g. the "pm" of "4:20 pm".
    let spec = match spec.split_once(' ') {
        Some((time, meridiem))
            if meridiem.eq_ignore_ascii_case("am") || meridiem.eq_ignore_ascii_case("pm") =>
        {
            time
        }
        _ => spec,
    };

    // Strip a glued-on meridiem, e.g. the "pm" of "4:20pm".
    let spec = {
        let lower = spec.to_ascii_lowercase();

        ["pm", "am"]
            .iter()
            .find(|suffix| lower.ends_with(*suffix))
            .map_or(spec, |suffix| &spec[..spec.len() - suffix.len()])
    };

    is_clock_time(spec)
}

/// Returns `true` if `s` is a clock time on the form `H:M` or `H:M:S`, e.g. `4:20` or `16:34:10`.
fn is_clock_time(s: &str) -> bool {
    let segments = s.split(':').count();

    (segments == 2 || segments == 3)
        && s.split(':')
            .all(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
}

/// Returns a random success message.
fn success_message() -> &'static str {
    SUCCESS_MESSAGES
        .iter()
        .choose(&mut rand::rng())
        .unwrap_or(&SUCCESS_MESSAGES[0])
}

/// The maximum number of pending alerts included in a listing.
const MAX_LISTED_ALERTS: usize = 3;

/// The maximum number of characters of an alert message included in a listing.
const MAX_LISTED_MESSAGE_CHARS: usize = 50;

/// The maximum length of a pending alerts listing, leaving room for the sender prefix, `PRIVMSG`
/// framing and line ending overhead within the classic 512-byte IRC line limit.
const MAX_LISTING_LENGTH: usize = 400;

/// Formats the reply to `.alert -l` for `pending`, ordered by the time the alerts are due.
///
/// At most [`MAX_LISTED_ALERTS`] alerts are included, and entries are only appended while the
/// listing stays within [`MAX_LISTING_LENGTH`]; the count in the header always reflects every
/// pending alert.
fn format_pending(pending: &[Alert]) -> String {
    let Some(next) = pending.first() else {
        return reply("Alert", "You have no pending alerts");
    };

    let mut listing = reply(
        "Alert",
        format!(
            "Pending alerts:\x0f {}\x0310 Next up: {}",
            pending.len(),
            format_entry(next)
        ),
    );

    for alert in pending.iter().take(MAX_LISTED_ALERTS).skip(1) {
        let entry = format_entry(alert);
        let separator = format!("\x0310, then: {entry}");

        if listing.len() + separator.len() > MAX_LISTING_LENGTH {
            break;
        }

        listing.push_str(&separator);
    }

    listing
}

/// Formats a pending alert as a `“message” Mon DDth HH:MM` entry.
fn format_entry(alert: &Alert) -> String {
    let message = alert
        .message
        .as_str()
        .truncate_with_suffix(MAX_LISTED_MESSAGE_CHARS, "…");
    let due = format_due(alert.time.with_timezone(&Local));

    format!("“\x0f{message}\x0310”\x0f {due}\x0310")
}

/// Formats the due time of an alert as `Sep 16th 11:21`.
fn format_due(time: DateTime<Local>) -> String {
    let day = time.day();

    format!(
        "{} {day}{} {}",
        time.format("%b"),
        ordinal_suffix(day),
        time.format("%H:%M")
    )
}

/// Returns the ordinal suffix of `day`, e.g. `st` for 1 and 21, `th` for 11 through 13.
const fn ordinal_suffix(day: u32) -> &'static str {
    if matches!(day % 100, 11..=13) {
        return "th";
    }

    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

#[cfg(test)]
mod tests {
    use sqlx::types::chrono::TimeZone;

    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.retry_delay, Duration::from_secs(30));
            assert_eq!(settings.sync_interval, Duration::from_mins(5));
            assert_eq!(settings.window, Duration::from_mins(15));
        }
        deserialize: {
            "retry_delay": "1m",
            "sync_interval": "2m",
            "window": "30m",
        } assert: {
            assert_eq!(settings.retry_delay, Duration::from_mins(1));
            assert_eq!(settings.sync_interval, Duration::from_mins(2));
            assert_eq!(settings.window, Duration::from_mins(30));
        }
    }

    #[test]
    fn splits_message_and_datetime() {
        assert_eq!(
            split_args("hello world at 4:20"),
            Some(("hello world", "4:20"))
        );
        assert_eq!(
            split_args("some longer message at 10:30 12/12/2032"),
            Some(("some longer message", "10:30 12/12/2032"))
        );
        assert_eq!(
            split_args("another message at 12/12/2032 10:30"),
            Some(("another message", "12/12/2032 10:30"))
        );
    }

    #[test]
    fn splits_relative_durations() {
        assert_eq!(
            split_args("check the oven in 10 minutes"),
            Some(("check the oven", "10 minutes"))
        );
    }

    #[test]
    fn splits_on_the_last_tense_marker() {
        assert_eq!(
            split_args("meet me at the station at 4:20"),
            Some(("meet me at the station", "4:20"))
        );
        assert_eq!(
            split_args("come in in 10 minutes"),
            Some(("come in", "10 minutes"))
        );
    }

    #[test]
    fn rejects_arguments_without_a_message_or_datetime() {
        assert_eq!(split_args(""), None);
        assert_eq!(split_args("at 4:20"), None);
        assert_eq!(split_args("hello world"), None);
        assert_eq!(split_args("hello world at"), None);
        assert_eq!(split_args("hello world in "), None);
        assert_eq!(split_args(" at 4:20"), None);
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(
            split_args("Hello World AT 4:20"),
            Some(("Hello World", "4:20"))
        );
        assert_eq!(
            split_args("hello world IN 10 minutes"),
            Some(("hello world", "10 minutes"))
        );
    }

    #[test]
    fn parses_future_datetimes() {
        let now = Local::now();

        let time = parse_time("12/12/2032 10:30", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_month_before_day() {
        let now = Local::now();

        let time = parse_time("10:30 12/12/2032", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_relative_durations() {
        let now = Local::now();

        let time = parse_time("10 minutes", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert!(local - now < chrono::TimeDelta::try_minutes(11).unwrap());
        assert!(local - now > chrono::TimeDelta::try_minutes(9).unwrap());
    }

    #[test]
    fn rolls_bare_times_forward_to_the_next_day() {
        let now = Local::now();

        let time = parse_time("4:20", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert_eq!(local.format("%H:%M").to_string(), "04:20");
    }

    #[test]
    fn rejects_past_dates() {
        let now = Local::now();

        assert_eq!(
            parse_time("1/1/2000", now).unwrap_err(),
            ParseTimeError::Past
        );
    }

    #[test]
    fn rejects_unparseable_datetimes() {
        let now = Local::now();

        assert_eq!(
            parse_time("not a datetime", now).unwrap_err(),
            ParseTimeError::Ambiguous
        );
    }

    #[test]
    fn tolerates_a_leading_article() {
        let now = Local::now();

        let time = parse_time("at 4:20", now).unwrap();

        assert!(time > now.with_timezone(&Utc));
    }

    #[test]
    fn detects_bare_times() {
        assert!(is_time_only("4:20"));
        assert!(is_time_only("16:34"));
        assert!(is_time_only("16:34:10"));
        assert!(is_time_only("4:20 pm"));
        assert!(is_time_only("4:20pm"));

        assert!(!is_time_only("12/12/2032 10:30"));
        assert!(!is_time_only("10:30 12/12/2032"));
        assert!(!is_time_only("tomorrow 10:30"));
        assert!(!is_time_only("next friday 8pm"));
        assert!(!is_time_only("10 minutes"));
    }

    /// Returns a pending alert fixture.
    fn alert(message: &str, time: DateTime<Utc>) -> Alert {
        Alert {
            id: 1,
            nickname: "smoke".into(),
            username: "smoke".into(),
            hostname: "smoke".into(),
            channel: "#smoke".into(),
            message: message.into(),
            time,
            created_at: time,
        }
    }

    /// Returns the UTC equivalent of a local wall-clock time.
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn parses_the_list_flag() {
        let opts: Opts = ALERT.parse_words("-l").unwrap();

        assert!(opts.list);
        assert_eq!(opts.input(), "");

        let opts: Opts = ALERT.parse_words("--list").unwrap();

        assert!(opts.list);
    }

    #[test]
    fn parses_message_and_datetime() {
        let opts: Opts = ALERT.parse_words("hello world at 4:20").unwrap();

        assert!(!opts.list);
        assert_eq!(opts.input(), "hello world at 4:20");
    }

    #[test]
    fn parses_empty_arguments() {
        let opts: Opts = ALERT.parse_words("").unwrap();

        assert!(!opts.list);
        assert_eq!(opts.input(), "");
    }

    #[test]
    fn keeps_messages_verbatim() {
        let opts: Opts = ALERT
            .parse_words(r#"don't "forget me"  at   4:20"#)
            .unwrap();

        assert_eq!(opts.input(), r#"don't "forget me" at 4:20"#);
    }

    #[test]
    fn rejects_unknown_flags() {
        assert!(matches!(
            ALERT.parse_words::<Opts>("-x").unwrap_err(),
            ArgsError::Usage(_)
        ));
    }

    #[test]
    fn ordinals() {
        assert_eq!(ordinal_suffix(1), "st");
        assert_eq!(ordinal_suffix(2), "nd");
        assert_eq!(ordinal_suffix(3), "rd");
        assert_eq!(ordinal_suffix(4), "th");
        assert_eq!(ordinal_suffix(11), "th");
        assert_eq!(ordinal_suffix(12), "th");
        assert_eq!(ordinal_suffix(13), "th");
        assert_eq!(ordinal_suffix(21), "st");
        assert_eq!(ordinal_suffix(22), "nd");
        assert_eq!(ordinal_suffix(23), "rd");
        assert_eq!(ordinal_suffix(31), "st");
    }

    #[test]
    fn formats_due_times() {
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 9, 16, 11, 21, 0).unwrap()),
            "Sep 16th 11:21"
        );
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 1, 1, 0, 5, 0).unwrap()),
            "Jan 1st 00:05"
        );
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 3, 13, 23, 59, 0).unwrap()),
            "Mar 13th 23:59"
        );
    }

    #[test]
    fn formats_no_pending_alerts() {
        assert_eq!(
            format_pending(&[]),
            "\x0310>\x0f\x02 Alert:\x02\x0310 You have no pending alerts"
        );
    }

    #[test]
    fn formats_a_single_pending_alert() {
        let pending = [alert("hello world", at(2026, 9, 16, 11, 21))];

        assert_eq!(
            format_pending(&pending),
            concat!(
                "\x0310>\x0f\x02 Alert:\x02\x0310 Pending alerts:\x0f 1",
                "\x0310 Next up: “\x0fhello world\x0310”\x0f Sep 16th 11:21\x0310",
            )
        );
    }

    #[test]
    fn formats_at_most_three_pending_alerts() {
        let pending = [
            alert("first", at(2026, 9, 16, 11, 21)),
            alert("second", at(2026, 9, 16, 11, 22)),
            alert("third", at(2026, 9, 16, 11, 23)),
            alert("fourth", at(2026, 9, 17, 9, 15)),
        ];

        assert_eq!(
            format_pending(&pending),
            concat!(
                "\x0310>\x0f\x02 Alert:\x02\x0310 Pending alerts:\x0f 4",
                "\x0310 Next up: “\x0ffirst\x0310”\x0f Sep 16th 11:21\x0310",
                "\x0310, then: “\x0fsecond\x0310”\x0f Sep 16th 11:22\x0310",
                "\x0310, then: “\x0fthird\x0310”\x0f Sep 16th 11:23\x0310",
            )
        );
    }

    #[test]
    fn truncates_long_messages() {
        let pending = [alert(&"a".repeat(80), at(2026, 9, 16, 11, 21))];

        let truncated = format!("{}…", "a".repeat(50));
        let expected = format!(
            concat!(
                "\x0310>\x0f\x02 Alert:\x02\x0310 Pending alerts:\x0f 1",
                "\x0310 Next up: “\x0f{}\x0310”\x0f Sep 16th 11:21\x0310",
            ),
            truncated
        );

        assert_eq!(format_pending(&pending), expected);
    }

    #[test]
    fn omits_entries_beyond_the_listing_budget() {
        // Each 50-character message is 100 bytes in UTF-8, so the third entry would push the
        // listing past the message length budget.
        let message = "æ".repeat(50);
        let pending = [
            alert(&message, at(2026, 9, 16, 11, 21)),
            alert(&message, at(2026, 9, 16, 11, 22)),
            alert(&message, at(2026, 9, 16, 11, 23)),
        ];

        let listing = format_pending(&pending);

        assert!(listing.len() <= MAX_LISTING_LENGTH);
        assert_eq!(listing.matches("Next up").count(), 1);
        assert_eq!(listing.matches("then:").count(), 1);
    }
}
