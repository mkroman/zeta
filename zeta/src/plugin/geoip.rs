use std::env;
use std::fmt::Display;

use argh::FromArgs;
use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use reqwest::redirect::Policy;
use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, error, info};
use url::Host;

use crate::Error as ZetaError;
use crate::command::Command as ZetaCommand;
use crate::consts::HTTP_TIMEOUT;

use super::{Author, Name, Plugin, Version};

const BASE_URL: &str = "https://api.ip2location.io";

pub struct GeoIp {
    pub client: reqwest::Client,
    api_key: String,
    command: ZetaCommand,
}

#[derive(Default)]
pub struct LookupResult(IpInfo);

#[derive(Debug, Error)]
pub enum Error {
    #[error("could not parse arguments")]
    ParseArguments,
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] reqwest::Error),
    #[error("http request failed")]
    Request(#[from] reqwest::Error),
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] hickory_resolver::ResolveError),
    #[error("domain resolved no records")]
    NoDomainRecords,
    #[error("invalid input")]
    InvalidInput,
}

/// Geographical lookup utility based on IP address
#[derive(FromArgs, Debug)]
pub struct Opts {
    /// the name of the domain to look to look up
    #[argh(positional)]
    name: String,
}

/// Represents geographical and network information for an IP address.
/// This struct is designed to be deserialized from a JSON response
/// providing details like location, timezone, and ASN data.
#[allow(unused)]
#[derive(Debug, Deserialize, Default)]
pub struct IpInfo {
    /// The IP address.
    pub ip: String,
    /// The two-letter ISO 3166-1 alpha-2 country code.
    pub country_code: String,
    /// The name of the country.
    pub country_name: String,
    /// The name of the region or state.
    pub region_name: String,
    /// The name of the city.
    pub city_name: String,
    /// The geographical latitude.
    pub latitude: f64,
    /// The geographical longitude.
    pub longitude: f64,
    /// The postal or zip code.
    pub zip_code: String,
    /// The time zone offset from UTC.
    pub time_zone: String,
    /// The Autonomous System Number (ASN).
    pub asn: String,
    /// The name of the entity that owns the Autonomous System.
    #[serde(rename = "as")]
    pub asn_name: String,
    /// Indicates whether the IP address is a known proxy.
    pub is_proxy: bool,
}

#[async_trait]
impl Plugin for GeoIp {
    fn new() -> GeoIp {
        let api_key =
            env::var("GEOIP_API_KEY").expect("missing GEOIP_API_KEY environment variable");

        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("could not build http client");

        let command = ZetaCommand::new(".geoip");

        GeoIp {
            client,
            api_key,
            command,
        }
    }

    fn name() -> Name {
        Name("geoip")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command
            && let Some(args) = self.command.parse(message)
        {
            let sub_args = shlex::split(args)
                .ok_or_else(|| ZetaError::PluginError(Box::new(Error::ParseArguments)))?;
            let sub_args_ref = sub_args.iter().map(String::as_ref).collect::<Vec<_>>();

            match Opts::from_args(&[".geoip"], &sub_args_ref) {
                Ok(opts) => match self.resolve(&opts.name).await {
                    Ok(result) => {
                        for line in result.to_string().lines() {
                            client
                                .send_privmsg(channel, line)
                                .map_err(ZetaError::IrcClientError)?;
                        }
                    }
                    Err(err) => {
                        client
                            .send_privmsg(
                                channel,
                                format!("\x0310>\x03\x02 GeoIP:\x02\x0310 {err}"),
                            )
                            .map_err(ZetaError::IrcClientError)?;
                    }
                },
                Err(err) => {
                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 GeoIP:\x02\x0310 {}", err.output),
                        )
                        .map_err(ZetaError::IrcClientError)?;
                }
            }
        }

        Ok(())
    }
}

impl Display for IpInfo {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();

        if !self.asn_name.is_empty() {
            parts.push(format!("AS:\x03 {}\x0310", self.asn_name));
        }

        if !self.asn.is_empty() {
            parts.push(format!("ASN:\x03 {}\x0310", self.asn));
        }

        if !self.country_name.is_empty() {
            parts.push(format!("Country:\x03 {}\x0310", self.country_name));
        }

        if !self.region_name.is_empty() {
            parts.push(format!("Region:\x03 {}\x0310", self.region_name));
        }

        if !self.city_name.is_empty() {
            parts.push(format!("City:\x03 {}\x0310", self.city_name));
        }

        if parts.is_empty() {
            write!(fmt, "No location data available")
        } else {
            write!(fmt, "{}", parts.join("\x0310 "))
        }
    }
}

impl Display for LookupResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info = &self.0;
        let ip = &info.ip;

        write!(
            fmt,
            "\x0310>\x03\x02 GeoIP\x02\x0310 (\x0f{ip}\x0310): {info}"
        )
    }
}

impl GeoIp {
    async fn resolve_domain(domain: &str) -> Result<String, Error> {
        match Host::parse(domain) {
            Ok(Host::Ipv4(addr)) => Ok(addr.to_string()),
            Ok(Host::Ipv6(addr)) => Ok(addr.to_string()),
            Ok(Host::Domain(domain)) => {
                let resolver = crate::dns::resolver();
                debug!(%domain, "resolving domain");

                resolver
                    .lookup_ip(domain)
                    .await
                    .map_err(Error::Resolve)
                    .map(|lookup| lookup.iter().next().ok_or_else(|| Error::NoDomainRecords))?
                    .map(|ip| ip.to_string())
            }
            Err(_) => Err(Error::InvalidInput),
        }
    }

    pub async fn resolve(&self, name: &str) -> Result<LookupResult, Error> {
        let ip = GeoIp::resolve_domain(name).await?;
        let params = [
            ("ip", ip.as_str()),
            ("key", &self.api_key),
            ("format", "json"),
        ];
        let request = self.client.get(BASE_URL).query(&params);
        let response = request.send().await?;

        match response.error_for_status() {
            Ok(response) => {
                info!(response = %response.status(), "resolved");
                let info: IpInfo = response.json().await.map_err(Error::Deserialize)?;
                Ok(LookupResult(info))
            }
            Err(err) => {
                error!(?err, %name, "error when querying for geoip");

                Err(err.into())
            }
        }
    }
}
