//! Running the `yt-dlp` child process and reading its output.
//!
//! The child's stdout is read line by line, so progress updates stream to the caller as `yt-dlp`
//! reports them, while stderr is drained by its own task into a bounded tail for the failure
//! message. The json dump is captured from stdout, and the downloaded files it reports are
//! verified to resolve inside the output directory before they are returned.

use std::{
    collections::VecDeque,
    io,
    path::Path,
    process::{ExitStatus, Stdio},
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader},
    process::Command,
};
use tracing::{debug, warn};
use zeta_url::RedactedUrl;

use super::args::build_args;
use super::progress::{Progress, parse_progress_line};
use super::{DownloadedFile, Error, JsonDump, YtDlp};

/// The maximum number of characters to include of `yt-dlp`'s stderr in error messages.
const STDERR_MESSAGE_LENGTH: usize = 300;

/// How long the failure path waits for the child's stderr to reach EOF before giving up.
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

impl YtDlp {
    /// Downloads the video at `url` into `output_dir`, naming the downloaded files after `id`,
    /// and streaming progress updates to `on_progress` as they are reported by `yt-dlp`.
    ///
    /// Videos are downloaded in a browser-compatible h264 format and merged into an mp4 container.
    /// Reported files that resolve outside of `output_dir` are dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if `yt-dlp` could not be spawned, exits with a failure, times out or
    /// produces output that can't be parsed.
    pub(crate) async fn download_with_progress(
        &self,
        url: &str,
        id: &str,
        output_dir: &Path,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<Vec<DownloadedFile>, Error> {
        let mut command = Command::new(&self.command);

        command
            .args(build_args(url, id, output_dir, &self.max_filesize))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Ensure the child gets killed if we're dropped (e.g. by a timeout).
            .kill_on_drop(true);

        debug!(
            url.full = %RedactedUrl::from_unparsed(url),
            process.command = %self.command,
            "downloading video with yt-dlp"
        );

        let mut child = command.spawn().map_err(Error::Io)?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Io(io::Error::other("yt-dlp stdout is not piped")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::Io(io::Error::other("yt-dlp stderr is not piped")))?;

        // Drain stderr in a separate task so it can never fill up its pipe and block the child.
        let stderr_task = tokio::spawn(drain_stderr(stderr));

        let output = tokio::time::timeout(self.download_timeout, async {
            let json = read_output(stdout, &mut on_progress).await?;
            let status = child.wait().await?;

            Ok::<RawOutput, Error>(RawOutput { status, json })
        })
        .await
        .map_err(|_| {
            stderr_task.abort();
            Error::Timeout
        })??;

        if !output.status.success() {
            // The child has exited, so its stderr reaches EOF shortly; a grandchild that inherited
            // the pipe must not hold the slot past the download timeout.
            let stderr = tokio::time::timeout(STDERR_DRAIN_TIMEOUT, stderr_task)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();

            return Err(Error::Failure(stderr.trim_end().to_string()));
        }

        stderr_task.abort();

        let downloads = verify_paths(
            output.json.ok_or(Error::NoJsonDump)?.requested_downloads,
            output_dir,
        )
        .await?;

        if downloads.is_empty() {
            return Err(Error::NoDownloads);
        }

        Ok(downloads)
    }
}

/// The raw output of a completed `yt-dlp` run.
struct RawOutput {
    /// The exit status of the `yt-dlp` process.
    status: ExitStatus,
    /// The single-video json dump, if one was reported.
    json: Option<JsonDump>,
}

/// Reads the piped stdout of a `yt-dlp` child until EOF, forwarding progress lines to
/// `on_progress` and capturing the json dump.
async fn read_output(
    stdout: impl AsyncRead + Unpin,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Option<JsonDump>, Error> {
    let mut lines = BufReader::new(stdout);
    let mut line = String::new();
    let mut json = None;

    loop {
        line.clear();

        if lines.read_line(&mut line).await? == 0 {
            break;
        }

        // `read_line` keeps the line terminator that `Lines::next_line` stripped.
        let output = line.trim_suffix('\n');
        let output = output.trim_suffix('\r');

        if let Some(progress) = parse_progress_line(output) {
            on_progress(progress);
        } else if let Ok(dump) = serde_json::from_str::<JsonDump>(output) {
            json = Some(dump);
        }
    }

    Ok(json)
}

/// Drains the given stderr stream, retaining only its last [`STDERR_MESSAGE_LENGTH`] characters
/// for the error message, with CRLF line endings normalized to `\n`.
///
/// The tail is kept in a bounded byte ring rather than collecting the whole stream: a verbose or
/// failing `yt-dlp` run must not be able to grow the bot's memory without bound. The ring holds
/// enough bytes for four-byte characters, so trimming it to characters at the end still keeps
/// the full tail.
async fn drain_stderr(mut stderr: impl AsyncRead + Unpin) -> String {
    /// Enough bytes to retain [`STDERR_MESSAGE_LENGTH`] four-byte characters.
    const TAIL_BYTES: usize = STDERR_MESSAGE_LENGTH * 4;

    let mut tail = VecDeque::with_capacity(TAIL_BYTES);
    let mut chunk = [0_u8; 4096];

    loop {
        let read = match stderr.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };

        for byte in &chunk[..read] {
            if tail.len() == TAIL_BYTES {
                tail.pop_front();
            }

            tail.push_back(*byte);
        }
    }

    // The ring can start (and, mid-stream, end) inside a character; the lossy conversion only
    // replaces the partial boundary character, which the trimming below drops when the tail is
    // longer than the retained character count.
    let text = String::from_utf8_lossy(tail.make_contiguous());
    // Match the line-based reader this replaced: a CRLF line ending is rendered as `\n`, so a
    // failure message from a CRLF-producing `yt-dlp` does not keep a stray `\r`.
    let text = if text.contains("\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        text
    };
    let start = text
        .char_indices()
        .rev()
        .nth(STDERR_MESSAGE_LENGTH - 1)
        .map_or(0, |(index, _)| index);

    text[start..].to_owned()
}

/// Returns the downloads whose reported file paths resolve inside `output_dir`, dropping any that
/// point outside of it.
async fn verify_paths(
    downloads: Vec<DownloadedFile>,
    output_dir: &Path,
) -> Result<Vec<DownloadedFile>, Error> {
    let base = tokio::fs::canonicalize(output_dir)
        .await
        .map_err(Error::Io)?;
    let mut verified = Vec::with_capacity(downloads.len());

    for download in downloads {
        // The reported path is normally absolute; fall back to resolving it relative to the
        // output directory.
        let path = match tokio::fs::canonicalize(&download.filepath).await {
            Ok(path) => Ok(path),
            Err(err) => tokio::fs::canonicalize(output_dir.join(&download.filepath))
                .await
                .map_err(|_| err),
        };

        match path {
            Ok(path) if path.starts_with(&base) => verified.push(download),
            _ => warn!(
                file.path = %download.filepath.display(),
                "ignoring downloaded file outside of the output directory"
            ),
        }
    }

    Ok(verified)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn read_output_streams_progress_and_captures_the_json_dump() {
        let stdout = &b"zeta-dl 512 1024 NA 256 2\n\
                        a line that is neither progress nor json\n\
                        zeta-dl 1024 1024 NA 256 0\n\
                        {\"id\": \"123\", \"requested_downloads\": [{\"filepath\": \"/downloads/123.mp4\", \"id\": \"123\", \"ext\": \"mp4\", \"vcodec\": \"avc1.640029\", \"acodec\": \"mp4a.40.2\"}]}\n"[..];

        let mut progress_updates = Vec::new();

        let json = read_output(stdout, &mut |progress| progress_updates.push(progress))
            .await
            .expect("reading stdout should not fail");

        // The progress lines were streamed to the callback, in order, and other output was
        // ignored.
        assert_eq!(
            progress_updates,
            vec![
                Progress {
                    downloaded: Some(512),
                    total: Some(1024),
                    speed: Some(256),
                    eta: Some(2),
                },
                Progress {
                    downloaded: Some(1024),
                    total: Some(1024),
                    speed: Some(256),
                    eta: Some(0),
                },
            ]
        );

        // The json dump was captured.
        let download = json
            .expect("the json dump should be captured")
            .requested_downloads
            .pop()
            .expect("the json dump should report the download");

        assert_eq!(download.filename().as_deref(), Some("123.mp4"));
        assert!(!download.is_unsupported_codec());
    }

    #[tokio::test]
    async fn read_output_handles_many_progress_updates_with_one_json_dump() {
        use std::fmt::Write as _;

        const UPDATES: usize = 100_000;

        let mut stdout = String::with_capacity(UPDATES * 32 + 256);

        for downloaded in 0..UPDATES {
            writeln!(stdout, "zeta-dl {downloaded} {UPDATES} NA 1024 0")
                .expect("writing to a string cannot fail");
        }

        writeln!(
            stdout,
            r#"{{"id": "123", "requested_downloads": [{{"filepath": "/downloads/123.mp4", "id": "123", "ext": "mp4", "vcodec": "h264"}}]}}"#
        )
        .expect("writing to a string cannot fail");

        let mut updates = Vec::with_capacity(UPDATES);

        let json = read_output(stdout.as_bytes(), &mut |progress| updates.push(progress))
            .await
            .expect("reading stdout should not fail");

        assert_eq!(updates.len(), UPDATES);
        assert_eq!(updates[0].downloaded, Some(0));
        assert_eq!(
            updates[UPDATES - 1],
            Progress {
                downloaded: Some(UPDATES as u64 - 1),
                total: Some(UPDATES as u64),
                speed: Some(1024),
                eta: Some(0),
            }
        );

        let download = json
            .expect("the json dump should be captured")
            .requested_downloads
            .pop()
            .expect("the json dump should report the download");

        assert_eq!(download.filename().as_deref(), Some("123.mp4"));
    }

    #[tokio::test]
    #[ignore = "performance measurement"]
    async fn bench_read_output_progress_lines() {
        use std::fmt::Write as _;
        use std::time::Instant;

        const UPDATES: usize = 100_000;
        const RUNS: usize = 20;

        let mut stdout = String::with_capacity(UPDATES * 32);

        for downloaded in 0..UPDATES {
            writeln!(stdout, "zeta-dl {downloaded} {UPDATES} NA 1024 0")
                .expect("writing to a string cannot fail");
        }

        let mut updates = Vec::with_capacity(UPDATES);
        let start = Instant::now();

        for _ in 0..RUNS {
            updates.clear();

            let json = read_output(stdout.as_bytes(), &mut |progress| updates.push(progress))
                .await
                .expect("reading stdout should not fail");

            assert!(json.is_none());
            assert_eq!(updates.len(), UPDATES);
        }

        let elapsed = start.elapsed();
        let lines = u128::from((UPDATES * RUNS) as u64);

        println!(
            "yt-dlp read_output: {lines} lines in {elapsed:?} ({} ns/line)",
            elapsed.as_nanos() / lines,
        );
    }

    #[test]
    #[ignore = "performance measurement"]
    fn bench_read_output_progress_lines_allocations() {
        use std::fmt::Write as _;

        const UPDATES: usize = 100_000;

        let mut stdout = String::with_capacity(UPDATES * 32);

        for downloaded in 0..UPDATES {
            writeln!(stdout, "zeta-dl {downloaded} {UPDATES} NA 1024 0")
                .expect("writing to a string cannot fail");
        }

        let mut updates = Vec::with_capacity(UPDATES);
        let info = allocation_counter::measure(|| {
            updates.clear();

            let json =
                futures::executor::block_on(read_output(stdout.as_bytes(), &mut |progress| {
                    updates.push(progress);
                }))
                .expect("reading stdout should not fail");

            assert!(json.is_none());
            assert_eq!(updates.len(), UPDATES);
        });

        println!(
            "yt-dlp read_output allocations: {UPDATES} lines, {} allocations, {} B",
            info.count_total, info.bytes_total,
        );
    }

    #[tokio::test]
    async fn verify_paths_keeps_files_inside_the_output_directory() {
        let output_dir = tempfile::tempdir().unwrap();
        let inside = output_dir.path().join("123.mp4");
        std::fs::write(&inside, b"junk").unwrap();

        // A reported absolute path inside the output directory is kept as reported.
        let absolute = DownloadedFile {
            filepath: inside.clone(),
            vcodec: Some("avc1.640029".to_string()),
        };
        // A reported relative path resolves against the output directory.
        let relative = DownloadedFile {
            filepath: PathBuf::from("123.mp4"),
            vcodec: None,
        };
        // A reported path pointing outside of the output directory is dropped.
        let outside = DownloadedFile {
            filepath: PathBuf::from("/etc/passwd"),
            vcodec: None,
        };

        let verified = verify_paths(vec![absolute, relative, outside], output_dir.path())
            .await
            .unwrap();

        assert_eq!(
            verified,
            vec![
                DownloadedFile {
                    filepath: inside,
                    vcodec: Some("avc1.640029".to_string()),
                },
                DownloadedFile {
                    filepath: PathBuf::from("123.mp4"),
                    vcodec: None,
                },
            ]
        );
    }

    #[tokio::test]
    async fn stderr_retains_only_the_tail() {
        let stderr = drain_stderr(&b"first line\nsecond line\n"[..]).await;

        assert_eq!(stderr, "first line\nsecond line\n");

        let long = "a".repeat(STDERR_MESSAGE_LENGTH + 20);
        let stderr = drain_stderr(format!("{long}\n").as_bytes()).await;

        assert_eq!(stderr.chars().count(), STDERR_MESSAGE_LENGTH);
        assert!(stderr.ends_with('\n'));
    }

    #[tokio::test]
    async fn stderr_retains_the_last_characters_of_a_multibyte_tail() {
        let text = "é".repeat(STDERR_MESSAGE_LENGTH + 20);
        let stderr = drain_stderr(text.as_bytes()).await;

        // The tail is cut by characters, not bytes: a multi-byte character is never split.
        assert_eq!(stderr.chars().count(), STDERR_MESSAGE_LENGTH);
        assert!(text.ends_with(&stderr));
    }

    #[tokio::test]
    async fn stderr_retains_the_last_characters_of_a_long_multibyte_tail() {
        // Longer than the internal byte ring, and ending with a two-byte character, so the ring
        // starts in the middle of a four-byte character.
        let text = format!("{}é", "😀".repeat(STDERR_MESSAGE_LENGTH + 1));
        let stderr = drain_stderr(text.as_bytes()).await;

        assert_eq!(stderr.chars().count(), STDERR_MESSAGE_LENGTH);
        assert!(text.ends_with(&stderr));
    }

    #[tokio::test]
    async fn stderr_normalizes_crlf_line_endings() {
        let stderr = drain_stderr(&b"first line\r\nsecond line\r\n"[..]).await;

        assert_eq!(stderr, "first line\nsecond line\n");

        let long = "a\r\n".repeat(STDERR_MESSAGE_LENGTH * 2);
        let stderr = drain_stderr(long.as_bytes()).await;

        assert_eq!(stderr.chars().count(), STDERR_MESSAGE_LENGTH);
        assert!(!stderr.contains('\r'));
    }
}
