//! Unit tests for the `.filter` command surface: argument parsing and validation, filter
//! construction, reply formatting, and the published filter handle.

use super::*;

use clap::CommandFactory;
use zeta_plugin::{NoSettings, Registered};
use zeta_utils::irc::{self, MAX_LISTING_LENGTH};
use zeta_utils::text;

use crate::args::{Add, Delete, Opts, Subcommand};
use crate::format::{changed_response, describe, listing};

#[test]
fn cli_definition_is_valid() {
    // clap reports contradictory definitions through asserts; evaluating them here turns a
    // bad definition into a test failure instead of a panic in debug builds.
    Opts::command().debug_assert();
}

#[tokio::test]
async fn publishes_a_filter_handle_for_the_host() {
    // The host's dispatcher consults the `FilterHandle`, which only sees the service
    // through the type-erased handle published here — without it, no URL is ever filtered.
    let ctx = zeta_test_support::test_context();
    let mut subscriptions = Subscriptions::new();

    <FilterPlugin as Registered>::new(&ctx, &NoSettings {}, &mut subscriptions).unwrap();

    assert!(ctx.shared().get::<FilterHandle>().is_some());
}

/// Parses `input` with the filter command spec and unpacks the `add` subcommand.
fn parse_add(input: &str) -> Add {
    let opts: Opts = FILTER
        .parse_words(input)
        .unwrap_or_else(|error| panic!("`{input}` should parse: {error}"));

    let Subcommand::Add(add) = opts.command else {
        panic!("`{input}` should be the add subcommand");
    };

    add
}

/// Parses `input` with the filter command spec and unpacks the `delete` subcommand.
fn parse_delete(input: &str) -> Delete {
    let opts: Opts = FILTER
        .parse_words(input)
        .unwrap_or_else(|error| panic!("`{input}` should parse: {error}"));

    let Subcommand::Delete(delete) = opts.command else {
        panic!("`{input}` should be the delete subcommand");
    };

    delete
}

#[test]
fn parses_add_with_repeated_hosts() {
    let add = parse_add("add --host imdb.com --host www.imdb.com --user other");

    assert_eq!(add.host, ["imdb.com", "www.imdb.com"]);
    assert_eq!(add.sender.user.as_deref(), Some("other"));
}

#[test]
fn builds_one_filter_per_host_and_path() {
    let add = parse_add("add --host a.com --host b.com --path /x/*");

    let filters = add.new_filters(Some("#chan"), "smoke");

    assert_eq!(filters.len(), 2);
    assert!(
        filters
            .iter()
            .all(|filter| filter.path.as_deref() == Some("/x/*"))
    );
    assert_eq!(filters[0].host.as_deref(), Some("a.com"));
    assert_eq!(filters[1].host.as_deref(), Some("b.com"));
    assert!(
        filters
            .iter()
            .all(|filter| filter.channel.as_deref() == Some("#chan"))
    );
}

#[test]
fn builds_a_hostless_filter_when_only_sender_criteria_are_given() {
    let add = parse_add("add --user *other");

    let filters = add.new_filters(None, "smoke");

    assert_eq!(filters.len(), 1);
    assert_eq!(filters[0].host, None);
    assert_eq!(filters[0].channel, None);
    assert_eq!(filters[0].username.as_deref(), Some("*other"));
}

#[test]
fn describes_added_filters() {
    let first = Filter {
        id: 12,
        channel: None,
        host: Some("a.com".into()),
        path: Some("/x/*".into()),
        nickname: None,
        username: None,
        hostname: None,
    };

    let cases = [
        (
            "a single filter",
            vec![Filter {
                id: 12,
                channel: Some("#foo".into()),
                host: Some("example.com".into()),
                ..first.clone()
            }],
            "> Filter: Added #12: host=example.com path=/x/* (#foo).",
        ),
        (
            "multiple filters",
            vec![
                first.clone(),
                Filter {
                    id: 13,
                    host: Some("b.com".into()),
                    ..first
                },
            ],
            "> Filter: Added 2 filters: #12: host=a.com path=/x/* (all channels), \
             #13: host=b.com path=/x/* (all channels)",
        ),
    ];

    for (case, filters, expected) in cases {
        assert_eq!(
            irc::strip_formatting(&changed_response("Filter", "Added", &filters)),
            expected,
            "for {case}"
        );
    }
}

#[test]
fn counts_are_pluralized() {
    assert_eq!(text::pluralize(1, "filter"), "1 filter");
    assert_eq!(text::pluralize(2, "filter"), "2 filters");
    assert_eq!(text::pluralize(0, "filter"), "0 filters");
}

#[test]
fn add_rejects_invalid_arguments() {
    let cases = [
        (
            "a channel without a prefix",
            "add --channel foo --host example.com",
            "did you mean #foo?",
        ),
        (
            "no criterion",
            "add",
            "at least one of --host, --path, --nick, --user or --hostname",
        ),
        (
            "channel and all-channels together",
            "add --channel #foo --all-channels --host example.com",
            "not both",
        ),
        (
            "a wildcard channel",
            "add --channel #foo* --host example.com",
            "does not support wildcards",
        ),
    ];

    for (case, input, expected) in cases {
        let error = parse_add(input).validate("#chan").unwrap_err();

        assert!(error.contains(expected), "for {case}: {error}");
    }
}

#[test]
fn repeated_identical_patterns_are_deduplicated() {
    let add = parse_add("add --host a.com --host a.com --host b.com");

    let filters = add.new_filters(None, "smoke");

    assert_eq!(filters.len(), 2);
    assert_eq!(filters[0].host.as_deref(), Some("a.com"));
    assert_eq!(filters[1].host.as_deref(), Some("b.com"));
}

#[test]
fn a_truncated_listing_names_the_dropped_filters() {
    let long = Filter {
        id: 1,
        channel: None,
        host: Some(format!("long.example/{}", "x".repeat(180))),
        path: None,
        nickname: None,
        username: None,
        hostname: None,
    };
    let second = Filter {
        id: 2,
        ..long.clone()
    };

    let message = listing(labeled_reply("Filter", "2 filters: "), &[long, second], 0);

    assert!(message.len() <= MAX_LISTING_LENGTH);
    assert!(message.contains("#1: "), "{message}");
    assert!(message.ends_with(" (+1 more)"), "{message}");
}

#[test]
fn add_defaults_to_the_current_channel() {
    let add = parse_add("add --host example.com");

    assert_eq!(add.validate("#chan").unwrap().as_deref(), Some("#chan"));
}

#[test]
fn parses_delete_arguments() {
    let by_id = parse_delete("delete 12 --force");

    assert_eq!(by_id.id, Some(12));
    assert!(by_id.force);
    assert!(by_id.criteria().is_none());

    let by_criteria = parse_delete("delete --host *.com");

    assert_eq!(by_criteria.id, None);
    assert_eq!(
        by_criteria.criteria().unwrap().host.as_deref(),
        Some("*.com")
    );
}

#[test]
fn repeated_switches_parse_in_subcommands() {
    let add = parse_add("add --all-channels --all-channels --host example.com");
    assert!(add.all_channels);

    let delete = parse_delete("delete 12 --force --force");
    assert!(delete.force);
}

#[test]
fn list_criteria_convert() {
    let opts: Opts = FILTER
        .parse_words("list --host *.com")
        .expect("`list --host *.com` should parse");

    let Subcommand::List(list) = opts.command else {
        panic!("expected the list subcommand");
    };

    let criteria = list.criteria();

    assert_eq!(criteria.host.as_deref(), Some("*.com"));
    assert!(criteria.channel.is_none());
}

#[test]
fn describes_filters() {
    let filter = Filter {
        id: 12,
        channel: Some("#foo".into()),
        host: Some("*.com".into()),
        path: None,
        nickname: None,
        username: Some("*other".into()),
        hostname: None,
    };

    assert_eq!(describe(&filter), "#12: host=*.com user=*other (#foo)");

    let global = Filter {
        id: 3,
        channel: None,
        host: None,
        path: Some("/title/*".into()),
        nickname: None,
        username: None,
        hostname: None,
    };

    assert_eq!(describe(&global), "#3: path=/title/* (all channels)");
}
