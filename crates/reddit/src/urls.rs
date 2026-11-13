//! URL classification for reddit.com and redd.it links.

use std::fmt::Write;

use url::Url;

use crate::Link;

/// The hosts [`classify_reddit_url`] recognizes.
///
/// Plugins subscribe to these hosts instead of repeating the list, so a host the classifier
/// accepts is never left unsubscribed — or subscribed without being handled.
pub const URL_HOSTS: &[&str] = &[
    "i.redd.it",
    "oauth.reddit.com",
    "old.reddit.com",
    "preview.redd.it",
    "redd.it",
    "reddit.com",
    "v.redd.it",
    "www.reddit.com",
];

/// Attempts to parse the given `url` as a reddit URL.
#[must_use]
pub fn classify_reddit_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some("v.redd.it" | "i.redd.it" | "preview.redd.it" | "redd.it") => {
            classify_redd_it_url(url)
        }
        Some("reddit.com" | "www.reddit.com" | "old.reddit.com" | "oauth.reddit.com") => {
            classify_reddit_com_url(url)
        }
        _ => None,
    }
}

/// The characters a subreddit name may contain beyond ASCII alphanumerics.
const SUBREDDIT_ALLOWED: &str = "_";

/// The characters a username may contain beyond ASCII alphanumerics.
const USERNAME_ALLOWED: &str = "_-";

/// Returns whether `segment` is a valid Reddit id.
///
/// Submission, comment and gallery ids are base36 while short-link ids are mixed-case base62,
/// so the shared alphabet is ASCII alphanumerics. Anything else — a percent-encoded separator,
/// a `t3_` fullname, a path or query fragment — is rejected before it can be interpolated into
/// a request URL.
#[must_use]
pub fn is_id(segment: &str) -> bool {
    zeta_url::is_identifier(segment, "")
}

/// Returns whether `segment` is a valid subreddit name — ASCII alphanumerics and underscores.
#[must_use]
pub fn is_subreddit(segment: &str) -> bool {
    zeta_url::is_identifier(segment, SUBREDDIT_ALLOWED)
}

/// Returns whether `segment` is a valid username — ASCII alphanumerics, hyphens and underscores.
#[must_use]
pub fn is_username(segment: &str) -> bool {
    zeta_url::is_identifier(segment, USERNAME_ALLOWED)
}

/// Parses reddit.com URLs
#[must_use]
pub fn classify_reddit_com_url(url: &Url) -> Option<Link> {
    match zeta_url::path_segments(url)?.as_slice() {
        // Direct link to a subreddit
        //
        // Parameters: `/r/<subreddit>`
        // Example: `/r/worldnews`
        ["r", subreddit] if is_subreddit(subreddit) => {
            Some(Link::Subreddit((*subreddit).to_string()))
        }
        // Direct link link to a submission page (i.e. full thread and comments)
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug][/]`
        // Example: `/r/nottheonion/comments/1u7eqe9/microsofts_new_outlook_takes_10_seconds_to_do/`
        ["r", subreddit, "comments", id] | ["r", subreddit, "comments", id, _]
            if is_subreddit(subreddit) && is_id(id) =>
        {
            Some(Link::Submission {
                id: (*id).to_string(),
                subreddit: (*subreddit).to_string(),
            })
        }
        // Direct link to a comment and its children for a submission
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug]/<comment_id>[/]`
        // Example: `/r/AskElectronics/comments/1u7evrr/what_is_the_best_course_of_action_for_wrong_width/orzo90y/`
        ["r", subreddit, "comments", submission_id, _, comment_id]
            if is_subreddit(subreddit) && is_id(submission_id) && is_id(comment_id) =>
        {
            Some(Link::Comment {
                id: (*comment_id).to_string(),
                submission: (*submission_id).to_string(),
                subreddit: (*subreddit).to_string(),
            })
        }
        // /r/<subreddit>/s/<id>[/]
        ["r", subreddit, "s", id] if is_subreddit(subreddit) && is_id(id) => {
            Some(Link::Shortened {
                id: (*id).to_string(),
                subreddit: (*subreddit).to_string(),
            })
        }
        // /comments/<id>[/]
        ["comments", id] if is_id(id) => Some(Link::CommentsPage {
            id: (*id).to_string(),
        }),
        // /gallery/<id>
        ["gallery", id] if is_id(id) => Some(Link::Gallery((*id).to_string())),
        // /video/<id>
        ["video", id] if is_id(id) => Some(Link::Video((*id).to_string())),
        // /user/<name>[/]
        ["user", username] if is_username(username) => Some(Link::User((*username).to_string())),
        _ => None,
    }
}

/// Parses redd.it URLs
fn classify_redd_it_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some("i.redd.it") => Some(Link::Image(url.path().to_string())),
        Some("v.redd.it") => url
            .path_segments()
            .and_then(|mut segment| segment.next())
            .filter(|id| is_id(id))
            .map(|id| Link::Video(id.to_string())),
        Some("preview.redd.it") => {
            let mut request_uri = url.path().to_string();

            if let Some(query) = url.query() {
                // Writing to a `String` cannot fail, so the write result needs no handling.
                let _ = write!(request_uri, "?{query}");
            }

            Some(Link::Preview(request_uri))
        }
        // `redd.it/<id>` is the short link for a submission; resolving it is the API's job, so
        // it is classified as the comments page it redirects to.
        Some("redd.it") => match zeta_url::path_segments(url)?.as_slice() {
            [id] if zeta_url::is_identifier(id, "") => Some(Link::CommentsPage {
                id: (*id).to_string(),
            }),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that every listed URL form classifies as `expected`.
    fn assert_classifies(urls: &[&str], expected: Option<&Link>) {
        for url_str in urls {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(
                classify_reddit_url(&url).as_ref(),
                expected,
                "unexpected classification of {url_str}"
            );
        }
    }

    #[test]
    fn parse_web_page_urls() {
        assert_classifies(
            &[
                "https://www.reddit.com/r/interestingasfuck/",
                "https://www.reddit.com/r/interestingasfuck",
            ],
            Some(&Link::Subreddit("interestingasfuck".to_string())),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/user/cealild/",
                "https://www.reddit.com/user/cealild",
            ],
            Some(&Link::User("cealild".to_string())),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/",
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions",
                "https://www.reddit.com/r/europe/comments/1nh144u/",
                "https://www.reddit.com/r/europe/comments/1nh144u",
            ],
            Some(&Link::Submission {
                id: "1nh144u".to_string(),
                subreddit: "europe".to_string(),
            }),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/ne86mgl/",
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/ne86mgl",
            ],
            Some(&Link::Comment {
                id: "ne86mgl".to_string(),
                submission: "1nh144u".to_string(),
                subreddit: "europe".to_string(),
            }),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/comments/1nh144u/",
                "https://www.reddit.com/comments/1nh144u",
            ],
            Some(&Link::CommentsPage {
                id: "1nh144u".to_string(),
            }),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/gallery/1nj9601",
                "https://www.reddit.com/gallery/1nj9601/",
            ],
            Some(&Link::Gallery("1nj9601".to_string())),
        );

        assert_classifies(
            &["https://www.reddit.com/r/linuxmemes/s/dmwUYLKTjd"],
            Some(&Link::Shortened {
                subreddit: "linuxmemes".to_string(),
                id: "dmwUYLKTjd".to_string(),
            }),
        );
    }

    #[test]
    fn parse_short_links() {
        assert_classifies(
            &["https://redd.it/1nh144u", "https://redd.it/1nh144u/"],
            Some(&Link::CommentsPage {
                id: "1nh144u".to_string(),
            }),
        );

        // A short link carries exactly one base36 id.
        assert_classifies(
            &[
                "https://redd.it/",
                "https://redd.it/1nh144u/extra",
                "https://redd.it/not an id",
            ],
            None,
        );
    }

    #[test]
    fn accepts_only_the_reddit_id_and_name_alphabet() {
        for id in ["1nh144u", "1NJ9601", "dmwUYLKTjd", "b2l87x1pyn7h1"] {
            assert!(is_id(id), "{id} should be a valid id");
        }

        for id in [
            "",
            "..",
            "a/b",
            "a?x",
            "a#frag",
            "%2F",
            "a b",
            "a-b",
            "t3_1nh144u",
        ] {
            assert!(!is_id(id), "{id:?} should not be a valid id");
        }

        for name in ["rust", "interestingasfuck", "some_sub", "Rust"] {
            assert!(is_subreddit(name), "{name} should be a valid subreddit");
        }

        for name in ["", "a-b", "a/b", "%2F", "a b", "..", "a.b"] {
            assert!(
                !is_subreddit(name),
                "{name:?} should not be a valid subreddit"
            );
        }

        for name in ["cealild", "some-user", "some_user"] {
            assert!(is_username(name), "{name} should be a valid username");
        }

        for name in ["", "a/b", "a.b", "%2F", "a b", ".."] {
            assert!(
                !is_username(name),
                "{name:?} should not be a valid username"
            );
        }
    }

    /// A crafted path segment must not classify: its value can be interpolated into request
    /// URLs, so a separator, query or fragment smuggled into the segment has to be rejected at
    /// the boundary.
    #[test]
    fn rejects_crafted_path_segments() {
        assert_classifies(
            &[
                "https://www.reddit.com/r/rust%2Fmemes",
                "https://www.reddit.com/r/rust%20memes",
                "https://www.reddit.com/r/rust-memes",
                "https://www.reddit.com/r/rust/comments/1nh144u%2Fextra",
                "https://www.reddit.com/r/rust/comments/1nh144u%3Ffoo",
                "https://www.reddit.com/r/rust/s/dmwUYLKTjd%2Fextra",
                "https://www.reddit.com/comments/1nh144u%3Ffoo",
                "https://www.reddit.com/gallery/1nj9601%23frag",
                "https://www.reddit.com/video/%2F",
                "https://www.reddit.com/video/%2e%2e",
                "https://www.reddit.com/user/a%2Fb",
                "https://v.redd.it/b2l87x1pyn7h1%2Fextra",
                "https://redd.it/1nh144u%2Fextra",
            ],
            None,
        );
    }

    /// The hosts the plugin subscribes to and the classifier recognizes must stay in sync.
    #[test]
    fn url_hosts_cover_the_classified_hosts() {
        let cases = [
            ("i.redd.it", "/gvjukykex8pf1.jpeg"),
            ("v.redd.it", "/b2l87x1pyn7h1"),
            ("preview.redd.it", "/nry00uecp5pf1.png"),
            ("redd.it", "/1nh144u"),
            ("reddit.com", "/r/rust"),
            ("www.reddit.com", "/r/rust"),
            ("old.reddit.com", "/r/rust"),
            ("oauth.reddit.com", "/r/rust"),
        ];

        assert_eq!(cases.len(), URL_HOSTS.len());

        for (host, path) in cases {
            assert!(URL_HOSTS.contains(&host), "{host} is not advertised");

            let url = Url::parse(&format!("https://{host}{path}")).unwrap();
            assert!(
                classify_reddit_url(&url).is_some(),
                "{host}{path} is advertised but does not classify"
            );
        }
    }

    #[test]
    fn parse_media_urls() {
        assert_classifies(
            &["https://i.redd.it/gvjukykex8pf1.jpeg"],
            Some(&Link::Image("/gvjukykex8pf1.jpeg".to_string())),
        );

        assert_classifies(
            &["https://preview.redd.it/nry00uecp5pf1.png?width=1497&format=png&auto=webp&s=69be11a8f3a211e485c44db89dc0f3023cdbfaf6"],
            Some(&Link::Preview("/nry00uecp5pf1.png?width=1497&format=png&auto=webp&s=69be11a8f3a211e485c44db89dc0f3023cdbfaf6".to_string())),
        );

        assert_classifies(
            &[
                "https://www.reddit.com/video/b2l87x1pyn7h1",
                "https://www.reddit.com/video/b2l87x1pyn7h1/",
                "https://v.redd.it/b2l87x1pyn7h1",
                "https://v.redd.it/b2l87x1pyn7h1/",
            ],
            Some(&Link::Video("b2l87x1pyn7h1".to_string())),
        );
    }
}
