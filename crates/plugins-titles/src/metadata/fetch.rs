//! Streaming page fetch and response decoding.
//!
//! Sends the request, streams the response body through a bounded channel to a blocking HTML
//! tokenizer, and decodes the received bytes as UTF-8. Tokenization stops as soon as the document
//! head has been received, aborting the rest of the download.

use std::borrow::Cow;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{BufferQueue, Tokenizer, TokenizerOpts};
use tokio::sync::mpsc;
use tracing::warn;
use url::Url;

use zeta_http as http;
use zeta_http::wreq;

use super::sink::HeadSink;
use super::{Error, PageMetadata};

/// The default maximum size of a response before we stop processing it.
const MAX_RESPONSE_SIZE: u64 = 2 * 1024 * 1024;

/// Fetches `url` and extracts the metadata of its document head.
///
/// The response is streamed through a push-based HTML tokenizer; as soon as the head section has
/// been tokenized, processing stops and the rest of the response is aborted.
///
/// # Errors
///
/// Returns an error if the request fails or the server returns an unsuccessful response.
pub(crate) async fn fetch_metadata(
    client: &wreq::Client,
    url: &Url,
) -> Result<PageMetadata, Error> {
    use futures::StreamExt;

    let response = http::emulated::send(client.get(url.as_str())).await?;

    // The HTML tokenizer is not `Send`, so the response body is streamed through a channel and
    // tokenized on a blocking thread. The bounded tokio channel provides backpressure: the reader
    // awaits a free slot instead of parking a tokio worker once the tokenizer falls behind, which
    // also throttles the download.
    let (sender, mut receiver) = mpsc::channel::<Result<bytes::Bytes, wreq::Error>>(4);

    let reader = tokio::spawn(async move {
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            if sender.send(chunk).await.is_err() {
                break;
            }
        }
    });

    let metadata = tokio::task::spawn_blocking(move || -> Result<PageMetadata, Error> {
        let tokenizer = Tokenizer::new(HeadSink::new(), TokenizerOpts::default());
        let queue = BufferQueue::default();
        let mut pending: Vec<u8> = Vec::new();
        let mut total = 0_u64;

        while let Some(chunk) = receiver.blocking_recv() {
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
        warn!(
            exception.message = %error,
            "reader task failed"
        );
    }

    Ok(metadata)
}

/// Converts `chunk` to UTF-8 text for the tokenizer, carrying incomplete multi-byte sequences in
/// `pending` over to the next chunk.
///
/// The text is borrowed from `chunk` when it is valid UTF-8 on its own and no bytes are pending;
/// otherwise it is owned.
///
/// Invalid sequences are replaced with `U+FFFD`, as if the bytes were decoded lossily.
#[must_use]
pub(super) fn decode_chunk<'a>(pending: &'a mut Vec<u8>, chunk: &'a [u8]) -> Cow<'a, str> {
    // The common case: the previous chunk ended on a character boundary and this one is valid
    // UTF-8. Borrow it instead of copying it through `pending`.
    if pending.is_empty()
        && let Ok(valid) = std::str::from_utf8(chunk)
    {
        return Cow::Borrowed(valid);
    }

    pending.extend_from_slice(chunk);
    let mut text = String::with_capacity(pending.len());
    let mut offset = 0;

    loop {
        match std::str::from_utf8(&pending[offset..]) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();

                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();

                if valid > 0 {
                    // The prefix is valid UTF-8 by construction.
                    if let Ok(prefix) = std::str::from_utf8(&pending[offset..offset + valid]) {
                        text.push_str(prefix);
                    }
                }

                match error.error_len() {
                    // An incomplete sequence at the end of the chunk is carried over as-is.
                    None => {
                        pending.drain(..offset + valid);

                        break;
                    }
                    Some(invalid_len) => {
                        text.push('\u{fffd}');
                        offset += valid + invalid_len;
                    }
                }
            }
        }
    }

    Cow::Owned(text)
}
