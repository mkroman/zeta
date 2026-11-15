//! Joining lists and iterators into English sentences.

use std::fmt::{self, Write as _};

/// Joining borrowed string lists into sentences.
///
/// Implemented for slices of string-like items — `&[&str]`, `Vec<String>`, and arrays (which
/// dereference to slices). The items are borrowed and the output is allocated once with its
/// exact capacity; for lazily produced values, use [`IteratorJoinExt`] instead.
pub trait ListJoinExt {
    /// Joins the items into a sentence with the Oxford comma: two items are joined by `and`,
    /// while the last of three or more follows a comma — `"rock and pop"` or
    /// `"rock, pop, and jazz"`.
    ///
    /// An empty list yields an empty string; a single item is returned unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::ListJoinExt;
    /// assert_eq!(["rock", "pop", "jazz"].oxford_join(), "rock, pop, and jazz");
    /// assert_eq!(["rock", "pop"].oxford_join(), "rock and pop");
    /// ```
    #[must_use]
    fn oxford_join(self) -> String;

    /// Joins the items with a custom separator and final separator — e.g.
    /// `(", ", " and ")` renders the non-Oxford `"a, b and c"` style.
    ///
    /// `separator` goes between consecutive items, and `final_separator` before the last item
    /// of two or more. An empty list yields an empty string.
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::ListJoinExt;
    /// assert_eq!(
    ///     ["a", "b", "c"].join_with_separators(", ", " and "),
    ///     "a, b and c"
    /// );
    /// assert_eq!(["a", "b", "c"].join_with_separators(", ", ", "), "a, b, c");
    /// ```
    #[must_use]
    fn join_with_separators(self, separator: &str, final_separator: &str) -> String;
}

impl<S: AsRef<str>> ListJoinExt for &'_ [S] {
    fn oxford_join(self) -> String {
        join_strs(self, ", ", " and ", ", and ")
    }

    fn join_with_separators(self, separator: &str, final_separator: &str) -> String {
        join_strs(self, separator, final_separator, final_separator)
    }
}

/// Joining iterators of displayable items into sentences.
///
/// This is the [`ListJoinExt`] counterpart for values that do not exist as a slice, such as
/// `map`, `filter`, and `split` chains. The items are streamed into the output — at most one
/// item is held back to decide the final separator — so the iterator is consumed only once and
/// needs no intermediate [`Vec`].
pub trait IteratorJoinExt {
    /// Joins the items into a sentence with the Oxford comma; see
    /// [`ListJoinExt::oxford_join`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::IteratorJoinExt;
    /// assert_eq!((1..=3).oxford_join(), "1, 2, and 3");
    /// assert_eq!("rock,pop,jazz".split(',').oxford_join(), "rock, pop, and jazz");
    /// ```
    #[must_use]
    fn oxford_join(self) -> String;

    /// Joins the items with a custom separator and final separator; see
    /// [`ListJoinExt::join_with_separators`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::IteratorJoinExt;
    /// assert_eq!(
    ///     (1..=3).join_with_separators(", ", ", and "),
    ///     "1, 2, and 3"
    /// );
    /// ```
    #[must_use]
    fn join_with_separators(self, separator: &str, final_separator: &str) -> String;
}

impl<I> IteratorJoinExt for I
where
    I: Iterator,
    I::Item: fmt::Display,
{
    fn oxford_join(self) -> String {
        join_display(self, ", ", " and ", ", and ")
    }

    fn join_with_separators(self, separator: &str, final_separator: &str) -> String {
        join_display(self, separator, final_separator, final_separator)
    }
}

/// Joins a slice of string-like items, borrowing each item and reserving the exact capacity.
fn join_strs<S: AsRef<str>>(
    items: &[S],
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) -> String {
    let content_len: usize = items.iter().map(|item| item.as_ref().len()).sum();
    let separator_len = match items.len() {
        0 | 1 => 0,
        2 => conjunction.len(),
        count => (count - 2) * separator.len() + long_conjunction.len(),
    };

    let mut joined = String::with_capacity(content_len + separator_len);
    join_list_into(
        &mut joined,
        items.iter().map(AsRef::as_ref),
        separator,
        conjunction,
        long_conjunction,
    );
    joined
}

/// Joins arbitrary displayable items, reserving capacity from the iterator's lower size hint.
fn join_display<I, T>(
    items: I,
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) -> String
where
    I: IntoIterator<Item = T>,
    T: fmt::Display,
{
    let items = items.into_iter();
    let mut joined = String::with_capacity(items.size_hint().0);
    join_list_into(&mut joined, items, separator, conjunction, long_conjunction);
    joined
}

/// Writes `items` into `out`, separating the parts as they are produced.
///
/// `conjunction` separates two items and `long_conjunction` the last of three or more — the
/// distinction is what makes the Oxford comma appear only where it belongs. One item is held
/// back at a time: an item is written only once the following one is known, which identifies
/// the final part without collecting the items first.
fn join_list_into<I, T>(
    out: &mut String,
    items: I,
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) where
    I: IntoIterator<Item = T>,
    T: fmt::Display,
{
    let mut items = items.into_iter();
    let Some(mut pending) = items.next() else {
        return;
    };
    let mut written = 0;

    for item in items {
        if written > 0 {
            out.push_str(separator);
        }

        let _ = write!(out, "{pending}");
        written += 1;
        pending = item;
    }

    if written == 1 {
        out.push_str(conjunction);
    } else if written > 1 {
        out.push_str(long_conjunction);
    }

    let _ = write!(out, "{pending}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_string_lists_with_an_oxford_comma() {
        let empty: &[&str] = &[];

        assert_eq!(empty.oxford_join(), "");
        assert_eq!(["a"].oxford_join(), "a");
        assert_eq!(["a", "b"].oxford_join(), "a and b");
        assert_eq!(["a", "b", "c"].oxford_join(), "a, b, and c");
        assert_eq!(["a", "b", "c", "d"].oxford_join(), "a, b, c, and d");
        assert_eq!(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()].oxford_join(),
            "a, b, and c"
        );
        let unicode = ["æ".to_owned(), "ø".to_owned(), "å".to_owned()];
        assert_eq!(unicode.oxford_join(), "æ, ø, and å");
    }

    #[test]
    fn joins_display_iterators_with_an_oxford_comma() {
        assert_eq!((1..=3).oxford_join(), "1, 2, and 3");
        assert_eq!(std::iter::empty::<&str>().oxford_join(), "");
        assert_eq!("a,b,c".split(',').oxford_join(), "a, b, and c");
        assert_eq!(std::iter::once(1.5_f64).oxford_join(), "1.5");
    }

    #[test]
    fn joins_one_shot_iterators() {
        // `from_fn` is not `Clone`, so counting the parts upfront would not compile.
        let mut count = 0;
        let items = std::iter::from_fn(|| {
            count += 1;
            (count < 4).then_some(count)
        });

        assert_eq!(items.oxford_join(), "1, 2, and 3");
    }

    #[test]
    fn joins_with_custom_separators() {
        assert_eq!(
            ["a", "b", "c"].join_with_separators(", ", " and "),
            "a, b and c"
        );
        assert_eq!(["a", "b", "c"].join_with_separators(", ", ", "), "a, b, c");
        assert_eq!(["a", "b"].join_with_separators(", ", " or "), "a or b");
        assert_eq!(
            ["a", "b", "c"].join_with_separators("; ", "; and "),
            "a; b; and c"
        );
        assert_eq!(["a", "b", "c"].join_with_separators("", ""), "abc");
        assert_eq!(
            ["a", "b", "c"]
                .into_iter()
                .join_with_separators(", ", " and "),
            "a, b and c"
        );
    }
}
