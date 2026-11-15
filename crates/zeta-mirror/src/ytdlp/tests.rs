//! Cross-cutting tests for the `yt-dlp` runner.
//!
//! The tests here cover the runner's configuration and the json dump model, plus the helper the
//! other modules' tests use to build a runner. Tests for a single submodule live with that
//! submodule.

use super::*;

impl YtDlp {
    /// Creates a runner that invokes the given command.
    #[must_use]
    pub(crate) fn with_command(command: &str) -> Self {
        Self {
            command: command.to_string(),
            max_filesize: "500M".to_string(),
            download_timeout: Duration::from_mins(10),
        }
    }
}

const SAMPLE_OUTPUT: &str = r#"{
        "id": "7541501431543532814",
        "title": "some video title",
        "requested_downloads": [
            {
                "filepath": "/tmp/example/7541501431543532814.mp4",
                "id": "7541501431543532814",
                "ext": "mp4",
                "vcodec": "avc1.640029",
                "acodec": "mp4a.40.2"
            }
        ]
    }"#;

#[test]
fn test_parse_output() {
    // Downloads are reported when present, and absent (not an error) otherwise.
    let output: JsonDump = serde_json::from_str(SAMPLE_OUTPUT).unwrap();
    assert_eq!(output.requested_downloads.len(), 1);

    let download = &output.requested_downloads[0];
    assert_eq!(
        download.filepath,
        PathBuf::from("/tmp/example/7541501431543532814.mp4")
    );
    assert_eq!(
        download.filename().as_deref(),
        Some("7541501431543532814.mp4")
    );
    assert!(!download.is_unsupported_codec());

    let output: JsonDump = serde_json::from_str("{}").unwrap();
    assert_eq!(output.requested_downloads, Vec::new());
}

#[test]
fn blank_command_falls_back_to_the_environment_then_the_default() {
    let options = |command: Option<&str>| YtDlpOptions {
        command: command.map(ToOwned::to_owned),
        max_filesize: "500M".to_string(),
        download_timeout: Duration::from_mins(10),
    };

    // A configured command wins over the environment and the default.
    assert_eq!(
        YtDlp::new(options(Some("/usr/local/bin/yt-dlp"))).command,
        "/usr/local/bin/yt-dlp"
    );

    // A blank command counts as unset and resolves exactly like an omitted one. The test
    // does not mutate the process environment; it computes the environment's contribution
    // with the same helper the runner uses.
    let expected = zeta_utils::settings::resolve_optional_setting(Some(" "), COMMAND_ENV)
        .unwrap_or_else(|| DEFAULT_COMMAND.to_owned());

    assert_eq!(YtDlp::new(options(Some("   "))).command, expected);
    assert_eq!(YtDlp::new(options(None)).command, expected);
}
