//! Evaluates Rust expressions on the online Rust Playground.
//!
//! The `.rs <expr>` command wraps the expression in `fn main() { println!("{:?}", { expr }); }`
//! and executes it on `play.rust-lang.org`, replying with the printed output — or the compiler
//! errors when it fails to build. Output has its control characters (including newlines)
//! stripped and is truncated to `max_output_length` characters with an ellipsis suffix; an
//! empty expression replies with usage.
//!
//! The playground channel (`stable`), build mode (`debug`), edition (`2024`), and output limit
//! are set in `[plugins.rust_playground]`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{http, plugin::prelude::*, utils::{Truncatable, strip_control_chars}};

const BASE_URL: &str = "https://play.rust-lang.org/execute";

/// Settings for the `rust_playground` plugin, from its `[plugins.rust_playground]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The release channel used for evaluation: `stable`, `beta` or `nightly`.
    pub channel: String,
    /// The build mode: `debug` or `release`.
    pub mode: String,
    /// The Rust edition: `2015`, `2018`, `2021` or `2024`.
    pub edition: String,
    /// The maximum output length, in characters.
    pub max_output_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: "stable".to_string(),
            mode: "debug".to_string(),
            edition: "2024".to_string(),
            max_output_length: 250,
        }
    }
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

/// Errors that can occur while evaluating code on the Rust Playground.
pub type Error = http::ApiError;

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

        let message = if expr.trim().is_empty() {
            reply("Rust Playground", "Usage: .rs\x0f <expr>")
        } else {
            match self.evaluate(expr).await {
                Ok(output) => reply("Rust Playground", &output),
                Err(e) => reply("Rust Playground", e),
            }
        };

        client.send_privmsg(channel, message)?;

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

        let result: ExecuteResponse =
            http::get_json(self.client.post(BASE_URL).json(&request)).await?;

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

/// Sanitizes the playground output for IRC: control characters (which include newlines) are
/// stripped, and the result is trimmed.
fn sanitize_output(s: &str) -> String {
    strip_control_chars(s).trim().to_owned()
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
