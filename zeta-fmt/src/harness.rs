//! A test harness for exercising the builder through both sink flavors.
//!
//! Every op sequence is rendered through the owned `String` sink and through
//! the lazy `Display` adapter, and the two flavors must be byte-identical —
//! the grammar suite asserts the owned path and thereby proves the lazy path
//! too.

#![allow(clippy::redundant_pub_crate)] // cfg(test)-gated helpers shared across module tests

use std::fmt;

use crate::{Banner, Reply};

/// Renders a reply through the owned flavor: `banner.string(...)`.
pub(crate) fn owned(banner: &Banner, write: impl FnOnce(&mut Reply<'_>)) -> String {
    banner.string(write)
}

/// Renders a reply through the lazy flavor, forcing it into a `String`.
///
/// Mirrors how `client.send_privmsg` consumes a `Display` argument: the
/// value is formatted once, into the sink's own buffer.
pub(crate) fn lazy(banner: &Banner, write: impl Fn(&mut Reply<'_>)) -> String {
    banner.render(write).to_string()
}

/// Renders a reply directly into a `Formatter`-backed sink — the shape
/// `Display` implementations of message pieces use.
pub(crate) fn into_formatter(
    banner: &Banner,
    write: impl FnOnce(&mut Reply<'_>),
) -> String {
    struct Formatted<'a, F>(&'a Banner, std::cell::RefCell<Option<F>>);

    impl<F: FnOnce(&mut Reply<'_>)> fmt::Display for Formatted<'_, F> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let mut reply = self.0.reply(f);

            if let Some(write) = self.1.borrow_mut().take() {
                write(&mut reply);
            }

            reply.finish();

            reply.error().map_or(Ok(()), Err)
        }
    }

    Formatted(banner, std::cell::RefCell::new(Some(write))).to_string()
}

/// Checks the emitted message against the modern IRC formatting grammar.
///
/// Returns the first violation, if any:
///
/// - every color code is `\x03` followed by exactly two ASCII digits, and a
///   background pair only ever follows the foreground digits;
/// - a `,<digit>` fragment never directly follows a foreground-only code
///   (the machine guards it with a bold cancel-pair);
/// - toggle attributes are balanced by the end of the message;
/// - no stray control characters appear anywhere.
/// - the message starts with the reply marker, so reply-detection such as
///   `titles`' feedback-loop check keeps working.
pub(crate) fn grammar(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    let mut bold = false;
    let mut italic = false;
    let mut underline = false;
    let mut strike = false;
    let mut mono = false;

    if !text.starts_with(crate::REPLY_PREFIX) {
        return Err(format!("must start with the reply marker: {text:?}"));
    }

    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];

        match byte {
            0x03 => {
                let Some(fg) = bytes.get(index + 1..index + 3) else {
                    return Err(format!("color code without two digits: {text:?}"));
                };

                if !fg.iter().all(u8::is_ascii_digit) {
                    return Err(format!("color code without two digits: {text:?}"));
                }

                index += 3;

                // Optional background: `<fg>,<bg>`.
                if bytes.get(index) == Some(&b',') {
                    match bytes.get(index + 1..index + 3) {
                        Some(bg) if bg.iter().all(u8::is_ascii_digit) => index += 3,
                        // A `,<digit>` fragment following a foreground-only
                        // code would be eaten as a background — it must have
                        // been guarded by the cancel-pair instead.
                        Some(bg) if bg[0].is_ascii_digit() => {
                            return Err(format!(
                                "unguarded `,<digit>` after a color code: {text:?}"
                            ));
                        }
                        _ => {}
                    }
                }
            }
            0x02 => {
                bold = !bold;
                index += 1;
            }
            0x1d => {
                italic = !italic;
                index += 1;
            }
            0x1f => {
                underline = !underline;
                index += 1;
            }
            0x1e => {
                strike = !strike;
                index += 1;
            }
            0x11 => {
                mono = !mono;
                index += 1;
            }
            0x0f => {
                // A reset clears every toggle attribute.
                bold = false;
                italic = false;
                underline = false;
                strike = false;
                mono = false;
                index += 1;
            }
            0x00..=0x1f | 0x7f => {
                return Err(format!("stray control character 0x{byte:02x}: {text:?}"));
            }
            _ => index += 1,
        }
    }

    if bold || italic || underline || strike || mono {
        return Err(format!("dangling toggles at the end: {text:?}"));
    }

    Ok(())
}
