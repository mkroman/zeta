//! Unit tests for command specifications, argument parsing, and the command cache.
//!
//! The cache tests drive the same `CACHE` the parsers use: they hold an entry busy, poison one
//! through a panicking value parser, and count command builds to prove reuse and isolation.

use std::sync::Arc;

use clap::{ColorChoice, Command, CommandFactory, FromArgMatches, Parser};
use zeta_test_support::assert_empty;

use super::cache::{CACHE, configure};
use super::*;

#[derive(Parser, Debug, PartialEq)]
/// Test options.
struct NameOpts {
    /// the required positional argument
    #[allow(dead_code)]
    name: String,
}

#[derive(Parser, Debug, PartialEq)]
/// Test options.
struct GreedyOpts {
    /// the message and datetime
    #[arg(num_args = 1.., trailing_var_arg = true)]
    #[allow(dead_code)]
    args: Vec<String>,
}

#[test]
fn trailing_args_matches_the_trigger_prefix() {
    const CMD: CommandSpec = CommandSpec::new("!test", "test");

    assert_eq!(CMD.trailing_args("!test --help"), Some("--help"));
    assert_eq!(CMD.trailing_args("!test"), Some(""));
    // Repeated whitespace between the trigger and the arguments collapses.
    assert_eq!(CMD.trailing_args("!test   --help"), Some("--help"));
    assert_eq!(CMD.trailing_args("!test  \t  args"), Some("args"));
    // Only the exact trigger prefix matches.
    assert_eq!(CMD.trailing_args("!testing --help"), None);
    // Ideographic space (U+3000) is 3 bytes — must not panic on byte slice.
    assert_eq!(CMD.trailing_args("!test\u{3000}args"), Some("args"));
}

#[test]
fn trigger_returns_trigger() {
    const CMD: CommandSpec = CommandSpec::new(".yt", "yt");

    assert_eq!(CMD.trigger(), ".yt");
}

#[test]
fn description_returns_description() {
    const CMD: CommandSpec = CommandSpec::new(".dig", "Look up DNS records for a domain");

    assert_eq!(CMD.description(), "Look up DNS records for a domain");
}

#[test]
fn equality_is_structural() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    assert_eq!(CMD, CommandSpec::new(".test", "test"));
    assert_ne!(CMD, CommandSpec::new(".other", "test"));
}

#[test]
fn parse_shell_tokenizes_like_a_shell() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// first argument
        first: String,
        /// second argument
        second: String,
        /// third argument
        third: String,
    }

    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let opts: Opts = CMD.parse_shell(r#"one "two words" th\ ree"#).unwrap();

    assert_eq!(
        opts,
        Opts {
            first: "one".into(),
            second: "two words".into(),
            third: "th ree".into(),
        }
    );
}

#[test]
fn parse_shell_reports_usage_on_error() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let err = CMD.parse_shell::<NameOpts>("").unwrap_err();

    // The string is replied to verbatim, so it must read as a usage message for the
    // command and name the missing argument; the exact layout belongs to clap.
    let ArgsError::Usage(usage) = err else {
        panic!("a missing argument should be a usage error");
    };
    assert!(usage.contains("Usage:"), "{usage}");
    assert!(usage.contains(".test"), "{usage}");
    assert!(usage.contains("<NAME>"), "{usage}");
}

#[test]
fn parse_shell_reports_quoting_error() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    assert_eq!(
        CMD.parse_shell::<NameOpts>("\"unbalanced"),
        Err(ArgsError::Quoting)
    );
}

#[test]
fn parse_words_splits_on_whitespace() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let opts: GreedyOpts = CMD.parse_words("  hello\tworld  ").unwrap();

    assert_eq!(opts.args, vec!["hello".to_owned(), "world".to_owned()]);

    let opts: GreedyOpts = CMD.parse_words("").unwrap();

    assert_empty!(opts.args);
}

#[test]
fn parse_words_keeps_quotes_and_apostrophes_verbatim() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let opts: GreedyOpts = CMD.parse_words(r#"don't "forget me" at 4:20"#).unwrap();

    assert_eq!(opts.args.join(" "), r#"don't "forget me" at 4:20"#);
}

#[test]
fn parse_words_parses_flags() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// list instead
        #[arg(short = 'l', long)]
        list: bool,
        /// the message and datetime
        #[arg(num_args = 1.., trailing_var_arg = true)]
        #[allow(dead_code)]
        args: Vec<String>,
    }

    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let opts: Opts = CMD.parse_words("-l hello world").unwrap();

    assert!(opts.list);
    assert_eq!(opts.args, vec!["hello".to_owned(), "world".to_owned()]);

    let opts: Opts = CMD.parse_words("-- don't panic").unwrap();

    assert!(!opts.list);
    assert_eq!(opts.args, vec!["don't".to_owned(), "panic".to_owned()]);
}

#[test]
fn repeated_switches_are_idempotent() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// list instead
        #[arg(short = 'l', long)]
        list: bool,
        /// the message and datetime
        #[arg(num_args = 1.., trailing_var_arg = true)]
        #[allow(dead_code)]
        args: Vec<String>,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".test", "test");

    let opts: Opts = CMD.parse_words("-l -l").expect("switches are idempotent");

    assert!(opts.list);

    // The second parse runs against the cached command; the relaxation must survive reuse.
    let opts: Opts = CMD
        .parse_words("--list -l -l")
        .expect("switches are idempotent with a cached command");

    assert!(opts.list);
}

#[test]
fn repeated_parses_reuse_the_cached_command() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// the query
        query: String,
        /// the language
        #[arg(long, default_value = "en")]
        language: String,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".test", "test");

    let first: Opts = CMD.parse_words("hello").unwrap();
    let second: Opts = CMD.parse_words("--language fr hello").unwrap();
    let third: Opts = CMD.parse_shell("\"hello\" --language de").unwrap();

    assert_eq!(
        first,
        Opts {
            query: "hello".into(),
            language: "en".into(),
        }
    );
    assert_eq!(
        second,
        Opts {
            query: "hello".into(),
            language: "fr".into(),
        }
    );
    assert_eq!(
        third,
        Opts {
            query: "hello".into(),
            language: "de".into(),
        }
    );
}

#[test]
fn repeated_parses_build_the_command_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use clap::{Arg, ArgMatches};

    /// Test options built by hand, so the test can count command builds.
    #[derive(Debug, PartialEq)]
    struct CountedOpts {
        /// the required positional argument
        name: String,
    }

    static BUILDS: AtomicUsize = AtomicUsize::new(0);

    impl CommandFactory for CountedOpts {
        fn command() -> Command {
            BUILDS.fetch_add(1, Ordering::SeqCst);

            Command::new("counted").arg(Arg::new("name").required(true))
        }

        fn command_for_update() -> Command {
            Self::command()
        }
    }

    impl FromArgMatches for CountedOpts {
        fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
            matches
                .get_one::<String>("name")
                .cloned()
                .map(|name| Self { name })
                .ok_or_else(|| {
                    clap::Error::raw(
                        clap::error::ErrorKind::MissingRequiredArgument,
                        "missing name",
                    )
                })
        }

        fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
            *self = Self::from_arg_matches(matches)?;

            Ok(())
        }
    }

    impl Parser for CountedOpts {}

    const CMD: CommandSpec = CommandSpec::with_args::<CountedOpts>(".counted", "test");

    let first: CountedOpts = CMD.parse_words("one").unwrap();
    let second: CountedOpts = CMD.parse_words("two").unwrap();
    let third: CountedOpts = CMD.parse_shell("three").unwrap();

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        1,
        "the command should be built once and then reused"
    );
    assert_eq!(first, CountedOpts { name: "one".into() });
    assert_eq!(second, CountedOpts { name: "two".into() });
    assert_eq!(
        third,
        CountedOpts {
            name: "three".into()
        }
    );
}

#[test]
fn a_busy_cached_command_parses_without_blocking() {
    use std::sync::mpsc;
    use std::time::Duration;

    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// the required positional argument
        name: String,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".busy", "test");

    let cached = CACHE.cached_command::<Opts>(".busy");
    let guard = cached
        .lock()
        .expect("the cached command lock is not poisoned");

    let (sender, receiver) = mpsc::channel();
    let parse = std::thread::spawn(move || {
        let parsed: Result<Opts, ArgsError> = CMD.parse_words("one");
        sender
            .send(parsed)
            .expect("the test thread should receive the parse result");
    });

    let parsed = match receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(parsed) => parsed,
        Err(error) => {
            drop(guard);
            let _ = parse.join();

            panic!("parsing a busy cached command blocked: {error}");
        }
    };

    drop(guard);

    assert_eq!(parsed.unwrap(), Opts { name: "one".into() });

    // Once the lock is released, parsing goes back to the cached command.
    let again: Opts = CMD.parse_words("two").unwrap();

    assert_eq!(again, Opts { name: "two".into() });
}

#[test]
fn a_poisoned_cached_command_is_replaced() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static PANICKED: AtomicBool = AtomicBool::new(false);

    /// A value parser that fails the first parse, leaving the cached command poisoned.
    // clap's value parser functions must return `Result`, even though this one only panics.
    #[allow(clippy::unnecessary_wraps)]
    fn panics_once(value: &str) -> Result<String, String> {
        assert!(
            PANICKED.swap(true, Ordering::SeqCst),
            "the first parse panics while holding the cached command"
        );

        Ok(value.to_owned())
    }

    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// the required positional argument
        #[arg(value_parser = panics_once)]
        name: String,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".poisoned", "test");

    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CMD.parse_words::<Opts>("one")
    }));

    assert!(first.is_err(), "the first parse should panic");

    // The poisoned entry is evicted, so this parse configures a fresh command.
    let second: Opts = CMD
        .parse_words("two")
        .expect("a poisoned command should be replaced");

    assert_eq!(second, Opts { name: "two".into() });
}

#[test]
fn repeated_missing_argument_reports_the_same_usage() {
    const CMD: CommandSpec = CommandSpec::with_args::<NameOpts>(".missing", "test");

    let first = CMD.parse_words::<NameOpts>("").unwrap_err();
    let second = CMD.parse_words::<NameOpts>("").unwrap_err();

    assert_eq!(first, second);

    let ArgsError::Usage(usage) = first else {
        panic!("a missing argument should be a usage error");
    };
    assert!(usage.contains(".missing"), "{usage}");
    assert!(usage.contains("<NAME>"), "{usage}");
}

#[test]
fn repeated_unknown_flag_reports_the_same_usage() {
    const CMD: CommandSpec = CommandSpec::with_args::<NameOpts>(".unknown", "test");

    let first = CMD.parse_words::<NameOpts>("-x").unwrap_err();
    let second = CMD.parse_words::<NameOpts>("-x").unwrap_err();

    assert_eq!(first, second);

    let ArgsError::Usage(usage) = first else {
        panic!("an unknown flag should be a usage error");
    };
    assert!(usage.contains("-x"), "{usage}");
}

#[test]
fn repeated_help_output_is_identical() {
    const CMD: CommandSpec = CommandSpec::with_args::<GreedyOpts>(".help", "test");

    // A successful parse first, so help is rendered from an already-built cached command.
    assert_eq!(
        CMD.parse_words::<GreedyOpts>("hello").unwrap().args,
        vec!["hello".to_owned()]
    );

    let first = CMD.parse_words::<GreedyOpts>("--help").unwrap_err();
    let second = CMD.parse_words::<GreedyOpts>("--help").unwrap_err();

    assert_eq!(first, second);

    let ArgsError::Usage(help) = first else {
        panic!("--help should return the rendered help");
    };
    assert!(help.contains(".help"), "{help}");
    assert!(help.contains("--help"), "{help}");
}

#[test]
fn cached_command_parses_subcommands_repeatedly() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// the subcommand
        #[command(subcommand)]
        command: Action,
    }

    #[derive(clap::Subcommand, Debug, PartialEq)]
    /// Test actions.
    enum Action {
        /// add an entry
        Add {
            /// the entry name
            name: String,
        },
        /// list the entries
        List,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".sub", "test");

    assert_eq!(
        CMD.parse_words::<Opts>("add one").unwrap(),
        Opts {
            command: Action::Add { name: "one".into() },
        }
    );
    assert_eq!(
        CMD.parse_words::<Opts>("list").unwrap(),
        Opts {
            command: Action::List,
        }
    );
    assert_eq!(
        CMD.parse_words::<Opts>("add two").unwrap(),
        Opts {
            command: Action::Add { name: "two".into() },
        }
    );
}

#[test]
fn cached_parses_match_freshly_configured_commands() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Opts {
        /// the query
        query: String,
        /// the language
        #[arg(long, default_value = "en")]
        language: String,
        /// verbose output
        #[arg(short, long)]
        verbose: bool,
    }

    const CMD: CommandSpec = CommandSpec::with_args::<Opts>(".compare", "test");

    // Populate the cache before the comparison.
    assert_eq!(CMD.parse_words::<Opts>("warm").unwrap().query, "warm");

    for args in [
        "hello",
        "--language fr hello",
        "-v hello",
        "-v -v hello",
        "--verbose --verbose hello",
        "",
        "-x",
        "--language",
        "one two",
        "--help",
    ] {
        let cached = CMD
            .parse_words::<Opts>(args)
            .map_err(|error| error.to_string());

        let fresh = configure(Opts::command(), CMD.trigger())
            .try_get_matches_from(std::iter::once(CMD.trigger()).chain(args.split_whitespace()))
            .map_err(|error| error.to_string())
            .and_then(|matches| {
                Opts::from_arg_matches(&matches).map_err(|error| error.to_string())
            });

        assert_eq!(cached, fresh, "arguments: {args:?}");
    }
}

#[test]
fn the_command_cache_initializes_each_command_once() {
    #[derive(Parser, Debug)]
    /// Test options.
    struct Opts {
        /// the query
        query: String,
    }

    let first = CACHE.cached_command::<Opts>(".once");
    let second = CACHE.cached_command::<Opts>(".once");

    assert!(
        Arc::ptr_eq(&first, &second),
        "the configured command is reused"
    );
}

#[test]
fn different_command_types_do_not_share_a_cache_entry() {
    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Left {
        /// left flag
        #[arg(long)]
        left: bool,
    }

    #[derive(Parser, Debug, PartialEq)]
    /// Test options.
    struct Right {
        /// right flag
        #[arg(long)]
        right: bool,
    }

    const LEFT: CommandSpec = CommandSpec::with_args::<Left>(".shared", "left");
    const RIGHT: CommandSpec = CommandSpec::with_args::<Right>(".shared", "right");

    assert!(LEFT.parse_words::<Left>("--left").unwrap().left);
    assert!(RIGHT.parse_words::<Right>("--right").unwrap().right);

    // Each type only accepts its own flag.
    assert!(LEFT.parse_words::<Left>("--right").is_err());
    assert!(RIGHT.parse_words::<Right>("--left").is_err());

    // The first entry still parses after the second type was configured.
    assert!(LEFT.parse_words::<Left>("--left").unwrap().left);
}

#[test]
fn the_same_type_gets_one_entry_per_trigger() {
    const ONE: CommandSpec = CommandSpec::with_args::<NameOpts>(".one", "test");
    const TWO: CommandSpec = CommandSpec::with_args::<NameOpts>(".two", "test");

    let Err(ArgsError::Usage(one)) = ONE.parse_words::<NameOpts>("") else {
        panic!("a missing argument should be a usage error");
    };
    let Err(ArgsError::Usage(two)) = TWO.parse_words::<NameOpts>("") else {
        panic!("a missing argument should be a usage error");
    };

    assert!(one.contains(".one"), "{one}");
    assert!(!one.contains(".two"), "{one}");
    assert!(two.contains(".two"), "{two}");
    assert!(!two.contains(".one"), "{two}");
}

#[test]
fn parse_words_reports_usage_on_error() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    let err = CMD.parse_words::<GreedyOpts>("-x").unwrap_err();

    // The reply must name the offending token; the rest of the layout is clap's.
    let ArgsError::Usage(usage) = err else {
        panic!("an unknown flag should be a usage error");
    };
    assert!(usage.contains("-x"), "{usage}");
}

#[test]
fn args_info_is_named_after_the_trigger() {
    /// Look up a domain name.
    #[derive(Parser)]
    struct Opts {
        /// the domain to look up
        #[allow(dead_code)]
        name: String,
    }

    const DIG: CommandSpec = CommandSpec::with_args::<Opts>(".dig", "dig");

    let info = DIG.args_info().unwrap();

    assert_eq!(info.get_bin_name(), Some(".dig"));
    assert_eq!(info.get_subcommands().count(), 0);
    // IRC replies cannot carry ANSI styling.
    assert_eq!(info.get_color(), ColorChoice::Never);
}

#[test]
fn args_info_returns_derived_information() {
    /// Look up a domain name.
    #[derive(Parser)]
    struct Opts {
        /// the domain to look up
        #[allow(dead_code)]
        name: String,
    }

    const DIG: CommandSpec =
        CommandSpec::with_args::<Opts>(".dig", "Look up DNS records for a domain");

    assert_eq!(DIG.description(), "Look up DNS records for a domain");

    let info = DIG.args_info().unwrap();
    let positional = info.get_positionals().next().unwrap();

    assert_eq!(positional.get_id().as_str(), "name");
}

#[test]
fn plain_command_has_no_args_info() {
    const CMD: CommandSpec = CommandSpec::new(".test", "test");

    assert!(CMD.args_info().is_none());
}
