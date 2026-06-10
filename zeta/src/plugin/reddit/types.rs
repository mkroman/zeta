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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parse_comments_listing_json() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/reddit/comments/1niz1ru.json");
        let text = std::fs::read_to_string(path).unwrap();
        let (item1, item2): (Item, Item) = crate::utils::parse_json(&text)?;

        assert!(matches!(item1, Item::Listing(_)));
        assert!(matches!(item2, Item::Listing(_)));

        Ok(())
    }
}
