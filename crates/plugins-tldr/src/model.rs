//! The `summary_labs` response model.
//!
//! Every field the endpoint sends is kept, read or not: the shape is observed from a recorded
//! browser session, and a field that becomes useful should not have to be re-discovered. Fields
//! whose real shape was only seen as `null` are tolerant values.

use serde::Deserialize;
use serde_json::Value;

/// The JSON body of a successful `mother/summary_labs` response.
#[derive(Debug, Deserialize)]
pub(crate) struct SummaryResponse {
    /// The summary as HTML, e.g. `<p>…</p>`.
    #[allow(dead_code)]
    output_text: Option<String>,
    /// The summary details; present on every success seen so far.
    #[serde(default)]
    output_data: Option<OutputData>,
    /// The total tokens the summary spent.
    #[allow(dead_code)]
    #[serde(default)]
    tokens: Option<i64>,
    /// The event kind, `final` on the one-shot endpoint.
    #[allow(dead_code)]
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl SummaryResponse {
    /// The summary as markdown: plain text with `-` bullets and a leading `Title:` line.
    ///
    /// This is the body the reply renders; the HTML `output_text` would need stripping, and the
    /// markdown field carries the same content.
    pub(crate) fn markdown(&self) -> Option<&str> {
        self.output_data
            .as_ref()
            .and_then(|data| data.markdown.as_deref())
    }
}

/// The details block of a summary response.
#[derive(Debug, Deserialize)]
pub(crate) struct OutputData {
    /// The summary status, `completed` on every success seen so far.
    #[allow(dead_code)]
    #[serde(default)]
    status: Option<String>,
    /// The page word/token counts, and reading time saved.
    #[allow(dead_code)]
    #[serde(default)]
    word_stats: Option<WordStats>,
    /// How long the summarization took, in seconds.
    #[allow(dead_code)]
    #[serde(default)]
    elapsed_seconds: Option<f64>,
    /// The summary as markdown.
    #[serde(default)]
    markdown: Option<String>,
    /// The model and spend metadata.
    #[allow(dead_code)]
    #[serde(default)]
    response_metadata: Option<Value>,
    /// Images embedded in the summary.
    #[allow(dead_code)]
    #[serde(default)]
    images: Option<Value>,
    /// The page title the summary starts from.
    #[allow(dead_code)]
    #[serde(default)]
    title: Option<String>,
    /// The page authors; only seen as `null` so far.
    #[allow(dead_code)]
    #[serde(default)]
    authors: Option<Value>,
    /// The page favicon.
    #[allow(dead_code)]
    #[serde(default)]
    favicon_url: Option<String>,
}

/// The word/token counts of the summarized page.
#[derive(Debug, Deserialize)]
pub(crate) struct WordStats {
    /// Tokens on the page.
    #[allow(dead_code)]
    #[serde(default)]
    n_tokens: Option<i64>,
    /// Words on the page.
    #[allow(dead_code)]
    #[serde(default)]
    n_words: Option<i64>,
    /// Pages the summary drew from.
    #[allow(dead_code)]
    #[serde(default)]
    n_pages: Option<i64>,
    /// Reading time saved, in minutes.
    #[allow(dead_code)]
    #[serde(default)]
    time_saved: Option<i64>,
    /// The requested summary length; only seen as `null` so far.
    #[allow(dead_code)]
    #[serde(default)]
    length: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_summary_response() {
        let response: SummaryResponse =
            serde_json::from_str(super::super::tests::summary_fixture()).expect("parses");

        assert_eq!(response.kind.as_deref(), Some("final"));
        assert_eq!(response.tokens, Some(1215));
        assert_eq!(
            response
                .markdown()
                .and_then(|markdown| markdown.lines().next()),
            Some(
                "Title: USA's finansminister vil have svar på Nvidias forbindelser til Kina | Udland | DR"
            ),
        );
    }

    #[test]
    fn parses_a_takeaway_response() {
        let response: SummaryResponse =
            serde_json::from_str(super::super::tests::takeaway_fixture()).expect("parses");

        let markdown = response.markdown().expect("the markdown is present");
        assert!(
            markdown
                .lines()
                .filter(|line| line.starts_with("- "))
                .count()
                > 3,
            "a takeaway is a bullet list: {markdown}"
        );
        assert_eq!(response.kind.as_deref(), Some("final"));
    }

    #[test]
    fn a_body_without_markdown_has_no_markdown() {
        let response: SummaryResponse =
            serde_json::from_str(r#"{"output_text": "<p>hi</p>"}"#).expect("parses");

        assert_eq!(response.markdown(), None);
    }
}
