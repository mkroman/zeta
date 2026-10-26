//! The data model of the Reddit API responses and of the links the URL classifier yields.

use serde::Deserialize;

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
    CommentsPage {
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

/// An item in a Reddit API listing response, tagged by the API's `kind` discriminator.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Item {
    /// A comment (`t1`).
    #[serde(rename = "t1")]
    Comment(Comment),
    /// A subreddit (`t5`).
    #[serde(rename = "t5")]
    Subreddit(Subreddit),
    /// A submission (`t3`).
    #[serde(rename = "t3")]
    Submission(Submission),
    /// A listing of other items.
    #[serde(rename = "Listing")]
    Listing(Listing),
    /// Any other item kind, kept as raw JSON.
    #[serde(untagged)]
    Other(serde_json::Value),
}

/// A listing of items, the container every API listing response wraps its children in.
#[derive(Debug, Deserialize)]
pub struct Listing {
    /// The items in the listing.
    pub children: Vec<Item>,
}

/// Details about a submission.
#[derive(Debug, Deserialize)]
pub struct Submission {
    /// The subreddit the submission is in.
    pub subreddit: String,
    /// The title of the submission.
    pub title: String,
    /// Number of upvotes.
    pub ups: u32,
    /// Upvote ratio.
    pub upvote_ratio: f32,
    /// The main selftext.
    pub selftext: String,
    /// The URL of the submission — the outlink, or the Reddit URL for a self post.
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
    pub crosspost_parent_list: Option<Vec<Self>>,
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
                .and_then(Self::reddit_video)
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
pub struct Comment {
    /// The unique base36 id of the comment.
    pub id: String,
    /// The comment body.
    pub body: String,
    /// The comment body as HTML.
    pub body_html: String,
    /// The subreddit the comment is in.
    pub subreddit: String,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parse_comments_listing_json() -> Result<(), Box<dyn std::error::Error>> {
        use tracing::error;
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/comments/1niz1ru.json");
        let text = std::fs::read_to_string(path).unwrap();
        let jd = &mut serde_json::Deserializer::from_str(&text);
        let (item1, item2): (Item, Item) =
            serde_path_to_error::deserialize(jd).inspect_err(|err| {
                let body = text
                    .chars()
                    .take(crate::LOGGED_BODY_LENGTH)
                    .collect::<String>();

                error!(?err, body = %body, "could not parse comments response");
            })?;

        // The response is the submission listing followed by the comment listing.
        let Item::Listing(submission_listing) = &item1 else {
            panic!("the first item should be a listing");
        };
        assert!(matches!(
            submission_listing.children.as_slice(),
            [Item::Submission(_)]
        ));

        let Item::Listing(comment_listing) = &item2 else {
            panic!("the second item should be a listing");
        };
        assert_eq!(comment_listing.children.len(), 9);
        assert!(
            comment_listing
                .children
                .iter()
                .all(|child| matches!(child, Item::Comment(_)))
        );

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
