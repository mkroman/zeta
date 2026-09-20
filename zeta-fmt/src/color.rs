//! The mIRC color palette understood by every IRC client.
//!
//! Only colors 00–15 are provided: they are the universally understood
//! palette ([modern IRC formatting](https://modern.ircdocs.horse/formatting)
//! §Colors). Extended colors 16–98 have client-defined RGB values, color 99
//! ("default") is not universally supported, and hex colors (0x04) and
//! reverse video (0x16) render inconsistently across clients — none of them
//! are exposed.

/// An mIRC color, 00–15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::exhaustive_enums)]
pub enum Color {
    /// 00 — white.
    White,
    /// 01 — black.
    Black,
    /// 02 — blue.
    Blue,
    /// 03 — green.
    Green,
    /// 04 — red.
    Red,
    /// 05 — brown.
    Brown,
    /// 06 — magenta.
    Magenta,
    /// 07 — orange.
    Orange,
    /// 08 — yellow.
    Yellow,
    /// 09 — light green.
    LightGreen,
    /// 10 — cyan, the reply scaffolding color.
    Cyan,
    /// 11 — light cyan.
    LightCyan,
    /// 12 — light blue.
    LightBlue,
    /// 13 — pink.
    Pink,
    /// 14 — grey.
    Grey,
    /// 15 — light grey.
    LightGrey,
}

impl Color {
    /// The numeric color code, e.g. 10 for [`Color::Cyan`].
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Returns the color for `code`, or [`None`] for anything outside 0–15.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::White),
            1 => Some(Self::Black),
            2 => Some(Self::Blue),
            3 => Some(Self::Green),
            4 => Some(Self::Red),
            5 => Some(Self::Brown),
            6 => Some(Self::Magenta),
            7 => Some(Self::Orange),
            8 => Some(Self::Yellow),
            9 => Some(Self::LightGreen),
            10 => Some(Self::Cyan),
            11 => Some(Self::LightCyan),
            12 => Some(Self::LightBlue),
            13 => Some(Self::Pink),
            14 => Some(Self::Grey),
            15 => Some(Self::LightGrey),
            _ => None,
        }
    }

    /// The wire form of the color: always two digits.
    ///
    /// Per [modern IRC formatting](https://modern.ircdocs.horse/formatting),
    /// two digits are always read for a color when available — so the
    /// zero-padded form (`04`, never `4`) makes text that begins with a
    /// digit display correctly instead of being eaten by the color code.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::White => "00",
            Self::Black => "01",
            Self::Blue => "02",
            Self::Green => "03",
            Self::Red => "04",
            Self::Brown => "05",
            Self::Magenta => "06",
            Self::Orange => "07",
            Self::Yellow => "08",
            Self::LightGreen => "09",
            Self::Cyan => "10",
            Self::LightCyan => "11",
            Self::LightBlue => "12",
            Self::Pink => "13",
            Self::Grey => "14",
            Self::LightGrey => "15",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_two_digit_padded() {
        assert_eq!(Color::White.wire(), "00");
        assert_eq!(Color::Red.wire(), "04");
        assert_eq!(Color::Cyan.wire(), "10");
        assert_eq!(Color::LightGrey.wire(), "15");
    }

    #[test]
    fn wire_form_round_trips() {
        for code in 0..=15u8 {
            let color = Color::from_code(code).expect("color in range");
            let wire = color.wire();
            let parsed = wire.parse::<u8>().expect("two digits");

            assert_eq!(Color::from_code(parsed), Some(color));
        }
    }

    #[test]
    fn out_of_range_codes_are_rejected() {
        assert_eq!(Color::from_code(16), None);
        assert_eq!(Color::from_code(99), None);
        assert_eq!(Color::from_code(255), None);
    }
}
