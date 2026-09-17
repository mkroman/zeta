use rand::prelude::IteratorRandom;
use serde::{Deserialize, Serialize};

use crate::plugin::prelude::*;

/// Settings for the choices plugin, from its `[plugins.choices]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The keywords that separate the options.
    #[serde(default = "default_or_keywords")]
    pub or_keywords: Vec<String>,
    /// The separator between individual options.
    #[serde(default = "default_option_separator")]
    pub option_separator: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            or_keywords: default_or_keywords(),
            option_separator: default_option_separator(),
        }
    }
}

/// Returns the default keywords that separate the options.
fn default_or_keywords() -> Vec<String> {
    vec!["eller".to_string()]
}

/// Returns the default separator between individual options.
fn default_option_separator() -> String {
    ", ".to_string()
}

pub struct Choices {
    settings: Settings,
}

#[async_trait]
impl Plugin<Context> for Choices {
    type Settings = Settings;

    fn new(_ctx: &Context, settings: &Settings) -> Result<Choices, ZetaError> {
        Ok(Choices {
            settings: settings.clone(),
        })
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command {
            let current_nickname = client.current_nickname();

            if let Some(msg) = strip_nick_prefix(inner_message, current_nickname)
                && let Some(options) = extract_options(msg, &self.settings)
            {
                let source_nickname = message.source_nickname().unwrap_or("");
                let mut rng = rand::rng();
                let selection = options.iter().choose(&mut rng).unwrap();

                client.send_privmsg(channel, format!("{source_nickname}: {selection}"))?;
            }
        }

        Ok(())
    }
}

fn strip_nick_prefix<'a>(s: &'a str, current_nickname: &'a str) -> Option<&'a str> {
    s.strip_prefix(current_nickname).and_then(|s| {
        if s.starts_with(", ") || s.starts_with(": ") {
            Some(&s[2..])
        } else {
            None
        }
    })
}

fn extract_options<'a>(s: &'a str, settings: &Settings) -> Option<Vec<&'a str>> {
    let (keyword, index) = settings
        .or_keywords
        .iter()
        .filter(|keyword| !keyword.is_empty())
        .filter_map(|keyword| s.find(keyword).map(|index| (keyword, index)))
        .min_by_key(|(_, index)| *index)?;

    let (first, last) = s.split_at(index);
    let last = last[keyword.len()..].trim();

    let mut options: Vec<&str> = if settings.option_separator.is_empty() {
        vec![first.trim()]
    } else {
        first
            .split(settings.option_separator.as_str())
            .map(str::trim)
            .collect()
    };

    // If the last option ends with a question mark, skip it.
    options.push(last.strip_suffix('?').unwrap_or(last));

    Some(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_should_strip_nick_prefix() {
        assert_eq!(
            strip_nick_prefix("zeta: hello world", "zeta"),
            Some("hello world")
        );
        assert_eq!(
            strip_nick_prefix("zeta, hello world", "zeta"),
            Some("hello world")
        );
    }

    #[test]
    fn it_should_not_extract_options_when_not_present() {
        assert_eq!(extract_options("hi", &Settings::default()), None);
    }

    #[test]
    fn it_should_extract_options() {
        let settings = Settings::default();

        assert_eq!(
            extract_options("a eller b", &settings),
            Some(vec!["a", "b"])
        );
        assert_eq!(
            extract_options("a, b, c eller d", &settings),
            Some(vec!["a", "b", "c", "d"])
        );
    }

    #[test]
    fn it_should_extract_options_stripping_questionmark() {
        let settings = Settings::default();

        assert_eq!(
            extract_options("a eller b?", &settings),
            Some(vec!["a", "b"])
        );
        assert_eq!(
            extract_options("a, b, c eller d?", &settings),
            Some(vec!["a", "b", "c", "d"])
        );
    }

    #[test]
    fn it_should_extract_options_with_custom_settings() {
        let settings = Settings {
            or_keywords: vec!["or".to_string()],
            option_separator: "; ".to_string(),
        };

        assert_eq!(
            extract_options("a; b or c?", &settings),
            Some(vec!["a", "b", "c"])
        );
    }

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert_eq!(settings.or_keywords, ["eller"]);
        assert_eq!(settings.option_separator, ", ");
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "or_keywords": ["or", "eller"],
            "option_separator": "; ",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.or_keywords, ["or", "eller"]);
        assert_eq!(settings.option_separator, "; ");
    }
}
