//! Send notifications to users when they show activity.
//!
//! Notifications are created with the `.notify <nick> <message>` command and are delivered to the
//! recipient the next time they are active in the channel the notification was created in, at
//! which point they are removed.

mod error;
mod model;
mod repository;
mod service;

// The module types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use {
    error::Error,
    model::{NewNotification, Notification},
    service::NotificationService,
};

use irc::proto::Prefix as IrcPrefix;
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::Local;
use tracing::error;

use crate::plugin::prelude::*;

/// The `.notify` command.
const NOTIFY: PluginCommand = PluginCommand::new(
    Prefix::new(".notify"),
    "Queue a notification for a user's next message",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[NOTIFY];

/// Settings for the notification plugin, from its `[plugins.notification]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    /// The maximum number of pending notifications a target may have in a channel.
    #[serde(default = "default_max_pending_per_target")]
    pub max_pending_per_target: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_pending_per_target: default_max_pending_per_target(),
        }
    }
}

/// Returns the default maximum number of pending notifications per target.
const fn default_max_pending_per_target() -> usize {
    10
}

/// Notification plugin.
///
/// Lets users queue notifications for other users with the `.notify <nick> <message>` command, and
/// delivers pending notifications to a user when they are active in a channel.
pub struct NotificationPlugin {
    /// The notification service.
    service: NotificationService,
}

#[async_trait]
impl Plugin<Context> for NotificationPlugin {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        Ok(NotificationPlugin {
            service: NotificationService::new(ctx.db.clone(), settings),
        })
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.service.load().await.map_err(plugin_err)?;

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

        if let Some(args) = NOTIFY.parse(msg) {
            let Some((target, message)) = parse_args(args) else {
                client.send_privmsg(
                    channel,
                    reply("Notification", "Usage: .notify\x0f <nick> <message>"),
                )?;

                return Ok(());
            };

            let notification = NewNotification {
                target: target.to_owned(),
                nickname: nickname.to_owned(),
                username: username.to_owned(),
                hostname: hostname.to_owned(),
                channel: channel.to_owned(),
                message: message.to_owned(),
            };

            match self.service.create(notification).await {
                Ok(_) => {
                    client.send_privmsg(channel, notice("The notification has been stored."))?;
                }
                Err(Error::TooManyPending(max)) => {
                    client.send_privmsg(
                        channel,
                        reply(
                            "Notification",
                            format!(
                                "{target} already has {max} pending notifications in this channel"
                            ),
                        ),
                    )?;
                }
                Err(err) => {
                    error!(?err, "could not store notification");
                    client.send_privmsg(
                        channel,
                        reply("Notification", "could not store the notification"),
                    )?;
                }
            }
        } else {
            let pending = self.service.take(channel, nickname).await;
            let mut sent_ids = Vec::with_capacity(pending.len());

            for notification in &pending {
                let message = &notification.message;
                let creator = &notification.nickname;
                let created_at = notification
                    .created_at
                    .with_timezone(&Local)
                    .format("%d/%m/%Y %H:%M:%S");

                if let Err(err) = client.send_privmsg(
                    channel,
                    format!(
                        "{nickname}:\x0310 Notification\x0f {message}\x0310 from\x0f {creator}\x0310 at\x0f {created_at}"
                    )
                ) {
                    error!(?err, "could not deliver notification");

                    break;
                }

                sent_ids.push(notification.id);

                if let Err(err) = self.service.delete_all(&sent_ids).await {
                    error!(
                        ?err,
                        count = sent_ids.len(),
                        "could not delete delivered notifications"
                    );
                }
            }
        }

        Ok(())
    }
}

fn parse_args(args: &str) -> Option<(&str, &str)> {
    let (target, message) = args.split_once(' ')?;

    (!message.trim().is_empty()).then_some((target, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.max_pending_per_target, 10);
        }
        deserialize: {
            "max_pending_per_target": 3,
        } assert: {
            assert_eq!(settings.max_pending_per_target, 3);
        }
    }

    #[test]
    fn parses_target_and_message() {
        assert_eq!(parse_args("foo hello there"), Some(("foo", "hello there")));
    }

    #[test]
    fn keeps_message_verbatim() {
        assert_eq!(parse_args("foo   padded  "), Some(("foo", "  padded  ")));
    }

    #[test]
    fn rejects_arguments_without_a_message() {
        assert_eq!(parse_args(""), None);
        assert_eq!(parse_args("foo"), None);
        assert_eq!(parse_args("foo "), None);
        assert_eq!(parse_args("foo   "), None);
    }
}
