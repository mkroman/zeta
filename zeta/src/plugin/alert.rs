//! Schedule alerts to be delivered in a channel at a given time.
//!
//! Alerts are created with the `.alert <message> <in|at> <datetime>` command, e.g.
//! `.alert hello world at 4:20` or `.alert another message in 10 minutes`, and support human
//! datetime expressions ("tomorrow 8pm", "next friday 10:30", "10 minutes", ..).
//!
//! Alerts are persisted in the database and cached in memory by the alert service. A scheduler
//! task broadcasts due alerts to a delivery task, which sends them in the channel they were
//! created in; alerts are removed from the cache and the database once sent.

mod error;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use {
    error::{Error, ParseTimeError},
    model::{Alert, NewAlert},
    service::AlertService,
};

use chrono::Days;
use interim::{Dialect, parse_date_string};
use irc::proto::Prefix as IrcPrefix;
use rand::prelude::IteratorRandom;
use sqlx::types::chrono::{DateTime, Local, Utc};
use tokio::sync::broadcast;
use tracing::{debug, error, warn};

use crate::plugin::prelude::*;

/// The `.alert` command.
const ALERT: Prefix = Prefix::new(".alert");

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

/// The tense of an alert's datetime expression.
///
/// Determines how the trailing datetime is interpreted; `at` expressions are parsed as absolute or
/// informal datetimes ("4:20", "12/12/2032 10:30", "tomorrow 8pm"), while `in` expressions are
/// parsed as durations relative to now ("10 minutes").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tense {
    /// The datetime is an absolute point in time.
    At,
    /// The datetime is a duration relative to now.
    In,
}

/// Alert plugin.
///
/// Lets users schedule alerts for themselves with the `.alert <message> <in|at> <datetime>`
/// command; a scheduler task broadcasts due alerts to a delivery task, started in [`loaded`],
/// which sends them in the channel the alert was created in.
///
/// [`loaded`]: Plugin::loaded
pub struct AlertPlugin {
    /// The alert service.
    service: AlertService,
    /// The receiver of due alerts, moved into the delivery task on load.
    receiver: Option<broadcast::Receiver<Alert>>,
}

impl AlertPlugin {
    /// Spawns the delivery task that sends due alerts over IRC as they are broadcast by the
    /// scheduler.
    fn start_delivery(client: &Client, mut receiver: broadcast::Receiver<Alert>) {
        let sender = client.sender();

        tokio::spawn(async move {
            debug!("starting alert delivery task");

            loop {
                let alert = match receiver.recv().await {
                    Ok(alert) => alert,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(?n, "missed due alerts");

                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        error!("alert broadcast channel is closed");

                        break;
                    }
                };

                debug!(?alert, "delivering alert");

                if let Err(err) = sender.send_privmsg(
                    &alert.channel,
                    format!("{}: {}", alert.nickname, alert.message),
                ) {
                    error!(?err, alert_id = alert.id, "could not deliver alert");
                }
            }
        });
    }
}

#[async_trait]
impl Plugin<Context> for AlertPlugin {
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        let service = AlertService::new(ctx.db.clone());

        Ok(AlertPlugin {
            receiver: Some(service.subscribe()),
            service,
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "alert".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        &[ALERT]
    }

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
            let Some((message, tense, time_spec)) = split_args(args) else {
                client.send_privmsg(
                    channel,
                    formatted("Usage: .alert\x0f <message> <in|at> <datetime>"),
                )?;

                return Ok(());
            };

            let time = match parse_time(tense, time_spec, Local::now()) {
                Ok(time) => time,
                Err(err) => {
                    client.send_privmsg(channel, formatted(&err.to_string()))?;

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
                        format!("{}\x0310 Alert stored for\x0f {local}.", success_message()),
                    )?;
                }
                Err(err) => {
                    error!(?err, "could not store alert");
                    client.send_privmsg(channel, formatted("could not store the alert"))?;
                }
            }
        }

        Ok(())
    }
}

/// Splits the command arguments into the alert message and its datetime expression.
///
/// The message and datetime are separated by the last occurrence of `at` or `in`, so messages
/// containing either word are handled correctly. Returns `None` if either part is missing.
#[must_use]
fn split_args(args: &str) -> Option<(&str, Tense, &str)> {
    let lower = args.to_ascii_lowercase();
    let at = lower.rfind(" at ");
    let r#in = lower.rfind(" in ");

    let index = at.max(r#in)?;

    let tense = if at == Some(index) { Tense::At } else { Tense::In };

    let message = &args[..index];
    let time_spec = args[index + " at ".len()..].trim();

    (!message.trim().is_empty() && !time_spec.is_empty()).then_some((message, tense, time_spec))
}

/// Parses the datetime expression of an alert.
///
/// The expression is parsed as a human datetime relative to `now`; if a bare time ("4:20")
/// resolves to the past it is rolled forward to the next day, while expressions containing a date
/// that occurs in the past are rejected.
///
/// Both tenses are currently parsed alike: `at` expressions are datetimes ("4:20", "12/12/2032
/// 10:30", "tomorrow 8pm") and `in` expressions are durations relative to `now` ("10 minutes").
///
/// # Errors
///
/// Returns [`ParseTimeError::Ambiguous`] if the expression could not be parsed, or
/// [`ParseTimeError::Past`] if the expression contains a date that occurs in the past.
fn parse_time(
    _tense: Tense,
    spec: &str,
    now: DateTime<Local>,
) -> Result<DateTime<Utc>, ParseTimeError> {
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
        let mut time = time;

        while time <= now {
            time = time
                .checked_add_days(Days::new(1))
                .ok_or(ParseTimeError::Ambiguous)?;
        }

        return Ok(time.with_timezone(&Utc));
    }

    Err(ParseTimeError::Past)
}

/// Strips a leading `at` or `on` article from the datetime expression.
fn strip_article(spec: &str) -> &str {
    for article in ["at ", "on "] {
        if spec.len() > article.len()
            && spec
                .get(..article.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(article))
        {
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
    let segments: Vec<&str> = s.split(':').collect();

    (segments.len() == 2 || segments.len() == 3)
        && segments
            .iter()
            .all(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
}

/// Returns a random success message.
fn success_message() -> &'static str {
    SUCCESS_MESSAGES
        .iter()
        .choose(&mut rand::rng())
        .unwrap_or(&SUCCESS_MESSAGES[0])
}

/// Formats `s` as an alert response.
fn formatted(s: &str) -> String {
    format!("\x0310>\x0f\x02 Alert\x02\x0310: {s}")
}

#[cfg(test)]
mod tests {
    use sqlx::types::chrono::TimeZone;

    use super::*;

    #[test]
    fn splits_message_and_datetime() {
        assert_eq!(
            split_args("hello world at 4:20"),
            Some(("hello world", Tense::At, "4:20"))
        );
        assert_eq!(
            split_args("some longer message at 10:30 12/12/2032"),
            Some(("some longer message", Tense::At, "10:30 12/12/2032"))
        );
        assert_eq!(
            split_args("another message at 12/12/2032 10:30"),
            Some(("another message", Tense::At, "12/12/2032 10:30"))
        );
    }

    #[test]
    fn splits_relative_durations() {
        assert_eq!(
            split_args("check the oven in 10 minutes"),
            Some(("check the oven", Tense::In, "10 minutes"))
        );
    }

    #[test]
    fn splits_on_the_last_tense_marker() {
        assert_eq!(
            split_args("meet me at the station at 4:20"),
            Some(("meet me at the station", Tense::At, "4:20"))
        );
        assert_eq!(
            split_args("come in in 10 minutes"),
            Some(("come in", Tense::In, "10 minutes"))
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
            Some(("Hello World", Tense::At, "4:20"))
        );
        assert_eq!(
            split_args("hello world IN 10 minutes"),
            Some(("hello world", Tense::In, "10 minutes"))
        );
    }

    #[test]
    fn parses_future_datetimes() {
        let now = Local::now();

        let time = parse_time(Tense::At, "12/12/2032 10:30", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_month_before_day() {
        let now = Local::now();

        let time = parse_time(Tense::At, "10:30 12/12/2032", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_relative_durations() {
        let now = Local::now();

        let time = parse_time(Tense::In, "10 minutes", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert!(local - now < chrono::TimeDelta::try_minutes(11).unwrap());
        assert!(local - now > chrono::TimeDelta::try_minutes(9).unwrap());
    }

    #[test]
    fn rolls_bare_times_forward_to_the_next_day() {
        let now = Local::now();

        let time = parse_time(Tense::At, "4:20", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert_eq!(local.format("%H:%M").to_string(), "04:20");
    }

    #[test]
    fn rejects_past_dates() {
        let now = Local::now();

        assert_eq!(
            parse_time(Tense::At, "1/1/2000", now).unwrap_err(),
            ParseTimeError::Past
        );
    }

    #[test]
    fn rejects_unparseable_datetimes() {
        let now = Local::now();

        assert_eq!(
            parse_time(Tense::At, "not a datetime", now).unwrap_err(),
            ParseTimeError::Ambiguous
        );
    }

    #[test]
    fn tolerates_a_leading_article() {
        let now = Local::now();

        let time = parse_time(Tense::At, "at 4:20", now).unwrap();

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
}
