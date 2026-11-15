//! Source scan that keeps raw error values out of channel replies.
//!
//! `AGENTS.md` — which is not committed — states the rule: a channel may only see an error's
//! reply-grade sentence, never an operator-grade `Display` (source text, URLs, SQL/serde detail,
//! dependency names, internal identifiers). Reply helpers therefore take an already-mapped
//! `&str`; passing an error value straight to one means its `Display` was never audited.
//!
//! This test follows `no_raw_transport_errors_outside_the_wrapper` in `zeta-http`: it scans every
//! bundled plugin so a new reply path cannot bypass the rule by living in a crate no one remembered
//! to add to a hand-maintained list.

use std::path::{Path, PathBuf};

/// The reply helpers whose last argument is the sentence sent to a channel.
///
/// `send_reply` and `send_reply_via` take the rendered sentence directly; `plain_reply` and
/// `labeled_reply` build it from theirs. Nested calls are inspected at every level, so the
/// `labeled_reply` inside `send_reply(client, channel, labeled_reply(…))` is checked too.
const REPLY_HELPERS: &[&str] = &[
    "plain_reply(",
    "labeled_reply(",
    "send_reply(",
    "send_reply_via(",
];

/// Bare expressions whose value is an error, not an audited reply sentence.
///
/// Only a plain identifier (or its `to_string()`) matches. A mapper call such as
/// `lookup_reply(&error)` is the expected shape and does not.
const RAW_ERROR_ARGUMENTS: &[&str] = &[
    "error",
    "err",
    "&error",
    "&err",
    "error.to_string()",
    "err.to_string()",
];

/// The intentional joke module, which does not follow the reply conventions.
const SKIPPED_PLUGIN: &str = "plugins-gay";

/// One deliberate, reviewed exception to the rule.
struct Exception {
    /// Path relative to the workspace `crates/` directory, with `/` separators.
    path: &'static str,
    /// The exact trimmed source line.
    line: &'static str,
    /// Why the raw value is safe to send here.
    reason: &'static str,
}

impl Exception {
    /// Whether `relative` and `line` identify this exception.
    fn matches(&self, relative: &str, line: &str) -> bool {
        self.path == relative && self.line == line
    }
}

/// The existing call sites where the raw error's `Display` is documented as reply-grade, or where
/// the bare value is already a rendered reply rather than an error.
const EXCEPTIONS: &[Exception] = &[
    Exception {
        path: "plugins-alert/src/lib.rs",
        line: r#"send_reply(client, channel, labeled_reply("Alert", err.to_string()))?;"#,
        reason: "the time parser's Display is the user-facing reason",
    },
    Exception {
        path: "plugins-coinmarketcap/src/lib.rs",
        line: "send_reply(client, channel, plain_reply(err.to_string()))?;",
        reason: "the client Error Display is documented reply-grade",
    },
    Exception {
        path: "plugins-dig/src/lib.rs",
        line: r#"send_reply(client, channel, labeled_reply("Dig", err.to_string()))?;"#,
        reason: "Error::Resolve Display is reply-grade after #565",
    },
    Exception {
        path: "plugins-filter/src/lib.rs",
        line: r#"send_reply(client, channel, labeled_reply("Filter", &error))?;"#,
        reason: "the validator error's Display is the user-facing reason",
    },
    Exception {
        path: "plugins-geoip/src/lib.rs",
        line: r#"send_reply(client, channel, labeled_reply("GeoIP", err))?;"#,
        reason: "err is the already-rendered reply string",
    },
    Exception {
        path: "plugins-kagi/src/lib.rs",
        line: "plain_reply(err)",
        reason: "the client Error Display is documented reply-grade (two call sites)",
    },
    Exception {
        path: "plugins-titles/src/lib.rs",
        line: "send_reply_via(&sender, &channel, plain_reply(error))",
        reason: "metadata::Error Display is documented reply-grade",
    },
];

/// Recursively collects every Rust source file under `dir` into `files`.
fn sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source directory") {
        let path = entry.expect("source directory entry").path();

        if path.is_dir() {
            sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// The text between the opening parenthesis at `open` and its balanced closing parenthesis.
///
/// Returns `None` when the call is not closed on this line: the scan is a single-line heuristic,
/// so a call split across lines is not judged. Strings and comments are not parsed either.
fn call_arguments(line: &str, open: usize) -> Option<&str> {
    let mut depth = 0usize;

    for (offset, character) in line[open..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;

                if depth == 0 {
                    return Some(&line[open + 1..open + offset]);
                }
            }
            _ => {}
        }
    }

    None
}

/// The last argument of `arguments`, ignoring commas nested in a call, index or block.
///
/// Commas inside string literals are not understood, which is fine: a sentence argument never ends
/// with a bare error identifier.
fn last_argument(arguments: &str) -> &str {
    let mut depth = 0usize;
    let mut start = 0usize;

    for (index, character) in arguments.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => start = index + 1,
            _ => {}
        }
    }

    arguments[start..].trim()
}

/// Whether `line` passes a bare error value to a reply helper as its last argument.
fn passes_raw_error(line: &str) -> bool {
    REPLY_HELPERS.iter().any(|helper| {
        line.match_indices(helper).any(|(position, _)| {
            // `foo_plain_reply(` is a different helper; `Box::plain_reply(` still counts.
            let preceded_by_identifier = line[..position]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');

            if preceded_by_identifier {
                return false;
            }

            let open = position + helper.len() - 1;

            call_arguments(line, open)
                .is_some_and(|arguments| RAW_ERROR_ARGUMENTS.contains(&last_argument(arguments)))
        })
    })
}

/// Scans every bundled plugin and reports raw error values reaching a reply helper.
#[test]
fn no_raw_error_values_are_passed_to_reply_helpers() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates_dir = manifest.parent().expect("the crate is a workspace member");

    let mut files = Vec::new();

    for entry in std::fs::read_dir(crates_dir).expect("read the crates directory") {
        let plugin = entry.expect("directory entry").path();

        if !plugin.is_dir() {
            continue;
        }

        let name = plugin
            .file_name()
            .map_or_default(|name| name.to_string_lossy());

        if !name.starts_with("plugins-") || name == SKIPPED_PLUGIN {
            continue;
        }

        let src = plugin.join("src");

        if src.is_dir() {
            sources(&src, &mut files);
        }
    }

    let found = files.len();

    assert!(
        found > 40,
        "the plugin scan found too few source files: {found}"
    );

    let mut exceptions_used = vec![false; EXCEPTIONS.len()];
    let mut violations = Vec::new();

    for file in &files {
        let source = std::fs::read_to_string(file).expect("read source file");
        let relative = file.trim_prefix(crates_dir).display().to_string();

        for (index, line) in source.lines().enumerate() {
            // Comments may name the rule, the helper or an example without being a call site.
            if line.trim_start().starts_with("//") || !passes_raw_error(line) {
                continue;
            }

            let trimmed = line.trim();

            if let Some(exception) = EXCEPTIONS
                .iter()
                .position(|exception| exception.matches(&relative, trimmed))
            {
                exceptions_used[exception] = true;
            } else {
                violations.push(format!("{relative}:{}: {trimmed}", index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "raw error values passed to reply helpers:\n{}\n\nMap the error to a reply-grade \
         sentence first (an exhaustive `reply()` or a per-operation mapper). If its `Display` is \
         deliberately reply-grade, add the exact line to `EXCEPTIONS` with a reason.",
        violations.join("\n")
    );

    let stale = EXCEPTIONS
        .iter()
        .zip(&exceptions_used)
        .filter(|(_, used)| !**used)
        .map(|(exception, _)| {
            format!(
                "{}: {} ({})",
                exception.path, exception.line, exception.reason
            )
        })
        .collect::<Vec<_>>();

    assert!(
        stale.is_empty(),
        "stale reply-safety exceptions (the line changed or was removed):\n{}",
        stale.join("\n")
    );
}
