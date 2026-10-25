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

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::prelude::*;
use zeta_http as http;
use zeta_utils::irc::strip_control_chars;
use zeta_utils::text::Truncatable;

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

/// Plugin for evaluating Rust code.
pub struct RustPlayground {
    client: reqwest::Client,
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

#[plugin(name = "rust_playground", settings = Settings)]
impl RustPlayground {
    // The `Plugin` trait requires the constructor to return `Result`, even when initialization
    // cannot fail.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = http::build_client(&ctx.config.http);

        Ok(Self {
            client,
            settings: settings.clone(),
        })
    }

    /// Replies to `.rs` with the playground's output for the expression.
    #[command(".rs", "Evaluate a Rust expression on the Rust playground", event)]
    async fn evaluate_command(
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

        let output = if result.success {
            sanitize_output(&result.stdout)
        } else {
            let errors = extract_errors(&result.stderr);
            if errors.is_empty() {
                // Fallback to raw stderr if no specific errors were found
                sanitize_output(&result.stderr)
            } else {
                format!("Compilation error(s): {}", errors.join(", "))
            }
        };

        Ok(output
            .truncate_with_suffix(self.settings.max_output_length, "…")
            .into_owned())
    }
}

/// Extracts compiler error messages from the stderr output.
fn extract_errors(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("error")?;

            let rest = if let Some(rest) = rest.strip_prefix('[') {
                let (code, rest) = rest.split_once(']')?;
                let digits = code.strip_prefix('E')?;
                if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                rest
            } else {
                rest
            };

            rest.strip_prefix(": ").map(str::to_owned)
        })
        .collect()
}

/// Sanitizes the playground output for IRC: control characters (which include newlines) are
/// stripped, and the result is trimmed.
fn sanitize_output(s: &str) -> String {
    strip_control_chars(s).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

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

    #[test]
    fn extracts_compiler_errors() {
        let stderr = "\
error[E0425]: cannot find value `x` in this scope
  --> src/main.rs:1:20
error: expected one of `!`, `(`, `::`, `;`, or `=`, found `}`
warning: unused variable: `y`
errors: not a compiler error
error[E]: also not a compiler error
";

        assert_eq!(
            extract_errors(stderr),
            [
                "cannot find value `x` in this scope",
                "expected one of `!`, `(`, `::`, `;`, or `=`, found `}`",
            ]
        );
    }

    #[test]
    fn extracts_no_errors_from_other_output() {
        for stderr in ["", "warning: unused variable: `x`", "error", "error[E123]"] {
            assert_eq!(extract_errors(stderr), Vec::<String>::new(), "{stderr}");
        }
    }

    #[test]
    fn sanitizes_playground_output() {
        // Control characters (which include newlines) are stripped...
        assert_eq!(sanitize_output("hello\nworld\r\t"), "helloworld");
        // ...as are formatting codes and their color parameters.
        assert_eq!(sanitize_output("\u{3}10hello\u{f}"), "hello");
        assert_eq!(sanitize_output("\u{4}FF0000hello"), "hello");
        // ...and the result is trimmed.
        assert_eq!(
            sanitize_output("  leading and trailing  \n"),
            "leading and trailing"
        );
        assert_eq!(sanitize_output("unchanged"), "unchanged");
    }
}
