mod client;
mod error;

pub use client::Client;
#[cfg(feature = "builder")]
pub use client::ClientBuilder;
pub use error::Error;

/// The maximum message length as per the IRCv3 spec.
///
/// The maximum length comes from the message tags spec
static MAX_MESSAGE_LENGTH: usize = 8191;

#[derive(Eq, PartialEq)]
/// Defines the strictness of the parser
pub enum Strictness {
    /// The parser is strict and will fail on leading whitespace
    Strict,
    /// The parser is lenient and will ignore leading whitespace
    Lenient,
}

/// The IRC Parser
pub struct IrcParser {
    strictness: Strictness,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Message<'a> {
    /// The message senders prefix. This is usually a server prefix or a user prefix
    prefix: Option<&'a [u8]>,
    command: &'a [u8],
    /// A UTF-8 validated string slice that contains the complete message tags prefix
    tags: Option<&'a str>,
    params: Option<&'a [u8]>,
}

impl<'a> Message<'a> {
    pub fn tags(&self) -> Option<&'a str> {
        self.tags
    }
}

impl IrcParser {
    /// Constructs a new `IrcParser` with the given `strictness`
    pub fn new(strictness: Strictness) -> IrcParser {
        IrcParser { strictness }
    }

    /// Returns whether the `IrcParser`ss trictness is set to strict
    pub fn is_strict(&self) -> bool {
        self.strictness == Strictness::Strict
    }

    /// Parses the given input byte slice
    pub fn parse<'a>(&self, mut input: &'a [u8]) -> Result<Message<'a>, Error> {
        if input.len() > MAX_MESSAGE_LENGTH {
            return Err(Error::LengthError);
        }

        // Return an error if we're doing strict parsing and the message begins with whitespace
        if self.is_strict() && input[0].is_ascii_whitespace() {
            return Err(Error::ParseError(0));
        }

        // Extract the message tags portion if the message begins with '@'
        let tags = if input[0] == b'@' {
            let tags = self.extract_tags(input)?;

            // Resize our input slice so that we're no longer looking at the message tags
            if let Some(tags) = tags {
                input = &input[tags.len() + 1..];
            }

            tags
        } else {
            None
        };

        Ok(Message {
            prefix: Some(b"hi!hi@hi"),
            command: b"PRIVMSG",
            tags,
            params: None,
        })
    }

    /// Extracts the tags from an input slice and returns a utf-8 validated string slice that
    /// contains the full string of message tags
    fn extract_tags<'a>(&self, input: &'a [u8]) -> Result<Option<&'a str>, Error> {
        // FIXME: I'm not even drunk, and this is the best I could come up with
        let mut offset = 0;

        for part in input.split(|b| *b == b' ') {
            if part[0] != b'@' {
                break;
            }

            offset += part.len() + 1;
        }

        Ok(Some(std::str::from_utf8(&input[..offset - 1])?))
    }
}
