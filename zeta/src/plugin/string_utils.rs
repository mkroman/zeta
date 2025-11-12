use std::fmt::Write;

use crate::plugin::prelude::*;

#[allow(clippy::struct_field_names)]
pub struct StringUtils {
    /// The `.b` string to bytes command trigger
    bytes_command: ZetaCommand,
    /// The `.len` string length command trigger
    length_command: ZetaCommand,
    /// The `.ord` command trigger
    ord_command: ZetaCommand,
    /// The `.rev` string reverse command trigger
    reverse_command: ZetaCommand,
    /// The `.uni` command trigger
    unicode_command: ZetaCommand,
}

#[async_trait]
impl Plugin for StringUtils {
    fn new() -> StringUtils {
        StringUtils::new()
    }

    fn name() -> Name {
        Name::from("string_utils")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command {
            if let Some(args) = self.bytes_command.parse(user_message) {
                if args.is_empty() {
                    client.send_privmsg(channel, formatted("Usage: .b\x0f <byte..>"))?;
                } else {
                    client.send_privmsg(channel, formatted(&str_to_hex_string(args)))?;
                }
            } else if let Some(args) = self.length_command.parse(user_message) {
                if args.is_empty() {
                    client.send_privmsg(channel, formatted("Usage: .len\x0f <string>"))?;
                } else {
                    client
                        .send_privmsg(channel, formatted(&format!("{}", args.chars().count())))?;
                }
            } else if let Some(args) = self.ord_command.parse(user_message) {
                if args.is_empty() {
                    client.send_privmsg(channel, formatted("Usage: .ord\x0f <chars..>"))?;
                } else {
                    let orded: Vec<String> = args.chars().map(|x| (x as u32).to_string()).collect();

                    client.send_privmsg(channel, formatted(&orded.join(", ")))?;
                }
            } else if let Some(args) = self.reverse_command.parse(user_message) {
                if args.is_empty() {
                    client.send_privmsg(channel, formatted("Usage: .rev\x0f <string>"))?;
                } else {
                    let reversed: String = args.chars().rev().collect();

                    client.send_privmsg(channel, formatted(&reversed))?;
                }
            } else if let Some(_args) = self.unicode_command.parse(user_message) {
            }
        }

        Ok(())
    }
}

fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
}

fn str_to_hex_string(s: &str) -> String {
    let mut buf = String::with_capacity(s.len() * 4);

    for b in s.bytes() {
        write!(buf, "\\x{b:x}").unwrap();
    }

    buf
}

impl StringUtils {
    pub fn new() -> StringUtils {
        let unicode_command = ZetaCommand::new(".uni");
        let bytes_command = ZetaCommand::new(".b");
        let ord_command = ZetaCommand::new(".ord");
        let length_command = ZetaCommand::new(".len");
        let reverse_command = ZetaCommand::new(".rev");

        StringUtils {
            bytes_command,
            length_command,
            ord_command,
            reverse_command,
            unicode_command,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_to_hex_string_test() {
        assert_eq!(
            str_to_hex_string("🏳️‍🌈"),
            r"\xf0\x9f\x8f\xb3\xef\xb8\x8f\xe2\x80\x8d\xf0\x9f\x8c\x88"
        );
    }
}
