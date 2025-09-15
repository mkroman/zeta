use std::fmt::Write;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use url::Url;

use super::{Author, Name, Plugin, Version};
use crate::{Error as ZetaError, plugin};

pub struct Reddit {
    client: reqwest::Client,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
}

/// A link to a Reddit resource.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum Link {
    /// Link to a specific subreddit.
    Subreddit(String),
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
    /// Link to a users profile.
    ///
    /// E.g.: `/user/EcstaticYesterday605`
    User(String),
    /// Link to a video via v.redd.it.
    Video(String),
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
    /// A link that redirects the user to the comments page for the relevant submission.
    Comments {
        /// The submission id.
        id: String,
    },
}

#[async_trait]
impl Plugin for Reddit {
    fn new() -> Self {
        Reddit {
            client: plugin::build_http_client(),
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
            && let Some(urls) = extract_urls(user_message)
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
                let () = self.process_url(link, channel, client).await?;
            }
        }

        Ok(())
    }

    async fn process_url(&self, link: Link, channel: &str, client: &Client) -> Result<(), Error> {
        match link {
            Link::Subreddit(subreddit) => {
                client
                    .send_privmsg(channel, format!("fetching subreddit {subreddit}"))
                    .unwrap();
            }
            _ => {}
        }

        Ok(())
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

/// Parses youtube.com URLs
fn extract_urls(s: &str) -> Option<Vec<Url>> {
    let urls: Vec<Url> = s
        .split(' ')
        .filter(|word| word.to_ascii_lowercase().starts_with("http"))
        .filter_map(|word| Url::parse(word).ok())
        .collect();

    if urls.is_empty() { None } else { Some(urls) }
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
}
