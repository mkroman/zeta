//! A plugin's reply identity: the banner every reply starts with.
//!
//! A [`Banner`] is defined once per plugin — `Banner::new("Dig")` — and
//! produces the shared visual convention: a teal `>` marker, the plugin's
//! bold name, and the scaffolding color that the reply continues in
//! ([`Banner::BARE`] omits the name). The start bytes always come from the
//! shared constants, so detection logic like "does this message look like a
//! bot reply" (`text.starts_with(REPLY_PREFIX)`) keeps working on anything
//! the crate produces.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

use crate::reply::{BANNER_STATE, Reply};
use crate::{BOLD, RESET, REPLY_PREFIX};

/// A plugin's reply identity.
///
/// # Examples
///
/// ```
/// use zeta_fmt::Banner;
///
/// const DIG: Banner = Banner::new("Dig");
///
/// assert_eq!(DIG.message("3 records"), "\x0310>\x0f\x02 Dig:\x02\x0310 3 records");
/// assert_eq!(Banner::BARE.message("no results"), "\x0310> no results");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    /// The kind of banner: a bold plugin name, a bespoke prefix, or bare.
    kind: BannerKind,
}

/// The kind of a [`Banner`].
///
/// The derived equality compares a function pointer for bespoke banners; like
/// `CommandSpec` in the plugin API, only compile-time (structural) equality
/// matters — never a meaningful runtime comparison.
#[allow(unpredictable_function_pointer_comparisons)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum BannerKind {
    /// A bold plugin name: `> Dig: …`.
    Named(Cow<'static, str>),
    /// A bespoke prefix, written by a plugin-supplied function: the machine
    /// cannot know its end state, so it should end with a reset — the plain
    /// state the builder continues from.
    Custom(WritePrefix),
    /// No name at all: `> …`.
    Bare,
}

/// The type of a bespoke banner's prefix writer.
type WritePrefix = fn(&mut Reply<'_>);

impl Banner {
    /// A banner with the plugin's display name, e.g. `Banner::new("Dig")`.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self {
            kind: BannerKind::Named(Cow::Borrowed(name)),
        }
    }

    /// A banner with a dynamically computed display name.
    ///
    /// The `const` [`Banner::new`] covers static names (the common case);
    /// this one accepts a computed name at the cost of an owned string.
    #[must_use]
    pub fn named(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind: BannerKind::Named(name.into()),
        }
    }

    /// A banner with a bespoke prefix, for replies whose scaffolding
    /// deviates from the convention (e.g. a differently-colored colon).
    ///
    /// The writer should end its prefix with a reset, so the builder
    /// continues from the plain state the machine expects.
    #[must_use]
    pub const fn custom(write: WritePrefix) -> Self {
        Self {
            kind: BannerKind::Custom(write),
        }
    }

    /// The bare banner: just the reply marker, no plugin name.
    ///
    /// For replies that are not attributed to a plugin.
    pub const BARE: Self = Self {
        kind: BannerKind::Bare,
    };

    /// Returns the banner's bytes: the marker and, for a named banner, the
    /// bold name and the scaffolding color.
    #[must_use]
    pub fn prefix(&self) -> String {
        let mut out = String::new();

        {
            let mut reply = self.reply(&mut out);
            reply.set_state(BANNER_STATE);
        }

        out
    }

    /// Formats `text` as a complete reply: the banner, then the text
    /// verbatim.
    #[must_use]
    pub fn message(&self, text: impl fmt::Display) -> String {
        let mut out = String::new();

        {
            let mut reply = self.reply(&mut out);
            let _ = write!(reply, "{text}");
        }

        out
    }

    /// Yields each line of `text` as a lazily-rendered reply, with the
    /// banner prefixed per line.
    ///
    /// IRC messages cannot contain line breaks, so multi-line output is
    /// sent one line at a time — each yielded item formats into the sink
    /// that sends it, without per-line allocations beyond the caller's own
    /// string.
    ///
    /// # Examples
    ///
    /// ```
    /// use zeta_fmt::Banner;
    ///
    /// const DIG: Banner = Banner::new("Dig");
    ///
    /// let text = "first\nsecond";
    /// let lines: Vec<String> = DIG.lines(text).map(|line| line.to_string()).collect();
    ///
    /// assert_eq!(lines.len(), 2);
    /// assert!(lines[0].ends_with("first"));
    /// assert!(lines[1].ends_with("second"));
    /// ```
    pub fn lines<'a>(&self, text: &'a str) -> impl Iterator<Item = LineDisplay<'a>> + 'a {
        text.lines().map({
            let banner = self.clone();

            move |line| LineDisplay { banner: banner.clone(), line }
        })
    }

    /// Mints a [`Reply`] builder writing into `sink`, starting with the
    /// banner's bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use zeta_fmt::Banner;
    ///
    /// let mut out = String::new();
    /// Banner::new("Dig").reply(&mut out).field("TTL:", 300);
    ///
    /// assert!(out.ends_with("TTL:\x0f 300"));
    /// ```
    pub fn reply<'s, W: fmt::Write>(&self, sink: &'s mut W) -> Reply<'s> {
        let mut reply = Reply::new(sink);
        self.write_prefix(&mut reply);

        reply
    }

    /// Renders a reply lazily: the banner and the segment produced by
    /// `write` are rendered when the returned value is consumed by a
    /// `Display` sink (e.g. `client.send_privmsg`), so composing the message
    /// allocates nothing.
    ///
    /// The closure must be callable repeatedly (a `Fn`), as rendering happens
    /// when the sink consumes the value; in practice that is exactly once.
    ///
    /// # Examples
    ///
    /// ```
    /// use zeta_fmt::Banner;
    ///
    /// const DIG: Banner = Banner::new("Dig");
    ///
    /// let workers = 3;
    /// let rendered = DIG.render(|reply| {
    ///     reply.field("Workers:", workers);
    /// }).to_string();
    ///
    /// assert_eq!(rendered, "\x0310>\x0f\x02 Dig:\x02\x0310 Workers:\x0f 3");
    /// ```
    #[must_use]
    pub fn render<F>(&self, write: F) -> Rendered<F>
    where
        F: Fn(&mut Reply<'_>),
    {
        Rendered {
            banner: self.clone(),
            write,
        }
    }

    /// Builds a reply string from a segment produced by `write`.
    ///
    /// The owned flavor of [`Banner::render`], for messages that must
    /// outlive the expression that composed them.
    #[must_use]
    pub fn string(&self, write: impl FnOnce(&mut Reply<'_>)) -> String {
        let mut out = String::new();

        {
            let mut reply = self.reply(&mut out);
            write(&mut reply);
            reply.finish();
        }

        out
    }

    /// Writes the banner's bytes into `sink`.
    pub(crate) fn write_prefix(&self, reply: &mut Reply<'_>) {
        match self.kind {
            BannerKind::Named(ref name) => {
                reply.raw(REPLY_PREFIX);
                reply.raw(RESET);
                reply.raw(BOLD);
                reply.raw(" ");
                reply.raw(name.as_ref());
                reply.raw(":");
                reply.raw(BOLD);
                reply.raw(crate::COLOR);
                reply.raw(" ");
                reply.set_state(BANNER_STATE);
            }
            BannerKind::Bare => {
                reply.raw(REPLY_PREFIX);
                reply.raw(" ");
                reply.set_state(BANNER_STATE);
            }
            BannerKind::Custom(write) => {
                // The machine starts in the plain state, which is exactly the
                // state a bespoke prefix ends in when it ends with a reset.
                write(reply);
            }
        }
    }
}

/// A lazily-rendered reply: the banner and the closure producing its body.
///
/// Returned by [`Banner::render`]; rendered when a `Display` sink consumes
/// it, with no intermediate allocation.
pub struct Rendered<F> {
    banner: Banner,
    write: F,
}

impl<F> fmt::Display for Rendered<F>
where
    F: Fn(&mut Reply<'_>),
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut reply = Reply::new(f);
        self.banner.write_prefix(&mut reply);
        (self.write)(&mut reply);
        reply.finish();

        reply.error().map_or(Ok(()), Err)
    }
}

/// One line of a multi-line reply, rendering lazily with the banner prefixed.
///
/// Yielded by [`Banner::lines`].
#[derive(Clone, Debug)]
pub struct LineDisplay<'a> {
    banner: Banner,
    line: &'a str,
}

impl fmt::Display for LineDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut reply = Reply::new(f);
        self.banner.write_prefix(&mut reply);
        reply.value(self.line);
        reply.finish();

        reply.error().map_or(Ok(()), Err)
    }
}
