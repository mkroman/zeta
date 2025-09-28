use std::fmt::Write;

use reqwest::StatusCode;
use serde::Deserialize;
use tracing::{debug, error, info};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
    utils::Truncatable,
};

pub const REDDIT_BASE_URL: &str = "https://www.reddit.com";

pub struct Reddit {
    client: reqwest::Client,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
    #[error("could not deserialize comments json: {0}")]
    DeserializeComments(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("could not deserialize subreddit json: {0}")]
    DeserializeSubreddit(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("subreddit was not found")]
    SubredditNotFound,
    #[error("submission not found")]
    SubmissionNotFound,
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),
    #[error("could not deserialize response as it is in unexpected format")]
    InvalidResponse,
}

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

#[async_trait]
impl Plugin for Reddit {
    fn new() -> Self {
        Reddit {
            client: http::build_client(),
        }
    }

    fn name() -> Name {
        Name("reddit")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
            self.process_urls(&urls, channel, client).await.unwrap();
        }

        Ok(())
    }
}

impl Reddit {
    pub async fn process_urls(
        &self,
        urls: &Vec<Url>,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in urls {
            if let Some(link) = Reddit::parse_reddit_url(url) {
                self.process_url(link, channel, client).await?;
            }
        }

        Ok(())
    }

    async fn process_url(&self, link: Link, channel: &str, client: &Client) -> Result<(), Error> {
        match link {
            Link::Gallery(id) | Link::Comments { id } | Link::Submission { id, .. } => {
                match self.submission(&id).await {
                    Ok(submission) => {
                        let title = submission.title;
                        let subreddit = submission.subreddit;

                        client
                            .send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                            .unwrap();
                    }
                    Err(err) => {
                        client
                            .send_privmsg(
                                channel,
                                format!("\x0310> could not fetch submission details: {err}"),
                            )
                            .unwrap();
                    }
                }
            }
            Link::Comment { submission, .. } => match self.submission(&submission).await {
                Ok(submission) => {
                    let title = submission.title;
                    let subreddit = submission.subreddit;

                    client
                        .send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                        .unwrap();
                }
                Err(err) => {
                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310> could not fetch submission details: {err}"),
                        )
                        .unwrap();
                }
            },
            Link::Subreddit(subreddit) => match self.subreddit_about_info(&subreddit).await {
                Ok(subreddit) => {
                    let title = subreddit.title;
                    let description = subreddit.public_description.truncate_with_suffix(250, "…");

                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 {title}:\x02\x0310 {description}"),
                        )
                        .unwrap();
                }
                Err(err) => {
                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310> could not fetch subreddit details: {err}"),
                        )
                        .unwrap();
                }
            },
            _ => {}
        }

        Ok(())
    }

    /// Fetches and returns details about a given submission.
    async fn submission(&self, article: &str) -> Result<Submission, Error> {
        debug!(%article, "requesting comments");
        let request = self
            .client
            .get(format!("{REDDIT_BASE_URL}/comments/{article}.json"));
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing comments");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let jd = &mut serde_json::Deserializer::from_str(&text);
                // The request returns 2 Listing ojects
                let (submission, comments): (Item, Item) = serde_path_to_error::deserialize(jd)
                    .inspect_err(|err| error!(?err, %text, "could not parse comments response"))
                    .map_err(Error::DeserializeComments)?;
                debug!(x = ?(&submission, comments), "finished parsing item");

                match submission {
                    Item::Listing(listing) => listing
                        .children
                        .into_iter()
                        .find_map(|x| match x {
                            Item::Submission(s) => Some(s),
                            _ => None,
                        })
                        .ok_or_else(|| Error::InvalidResponse),
                    _ => Err(Error::InvalidResponse),
                }
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                info!(%article, %err, "could not fetch comments for article");

                Err(Error::SubmissionNotFound)
            }
            Err(err) => Err(Error::Http(err)),
        }
    }

    /// Fetches and returns details about the subreddit.
    #[tracing::instrument(skip(self))]
    async fn subreddit_about_info(&self, name: &str) -> Result<Subreddit, Error> {
        let request = self
            .client
            .get(format!("{REDDIT_BASE_URL}/r/{name}/about.json"));
        debug!(%name, "requesting subreddit details");
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing subreddit");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let jd = &mut serde_json::Deserializer::from_str(&text);
                let item: Item = serde_path_to_error::deserialize(jd)
                    .inspect_err(|err| error!(?err, %text, "could not parse subreddit response"))
                    .map_err(Error::DeserializeSubreddit)?;
                debug!(?item, "finished parsing item");

                match item {
                    Item::Subreddit(subreddit) => Ok(subreddit),
                    _ => Err(Error::InvalidResponse),
                }
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                info!(%name, %err, "subreddit not found");

                Err(Error::SubredditNotFound)
            }
            Err(err) => Err(Error::Http(err)),
        }
    }

    /// Attempts to parse the given `url` as a reddit URL.
    pub fn parse_reddit_url(url: &Url) -> Option<Link> {
        match url.host_str() {
            Some("v.redd.it" | "i.redd.it" | "preview.redd.it") => parse_redd_it_url(url),
            Some("reddit.com" | "www.reddit.com") => parse_reddit_com_url(url),
            _ => None,
        }
    }
}

/// Parses reddit.com URLs
fn parse_reddit_com_url(url: &Url) -> Option<Link> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // Link to a subreddit
        ["r", subreddit] | ["r", subreddit, ""] => Some(Link::Subreddit((*subreddit).to_string())),
        // /r/<subreddit>/comments/<id>/[title_slug][/]
        ["r", subreddit, "comments", id]
        | ["r", subreddit, "comments", id, _]
        | ["r", subreddit, "comments", id, _, ""] => Some(Link::Submission {
            id: (*id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // /r/<subreddit>/comments/<id>/[title_slug]/<comment_id>[/]
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

            assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
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

                assert_eq!(Reddit::parse_reddit_url(&url), expected);
            }
        }
    }

    #[test]
    fn parse_comments_listing_json() -> Result<(), Box<dyn std::error::Error>> {
        let text = include_str!("../../tests/fixtures/reddit/comments/1niz1ru.json");
        let jd = &mut serde_json::Deserializer::from_str(text);
        let (item1, item2): (Item, Item) = serde_path_to_error::deserialize(jd)
            .inspect_err(|err| error!(?err, %text, "could not parse comments response"))?;

        assert!(matches!(item1, Item::Listing(_)));
        assert!(matches!(item2, Item::Listing(_)));

        Ok(())
    }
}
