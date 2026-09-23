//! Instagram's anonymous GraphQL API.
//!
//! Mirrors the logged-out flow `yt-dlp` uses: a first request to the site's front page provides
//! the CSRF token (sent both as a cookie and in the `X-CSRFToken` header) and the LSD token
//! embedded in one of its scripts, which a POST to the GraphQL endpoint then needs alongside the
//! numeric media id. The response carries the media's author and caption for posts that are
//! anonymously accessible. The tokens are stable across requests, so they are fetched once and
//! reused for subsequent media.
//!
//! Instagram rotates the `doc_id` identifying the GraphQL query; the one used here is the one
//! pinned by `yt-dlp`, whose downloads depend on it.

use serde::Deserialize;
use tracing::debug;
use url::form_urlencoded::Serializer;
use wreq::header::{COOKIE, SET_COOKIE};

use crate::error::WreqError;

use super::meta::{MediaDetails, shortcode_to_media_pk};

/// The site URL whose response provides the session tokens.
const INSTAGRAM_FRONT_PAGE: &str = "https://www.instagram.com/";

/// The GraphQL endpoint.
const GRAPHQL_ENDPOINT: &str = "https://www.instagram.com/api/graphql";

/// The app id Instagram's web client identifies itself with.
const IG_APP_ID: &str = "936619743392459";

/// The `doc_id` selecting the logged-out post query, as pinned by `yt-dlp`.
const DOC_ID: &str = "27130156389949648";

/// The friendly name of the logged-out post query.
const FRIENDLY_NAME: &str = "PolarisLoggedOutDesktopWWWPostRootContentQuery";

/// The tokens the front page provides for a GraphQL request.
///
/// They are stable across requests, so they are fetched once and reused for subsequent media.
#[derive(Clone, Debug)]
pub struct SessionTokens {
    /// The LSD token embedded in the scripts of the front page.
    pub(crate) lsd: String,
    /// The CSRF token carried by the `csrftoken` cookie the front page sets, if any.
    pub(crate) csrf: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The request failed, or the response body could not be read.
    #[error("request error: {0}")]
    Request(WreqError),
    /// The server responded with an unsuccessful status code.
    #[error("the graphql request failed with status {0}")]
    Status(wreq::StatusCode),
    /// The front page did not provide the LSD token the GraphQL request needs.
    #[error("the front page did not provide an lsd token")]
    MissingLsdToken,
    /// The media is not anonymously accessible, or the response could not be understood.
    #[error("the media is not anonymously accessible")]
    Gated,
}

impl From<wreq::Error> for Error {
    /// Wraps `error` in a [`WreqError`], which redacts the request URL.
    fn from(error: wreq::Error) -> Self {
        Self::Request(error.into())
    }
}

/// The GraphQL response for a logged-out media request.
#[derive(Debug, Default, Deserialize)]
struct GraphqlResponse {
    data: Option<GraphqlData>,
}

#[derive(Debug, Default, Deserialize)]
struct GraphqlData {
    xig_polaris_media: Option<PolarisMedia>,
}

#[derive(Debug, Default, Deserialize)]
struct PolarisMedia {
    if_not_gated_logged_out: Option<MediaItem>,
}

#[derive(Debug, Default, Deserialize)]
struct MediaItem {
    #[serde(default)]
    caption: Option<Caption>,
    #[serde(default)]
    user: Option<User>,
}

#[derive(Debug, Default, Deserialize)]
struct Caption {
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct User {
    username: Option<String>,
}

/// Fetches the details of the media with the given shortcode through the anonymous GraphQL API.
///
/// The `tokens` come from [`fetch_session`] and are expected to be reused across media. When
/// `session_cookie` is configured, it is sent along, so media the session can read is
/// summarized.
///
/// # Errors
///
/// Returns an error if the media is not anonymously accessible or the request fails.
pub async fn fetch_media_details(
    client: &wreq::Client,
    shortcode: &str,
    tokens: &SessionTokens,
    session_cookie: Option<&str>,
) -> Result<MediaDetails, Error> {
    let media_pk = shortcode_to_media_pk(shortcode).ok_or(Error::Gated)?;

    let media_details = graphql(client, tokens, session_cookie, media_pk).await?;

    debug!(%shortcode, "fetched media details through the graphql api");

    Ok(media_details)
}

/// Requests the front page and returns the tokens it sets up for the GraphQL request.
///
/// The CSRF token is carried by the `csrftoken` cookie the front page sets.
///
/// # Errors
///
/// Returns an error if the front page could not be fetched or carried no LSD token.
pub async fn fetch_session(
    client: &wreq::Client,
    session_cookie: Option<&str>,
) -> Result<SessionTokens, Error> {
    let mut request = client.get(INSTAGRAM_FRONT_PAGE);

    if let Some(session_cookie) = session_cookie {
        request = request.header(COOKIE, format!("sessionid={session_cookie}"));
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        return Err(Error::Status(response.status()));
    }

    let csrf = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|cookie| cookie.to_str().ok())
        .find_map(extract_csrftoken);

    let body = response.text().await?;
    let lsd = extract_lsd_token(&body).ok_or(Error::MissingLsdToken)?;

    Ok(SessionTokens { lsd: lsd.to_string(), csrf })
}

/// Returns the `csrftoken` cookie value carried by a `Set-Cookie` header value, if any.
fn extract_csrftoken(cookie: &str) -> Option<String> {
    let pair = cookie.split(';').next()?;
    let value = pair.strip_prefix("csrftoken=")?;
    (!value.is_empty()).then(|| value.to_string())
}

/// Returns the LSD token embedded in the scripts of the front page, if present.
#[must_use]
fn extract_lsd_token(body: &str) -> Option<&str> {
    const PREFIX: &str = r#"["LSD",[],{"token":""#;

    let start = body.find(PREFIX)? + PREFIX.len();
    let rest = &body[start..];
    let end = rest.find('"')?;

    (!rest[..end].is_empty()).then(|| &rest[..end])
}

/// POSTs the logged-out media query for the given numeric media id.
///
/// The CSRF token is sent both as a cookie and in the `X-CSRFToken` header, as `yt-dlp` does;
/// the configured session cookie is sent along, so media the session can read is summarized.
async fn graphql(
    client: &wreq::Client,
    tokens: &SessionTokens,
    session_cookie: Option<&str>,
    media_pk: u128,
) -> Result<MediaDetails, Error> {
    let mut cookies = Vec::new();

    if let Some(session_cookie) = session_cookie {
        cookies.push(format!("sessionid={session_cookie}"));
    }

    if let Some(csrf) = &tokens.csrf {
        cookies.push(format!("csrftoken={csrf}"));
    }

    let mut request = client
        .post(GRAPHQL_ENDPOINT)
        .header("Accept", "*/*")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("X-IG-App-ID", IG_APP_ID)
        .header("X-ASBD-ID", "359341")
        .header("X-IG-WWW-Claim", "0")
        .header("X-FB-Friendly-Name", FRIENDLY_NAME)
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Origin", "https://www.instagram.com")
        .header("Referer", "https://www.instagram.com/")
        .header("X-FB-LSD", &tokens.lsd);

    if let Some(csrf) = &tokens.csrf {
        request = request.header("X-CSRFToken", csrf);
    }

    if !cookies.is_empty() {
        request = request.header(COOKIE, cookies.join("; "));
    }

    let body = Serializer::new(String::new())
        .append_pair("lsd", &tokens.lsd)
        .append_pair("fb_api_caller_class", "RelayModern")
        .append_pair("fb_api_req_friendly_name", FRIENDLY_NAME)
        .append_pair("server_timestamps", "true")
        .append_pair("variables", &format!(r#"{{"media_id":{media_pk}}}"#))
        .append_pair("doc_id", DOC_ID)
        .finish();

    let response = request.body(body).send().await?;

    if !response.status().is_success() {
        return Err(Error::Status(response.status()));
    }

    let text = response.text().await?;

    match serde_json::from_str::<GraphqlResponse>(&text) {
        Ok(GraphqlResponse {
            data: Some(GraphqlData {
                xig_polaris_media:
                    Some(PolarisMedia {
                        if_not_gated_logged_out: Some(MediaItem { caption, user }),
                    }),
            }),
        }) => {
            let author = user.and_then(|user| user.username).filter(|username| !username.is_empty());
            let caption = caption.and_then(|caption| caption.text).filter(|text| !text.is_empty());

            if author.is_none() && caption.is_none() {
                return Err(Error::Gated);
            }

            Ok(MediaDetails { author, caption })
        }
        Ok(_) => Err(Error::Gated),
        Err(error) => {
            debug!(%error, "the graphql response was not the expected json");

            Err(Error::Gated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_lsd_token() {
        // The shape of the assignment found in the front page's scripts.
        let body = r#"some prefix ["LSD",[],{"token":"AVqe9xXyZ0"} other stuff"#;
        assert_eq!(extract_lsd_token(body), Some("AVqe9xXyZ0"));

        // Missing or empty tokens produce nothing.
        assert_eq!(extract_lsd_token("no token here"), None);
        assert_eq!(extract_lsd_token(r#"["LSD",[],{"token":""}"#), None);
    }

    #[test]
    fn test_extract_csrftoken() {
        let cookie = "csrftoken=abc123; expires=Sat, 01 Jan 2028 00:00:00 GMT; Path=/";
        assert_eq!(extract_csrftoken(cookie), Some("abc123".to_string()));

        assert_eq!(extract_csrftoken("sessionid=xyz"), None);
        assert_eq!(extract_csrftoken("csrftoken="), None);
    }

    #[test]
    fn test_parse_graphql_response() {
        let body = r#"{
            "data": {
                "xig_polaris_media": {
                    "if_not_gated_logged_out": {
                        "user": {"username": "user.name"},
                        "caption": {"text": "the caption"}
                    }
                }
            }
        }"#;
        let item = serde_json::from_str::<GraphqlResponse>(body)
            .unwrap()
            .data
            .unwrap()
            .xig_polaris_media
            .unwrap()
            .if_not_gated_logged_out
            .unwrap();

        assert_eq!(item.user.unwrap().username.as_deref(), Some("user.name"));
        assert_eq!(item.caption.unwrap().text.as_deref(), Some("the caption"));

        // A gated media has no logged-out payload.
        let body = r#"{"data": {"xig_polaris_media": {}}}"#;
        let item = serde_json::from_str::<GraphqlResponse>(body)
            .unwrap()
            .data
            .unwrap()
            .xig_polaris_media
            .unwrap()
            .if_not_gated_logged_out
            .is_none();
        assert!(item);
    }
}
