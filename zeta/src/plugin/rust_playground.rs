//! Rust Playground integration.
//!
//! Evaluates Rust code using the online Rust Playground.

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*, utils::Truncatable};

const BASE_URL: &str = "https://play.rust-lang.org/execute";

/// Settings for the rust_playground plugin, from its `[plugins.rust_playground]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    /// The release channel used for evaluation: `stable`, `beta` or `nightly`.
    #[serde(default = "default_channel")]
    pub channel: String,
    /// The build mode: `debug` or `release`.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// The Rust edition: `2015`, `2018`, `2021` or `2024`.
    #[serde(default = "default_edition")]
    pub edition: String,
    /// The maximum output length, in characters.
    #[serde(default = "default_max_output_length")]
    pub max_output_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            mode: default_mode(),
            edition: default_edition(),
            max_output_length: default_max_output_length(),
        }
    }
}

/// Returns the default release channel.
fn default_channel() -> String {
    "stable".to_string()
}

/// Returns the default build mode.
fn default_mode() -> String {
    "debug".to_string()
}

/// Returns the default Rust edition.
fn default_edition() -> String {
    "2024".to_string()
}

/// Returns the default maximum output length, in characters.
const fn default_max_output_length() -> usize {
    250
}

/// The `.rs` command.
const RUST_PLAYGROUND: CommandSpec = CommandSpec::new(
    ".rs",
    "Evaluate a Rust expression on the Rust playground",
);

/// Plugin for evaluating Rust code.
pub struct RustPlayground {
    client: reqwest::Client,
    error_regex: Regex,
    settings: Settings,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// The request payload sent to the Rust Playground.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecuteRequest<'a> {
    channel: &'a str,
    mode: &'a str,
    edition: &'a str,
    crate_type: &'a str,
    tests: bool,
    code: String,
    backtrace: bool,
}

/// The response payload received from the Rust Playground.
#[derive(Deserialize)]
struct ExecuteResponse {
    success: bool,
    stdout: String,
    stderr: String,
}

#[async_trait]
impl Plugin<Context> for RustPlayground {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(RUST_PLAYGROUND);
        let client = http::build_client(&ctx.config.http);
        // Regex to extract error messages from stderr (e.g. "error[E0425]: cannot find value...")
        let error_regex = Regex::new(r"(?m)^error(?:\[E\d+\])?: (.*?)$").expect("invalid regex");

        Ok(Self {
            client,
            error_regex,
            settings: settings.clone(),
        })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let expr = command.args();

        // Early return if input is empty
        if expr.trim().is_empty() {
            client.send_privmsg(channel, reply("Rust Playground", "Usage: .rs\x0f <expr>"))?;
            return Ok(());
        }

        match self.evaluate(expr).await {
            Ok(output) => {
                client.send_privmsg(channel, reply("Rust Playground", &output))?;
            }
            Err(e) => {
                warn!("rust playground error: {}", e);
                client.send_privmsg(
                    channel,
                    reply("Rust Playground", format!("http error: {e}")),
                )?;
            }
        }

        Ok(())
    }
}

impl RustPlayground {
    /// Evaluates the given expression on the Rust Playground.
    async fn evaluate(&self, expr: &str) -> Result<String, Error> {
        // Wrap the expression in a main function and print macro
        let code = format!(r#"fn main() {{ println!("{{:?}}", {{ {expr} }}); }}"#);

        let request = ExecuteRequest {
            channel: &self.settings.channel,
            mode: &self.settings.mode,
            edition: &self.settings.edition,
            crate_type: "bin",
            tests: false,
            code,
            backtrace: false,
        };

        debug!("sending code to rust playground");

        let response = self.client.post(BASE_URL).json(&request).send().await?;

        let result: ExecuteResponse = response.error_for_status()?.json().await?;

        if result.success {
            let output = sanitize_output(&result.stdout);
            Ok(output
                .truncate_with_suffix(self.settings.max_output_length, "…")
                .into_owned())
        } else {
            let errors = self.extract_errors(&result.stderr);
            let output = if errors.is_empty() {
                // Fallback to raw stderr if no specific errors were found
                sanitize_output(&result.stderr)
            } else {
                format!("Compilation error(s): {}", errors.join(", "))
            };
            Ok(output
                .truncate_with_suffix(self.settings.max_output_length, "…")
                .into_owned())
        }
    }

    /// Extracts compiler error messages from the stderr output.
    fn extract_errors(&self, stderr: &str) -> Vec<String> {
        self.error_regex
            .captures_iter(stderr)
            .map(|cap| cap[1].to_string())
            .collect()
    }
}

/// Sanitizes output by removing control characters (0x00-0x19, 0x7F).
/// This includes newlines, which is desirable for IRC.
fn sanitize_output(s: &str) -> String {
    s.chars()
        .filter(|&c| c as u32 > 25 && c as u32 != 127)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.channel, "stable");
            assert_eq!(settings.mode, "debug");
            assert_eq!(settings.edition, "2024");
            assert_eq!(settings.max_output_length, 250);
        }
        deserialize: {
            "channel": "nightly",
            "mode": "release",
            "edition": "2021",
            "max_output_length": 100,
        } assert: {
            assert_eq!(settings.channel, "nightly");
            assert_eq!(settings.mode, "release");
            assert_eq!(settings.edition, "2021");
            assert_eq!(settings.max_output_length, 100);
        }
    }
}
