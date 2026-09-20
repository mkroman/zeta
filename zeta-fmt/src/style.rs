//! A target formatting state: colors and text attributes.
//!
//! [`Style`] names what text should look like; the reply builder diffs the
//! current state against it and emits only the control codes the state
//! change requires. Styles are `const`-constructible, `Copy`, and hold no
//! heap data — they exist so operations can declare a target, and the
//! machine takes care of getting there minimally.

use crate::color::Color;

/// A target formatting state: colors and toggle attributes.
///
/// The bool count mirrors the wire format's toggle attributes (bold, italic,
/// underline, strikethrough, monospace); they are independent dimensions the
/// wire can only express as individual toggle codes, so a bitfield would
/// obscure the const builder for no measurable gain.
///
/// # Examples
///
/// ```
/// use zeta_fmt::{Color, Style};
///
/// // Const-built combinations.
/// const CRITICAL: Style = Style::new().fg(Color::Red).bold();
///
/// assert_eq!(CRITICAL, Style::new().fg(Color::Red).bold());
/// ```
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// The foreground color, or `None` for the client's default.
    pub(crate) fg: Option<Color>,
    /// The background color, or `None` for the client's default.
    pub(crate) bg: Option<Color>,
    /// Bold.
    pub(crate) bold: bool,
    /// Italic (0x1d).
    pub(crate) italic: bool,
    /// Underline (0x1f).
    pub(crate) underline: bool,
    /// Strikethrough (0x1e).
    pub(crate) strike: bool,
    /// Monospace (0x11).
    pub(crate) mono: bool,
}

impl Style {
    /// The default style: plain text, no colors, no toggles.
    ///
    /// A [`Reply`](crate::Reply) in the default style renders values plain,
    /// which is what interpolating user data uses.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            mono: false,
        }
    }

    /// Sets the foreground color.
    #[must_use]
    pub const fn fg(self, color: Color) -> Self {
        Self {
            fg: Some(color),
            ..self
        }
    }

    /// Sets the background color.
    ///
    /// The wire format expresses a background only alongside a foreground,
    /// so a background alone is sent with color 99 as the foreground
    /// placeholder (`\x0399,<bg>`). Color 99 is
    /// [not universally supported](https://modern.ircdocs.horse/formatting#colors-16-98);
    /// prefer [`Style::fg`] together with a background where possible.
    #[must_use]
    pub const fn bg(self, color: Color) -> Self {
        Self {
            bg: Some(color),
            ..self
        }
    }

    /// Enables bold text.
    #[must_use]
    pub const fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    /// Enables italicized text.
    #[must_use]
    pub const fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }

    /// Enables underlined text.
    #[must_use]
    pub const fn underline(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }

    /// Enables strikethrough'd text.
    #[must_use]
    pub const fn strike(self) -> Self {
        Self { strike: true, ..self }
    }

    /// Enables monospace'd text.
    #[must_use]
    pub const fn mono(self) -> Self {
        Self { mono: true, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_combinations() {
        let styled = Style::new().fg(Color::Red).bold().mono();

        assert_eq!(styled.fg, Some(Color::Red));
        assert!(styled.bold);
        assert!(styled.mono);
        assert!(!styled.italic);
    }

    #[test]
    fn background_alone_sets_no_foreground() {
        let style = Style::new().bg(Color::Red);

        assert_eq!(style.fg, None);
        assert_eq!(style.bg, Some(Color::Red));
    }

    #[test]
    fn default_is_plain() {
        let style = Style::default();
        let plain = Style::new();

        assert_eq!(style, plain);
    }
}
