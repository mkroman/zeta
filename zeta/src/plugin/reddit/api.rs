use std::fmt::Write;

use reqwest::{StatusCode, header::LOCATION};
use tracing::{debug, info};
use url::Url;

use super::{Error, Item, Link, REDDIT_BASE_URL, Reddit, Submission, Subreddit};

impl Reddit {
    /// Fetches and returns details about a given submission.
    pub(super) async fn submission(&self, article: &str) -> Result<Submission, Error> {
        debug!(%article, "requesting comments");
        let request = self
            .client
            .get(format!("{REDDIT_BASE_URL}/comments/{article}.json"));
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing comments");

                let text = response.text().await.map_err(Error::Reqwest)?;
                // The request returns 2 Listing objects
                let (submission, comments): (Item, Item) =
                    crate::utils::parse_json(&text).map_err(Error::DeserializeComments)?;
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
    pub(super) async fn subreddit_about_info(&self, name: &str) -> Result<Subreddit, Error> {
        let request = self
            .client
            .get(format!("{REDDIT_BASE_URL}/r/{name}/about.json"));
        debug!(%name, "requesting subreddit details");
        let response = request.send().await.map_err(Error::Reqwest)?;

        match response.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing subreddit");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let item: Item =
                    crate::utils::parse_json(&text).map_err(Error::DeserializeSubreddit)?;
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

    pub async fn resolve_shortened_link(&self, subreddit: &str, id: &str) -> Result<Link, Error> {
        let request = self
            .client
            .head(format!("{REDDIT_BASE_URL}/r/{subreddit}/s/{id}"));
        debug!(%subreddit, %id, "requesting short link to find redirect location");
        let response = request.send().await.map_err(Error::Reqwest)?;
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| Error::InvalidRedirect)?
            .to_str()
            .map_err(Error::RedirectUrlEncoding)?;

        debug!(%location, "parsing the url");
        let url = Url::parse(location).map_err(|_| Error::InvalidRedirect)?;

        match parse_reddit_com_url(&url) {
            Some(x @ (Link::Comment { .. } | Link::Submission { .. })) => Ok(x),
            _ => Err(Error::RedirectRedditLink),
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
}
