use std::collections::BTreeMap;

mod error;

pub use error::Error;

/// The maximum message length as per the IRCv3 spec.
///
/// The maximum length comes from the message tags spec
static MAX_MESSAGE_LENGTH: usize = 8191;

#[derive(Eq, PartialEq)]
/// Defines the mode of strictness of the parser
pub enum Mode {
    /// The parser is strict and will fail on leading whitespace
    Strict,
    /// The parser is lenient and will ignore leading whitespace
    Lenient,
}

/// The IRC Parser
pub struct IrcParser {
    mode: Mode,
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

/// The command prefix - this can either be a hostname or a usermask, where the usermask is in the
/// format `Prefix::UserMask(nickname, username, hostname)` and the hostname is in the format
/// `Prefix::HostName(hostname)`
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Prefix<'a> {
    /// The servers hostname
    HostName(&'a [u8]),
    /// The users nickname, username and hostname
    UserMask {
        nick: &'a [u8],
        user: &'a [u8],
        host: &'a [u8],
    },
}

#[derive(Debug, Eq, PartialEq)]
pub struct Message<'a> {
    /// The message senders prefix. This is usually a server prefix or a user prefix
    prefix: Option<Prefix<'a>>,
    command: &'a [u8],
    /// List of message tags that holds references to the relevant message tags
    tags: Option<BTreeMap<&'a str, Option<&'a str>>>,
    /// List of parameters
    params: Option<Vec<&'a [u8]>>,
}

impl<'a> Message<'a> {
    /// Returns the message tags potion as a validated utf-8 string slice if present, `None`
    /// otherwise
    pub fn tags(&self) -> Option<&BTreeMap<&'a str, Option<&'a str>>> {
        self.tags.as_ref()
    }

    /// Returns the prefix portion as a byte slice if it's present, otherwise it returns `None`
    pub fn prefix(&self) -> Option<Prefix> {
        self.prefix.clone()
    }

    /// Returns the command portion as a byte slice
    pub fn command(&self) -> &'a [u8] {
        self.command
    }

    /// Returns the list of parameters, if any
    pub fn params(&self) -> Option<&Vec<&'a [u8]>> {
        self.params.as_ref()
    }
}

impl IrcParser {
    /// Constructs a new `IrcParser` with the given `strictness`
    pub fn new(mode: Mode) -> IrcParser {
        IrcParser { mode }
    }

    /// Returns whether the `IrcParser` strictness is set to strict
    pub fn is_strict(&self) -> bool {
        self.mode == Mode::Strict
    }

    /// Takes an input string slice that has already been utf-8 validated and parses each key-value
    /// pair or opaque identifiers and returns a BTreeMap
    fn parse_tags<'a>(input: &'a str) -> Result<BTreeMap<&'a str, Option<&'a str>>, Error> {
        // TODO: unescaping of values
        let mut result = BTreeMap::new();

        for pair in input.split(';') {
            let (key, value) = if let Some(pos) = pair.bytes().position(|x| x == b'=') {
                // The tag is in the format of `key=value`
                (&pair[0..pos], Some(&pair[pos + 1..]))
            } else {
                // The tag is an opaque identifier
                (pair, None)
            };

            result.insert(key, value);
        }

        Ok(result)
    }

    /// Parses the input stream for parameters and returns an optional vector
    fn parse_params<'a>(input: &'a [u8]) -> Result<Option<Vec<&'a [u8]>>, Error> {
        let mut result = Vec::new();
        let mut pos = 0usize;

        for part in input.split(|x| *x == b' ') {
            if part.is_empty() {
                break;
            }

            if part[0] == b':' {
                result.push(&input[pos + 1..]);
                break;
            } else {
                result.push(&part);
            }

            pos += part.len() + 1;
        }

        Ok(Some(result))
    }

    /// Parses the given input byte slice
    pub fn parse<'a>(&self, mut input: &'a [u8]) -> Result<Message<'a>, Error> {
        // Throw an error for any input that is longer than `MAX_MESSAGE_LENGTH`
        if input.len() > MAX_MESSAGE_LENGTH || input.is_empty() {
            return Err(Error::LengthError);
        }

        // Return an error if we're doing strict parsing and the message begins with whitespace
        if input[0].is_ascii_whitespace() && self.is_strict() {
            return Err(Error::ParseError(0));
        }

        // Extract the message tags portion if the message begins with '@'
        let tags = if input[0] == b'@' {
            let tags = self.extract_tags(input)?;

            // Advance the starting point of the input slice
            //
            // We add 1 to the length because `extract_tags` omits the following space
            // character
            input = &input[tags.len() + 1..];

            let tags = IrcParser::parse_tags(&tags[1..])?;

            Some(tags)
        } else {
            None
        };

        // Extract the prefix - this can either be a server prefix or a user hostmask prefix
        let prefix = if !input.is_empty() && input[0] == b':' {
            if let Some(pos) = input.iter().position(|x| *x == b' ') {
                let prefix = &input[1..pos];

                input = &input[pos + 1..];

                // If the prefix is in the format of nick!user@host, then parse it
                if let Some(pos) = prefix.iter().position(|x| *x == b'!') {
                    let nick = &prefix[..pos];
                    let pos = pos + 1;

                    let host_start_pos = prefix[pos..]
                        .iter()
                        .position(|x| *x == b'@')
                        .ok_or_else(|| Error::InvalidPrefixError)?
                        + 1;

                    let user = &prefix[pos..pos + host_start_pos - 1];
                    let host = &prefix[pos + host_start_pos..];

                    Some(Prefix::UserMask { nick, user, host })
                } else {
                    Some(Prefix::HostName(prefix))
                }
            } else {
                None
            }
        } else {
            None
        };

        // Extract the command
        let command = if let Some(pos) = input
            .iter()
            .position(|b| !b.is_ascii_alphabetic() && !b.is_ascii_digit())
        {
            &input[..pos]
        } else {
            // Edge-case where there might be no command
            if input.is_empty() {
                return Err(Error::EndOfStreamError);
            }

            // Consider the remaining data in the input to be a command
            let res = input;

            input = &input[res.len()..];

            res
        };

        // Extract params
        let params = if !input.is_empty() {
            IrcParser::parse_params(&input)?
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
    /// contains all the message tags
    fn extract_tags<'a>(&self, input: &'a [u8]) -> Result<&'a str, Error> {
        if let Some(pos) = input.iter().position(|x| *x == b' ') {
            let subslice = &input[..pos];

            Ok(std::str::from_utf8(subslice)?)
        } else {
            Err(Error::EndOfStreamError)
        }
    }
}
