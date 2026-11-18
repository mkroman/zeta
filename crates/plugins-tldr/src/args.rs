//! The `.tldr` command arguments.
//!
//! The options mirror the Kagi extension's summarize UI: the summary style, the output
//! language, and — instead of the extension's always-shown popup — how much of the summary the
//! reply carries. The value lists live on the preference enums, so the flags and `.set` can
//! never disagree about what a value is.

use clap::Parser;
use url::Url;

use crate::preferences::{Delivery, Language, SummaryType};

/// The `.tldr` command arguments.
#[derive(Parser, Debug)]
pub(crate) struct Opts {
    /// summary style: `summary` or `takeaway` (key moments)
    #[arg(short = 't', long = "type", value_enum, ignore_case = true)]
    pub(crate) summary_type: Option<SummaryType>,

    /// output language code, e.g. `EN`, `DA`, `DOC` (document language)
    #[arg(
        short = 'l',
        long = "language",
        value_enum,
        ignore_case = true,
        value_name = "LANG"
    )]
    pub(crate) language: Option<Language>,

    /// reply delivery: `truncated` (one line) or `full` (multiple lines)
    #[arg(short = 'd', long = "delivery", value_enum, ignore_case = true)]
    pub(crate) delivery: Option<Delivery>,

    /// the page to summarize
    pub(crate) url: String,
}

/// Parses the command's URL argument, requiring an http(s) page.
///
/// The endpoint fetches the page server-side, so anything but a well-formed http(s) URL is
/// rejected before a request is spent on it.
pub(crate) fn page_url(raw: &str) -> Option<Url> {
    match Url::parse(raw.trim()) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => Some(url),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_url_requires_http_s() {
        assert!(page_url("https://www.dr.dk/nyheder").is_some());
        assert!(page_url("http://example.com/a?b=c#d").is_some());
        assert!(page_url("ftp://example.com/file").is_none());
        assert!(page_url("not a url at all").is_none());
        assert!(page_url("file:///etc/passwd").is_none());
    }
}
