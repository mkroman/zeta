//! Error rendering helpers.

use std::error::Error;
use std::fmt;

/// Renders an error and its [`Error::source`] chain as one line for a log event.
///
/// [`Display`](fmt::Display) on an error is the short, user-facing sentence; the source chain is
/// the operator detail. This adapter walks that chain without making it available to reply
/// formatting, and skips a source whose text already ends the message.
#[derive(Clone, Copy, Debug)]
pub struct Chain<'a>(pub &'a (dyn Error + 'static));

impl fmt::Display for Chain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut message = self.0.to_string();
        let mut source = self.0.source();

        while let Some(cause) = source {
            let text = cause.to_string();

            if !text.is_empty() && !message.ends_with(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }

            source = cause.source();
        }

        f.write_str(&message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The deepest error in the chain.
    #[derive(Debug)]
    struct Root;

    impl fmt::Display for Root {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("root")
        }
    }

    impl Error for Root {}

    /// An error whose source is [`Root`].
    #[derive(Debug)]
    struct Middle {
        source: Root,
    }

    impl fmt::Display for Middle {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("middle")
        }
    }

    impl Error for Middle {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.source)
        }
    }

    /// An error whose source is [`Middle`].
    #[derive(Debug)]
    struct Outer {
        source: Middle,
    }

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer")
        }
    }

    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.source)
        }
    }

    #[test]
    fn includes_the_whole_source_chain() {
        let error = Outer {
            source: Middle { source: Root },
        };

        assert_eq!(Chain(&error).to_string(), "outer: middle: root");
    }

    #[test]
    fn does_not_repeat_a_source_already_in_the_message() {
        #[derive(Debug)]
        struct Repeated {
            source: Root,
        }

        impl fmt::Display for Repeated {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("outer: root")
            }
        }

        impl Error for Repeated {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.source)
            }
        }

        let error = Repeated { source: Root };

        assert_eq!(Chain(&error).to_string(), "outer: root");
    }
}
