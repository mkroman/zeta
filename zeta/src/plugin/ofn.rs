//! Old Fucking News, aka URL post history.
//!
//! This plugin tracks URLs that are posted in channels and notifies when the URL has been posted
//! before.

use std::borrow::ToOwned;

use irc::client::prelude::Prefix;
use tracing::{debug, error};
use url::Url;

use crate::{
    plugin::{ofn::model::InsertUrlRecord, prelude::*},
    url::ExtractUrlsExt,
};

mod model;

use model::UrlRecord;

const IGNORED_QUERY_PARAMS: [&str; 5] = [
    "utm_medium",
    "utm_source",
    "utm_content",
    "utm_campaign",
    "utm_term",
];

pub struct Ofn {}

/// Holds information about the origin of a URL, i.e. where it was posted and by whom.
#[derive(Debug)]
#[non_exhaustive]
pub struct ChannelMessageOrigin {
    /// The name of the channel.
    channel: String,
    /// The identifier of the network.
    network: String,
    /// The nickname of the sender.
    nickname: String,
    /// The username of the sender.
    username: String,
    /// The hostname of the sender.
    hostname: String,
}

pub struct Report {
    /// List of URL records that were found in the database.
    found: Vec<UrlRecord>,
    /// List of URLs that weren't found and thus inserted into the database.
    inserted: Vec<InsertUrlRecord>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not query database: {0}")]
    QueryDatabase(#[source] sqlx::Error),
    #[error("could not insert url in database: {0}")]
    InsertUrl(#[source] sqlx::Error),
    #[error("can't insert url with no host")]
    InsertUrlNoHost,
}

#[async_trait]
impl Plugin<Context> for Ofn {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Self::new())
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "ofn".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(ref channel, ref msg) = message.command else {
            return Ok(());
        };

        let Some(Prefix::Nickname(ref nickname, ref username, ref hostname)) = message.prefix
        else {
            return Ok(());
        };

        let urls: Vec<Url> = msg.urls().collect();
        let origin = ChannelMessageOrigin {
            channel: channel.to_owned(),
            // TODO: support multiple networks
            network: "irc.rwx.im:6697".to_owned(),
            nickname: nickname.to_owned(),
            username: username.to_owned(),
            hostname: hostname.to_owned(),
        };

        match self.process_urls(ctx, &origin, &urls).await {
            Ok(report) => {
                for url in report.found {
                    let nick = url.nickname;
                    let created_at = url.created_at;

                    client.send_privmsg(
                        channel,
                        format!("{nickname}: OFN - posted by {nick} @ {created_at}"),
                    )?;
                }

                if !report.inserted.is_empty() {
                    debug!("inserted {} new urls", report.inserted.len());
                }
            }
            Err(error) => {
                client.send_privmsg(channel, error)?;
            }
        }

        Ok(())
    }
}

impl Ofn {
    pub const fn new() -> Ofn {
        Ofn {}
    }

    /// Processes the given list of `urls` with the associated `origin` by querying them from the
    /// database and inserting any of the URLs that aren't already present.
    ///
    /// Returns a [`Report`] that contains information about URLs that were already known and URLs
    /// that were added to the database.
    async fn process_urls(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin,
        urls: &[Url],
    ) -> Result<Report, Error> {
        let mut found = vec![];
        let mut inserted = vec![];

        for url in urls {
            if let Some(record) = self.find_url(ctx, origin, url).await? {
                found.push(record);
            } else {
                match self.insert_url(ctx, origin, url).await {
                    Ok(insert) => {
                        debug!(?origin, ?url, "inserted url");
                        inserted.push(insert);
                    }
                    Err(error) => {
                        error!("could not insert url: {error}");
                    }
                }
            }
        }

        Ok(Report { found, inserted })
    }

    /// Find an return a [`UrlRecord`] for the given `url` and associated `origin` if present in the
    /// database.
    ///
    /// Returns `Ok(None)` if not present.
    async fn find_url(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin,
        url: &Url,
    ) -> Result<Option<UrlRecord>, Error> {
        let host = url.host_str().ok_or_else(|| Error::InsertUrlNoHost)?;

        let result = sqlx::query_file_as!(
            UrlRecord,
            "queries/find_url_record.sql",
            url.scheme(),
            host,
            url.port_or_known_default().map(i32::from),
            url.path(),
            url.query(),
            origin.channel,
            origin.network
        )
        .fetch_optional(&ctx.db)
        .await
        .map_err(Error::QueryDatabase)?;

        Ok(result)
    }

    /// Inserts the given `url` into the database with the associated `origin`.
    ///
    /// Returns the [`InsertUrlRecord`] used for the operation if the insert was successful used for
    /// the operation if the insert was successful.
    async fn insert_url(
        &self,
        ctx: &Context,
        origin: &ChannelMessageOrigin,
        url: &Url,
    ) -> Result<InsertUrlRecord, Error> {
        let host = url
            .host_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| Error::InsertUrlNoHost)?;

        let insert = InsertUrlRecord {
            scheme: url.scheme().to_owned(),
            host,
            port: url.port_or_known_default().map(i32::from),
            path: url.path().to_owned(),
            query: url.query().map(ToOwned::to_owned),
            fragment: url.fragment().map(ToOwned::to_owned),
            nickname: origin.nickname.clone(),
            username: origin.username.clone(),
            hostname: origin.hostname.clone(),
            channel: origin.channel.clone(),
            network_id: origin.network.clone(),
        };

        debug!(?insert, "inserting url to database");

        sqlx::query_file!(
            "queries/insert_url_record.sql",
            insert.scheme,
            insert.host,
            insert.port,
            insert.path,
            insert.query,
            insert.fragment,
            insert.nickname,
            insert.username,
            insert.hostname,
            insert.channel,
            insert.network_id
        )
        .fetch_one(&ctx.db)
        .await
        .map_err(Error::InsertUrl)?;

        Ok(insert)
    }
}
