use std::fmt::Write;
use std::time::Duration;

use serde::Deserialize;
use url::Url;

mod client;
mod error;

pub use client::Client;
pub use error::Error;

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

/// Details about a submission.
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
    /// The unique base36 id of the submission.
    #[serde(default)]
    pub id: Option<String>,
    /// Media details of the submission, present for hosted video.
    #[serde(default)]
    pub secure_media: Option<Media>,
    /// Media details of the submission; equivalent to `secure_media` for hosted video.
    #[serde(default)]
    pub media: Option<Media>,
    /// The parent submissions when the submission is a crosspost.
    ///
    /// The media of a crosspost lives on its parent.
    #[serde(default)]
    pub crosspost_parent_list: Option<Vec<Submission>>,
}

impl Submission {
    /// Returns the URL of the hosted video of the submission, if any.
    ///
    /// The DASH manifest is preferred (it carries both video and audio), then the HLS playlist,
    /// then the fallback URL (video only). Crossposted videos are resolved through their parent
    /// submission.
    #[must_use]
    pub fn video_url(&self) -> Option<&str> {
        let video = self.reddit_video()?;

        video
            .dash_url
            .as_deref()
            .or(video.hls_url.as_deref())
            .or(video.fallback_url.as_deref())
    }

    /// Returns the hosted video details of the submission, if any.
    fn reddit_video(&self) -> Option<&RedditVideo> {
        let own = self
            .secure_media
            .as_ref()
            .and_then(|media| media.reddit_video.as_ref())
            .or_else(|| {
                self.media
                    .as_ref()
                    .and_then(|media| media.reddit_video.as_ref())
            });

        own.or_else(|| {
            self.crosspost_parent_list
                .as_ref()?
                .first()
                .and_then(Submission::reddit_video)
        })
    }
}

/// Media details of a submission.
#[derive(Debug, Deserialize)]
pub struct Media {
    /// Details about the hosted video of the submission, if any.
    #[serde(default)]
    pub reddit_video: Option<RedditVideo>,
}

/// Details about a hosted video.
#[derive(Debug, Deserialize)]
pub struct RedditVideo {
    /// The URL of a single progressive video stream (without audio).
    #[serde(default)]
    pub fallback_url: Option<String>,
    /// The URL of the DASH manifest (video and audio).
    #[serde(default)]
    pub dash_url: Option<String>,
    /// The URL of the HLS playlist (video and audio).
    #[serde(default)]
    pub hls_url: Option<String>,
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
pub fn classify_reddit_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some("v.redd.it" | "i.redd.it" | "preview.redd.it") => classify_redd_it_url(url),
        Some("reddit.com" | "www.reddit.com" | "old.reddit.com" | "oauth.reddit.com") => {
            classify_reddit_com_url(url)
        }
        _ => None,
    }
}

/// Parses reddit.com URLs
fn classify_reddit_com_url(url: &Url) -> Option<Link> {
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
        // /video/<id>
        ["video", id] => Some(Link::Video((*id).to_string())),
        // /user/<name>[/]
        ["user", username] | ["user", username, ""] => Some(Link::User((*username).to_string())),
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
            .map(|id| Link::Video(id.to_string())),
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

            assert_eq!(classify_reddit_url(&url), expected);
        }
    }

    #[test]
    fn classify_video_urls() {
        let test_cases = [
            (
                "https://www.reddit.com/video/b2l87x1pyn7h1",
                Some(Link::Video("b2l87x1pyn7h1".to_string())),
            ),
            (
                "https://v.redd.it/b2l87x1pyn7h1",
                Some(Link::Video("b2l87x1pyn7h1".to_string())),
            ),
            (
                "https://v.redd.it/b2l87x1pyn7h1/",
                Some(Link::Video("b2l87x1pyn7h1".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
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

                assert_eq!(classify_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_comments_listing_json() -> Result<(), Box<dyn std::error::Error>> {
        use tracing::error;
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/comments/1niz1ru.json");
        let text = std::fs::read_to_string(path).unwrap();
        let jd = &mut serde_json::Deserializer::from_str(&text);
        let (item1, item2): (Item, Item) = serde_path_to_error::deserialize(jd)
            .inspect_err(|err| error!(?err, %text, "could not parse comments response"))?;

        assert!(matches!(item1, Item::Listing(_)));
        assert!(matches!(item2, Item::Listing(_)));

        Ok(())
    }

    /// A minimal submission with hosted video, as returned by the API.
    fn video_submission_json() -> serde_json::Value {
        serde_json::json!({
            "subreddit": "SteamDeck",
            "title": "Offline WoW installer is finally complete",
            "ups": 1,
            "upvote_ratio": 1.0,
            "selftext": "",
            "url": "https://v.redd.it/pxtf7mx2xqzg1",
            "id": "1t6gdp8",
            "secure_media": {
                "reddit_video": {
                    "fallback_url": "https://v.redd.it/pxtf7mx2xqzg1/DASH_720.mp4?source=fallback",
                    "dash_url": "https://v.redd.it/pxtf7mx2xqzg1/DASHPlaylist.mpd?a=123",
                    "hls_url": "https://v.redd.it/pxtf7mx2xqzg1/HLSPlaylist.m3u8?a=123",
                }
            },
        })
    }

    #[test]
    fn submission_video_media_parses() {
        let submission: Submission =
            serde_json::from_value(video_submission_json()).expect("could not parse submission");

        assert_eq!(submission.id.as_deref(), Some("1t6gdp8"));
        assert_eq!(
            submission.video_url(),
            Some("https://v.redd.it/pxtf7mx2xqzg1/DASHPlaylist.mpd?a=123")
        );
    }

    #[test]
    fn submission_without_video_media_has_no_video_url() {
        let submission: Submission = serde_json::from_value(serde_json::json!({
            "subreddit": "europe",
            "title": "A street in Bologna",
            "ups": 1,
            "upvote_ratio": 1.0,
            "selftext": "",
            "url": "https://i.redd.it/gvjukykex8pf1.jpeg",
            "id": "1nh144u",
            "media": null,
            "secure_media": null,
            "crosspost_parent_list": null,
        }))
        .expect("could not parse submission");

        assert_eq!(submission.video_url(), None);
    }

    #[test]
    fn crosspost_video_url_resolves_through_parent() {
        let mut parent = video_submission_json();
        parent["id"] = serde_json::json!("1t5abcd");

        let submission: Submission = serde_json::from_value(serde_json::json!({
            "subreddit": "interestingasfuck",
            "title": "Offline WoW installer is finally complete",
            "ups": 1,
            "upvote_ratio": 1.0,
            "selftext": "",
            "url": "https://v.redd.it/pxtf7mx2xqzg1",
            "id": "1t6xyz9",
            "secure_media": null,
            "crosspost_parent_list": [parent],
        }))
        .expect("could not parse submission");

        assert_eq!(
            submission.video_url(),
            Some("https://v.redd.it/pxtf7mx2xqzg1/DASHPlaylist.mpd?a=123")
        );
    }

    #[test]
    fn submission_video_url_falls_back_from_secure_media_to_media() {
        let mut json = video_submission_json();
        json["secure_media"] = serde_json::json!({});
        json["media"] = serde_json::json!({
            "reddit_video": {
                "fallback_url": "https://v.redd.it/pxtf7mx2xqzg1/DASH_720.mp4?source=fallback",
            }
        });

        let submission: Submission =
            serde_json::from_value(json).expect("could not parse submission");

        assert_eq!(
            submission.video_url(),
            Some("https://v.redd.it/pxtf7mx2xqzg1/DASH_720.mp4?source=fallback")
        );
    }

    #[test]
    fn submission_video_url_falls_back_to_hls_and_fallback() {
        let mut json = video_submission_json();
        json["secure_media"]["reddit_video"]["dash_url"] = serde_json::Value::Null;

        let submission: Submission =
            serde_json::from_value(json).expect("could not parse submission");
        assert_eq!(
            submission.video_url(),
            Some("https://v.redd.it/pxtf7mx2xqzg1/HLSPlaylist.m3u8?a=123")
        );

        let mut json = video_submission_json();
        json["secure_media"]["reddit_video"] = serde_json::json!({
            "fallback_url": "https://v.redd.it/pxtf7mx2xqzg1/DASH_720.mp4?source=fallback",
        });

        let submission: Submission =
            serde_json::from_value(json).expect("could not parse submission");
        assert_eq!(
            submission.video_url(),
            Some("https://v.redd.it/pxtf7mx2xqzg1/DASH_720.mp4?source=fallback")
        );
    }
}
