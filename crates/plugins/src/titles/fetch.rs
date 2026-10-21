//! Streaming fetch of a page's document head.

use std::sync::mpsc;

use futures::StreamExt;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{BufferQueue, Tokenizer, TokenizerOpts};
use thiserror::Error;
use tracing::warn;
use url::Url;
use wreq::StatusCode;

use super::metadata::{HeadSink, PageMetadata};
use zeta_http::WreqError;
use zeta_url::redact_url;

/// The maximum number of response bytes read before settling for the metadata captured so far.
const MAX_RESPONSE_SIZE: u64 = 2 * 1024 * 1024;

/// Errors that can occur while fetching a page.
#[derive(Debug, Error)]
pub(super) enum Error {
    /// The server returned an unsuccessful response.
    #[error("{0}")]
    Status(StatusCode),
    /// The request failed.
    #[error("{0}")]
    Request(WreqError),
    /// The tokenizer task failed.
    #[error("could not tokenize the response")]
    Task(#[from] tokio::task::JoinError),
}

impl From<wreq::Error> for Error {
    /// Wraps `error` in a [`WreqError`], which redacts the request URL: a logged request URL
    /// can carry a credential in its query string.
    fn from(error: wreq::Error) -> Self {
        Self::Request(error.into())
    }
}

/// Fetches `url` and extracts the metadata of its document head.
///
/// The response is streamed through a push-based HTML tokenizer; as soon as the head section has
/// been tokenized, processing stops and the rest of the response is aborted.
///
/// # Errors
///
/// Returns an error if the request fails or the server returns an unsuccessful response.
pub(super) async fn fetch_metadata(
    client: &wreq::Client,
    url: &Url,
) -> Result<PageMetadata, Error> {
    let response = client.get(url.as_str()).send().await?;
    let status = response.status();

    if !status.is_success() {
        return Err(Error::Status(status));
    }

    // The HTML tokenizer is not `Send`, so the response body is streamed through a channel and
    // tokenized on a blocking thread. The channel is bounded so that the reader stalls — and the
    // download is throttled — once the tokenizer falls behind.
    let (sender, receiver) = mpsc::sync_channel::<Result<Vec<u8>, wreq::Error>>(4);

    let reader = tokio::spawn(async move {
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let sent = match chunk {
                Ok(chunk) => sender.send(Ok(chunk.to_vec())).is_ok(),
                Err(error) => sender.send(Err(error)).is_ok(),
            };

            if !sent {
                break;
            }
        }
    });

    let metadata = tokio::task::spawn_blocking(move || -> Result<PageMetadata, Error> {
        let tokenizer = Tokenizer::new(HeadSink::new(), TokenizerOpts::default());
        let queue = BufferQueue::default();
        let mut pending: Vec<u8> = Vec::new();
        let mut total = 0_u64;

        while let Ok(chunk) = receiver.recv() {
            // A stream error before the head has been tokenized fails the fetch. Errors after
            // that point are never observed — the tokenizer stops as soon as it is done.
            let chunk = chunk.map_err(Error::from)?;
            total += chunk.len() as u64;

            // Stop processing oversized responses and settle for the metadata collected so far.
            if total > MAX_RESPONSE_SIZE {
                break;
            }

            let text = decode_chunk(&mut pending, &chunk);
            queue.push_back(StrTendril::from_slice(&text));

            let _ = tokenizer.feed(&queue);

            if tokenizer.sink.is_done() {
                break;
            }
        }

        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending);
            queue.push_back(StrTendril::from_slice(&text));
            let _ = tokenizer.feed(&queue);
        }

        Ok(tokenizer.sink.into_metadata())
    });

    let metadata = metadata.await??;

    // The reader finishes once the tokenizer stops reading from the channel, aborting the rest of
    // the response.
    if let Err(error) = reader.await {
        warn!(url.full = %redact_url(url), %error, "reader task failed");
    }

    Ok(metadata)
}

/// Converts `chunk` to UTF-8 text for the tokenizer, carrying incomplete multi-byte sequences in
/// `pending` over to the next chunk.
///
/// Invalid sequences are replaced with `U+FFFD`, as if the bytes were decoded lossily.
#[must_use]
fn decode_chunk(pending: &mut Vec<u8>, chunk: &[u8]) -> String {
    pending.extend_from_slice(chunk);
    let mut text = String::new();

    loop {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();

                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();

                if valid > 0 {
                    // The prefix is valid UTF-8 by construction.
                    if let Ok(prefix) = std::str::from_utf8(&pending[..valid]) {
                        text.push_str(prefix);
                    }
                }

                match error.error_len() {
                    // An incomplete sequence at the end of the chunk is carried over as-is.
                    None => {
                        pending.drain(..valid);

                        break;
                    }
                    Some(invalid_len) => {
                        text.push('\u{fffd}');
                        pending.drain(..valid + invalid_len);
                    }
                }
            }
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_split_utf8_sequences_across_chunks() {
        let bytes = "a—b".as_bytes(); // em-dash is three bytes
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &bytes[..2]), "a");
        assert_eq!(decode_chunk(&mut pending, &bytes[2..]), "—b");
        assert_eq!(pending, Vec::<u8>::new());
    }

    #[test]
    fn replaces_invalid_sequences() {
        let mut pending = Vec::new();

        assert_eq!(decode_chunk(&mut pending, &[0xFF, b'a']), "\u{fffd}a");
        assert_eq!(decode_chunk(&mut pending, b"b"), "b");
        assert_eq!(decode_chunk(&mut pending, "æ".as_bytes()), "æ");
        assert_eq!(pending, Vec::<u8>::new());
    }
}
