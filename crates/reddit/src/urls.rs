//! URL classification for reddit.com and redd.it links.

use std::fmt::Write;

use url::Url;

use crate::Link;

/// The `i.redd.it` hostname, serving direct image links.
const I_REDD_IT_HOST: &str = "i.redd.it";

/// The `oauth.reddit.com` hostname.
const OAUTH_REDDIT_COM_HOST: &str = "oauth.reddit.com";

/// The `old.reddit.com` hostname.
const OLD_REDDIT_COM_HOST: &str = "old.reddit.com";

/// The `preview.redd.it` hostname, serving preview images.
const PREVIEW_REDD_IT_HOST: &str = "preview.redd.it";

/// The `redd.it` short-link hostname.
const REDD_IT_HOST: &str = "redd.it";

/// The `reddit.com` hostname.
const REDDIT_COM_HOST: &str = "reddit.com";

/// The `v.redd.it` hostname, serving videos.
const V_REDD_IT_HOST: &str = "v.redd.it";

/// The `www.reddit.com` hostname.
const WWW_REDDIT_COM_HOST: &str = "www.reddit.com";

/// The hosts [`classify_reddit_url`] recognizes.
///
/// Plugins subscribe to these hosts instead of repeating the list, so a host the classifier
/// accepts is never left unsubscribed — or subscribed without being handled.
pub const URL_HOSTS: &[&str] = &[
    I_REDD_IT_HOST,
    OAUTH_REDDIT_COM_HOST,
    OLD_REDDIT_COM_HOST,
    PREVIEW_REDD_IT_HOST,
    REDD_IT_HOST,
    REDDIT_COM_HOST,
    V_REDD_IT_HOST,
    WWW_REDDIT_COM_HOST,
];

/// Attempts to parse the given `url` as a reddit URL.
#[must_use]
pub fn classify_reddit_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some(V_REDD_IT_HOST | I_REDD_IT_HOST | PREVIEW_REDD_IT_HOST | REDD_IT_HOST) => {
            classify_redd_it_url(url)
        }
        Some(
            REDDIT_COM_HOST | WWW_REDDIT_COM_HOST | OLD_REDDIT_COM_HOST | OAUTH_REDDIT_COM_HOST,
        ) => classify_reddit_com_url(url),
        _ => None,
    }
}

/// Parses reddit.com URLs
#[must_use]
pub fn classify_reddit_com_url(url: &Url) -> Option<Link> {
    match zeta_url::path_segments(url)?.as_slice() {
        // Direct link to a subreddit
        //
        // Parameters: `/r/<subreddit>`
        // Example: `/r/worldnews`
        ["r", subreddit] => Some(Link::Subreddit((*subreddit).to_string())),
        // Direct link link to a submission page (i.e. full thread and comments)
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug][/]`
        // Example: `/r/nottheonion/comments/1u7eqe9/microsofts_new_outlook_takes_10_seconds_to_do/`
        ["r", subreddit, "comments", id] | ["r", subreddit, "comments", id, _] => {
            Some(Link::Submission {
                id: (*id).to_string(),
                subreddit: (*subreddit).to_string(),
            })
        }
        // Direct link to a comment and its children for a submission
        //
        // Parameters: `/r/<subreddit>/comments/<id>/[title_slug]/<comment_id>[/]`
        // Example: `/r/AskElectronics/comments/1u7evrr/what_is_the_best_course_of_action_for_wrong_width/orzo90y/`
        ["r", subreddit, "comments", submission_id, _, comment_id] => Some(Link::Comment {
            id: (*comment_id).to_string(),
            submission: (*submission_id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // /r/<subreddit>/s/<id>[/]
        ["r", subreddit, "s", id] => Some(Link::Shortened {
            id: (*id).to_string(),
            subreddit: (*subreddit).to_string(),
        }),
        // /comments/<id>[/]
        ["comments", id] => Some(Link::CommentsPage {
            id: (*id).to_string(),
        }),
        // /gallery/<id>
        ["gallery", id] => Some(Link::Gallery((*id).to_string())),
        // /video/<id>
        ["video", id] => Some(Link::Video((*id).to_string())),
        // /user/<name>[/]
        ["user", username] => Some(Link::User((*username).to_string())),
        _ => None,
    }
}

/// Parses redd.it URLs
fn classify_redd_it_url(url: &Url) -> Option<Link> {
    match url.host_str() {
        Some(I_REDD_IT_HOST) => Some(Link::Image(url.path().to_string())),
        Some(V_REDD_IT_HOST) => url
            .path_segments()
            .and_then(|mut segment| segment.next())
            .map(|id| Link::Video(id.to_string())),
        Some(PREVIEW_REDD_IT_HOST) => {
            let mut request_uri = url.path().to_string();

            if let Some(query) = url.query() {
                // Writing to a `String` cannot fail, so the write result needs no handling.
                let _ = write!(request_uri, "?{query}");
            }

            Some(Link::Preview(request_uri))
        }
        // `redd.it/<id>` is the short link for a submission; resolving it is the API's job, so
        // it is classified as the comments page it redirects to.
        Some(REDD_IT_HOST) => match zeta_url::path_segments(url)?.as_slice() {
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

    /// Every host the plugin subscribes to must be recognized by the classifier.
    #[test]
    fn url_hosts_cover_the_classified_hosts() {
        for host in URL_HOSTS {
            let path = match *host {
                REDD_IT_HOST => "/1nh144u",
                I_REDD_IT_HOST | V_REDD_IT_HOST | PREVIEW_REDD_IT_HOST => "/test.jpg",
                REDDIT_COM_HOST
                | WWW_REDDIT_COM_HOST
                | OLD_REDDIT_COM_HOST
                | OAUTH_REDDIT_COM_HOST => "/r/rust",
                other => panic!("no test path for {other}"),
            };

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
