//! Parsing of the progress lines emitted by `yt-dlp`.
//!
//! The lines follow [`PROGRESS_TEMPLATE`](super::args::PROGRESS_TEMPLATE) and start with
//! [`PROGRESS_PREFIX`], which tells them apart from the json dump and `yt-dlp`'s other output.

/// The marker prefixing the progress lines emitted with `--progress-template`, used to tell them
/// apart from `yt-dlp`'s other output.
const PROGRESS_PREFIX: &str = "zeta-dl";

/// A progress update of a running download, as reported by `yt-dlp`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Progress {
    /// The number of bytes downloaded so far.
    pub downloaded: Option<u64>,
    /// The total size of the download in bytes, if known or estimated upfront.
    pub total: Option<u64>,
    /// The current download speed in bytes per second.
    pub speed: Option<u64>,
    /// The estimated time remaining in seconds.
    pub eta: Option<u64>,
}

impl Progress {
    /// Returns the downloaded fraction of the total size, if the total size is known.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total? as f64;
        let downloaded = self.downloaded.unwrap_or_default() as f64;

        (total > 0.0).then_some(downloaded / total)
    }
}

/// Parses a progress line emitted with [`PROGRESS_TEMPLATE`](super::args::PROGRESS_TEMPLATE),
/// returning `None` for any other line.
pub(super) fn parse_progress_line(line: &str) -> Option<Progress> {
    let fields = line.strip_prefix(PROGRESS_PREFIX)?.split_ascii_whitespace();

    let mut fields = fields.map(parse_size_field);
    let downloaded = fields.next()?;
    let total = fields.next()?;
    let total_estimate = fields.next()?;
    let speed = fields.next()?;
    let eta = fields.next()?;

    Some(Progress {
        downloaded,
        // The exact size is unknown ("NA") for some downloads; fall back to the estimate.
        total: total.or(total_estimate),
        speed,
        eta,
    })
}

/// Parses a raw `yt-dlp` progress field, returning `None` if it is `NA` or not a number.
fn parse_size_field(field: &str) -> Option<u64> {
    field.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_progress_line() {
        let progress = parse_progress_line("zeta-dl 512 1024 NA 256 2").unwrap();
        assert_eq!(
            progress,
            Progress {
                downloaded: Some(512),
                total: Some(1024),
                speed: Some(256),
                eta: Some(2),
            }
        );
        assert_eq!(progress.fraction(), Some(0.5));

        // The estimated size is used when the exact size is unknown.
        let progress = parse_progress_line("zeta-dl 100 NA 400 10 30").unwrap();
        assert_eq!(progress.total, Some(400));
        assert_eq!(progress.fraction(), Some(0.25));

        // Fields that are unknown are reported as absent.
        assert_eq!(
            parse_progress_line("zeta-dl NA NA NA NA NA"),
            Some(Progress::default())
        );

        // Anything else is not a progress line.
        assert_eq!(parse_progress_line(r#"{"id": "123"}"#), None);
        assert_eq!(parse_progress_line(PROGRESS_PREFIX), None);
        assert_eq!(parse_progress_line(""), None);
    }
}
