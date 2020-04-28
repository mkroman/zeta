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

trait SliceExt {
    /// Iterates through a slice to find the offset of a single byte
    ///
    /// Returns `Some(offset)` if the byte is found, `None` otherwise
    fn find_byte_offset<P>(&self, byte: P) -> Option<usize>
    where
        P: Fn(u8) -> bool;
}

impl SliceExt for &[u8] {
    fn find_byte_offset<P>(&self, pred: P) -> Option<usize>
    where
        P: Fn(u8) -> bool,
    {
        for i in 0..self.len() {
            // Get the byte for this iteration index. We can do this safely because our for loop
            // never passes the end of the slice
            let b = unsafe { self.get_unchecked(i) };

            if pred(*b) {
                return Some(i);
            }
        }

        None
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Message<'a> {
    /// The message senders prefix. This is usually a server prefix or a user prefix
    prefix: Option<&'a [u8]>,
    command: &'a [u8],
    /// A validated UTF-8 string slice that contains the complete message tags prefix
    tags: Option<&'a str>,
    params: Option<&'a [u8]>,
}

impl<'a> Message<'a> {
    /// Returns the message tags potion as a validated utf-8 string slice if present, `None`
    /// otherwise
    pub fn tags(&self) -> Option<&'a str> {
        self.tags
    }

    /// Returns the prefix portion as a byte slice if it's present, otherwise it returns `None`
    pub fn prefix(&self) -> Option<&'a [u8]> {
        self.prefix
    }

    /// Returns the command portion as a byte slice
    pub fn command(&self) -> &'a [u8] {
        self.command
    }
}

impl IrcParser {
    /// Constructs a new `IrcParser` with the given `strictness`
    pub fn new(strictness: Strictness) -> IrcParser {
        IrcParser { strictness }
    }

    /// Returns whether the `IrcParser` strictness is set to strict
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
                // We add 1 to the length because `extract_tags` omits the following space
                // character
                input = &input[tags.len() + 1..];
            }

            tags
        } else {
            None
        };

        // Extract the prefix - this can either be a server prefix or a user hostmask prefix
        let prefix = if input[0] == b':' {
            let res = input.find_byte_offset(|b| b == b' ');

            if let Some(offset) = res {
                let res = &input[1..offset];

                input = &input[offset + 1..];

                Some(res)
            } else {
                None
            }
        } else {
            None
        };

        // Extract the command
        let command = if let Some(offset) =
            input.find_byte_offset(|b| !b.is_ascii_alphabetic() && !b.is_ascii_digit())
        {
            &input[..offset]
        } else {
            // Edge-case where there might be no command
            if input.is_empty() {
                return Err(Error::ParseError(100));
            }

            // Consider the remaining data in the input to be a command
            let res = input;

            input = &input[res.len()..];

            res
        };

        // Extract params
        let params = if !input.is_empty() {
            self.extract_params(&input)?
        } else {
            None
        };

        Ok(Message {
            prefix,
            command,
            tags,
            params,
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

    /// Extracts the parameters from an input slice
    fn extract_params<'a>(&self, _input: &'a [u8]) -> Result<Option<&'a [u8]>, Error> {
        Ok(Some(b""))
    }
}
