//! Error types

use miette::Diagnostic;
use thiserror::Error;

pub use irc::error::Error as IrcError;

#[cfg(feature = "database")]
pub use sqlx::{Error as SqlxError, migrate::MigrateError as SqlxMigrateError};

/// Application errors for database, IRC, and plugin operations.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    /// Failed to establish a connection to the database.
    #[error("Cannot connect to database")]
    #[cfg(feature = "database")]
    OpenDatabase(#[source] SqlxError),
    /// Failed to create the IRC client.
    #[error("Could not create IRC client")]
    IrcClient(#[source] IrcError),
    /// Failed to register with the IRC server.
    #[error("Could not send registration details for IRC")]
    IrcRegistration(#[source] irc::error::Error),
    /// Failed to acquire a database connection from the connection pool.
    #[error("Could not acquire a connection from the connection pool")]
    #[cfg(feature = "database")]
    DatabasePool(#[source] SqlxError),
    /// Database schema migration failed.
    #[error("Database migration failed")]
    #[cfg(feature = "database")]
    DatabaseMigration(#[source] SqlxMigrateError),
    /// General IRC communication error.
    #[error("IRC error")]
    Irc(#[from] IrcError),
}

/// Request errors rendered once at construction, so formatting one can never put a credential
/// into a log line, a span attribute or a channel reply — and so the guarantee cannot rest on
/// someone remembering to strip a URL before an error is logged.
///
/// These APIs carry their key in the query string — and a proxy URL may carry one as
/// `user:password@host` — while `reqwest::Error` and `wreq::Error` print their request URL in
/// both `Display` and `Debug`. The wrappers below are the only place in this crate that touch
/// `without_url()`/`without_uri()`, and they keep no transport error at all: everything a
/// formatted output can reach is a redacted string.
///
/// [`Display`](std::fmt::Display) is the short, classified rendering a channel reply needs
/// (`could not connect (api.example.com)`), while [`RequestError::full`] is what a log line
/// gets: error kind, source chain and redacted URL. [`std::error::Error::source`] reports no
/// source — the chain is already rendered by `full()`, and returning it would print it twice.
#[cfg(any(feature = "http", feature = "mirror", feature = "emulated"))]
mod request {
    use std::error::Error as StdError;
    use std::fmt;
    use std::fmt::Write as _;

    #[cfg(any(feature = "http", feature = "mirror"))]
    use crate::url::redact_url;

    /// Renders the message a log line gets: `kind`, its source chain and its redacted URL.
    ///
    /// `skip` drops that many links from the front of the chain: `wreq::Error` prints its first
    /// source itself, `reqwest::Error` prints none of them. A link that already ends the
    /// message with its own source — wreq's `DnsError` does — is not appended a second time.
    fn detail(
        kind: &str,
        error: &(dyn StdError + 'static),
        skip: usize,
        url: Option<&str>,
    ) -> String {
        let mut message = String::from(kind);
        let mut cause = error.source();

        for _ in 0..skip {
            cause = cause.and_then(|link| link.source());
        }

        while let Some(link) = cause {
            let text = link.to_string();

            if !text.is_empty() && !message.ends_with(&text) {
                let _ = write!(message, ": {text}");
            }

            cause = link.source();
        }

        if let Some(url) = url {
            let _ = write!(message, " for url ({url})");
        }

        message
    }

    /// Writes the short rendering a channel reply needs: the classified failure and the host it
    /// happened to, never the path (which a user can make arbitrarily long) or the cause chain.
    fn write_short(
        f: &mut fmt::Formatter<'_>,
        kind: &str,
        host: Option<&str>,
        timeout: bool,
        connect: bool,
    ) -> fmt::Result {
        if timeout {
            f.write_str("request timed out")?;
        } else if connect {
            f.write_str("could not connect")?;
        } else {
            f.write_str(kind)?;
        }

        host.map_or(Ok(()), |host| write!(f, " ({host})"))
    }

    /// The low-cardinality [`error.type`] value for a classified transport failure: the
    /// attribute OpenTelemetry wants on a failed client span instead of a longer message.
    ///
    /// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
    const fn classify(timeout: bool, connect: bool) -> &'static str {
        if timeout {
            "timeout"
        } else if connect {
            "connection_error"
        } else {
            "other"
        }
    }

    /// A `reqwest::Error` whose URL has been redacted and whose message is rendered once.
    ///
    /// The URL is stripped from the wrapped error — which is dropped — and kept here in its
    /// redacted form, so [`Display`](std::fmt::Display), [`Debug`](std::fmt::Debug) and the
    /// source chain are all free of the query string by construction.
    #[cfg(any(feature = "http", feature = "mirror"))]
    #[derive(Debug)]
    pub struct RequestError {
        /// The transport error's own message, with its URL stripped ("error sending request").
        kind: String,
        /// [`kind`] with the source chain and the redacted URL appended — what a log line gets.
        detail: String,
        /// The request URL without its query string or userinfo.
        url: Option<String>,
        /// The host the request went to, for the short rendering.
        host: Option<String>,
        /// Whether the request timed out.
        timeout: bool,
        /// Whether the request failed before it could be sent.
        connect: bool,
        /// Whether the failure happened while sending the request rather than while reading
        /// its response.
        request: bool,
    }

    #[cfg(any(feature = "http", feature = "mirror"))]
    impl RequestError {
        /// Returns the request URL without its query string or userinfo, if the error carries
        /// one.
        #[must_use]
        pub fn url(&self) -> Option<&str> {
            self.url.as_deref()
        }

        /// Returns the full rendering — error kind, source chain and redacted URL — for a log
        /// line, where naming the request that failed matters more than staying short.
        #[must_use]
        pub fn full(&self) -> &str {
            &self.detail
        }

        /// Returns the [`error.type`] value for this failure, for a span or log field.
        ///
        /// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
        #[must_use]
        pub const fn error_type(&self) -> &'static str {
            classify(self.timeout, self.connect)
        }

        /// Returns whether the request timed out.
        #[must_use]
        pub const fn is_timeout(&self) -> bool {
            self.timeout
        }

        /// Returns whether the request failed before it could be sent, e.g. because the
        /// connection was refused or the host could not be resolved.
        #[must_use]
        pub const fn is_connect(&self) -> bool {
            self.connect
        }

        /// Returns whether the failure happened while sending the request rather than while
        /// reading its response.
        #[must_use]
        pub const fn is_request(&self) -> bool {
            self.request
        }
    }

    #[cfg(any(feature = "http", feature = "mirror"))]
    impl From<reqwest::Error> for RequestError {
        fn from(error: reqwest::Error) -> Self {
            let url = error.url().map(redact_url);
            let host = error
                .url()
                .and_then(|url| url.host_str().map(str::to_string));
            let timeout = error.is_timeout();
            let connect = error.is_connect();
            let request = error.is_request();

            let error = error.without_url();
            let kind = error.to_string();
            let detail = detail(&kind, &error, 0, url.as_deref());

            Self {
                kind,
                detail,
                url,
                host,
                timeout,
                connect,
                request,
            }
        }
    }

    #[cfg(any(feature = "http", feature = "mirror"))]
    impl fmt::Display for RequestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_short(
                f,
                &self.kind,
                self.host.as_deref(),
                self.timeout,
                self.connect,
            )
        }
    }

    #[cfg(any(feature = "http", feature = "mirror"))]
    impl StdError for RequestError {}

    /// A `wreq::Error` whose URI has been redacted and whose message is rendered once.
    ///
    /// The same contract as [`RequestError`]: the browser-emulated client prints its request URI
    /// in `Display` and `Debug`, and the metadata proxy instagram falls back to can carry a
    /// credential in its query.
    #[cfg(feature = "emulated")]
    #[derive(Debug)]
    pub struct WreqError {
        /// The transport error's own message, with its URI stripped.
        kind: String,
        /// [`kind`] with the source chain and the redacted URI appended — what a log line gets.
        detail: String,
        /// The request URI without its query string or userinfo.
        url: Option<String>,
        /// The host the request went to, for the short rendering.
        host: Option<String>,
        /// Whether the request timed out.
        timeout: bool,
        /// Whether the request failed before it could be sent.
        connect: bool,
    }

    #[cfg(feature = "emulated")]
    impl WreqError {
        /// Returns the request URI without its query string or userinfo, if the error carries
        /// one. An origin-form URI (no scheme or authority) is returned as its path alone.
        #[must_use]
        pub fn url(&self) -> Option<&str> {
            self.url.as_deref()
        }

        /// Returns the full rendering — error kind, source chain and redacted URI — for a log
        /// line, where naming the request that failed matters more than staying short.
        #[must_use]
        pub fn full(&self) -> &str {
            &self.detail
        }

        /// Returns the [`error.type`] value for this failure, for a span or log field.
        ///
        /// [`error.type`]: https://opentelemetry.io/docs/specs/semconv/registry/attributes/error/
        #[must_use]
        pub const fn error_type(&self) -> &'static str {
            classify(self.timeout, self.connect)
        }

        /// Returns whether the request timed out.
        #[must_use]
        pub const fn is_timeout(&self) -> bool {
            self.timeout
        }

        /// Returns whether the request failed before it could be sent.
        #[must_use]
        pub const fn is_connect(&self) -> bool {
            self.connect
        }
    }

    #[cfg(feature = "emulated")]
    impl From<wreq::Error> for WreqError {
        fn from(error: wreq::Error) -> Self {
            let url = error.uri().map(redact_uri);
            let host = error.uri().and_then(|uri| uri.host().map(str::to_string));
            let timeout = error.is_timeout();
            let connect = error.is_connect();

            let error = error.without_uri();
            let kind = error.to_string();
            // `wreq::Error` renders its own first source, so the chain starts one link deeper.
            let detail = detail(&kind, &error, 1, url.as_deref());

            Self {
                kind,
                detail,
                url,
                host,
                timeout,
                connect,
            }
        }
    }

    #[cfg(feature = "emulated")]
    impl fmt::Display for WreqError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_short(
                f,
                &self.kind,
                self.host.as_deref(),
                self.timeout,
                self.connect,
            )
        }
    }

    #[cfg(feature = "emulated")]
    impl StdError for WreqError {}

    /// Returns `uri` without its query string or userinfo.
    ///
    /// `wreq` hands out a [`wreq::Uri`], which has no redaction of its own: `path()` already
    /// drops the query, and the authority keeps only what follows the last `@`.
    #[cfg(feature = "emulated")]
    fn redact_uri(uri: &wreq::Uri) -> String {
        let mut redacted = String::new();

        if let Some(scheme) = uri.scheme_str() {
            redacted.push_str(scheme);
            redacted.push_str("://");
        }

        if let Some(authority) = uri.authority() {
            let authority = authority.as_str();
            let userinfo = authority
                .rfind('@')
                .map_or(authority, |at| &authority[at + 1..]);

            redacted.push_str(userinfo);
        }

        redacted.push_str(uri.path());

        redacted
    }
}

#[cfg(any(feature = "http", feature = "mirror"))]
pub use request::RequestError;

#[cfg(feature = "emulated")]
pub use request::WreqError;

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Collects every Rust source file under `dir` into `files`.
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

    /// Whether `line` declares a tuple variant (or field) holding a raw transport error, e.g.
    /// `Request(#[from] reqwest::Error)`. A `reqwest::Error` used as a function parameter or a
    /// generic argument does not match: those never end up in a formatted output on their own.
    fn holds_raw_transport_error(line: &str) -> bool {
        let line = line.trim();

        ["reqwest::Error", "wreq::Error"].into_iter().any(|ty| {
            line.find(ty).is_some_and(|position| {
                line[..position].trim_end().rfind('(').is_some_and(|open| {
                    let between = line[open + 1..position].trim();

                    between.is_empty() || between.starts_with("#[")
                })
            })
        })
    }

    /// Whether `line` strips a transport error's URL, which only the wrapper may do.
    fn strips_url(line: &str) -> bool {
        line.contains("without_url()") || line.contains("without_uri()")
    }

    /// The rule that keeps credentials out of formatted output is enforced here, because
    /// `AGENTS.md` — which states it — is not committed: no error enum anywhere may hold a raw
    /// transport error, and `without_url()`/`without_uri()` may only appear in the wrapper.
    #[test]
    fn no_raw_transport_errors_outside_the_wrapper() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        sources(&manifest.join("src"), &mut files);

        let violations = files
            .iter()
            .flat_map(|file| {
                let source = std::fs::read_to_string(file).expect("read source file");
                let relative = file
                    .strip_prefix(manifest)
                    .unwrap_or(file)
                    .display()
                    .to_string();

                source
                    .lines()
                    .enumerate()
                    .filter_map(|(index, line)| {
                        // Comments may name the type or the helper to explain the rule.
                        if line.trim_start().starts_with("//") {
                            return None;
                        }

                        let raw_error = holds_raw_transport_error(line);
                        let strips = strips_url(line) && relative != "src/error.rs";

                        if raw_error || strips {
                            Some(format!("{relative}:{}: {}", index + 1, line.trim()))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        assert!(
            violations.is_empty(),
            "raw transport errors outside `error.rs`:\n{}",
            violations.join("\n")
        );
    }
}
