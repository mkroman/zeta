//! The reply builder: a state-tracked formatter for IRC replies.
//!
//! A [`Reply`] is minted from a [`Banner`](crate::Banner) (the plugin's reply
//! identity) and written through labeled operations — `label`, `value`,
//! `field`, ... — each of which declares a *target* formatting state. The
//! machine diffs the current state against the target and emits each control
//! code only when the state actually changes: redundant and duplicate codes
//! are unrepresentable, not cleaned up afterwards.
//!
//! Values interpolated through `value`, `field`, `data` and the toggle
//! operations are stripped of control characters, so user data cannot
//! smuggle formatting codes, carriage returns or line feeds into a reply.
//! Text directly following a color code is additionally guarded against the
//! wire format's [mistaken eating of
//! text](https://modern.ircdocs.horse/formatting#mistaken-eating-of-text):
//! a fragment beginning `,<digit>` would parse as a background color, so a
//! bold cancel-pair is inserted first.
//!
//! Every operation writes into the sink the reply was minted over — a
//! `String`, or the `Formatter` of a [`Display`](fmt::Display)
//! implementation — so composing a message never allocates beyond the
//! sink's own buffer.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

use crate::color::Color;
use crate::style::Style;
use crate::{BOLD, RESET};

/// The reply scaffolding color: mIRC color 10, cyan.
const SCAFFOLD: Color = Color::Cyan;

/// The wire form of the default foreground color, used as the placeholder
/// when expressing a background without a foreground.
///
/// Color 99 is [not universally
/// supported](https://modern.ircdocs.horse/formatting#colors-16-98); it is
/// never tracked as state — the machine only writes it as the foreground
/// slot of a `<fg>,<bg>` pair.
const DEFAULT_FG: &str = "99";

/// Italic text (0x1d), a toggle.
const ITALIC: &str = "\x1d";
/// Underlined text (0x1f), a toggle.
const UNDERLINE: &str = "\x1f";
/// Strikethrough'd text (0x1e), a toggle.
const STRIKE: &str = "\x1e";
/// Monospace'd text (0x11), a toggle.
const MONO: &str = "\x11";

/// The machine's formatting state: colors and toggle attributes.
///
/// The bool count mirrors the wire format's toggle attributes (bold, italic,
/// underline, strikethrough, monospace); packing them would obscure the
/// state-tracking code for no measurable gain.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct State {
    /// The active foreground color, if any.
    pub(crate) fg: Option<Color>,
    /// The active background color, if any.
    pub(crate) bg: Option<Color>,
    /// Bold.
    pub(crate) bold: bool,
    /// Italic.
    pub(crate) italic: bool,
    /// Underline.
    pub(crate) underline: bool,
    /// Strikethrough.
    pub(crate) strike: bool,
    /// Monospace.
    pub(crate) mono: bool,
}

impl State {
    /// Whether every attribute is off: the plain state values render in.
    const fn is_plain(self) -> bool {
        self.fg.is_none()
            && self.bg.is_none()
            && !self.bold
            && !self.italic
            && !self.underline
            && !self.strike
            && !self.mono
    }
}

/// The eating hazard the next text write must guard against, if any.
///
/// Per [modern IRC formatting](https://modern.ircdocs.horse/formatting),
/// two digits are always read for a color when available: after a foreground
/// code, a `,<digit>` fragment parses as a background color. After a bare
/// color code (a reset), the *first* digit would parse as a color. The
/// machine inserts a bold cancel-pair (`\x02\x02`, a visual no-op) between
/// the code and the text when the following text could be eaten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arm {
    /// A foreground code was just emitted; a `,<digit>` text start is a hazard.
    Color,
    /// A bare color code was just emitted; a leading digit is a hazard.
    Bare,
}

/// A `fmt::Write` adapter that strips control characters and applies the
/// mistaken-eating-of-text guard.
///
/// The guard applies to the first chunk written: for string arguments the
/// whole text is one chunk; for composed `Display` values, the first chunk
/// is inspected on a best-effort basis.
struct ContentWriter<'w> {
    /// The sink content is written into.
    sink: &'w mut dyn fmt::Write,
    /// Whether control characters are stripped from written chunks.
    filter: bool,
    /// The eating hazard of the preceding transition, if any.
    arm: Option<Arm>,
    /// Whether no chunk has been written yet (the guard applies once).
    first: bool,
    /// The number of bytes written through this writer.
    written: usize,
}

impl fmt::Write for ContentWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.first {
            self.first = false;

            let hazard = match self.arm {
                Some(Arm::Color) => {
                    text.as_bytes().first() == Some(&b',')
                        && text.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
                }
                Some(Arm::Bare) => text.as_bytes().first().is_some_and(u8::is_ascii_digit),
                None => false,
            };

            if hazard {
                self.sink.write_str("\x02\x02")?;
                self.written += 2;
            }
        }

        if self.filter {
            match crate::strip::strip_control_chars(text) {
                Cow::Borrowed(stripped) => {
                    self.sink.write_str(stripped)?;
                    self.written += stripped.len();
                }
                Cow::Owned(stripped) => {
                    self.sink.write_str(&stripped)?;
                    self.written += stripped.len();
                }
            }
        } else {
            self.sink.write_str(text)?;
            self.written += text.len();
        }

        Ok(())
    }
}

/// A reply under composition.
///
/// Minted from a [`Banner`](crate::Banner): `banner.reply(&mut sink)` writes
/// into any [`fmt::Write`] sink; `banner.render(|reply| ...)` returns a
/// value that renders lazily when a `Display` sink consumes it.
///
/// # Examples
///
/// ```
/// use zeta_fmt::{Banner, Color, Style};
///
/// const BANNER: Banner = Banner::new("Health");
///
/// let mut out = String::new();
/// BANNER.reply(&mut out)
///     .field("Workers:", 3)
///     .sep(" (")
///     .value(2)
///     .label(" scheduled");
///
/// assert_eq!(
///     out,
///     "\x0310>\x0f\x02 Health:\x02\x0310 Workers:\x0f 3\x0310 (\x0f2\x0310 scheduled"
/// );
/// ```
pub struct Reply<'s> {
    /// The sink every byte is written into.
    sink: &'s mut dyn fmt::Write,
    /// The formatting state the sink is currently in.
    state: State,
    /// The number of bytes written so far.
    len: usize,
    /// The number of fields written, for `field`'s inter-field spacing.
    fields: usize,
    /// The eating hazard of the preceding transition, if any.
    arm: Option<Arm>,
    /// The first formatting error encountered while writing, if any.
    error: Option<fmt::Error>,
}

impl fmt::Debug for Reply<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reply")
            .field("state", &self.state)
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl<'s> Reply<'s> {
    /// Wraps `sink` as a reply in the plain state.
    pub(crate) fn new(sink: &'s mut dyn fmt::Write) -> Self {
        Self {
            sink,
            state: State::default(),
            len: 0,
            fields: 0,
            arm: None,
            error: None,
        }
    }

    /// The number of bytes written so far, for budget checks.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has been written yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Writes `text` verbatim, without touching the tracked state.
    ///
    /// For trusted, already-formatted fragments — pre-built banners, or a
    /// fragment whose end state is known. Content written through the
    /// labeled operations keeps the tracked state accurate; `raw` does not.
    pub fn raw(&mut self, text: &str) -> &mut Self {
        self.sink_write(text);
        self
    }

    /// Writes cyan scaffolding: labels, separators, parentheses.
    ///
    /// Scaffolding is author-controlled and written verbatim.
    pub fn label(&mut self, text: impl fmt::Display) -> &mut Self {
        self.enter(State::from_style(Style::new().fg(SCAFFOLD)));
        self.write_content(text, false);
        self
    }

    /// Writes cyan scaffolding at a listing boundary: separators.
    ///
    /// An alias of [`Reply::label`] for separator call sites.
    pub fn sep(&mut self, text: impl fmt::Display) -> &mut Self {
        self.label(text)
    }

    /// Writes a plain value: a datum, reset-wrapped and stripped of control
    /// characters.
    ///
    /// The space between a label's colon and the value is written inside
    /// this slot, so leading spacing travels with the value.
    pub fn value(&mut self, text: impl fmt::Display) -> &mut Self {
        self.enter(State::default());
        self.write_content(text, true);
        self
    }

    /// Writes a plain value, filling nothing when it is absent.
    ///
    /// A `None` writes nothing at all: no reset, no text — the skipped slot
    /// emits zero bytes. Errors are not slots: a `Result` is converted by
    /// the caller — `.ok()` to accept absence, `unwrap_or` to substitute —
    /// or handled by the handler's own branch, which replies with a
    /// different message entirely.
    pub fn value_opt(&mut self, text: Option<impl fmt::Display>) -> &mut Self {
        match text {
            Some(text) => self.value(text),
            None => self,
        }
    }

    /// Writes `Label: value` — the house shape.
    ///
    /// The space between the label's colon and the value is written inside
    /// the plain value slot, and a leading scaffold space is written before
    /// every field but the first — the hand-written convention, encoded
    /// once.
    pub fn field(&mut self, label: impl fmt::Display, value: impl fmt::Display) -> &mut Self {
        if self.fields > 0 {
            self.enter(State::from_style(Style::new().fg(SCAFFOLD)));
            self.write_content(" ", false);
        }

        self.enter(State::from_style(Style::new().fg(SCAFFOLD)));
        self.write_content(label, false);
        self.enter(State::default());
        self.sink_write(" ");
        self.write_content(value, true);
        self.fields += 1;

        self
    }

    /// Writes `Label: value`, filling nothing when the value is absent.
    ///
    /// A `None` skips the whole field: no label, no leading space, no
    /// separator — the field vanishes instead of leaving a dangling header.
    pub fn field_opt(
        &mut self,
        label: impl fmt::Display,
        value: Option<impl fmt::Display>,
    ) -> &mut Self {
        match value {
            Some(value) => self.field(label, value),
            None => self,
        }
    }

    /// Writes text in an arbitrary style, verbatim.
    ///
    /// For author-controlled fragments — colored status text, bold labels.
    /// The machine diffs the tracked state against `style` and emits only
    /// the codes the transition requires.
    pub fn text(&mut self, style: Style, text: impl fmt::Display) -> &mut Self {
        self.enter(State::from_style(style));
        self.write_content(text, false);
        self
    }

    /// Writes user data in an arbitrary style, stripped of control
    /// characters.
    ///
    /// For styled *user data* — a colored nickname, a styled message. Unlike
    /// [`Reply::text`], the content is filtered, so user data cannot smuggle
    /// formatting into the reply.
    pub fn data(&mut self, style: Style, text: impl fmt::Display) -> &mut Self {
        self.enter(State::from_style(style));
        self.write_content(text, true);
        self
    }

    /// Writes text in a single foreground color.
    ///
    /// A lingering background color is cleared first: the wire format can
    /// clear a background only by resetting all colors (a foreground code
    /// leaves the background untouched), so the machine emits `\x0f` and
    /// re-applies the foreground.
    pub fn fg(&mut self, color: Color, text: impl fmt::Display) -> &mut Self {
        self.text(Style::new().fg(color), text)
    }

    /// Writes text with both a foreground and a background color.
    pub fn highlight(&mut self, fg: Color, bg: Color, text: impl fmt::Display) -> &mut Self {
        self.text(Style::new().fg(fg).bg(bg), text)
    }

    /// Writes spoilered text: the same color in foreground and background,
    /// readable on hover or selection.
    pub fn spoiler(&mut self, color: Color, text: impl fmt::Display) -> &mut Self {
        self.highlight(color, color, text)
    }

    /// Writes bold text — a state-tracked toggle that preserves the
    /// current colors, closed by the next operation's transition or by
    /// [`Reply::finish`].
    pub fn bold(&mut self, text: impl fmt::Display) -> &mut Self {
        self.emit_toggle("\x02", self.state.bold);
        self.state.bold = true;
        self.write_content(text, true);
        self
    }

    /// Writes italicized text — a state-tracked toggle.
    pub fn italic(&mut self, text: impl fmt::Display) -> &mut Self {
        self.emit_toggle(ITALIC, self.state.italic);
        self.state.italic = true;
        self.write_content(text, true);
        self
    }

    /// Writes underlined text — a state-tracked toggle.
    pub fn underline(&mut self, text: impl fmt::Display) -> &mut Self {
        self.emit_toggle(UNDERLINE, self.state.underline);
        self.state.underline = true;
        self.write_content(text, true);
        self
    }

    /// Writes strikethrough'd text — a state-tracked toggle.
    pub fn strike(&mut self, text: impl fmt::Display) -> &mut Self {
        self.emit_toggle(STRIKE, self.state.strike);
        self.state.strike = true;
        self.write_content(text, true);
        self
    }

    /// Writes monospace'd text — a state-tracked toggle.
    pub fn mono(&mut self, text: impl fmt::Display) -> &mut Self {
        self.emit_toggle(MONO, self.state.mono);
        self.state.mono = true;
        self.write_content(text, true);
        self
    }

    /// Emits `code` when the toggle it represents is currently off.
    fn emit_toggle(&mut self, code: &'static str, on: bool) {
        if !on {
            self.sink_write(code);
        }
    }

    /// Writes a quoted value: cyan `“` and `”` around a plain value.
    pub fn quoted(&mut self, text: impl fmt::Display) -> &mut Self {
        self.label("\u{201c}");
        self.value(text);
        self.label("\u{201d}");
        self
    }

    /// Writes `segment` only when `condition` holds.
    ///
    /// A skipped segment writes nothing at all — including any separator or
    /// spacing that only existed to precede it.
    pub fn when(&mut self, condition: bool, segment: impl FnOnce(&mut Reply<'_>)) -> &mut Self {
        if condition {
            segment(self);
        }

        self
    }

    /// Writes `segment` with the inner value when `option` is `Some`.
    pub fn when_some<T>(
        &mut self,
        option: Option<T>,
        segment: impl FnOnce(&mut Reply<'_>, T),
    ) -> &mut Self {
        if let Some(value) = option {
            segment(self, value);
        }

        self
    }

    /// Ends the reply: closes any dangling toggle attributes, leaving
    /// trailing colors elided.
    ///
    /// Called automatically by the one-shot formatters and the lazy
    /// renderer; a manually managed sink need not call it.
    pub fn finish(&mut self) -> &mut Self {
        for code in [
            (self.state.bold, "\x02"),
            (self.state.italic, ITALIC),
            (self.state.underline, UNDERLINE),
            (self.state.strike, STRIKE),
            (self.state.mono, MONO),
        ] {
            if code.0 {
                self.sink_write(code.1);
            }
        }

        self.state.bold = false;
        self.state.italic = false;
        self.state.underline = false;
        self.state.strike = false;
        self.state.mono = false;
        self.arm = None;

        self
    }

    /// Returns the first formatting error encountered while writing, if any.
    ///
    /// # Errors
    ///
    /// Returns the error, if one occurred while writing to the sink.
    #[must_use]
    pub const fn error(&self) -> Option<fmt::Error> {
        self.error
    }

    /// Writes `text` through the sink, counting its bytes.
    fn sink_write(&mut self, text: &str) {
        if self.error.is_none() {
            match self.sink.write_str(text) {
                Ok(()) => self.len += text.len(),
                Err(error) => self.error = Some(error),
            }
        }
    }

    /// Writes content through the eating-guard and, when `filter` is set,
    /// the control-character filter.
    fn write_content(&mut self, text: impl fmt::Display, filter: bool) {
        if self.error.is_some() {
            return;
        }

        let mut writer = ContentWriter {
            sink: &mut *self.sink,
            filter,
            arm: self.arm,
            first: true,
            written: 0,
        };

        let result = write!(writer, "{text}");

        self.len += writer.written;
        self.arm = None;

        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    /// Writes a bare color code followed by the two-digit forms of the given
    /// colors, arming the eating guard.
    ///
    /// The guard depends on the form: a bare code eats a leading digit of
    /// the following text, a foreground code eats a `,<digit>` text start,
    /// and a foreground+background pair leaves no slot for further digits —
    /// no hazard.
    fn push_color(&mut self, fg: Option<&str>, bg: Option<&str>) {
        self.sink_write("\x03");

        if let Some(fg) = fg {
            self.sink_write(fg);

            if let Some(bg) = bg {
                self.sink_write(",");
                self.sink_write(bg);
            }
        }

        self.arm = match (fg, bg) {
            (None, _) => Some(Arm::Bare),
            (Some(_), Some(_)) => None,
            (Some(_), None) => Some(Arm::Color),
        };
    }

    /// Diffs the tracked state against `target`, emitting only the control
    /// codes the transition requires.
    fn enter(&mut self, target: State) {
        let current = self.state;

        // The one-shot reset: when every attribute is going off, a single
        // `\x0f` is cheaper than closing toggles and colors individually.
        if target.is_plain() {
            if !current.is_plain() {
                self.sink_write(RESET);
                self.state = State::default();
                self.arm = None;
            }

            return;
        }

        if current.fg != target.fg || current.bg != target.bg {
            match (target.fg, target.bg) {
                (Some(fg), Some(bg)) => {
                    if current.bg == Some(bg) {
                        // A foreground code leaves the background untouched.
                        if current.fg != Some(fg) {
                            self.push_color(Some(fg.wire()), None);
                        }
                    } else {
                        self.push_color(Some(fg.wire()), Some(bg.wire()));
                    }

                    self.state.fg = Some(fg);
                    self.state.bg = Some(bg);
                }
                (Some(fg), None) => {
                    if current.bg.is_some() {
                        // Clearing a background has no direct wire form;
                        // reset all colors (and toggles) and re-apply the
                        // foreground.
                        self.sink_write(RESET);
                        self.state = State::default();
                        self.arm = None;
                    }

                    if self.state.fg != Some(fg) {
                        self.push_color(Some(fg.wire()), None);
                    }

                    self.state.fg = Some(fg);
                }
                (None, Some(bg)) => {
                    if current.fg.is_some() || current.bg != Some(bg) {
                        self.push_color(Some(DEFAULT_FG), Some(bg.wire()));
                    }

                    self.state.fg = None;
                    self.state.bg = Some(bg);
                }
                (None, None) => {
                    // Colors off while toggles survive: a bare color code
                    // resets colors only, and the following text must be
                    // guarded against a leading digit.
                    self.sink_write("\x03");
                    self.state.fg = None;
                    self.state.bg = None;
                }
            }
        }

        // Toggle diffs.
        for (code, (was, now)) in [
            (BOLD, (current.bold, target.bold)),
            (ITALIC, (current.italic, target.italic)),
            (UNDERLINE, (current.underline, target.underline)),
            (STRIKE, (current.strike, target.strike)),
            (MONO, (current.mono, target.mono)),
        ] {
            if was != now {
                // A toggle character terminates the color payload of a
                // preceding code, so the following text can no longer be
                // eaten — the guard is no longer needed.
                self.arm = None;
                self.sink_write(code);
            }
        }

        self.state = target;
    }
}


impl Reply<'_> {
    /// Sets the tracked state directly, after writing verbatim bytes.
    pub(crate) const fn set_state(&mut self, state: State) {
        self.state = state;
        self.arm = None;
    }
}

/// The formatting state a banner leaves the reply in: the scaffolding color
/// is on, no toggles are active.
pub(crate) const BANNER_STATE: State = State {
    fg: Some(Color::Cyan),
    bg: None,
    bold: false,
    italic: false,
    underline: false,
    strike: false,
    mono: false,
};

impl fmt::Write for Reply<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.raw(text);

        Ok(())
    }
}

impl State {
    /// Builds a state from a target [`Style`].
    const fn from_style(style: Style) -> Self {
        Self {
            fg: style.fg,
            bg: style.bg,
            bold: style.bold,
            italic: style.italic,
            underline: style.underline,
            strike: style.strike,
            mono: style.mono,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{grammar, into_formatter, lazy, owned};
    use crate::banner::Banner;

    const BANNER: Banner = Banner::new("Dig");

    /// Builds an operation sequence as a closure over a fresh reply.
    macro_rules! seq {
        ($($ops:tt)*) => {
            |reply: &mut Reply<'_>| {
                reply $($ops)*;
            }
        };
    }

    #[test]
    fn label_and_value_after_the_banner() {
        let expected = "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3";

        assert_eq!(owned(&BANNER, seq!(.label("Workers:").value(" 3"))), expected);
    }

    #[test]
    fn label_after_a_value_reopens_the_scaffold_once() {
        let expected = "\x0310>\x0f\x02 Dig:\x02\x0310 label:\x0f 3\x0310next: ";

        assert_eq!(
            owned(&BANNER, seq!(.label("label:").value(" 3").label("next: "))),
            expected
        );
    }

    #[test]
    fn consecutive_values_emit_a_single_reset() {
        let expected = "\x0310>\x0f\x02 Dig:\x02\x0310 \x0ffirstsecond";

        assert_eq!(owned(&BANNER, seq!(.value("first").value("second"))), expected);
    }

    #[test]
    fn fields_reproduce_the_hand_written_convention() {
        let expected = "\x0310>\x0f\x02 Health:\x02\x0310 Workers:\x0f 3\x0310 Tasks:\x0f 5";
        let ops = seq!(.field("Workers:", 3).field("Tasks:", 5));

        assert_eq!(owned(&Banner::new("Health"), ops), expected);
        assert_eq!(lazy(&Banner::new("Health"), ops), expected);
        assert_eq!(into_formatter(&Banner::new("Health"), ops), expected);
    }

    #[test]
    fn the_first_field_leaves_the_banners_trailing_space() {
        let expected = "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3";

        assert_eq!(owned(&BANNER, seq!(.field("Workers:", 3))), expected);
    }

    #[test]
    fn a_message_ending_on_a_value_carries_no_trailing_color() {
        let text = owned(&BANNER, seq!(.field("Workers:", 3)));

        assert!(!text.ends_with("\x0310"), "{text:?}");
        grammar(&text).expect("grammar");
    }

    #[test]
    fn value_opt_none_fills_nothing() {
        let none: Option<u32> = None;

        assert_eq!(
            owned(&BANNER, seq!(.label("label:").value_opt(none))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 label:"
        );
    }

    #[test]
    fn value_opt_some_writes_the_value() {
        assert_eq!(
            owned(&BANNER, seq!(.label("label:").value_opt(Some(" 3")))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 label:\x0f 3"
        );
    }

    #[test]
    fn field_opt_none_skips_the_whole_field() {
        let none: Option<u32> = None;

        assert_eq!(
            owned(&BANNER, seq!(.field("Workers:", 3).field_opt("DB:", none))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3"
        );
    }

    #[test]
    fn field_opt_some_writes_the_field() {
        assert_eq!(
            owned(&BANNER, seq!(.field_opt("DB:", Some(3)))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 DB:\x0f 3"
        );
    }

    #[test]
    fn when_false_skips_the_segment_entirely() {
        assert_eq!(
            owned(&BANNER, seq!(.when(false, |out| { out.label("skipped:"); }))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 "
        );
    }

    #[test]
    fn when_some_passes_the_inner_value() {
        assert_eq!(
            owned(&BANNER, seq!(.when_some(Some(3), |out, db| { out.field("DB:", db); }))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 DB:\x0f 3"
        );
    }

    #[test]
    fn fg_switches_colors() {
        let ops = seq!(.fg(Color::Green, "up").sep(" — ").fg(Color::Red, "down"));

        assert_eq!(
            owned(&Banner::BARE, ops),
            "\x0310> \x0303up\x0310 — \x0304down"
        );
    }

    #[test]
    fn fg_clears_a_lingering_background() {
        let ops = seq!(.highlight(Color::Red, Color::Brown, "tagged").label(" label:"));

        assert_eq!(
            owned(&Banner::BARE, ops),
            "\x0310> \x0304,05tagged\x0f\x0310 label:"
        );
    }

    #[test]
    fn highlight_writes_the_pair_form() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.highlight(Color::Red, Color::Brown, "tagged"))),
            "\x0310> \x0304,05tagged"
        );
    }

    #[test]
    fn back_to_back_identical_highlights_emit_one_code() {
        let ops = seq!(.highlight(Color::Red, Color::Brown, "first").highlight(Color::Red, Color::Brown, "second"));

        assert_eq!(
            owned(&Banner::BARE, ops),
            "\x0310> \x0304,05firstsecond"
        );
    }

    #[test]
    fn background_alone_uses_the_default_foreground_placeholder() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.text(Style::new().bg(Color::Red), "alerted"))),
            "\x0310> \x0399,04alerted"
        );
    }

    #[test]
    fn spoiler_uses_equal_foreground_and_background() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.spoiler(Color::Green, "hidden"))),
            "\x0310> \x0303,03hidden"
        );
    }

    #[test]
    fn back_to_back_bolds_toggle_once() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.bold("bold").bold(" and bolder"))),
            "\x0310> \x02bold and bolder\x02",
        );
    }

    #[test]
    fn toggles_combine_with_colors() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.text(Style::new().fg(Color::Red).bold(), "critical"))),
            "\x0310> \x0304\x02critical\x02"
        );
    }

    #[test]
    fn entering_a_value_clears_toggles_with_the_reset() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.bold("bold").value(42))),
            "\x0310> \x02bold\x0f42"
        );
    }

    #[test]
    fn entering_scaffolding_closes_an_open_toggle() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.bold("bold").sep(" plain"))),
            "\x0310> \x02bold\x02 plain"
        );
    }

    #[test]
    fn values_are_stripped_of_control_characters() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.value("hi \x02there\r\nfriend"))),
            "\x0310> \x0fhi therefriend"
        );
    }

    #[test]
    fn values_cannot_inject_colors() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.value("\x0310evil"))),
            "\x0310> \x0f10evil"
        );
    }

    #[test]
    fn styled_data_is_stripped_too() {
        assert_eq!(
            owned(&Banner::BARE, seq!(.data(Style::new().fg(Color::Cyan).bold(), "\x0f clean"))),
            "\x0310> \x02 clean\x02"
        );
    }

    #[test]
    fn a_comma_digit_text_start_after_a_color_code_is_guarded() {
        assert_eq!(
            owned(&BANNER, seq!(.label("label:").value(" 5").label(",13 votes"))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 label:\x0f 5\x0310\x02\x02,13 votes"
        );
    }

    #[test]
    fn a_digit_text_start_after_a_color_code_is_safe() {
        assert_eq!(
            owned(&BANNER, seq!(.label("label:").value(" 5").label("13 votes"))),
            "\x0310>\x0f\x02 Dig:\x02\x0310 label:\x0f 5\x031013 votes"
        );
    }

    #[test]
    fn a_toggle_between_the_code_and_the_text_removes_the_hazard() {
        // The toggle character terminates the color spec, so the text is no
        // longer directly after the code and needs no cancel-pair.
        assert_eq!(
            owned(&Banner::BARE, seq!(.text(Style::new().bold(), "13 things"))),
            "\x0310> \x03\x0213 things\x02"
        );
    }

    #[test]
    fn quoted_wraps_a_value_in_cyan_quotes() {
        assert_eq!(
            owned(&Banner::new("Alert"), seq!(.quoted("hello world"))),
            "\x0310>\x0f\x02 Alert:\x02\x0310 \u{201c}\x0fhello world\x0310\u{201d}"
        );
    }

    #[test]
    fn lines_prefix_every_line_with_the_banner() {
        let text = "first\nsecond";
        let lines: Vec<String> = BANNER.lines(text).map(|line| line.to_string()).collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "\x0310>\x0f\x02 Dig:\x02\x0310 \x0ffirst");
        assert_eq!(lines[1], "\x0310>\x0f\x02 Dig:\x02\x0310 \x0fsecond");
    }

    #[test]
    fn len_tracks_the_bytes_written() {
        let mut buf = String::new();
        {
            let mut reply = BANNER.reply(&mut buf);
            reply.field("Workers:", 3);
        }

        assert_eq!(buf.len(), "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3".len());
    }

    #[test]
    fn a_custom_banner_writes_its_own_prefix() {
        fn imdb_prefix(reply: &mut Reply<'_>) {
            reply.raw("\x0310>\x0f\x02 IMDb\x02\x0310:\x0f");
        }

        const IMDB: Banner = Banner::custom(imdb_prefix);

        assert_eq!(
            owned(&IMDB, seq!(.field(" Plot:", "things"))),
            "\x0310>\x0f\x02 IMDb\x02\x0310:\x0f\x0310 Plot:\x0f things"
        );
    }

    #[test]
    fn owned_lazy_and_formatter_flavors_are_byte_identical() {
        let ops = seq!(
            .field("Workers:", 3)
            .sep(" — ")
            .bold("bold")
            .value(42)
            .label(" tail:")
        );

        let owned = owned(&BANNER, ops);
        let lazy = lazy(&BANNER, ops);
        let formatter = into_formatter(&BANNER, ops);

        assert_eq!(owned, lazy);
        assert_eq!(owned, formatter);
        grammar(&owned).expect("grammar");
    }

    #[test]
    fn representative_sequences_satisfy_the_grammar() {
        for text in [
            owned(&BANNER, seq!(.field("Workers:", 3))),
            owned(&Banner::BARE, seq!(.fg(Color::Green, "up").sep(" — ").fg(Color::Red, "down"))),
            owned(&Banner::BARE, seq!(.highlight(Color::Red, Color::Brown, "tagged"))),
            owned(&Banner::BARE, seq!(.text(Style::new().bg(Color::Red), "alerted"))),
            owned(&Banner::BARE, seq!(.bold("bold").value(42))),
            owned(&Banner::BARE, seq!(.spoiler(Color::Green, "hidden"))),
            owned(&BANNER, seq!(.label(",13 votes"))),
            owned(&Banner::BARE, seq!(.data(Style::new().fg(Color::Cyan).bold(), "\x0f clean"))),
        ] {
            grammar(&text).unwrap_or_else(|error| panic!("{error}"));
        }
    }

    /// A tiny xorshift generator keeps the fuzzer dependency-free.
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[allow(clippy::cast_possible_truncation)] // the index is modulo the slice length
    fn pick<'a>(state: &mut u64, texts: &[&'a str]) -> &'a str {
        texts[(xorshift(state) % texts.len() as u64) as usize]
    }

    #[test]
    fn randomized_op_sequences_satisfy_the_grammar() {
        // Scaffold operations are trusted (written verbatim), so they only
        // receive clean fragments; data operations receive user data,
        // including control characters that must be stripped.
        let clean = ["a", "13 votes", ",13 votes", " ", ",,,"];
        let dirty = ["a", "13 votes", ",13 votes", " ", ",,,", "\x02evil\r\nline"];

        let mut state = 0x2545_F49F_4F6C_DD1Du64;

        for _ in 0..300 {
            let mut buf = String::new();
            {
                let mut reply = Banner::BARE.reply(&mut buf);

                for _ in 0..24 {
                    match xorshift(&mut state) % 7 {
                        0 => {
                            reply.label(pick(&mut state, &clean));
                        }
                        1 => {
                            reply.sep(pick(&mut state, &clean));
                        }
                        2 => {
                            reply.value(pick(&mut state, &dirty));
                        }
                        3 => {
                            let label = pick(&mut state, &clean);
                            let text = pick(&mut state, &dirty);
                            reply.field(label, text);
                        }
                        4 => {
                            let color = Color::from_code((xorshift(&mut state) % 16) as u8).expect("color");
                            let text = pick(&mut state, &clean);
                            reply.fg(color, text);
                        }
                        5 => {
                            reply.bold("bold");
                        }
                        _ => {
                            reply.quoted("hello");
                        }
                    }
                }
                reply.finish();
            }

            grammar(&buf).unwrap_or_else(|error| panic!("{error}: {buf:?}"));
        }
    }
}
