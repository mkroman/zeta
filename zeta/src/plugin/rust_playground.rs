//! Rust Playground integration.
//!
//! Evaluates Rust code using the online Rust Playground.

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*, utils::Truncatable};

const BASE_URL: &str = "https://play.rust-lang.org/execute";

/// Plugin for evaluating Rust code.
pub struct RustPlayground {
    client: reqwest::Client,
    command: ZetaCommand,
    error_regex: Regex,
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
impl Plugin for RustPlayground {
    fn new() -> Self {
        let client = http::build_client();
        let command = ZetaCommand::new(".rs");
        // Regex to extract error messages from stderr (e.g. "error[E0425]: cannot find value...")
        let error_regex = Regex::new(r"(?m)^error(?:\[E\d+\])?: (.*?)$").expect("invalid regex");

        Self {
            client,
            command,
            error_regex,
        }
    }

    fn name() -> Name {
        Name::from("rust_playground")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("1.0")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(expr) = self.command.parse(user_message)
        {
            // Early return if input is empty
            if expr.trim().is_empty() {
                client.send_privmsg(channel, formatted("Usage: .rs\x0f <expr>"))?;
                return Ok(());
            }

            match self.evaluate(expr).await {
                Ok(output) => {
                    client.send_privmsg(channel, formatted(&output))?;
                }
                Err(e) => {
                    warn!("rust playground error: {}", e);
                    client.send_privmsg(channel, formatted(&format!("http error: {e}")))?;
                }
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
            channel: "stable",
            mode: "debug",
            edition: "2024",
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
            Ok(output.truncate_with_suffix(250, "…").into_owned())
        } else {
            let errors = self.extract_errors(&result.stderr);
            let output = if errors.is_empty() {
                // Fallback to raw stderr if no specific errors were found
                sanitize_output(&result.stderr)
            } else {
                format!("Compilation error(s): {}", errors.join(", "))
            };
            Ok(output.truncate_with_suffix(250, "…").into_owned())
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

/// Applies IRC formatting to the message.
fn formatted(msg: &str) -> String {
    format!("\x0310>\x0F\x02 Rust Playground:\x02\x0310 {msg}")
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
