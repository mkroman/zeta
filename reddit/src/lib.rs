use std::fmt::Write;
use std::time::Duration;

use serde::Deserialize;

mod client;
mod error;

pub use client::Client;
pub use error::Error;
use url::Url;

/// Reddit API base URL.
pub const BASE_URL: &str = "https://www.reddit.com";
pub const OAUTH_BASE_URL: &str = "https://oauth.reddit.com";
/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("rust:reddit:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");
/// The duration before a HTTP request times out.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// A link to a Reddit resource.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Link {
    /// Link to a comment.
    ///
    /// E.g.: `/r/europe/comments/1ngmbks/a_street_in_bologna/ne581kz/`
    /// Where the fields are: `/r/<subreddit>/comments/<submission>/_/<id>`
    Comment {
        /// The unique comment id.
        id: String,
        /// The unique submission id.
        submission: String,
        /// The subreddit of the submission with the comment.
        subreddit: String,
    },
    /// A link that redirects the user to the comments page for the relevant submission.
    Comments {
        /// The submission id.
        id: String,
    },
    /// Link to a gallery with multiple images.
    Gallery(String),
    /// Link to an image via i.redd.it.
    Image(String),
    /// Link to an image via preview.redd.it.
    Preview(String),
    /// A shortened subreddit link (e.g. `/r/<subreddit>/s/<id>`)
    Shortened {
        /// The unique id of the shortened URL.
        id: String,
        /// The subreddit.
        subreddit: String,
    },
    /// Link to a specific submission in a specific subreddit.
    ///
    /// E.g.: `/r/europe/comments/1ngmbks/a_street_in_bologna/`
    /// Where the fields are: `/r/<subreddit>/comments/<id>`
    Submission {
        /// The unique id of the submission.
        id: String,
        /// The name of the subreddit the submission is in.
        subreddit: String,
    },
    /// Link to a specific subreddit.
    Subreddit(String),
    /// Link to a users profile.
    ///
    /// E.g.: `/user/EcstaticYesterday605`
    User(String),
    /// Link to a video via v.redd.it.
    Video(String),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "data")]
#[allow(unused)]
pub enum Item {
    #[serde(rename = "t1")]
    Comment(Comment),
    #[serde(rename = "t5")]
    Subreddit(Subreddit),
    #[serde(rename = "t3")]
    Submission(Submission),
    #[serde(rename = "Listing")]
    Listing(Listing),
    #[serde(untagged)]
    Other(serde_json::Value),
}

#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Listing {
    // Not sure what this is.
    pub dist: Option<usize>,
    pub after: Option<String>,
    pub before: Option<String>,
    pub modhash: Option<String>,
    pub geo_filter: Option<String>,
    pub children: Vec<Item>,
}

/// Details about a Subreddit.
#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Submission {
    pub subreddit: String,
    pub title: String,
    /// Number of upvotes.
    pub ups: u32,
    /// Upvote ratio.
    pub upvote_ratio: f32,
    /// The main selftext.
    pub selftext: String,
    pub url: String,
}

/// Details about a Subreddit.
#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Subreddit {
    /// Display name of the subreddit.
    pub display_name: String,
    /// Title of the subreddit.
    pub title: String,
    /// Public description.
    pub public_description: String,
    /// Number of subscribers.
    pub subscribers: u32,
    /// Relative URL.
    pub url: String,
}

/// Details about a comment.
#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Comment {
    pub id: String,
    pub body: String,
    pub body_html: String,
    pub subreddit: String,
}

/// Attempts to parse the given `url` as a reddit URL.
pub fn parse_reddit_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some("v.redd.it" | "i.redd.it" | "preview.redd.it") => parse_redd_it_url(url),
        Some("reddit.com" | "www.reddit.com" | "old.reddit.com") => parse_reddit_com_url(url),
        _ => None,
    }
}

/// Parses reddit.com URLs
fn parse_reddit_com_url(url: &Url) -> Option<Link> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // Direct link to a subreddit
        //
        // Parameters: `/r/<subreddit>`
        // Example: `/r/worldnews`
        ["r", subreddit] | ["r", subreddit, ""] => Some(Link::Subreddit((*subreddit).to_string())),
        // Direct link link to a submission page (i.e. full thread and comments)
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug][/]`
        // Example: `/r/nottheonion/comments/1u7eqe9/microsofts_new_outlook_takes_10_seconds_to_do/`
        ["r", subreddit, "comments", id]
        | ["r", subreddit, "comments", id, _]
        | ["r", subreddit, "comments", id, _, ""] => Some(Link::Submission {
            id: (*id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // Direct link to a comment and its children for a submission
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug]/<comment_id>[/]`
        // Example: `/r/AskElectronics/comments/1u7evrr/what_is_the_best_course_of_action_for_wrong_width/orzo90y/`
        ["r", subreddit, "comments", submission_id, _, comment_id]
        | ["r", subreddit, "comments", submission_id, _, comment_id, ""] => Some(Link::Comment {
            id: (*comment_id).to_string(),
            submission: (*submission_id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // /r/<subreddit>/s/<id>[/]
        ["r", subreddit, "s", id] | ["r", subreddit, "s", id, ""] => Some(Link::Shortened {
            id: (*id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // /comments/<id>[/]
        ["comments", id] | ["comments", id, ""] => Some(Link::Comments {
            id: (*id).to_string(),
        }),
        // /gallery/<id>
        ["gallery", id] => Some(Link::Gallery((*id).to_string())),
        // /user/<name>[/]
        ["user", username] | ["user", username, ""] => Some(Link::User((*username).to_string())),
        _ => None,
    }
}

/// Parses redd.it URLs
fn parse_redd_it_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some("i.redd.it") => Some(Link::Image(url.path().to_string())),
        Some("v.redd.it") => Some(Link::Video(url.path().to_string())),
        Some("preview.redd.it") => {
            let mut request_uri = url.path().to_string();

            if let Some(query) = url.query() {
                write!(request_uri, "?{query}").ok()?;
            }

            Some(Link::Preview(request_uri))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parse_subreddit_urls() {
        let test_cases = [(
            &[
                "https://www.reddit.com/r/interestingasfuck/",
                "https://www.reddit.com/r/interestingasfuck",
            ],
            Some(Link::Subreddit("interestingasfuck".to_string())),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_submission_urls() {
        let test_cases = [(
            &[
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/",
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions",
                "https://www.reddit.com/r/europe/comments/1nh144u/",
                "https://www.reddit.com/r/europe/comments/1nh144u",
            ],
            Some(Link::Submission {
                id: "1nh144u".to_string(),
                subreddit: "europe".to_string(),
            }),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_submission_comment_urls() {
        let test_cases = [(
            &[
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/ne86mgl/",
                "https://www.reddit.com/r/europe/comments/1nh144u/germany_are_2025_eurobasket_champions/ne86mgl",
            ],
            Some(Link::Comment {
                id: "ne86mgl".to_string(),
                submission: "1nh144u".to_string(),
                subreddit: "europe".to_string(),
            }),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_user_urls() {
        let test_cases = [(
            &[
                "https://www.reddit.com/user/cealild/",
                "https://www.reddit.com/user/cealild",
            ],
            Some(Link::User("cealild".to_string())),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_shortened_video_urls() {
        let test_cases = [
            (
                &["https://v.redd.it/n4p472c4u7pf1"],
                Some(Link::Video("/n4p472c4u7pf1".to_string())),
            ),
            (
                &["https://v.redd.it/n4p472c4u7pf1/"],
                Some(Link::Video("/n4p472c4u7pf1/".to_string())),
            ),
        ];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_shortened_image_urls() {
        let test_cases = [(
            &["https://i.redd.it/gvjukykex8pf1.jpeg"],
            Some(Link::Image("/gvjukykex8pf1.jpeg".to_string())),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_preview_urls() {
        let test_cases = [(
            &["https://preview.redd.it/nry00uecp5pf1.png?width=1497&format=png&auto=webp&s=69be11a8f3a211e485c44db89dc0f3023cdbfaf6"],
            Some(Link::Preview("/nry00uecp5pf1.png?width=1497&format=png&auto=webp&s=69be11a8f3a211e485c44db89dc0f3023cdbfaf6".to_string())),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_shortened_comment_urls() {
        let test_cases = [(
            "https://www.reddit.com/r/linuxmemes/s/dmwUYLKTjd",
            Some(Link::Shortened {
                subreddit: "linuxmemes".to_string(),
                id: "dmwUYLKTjd".to_string(),
            }),
        )];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_reddit_url(&url), expected);
        }
    }

    #[test]
    fn parse_comment_redirect_urls() {
        let test_cases = [(
            &[
                "https://www.reddit.com/comments/1nh144u/",
                "https://www.reddit.com/comments/1nh144u",
            ],
            Some(Link::Comments {
                id: "1nh144u".to_string(),
            }),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_gallery_urls() {
        let test_cases = [(
            &["https://www.reddit.com/gallery/1nj9601"],
            Some(Link::Gallery("1nj9601".to_string())),
        )];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_comments_listing_json() -> Result<(), Box<dyn std::error::Error>> {
        use tracing::error;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reddit/comments/1niz1ru.json");
        let text = std::fs::read_to_string(path).unwrap();
        let jd = &mut serde_json::Deserializer::from_str(&text);
        let (item1, item2): (Item, Item) = serde_path_to_error::deserialize(jd)
            .inspect_err(|err| error!(?err, %text, "could not parse comments response"))?;

        assert!(matches!(item1, Item::Listing(_)));
        assert!(matches!(item2, Item::Listing(_)));

        Ok(())
    }
}
