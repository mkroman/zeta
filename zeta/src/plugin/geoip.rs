//! Looks up the geolocation and network ownership of IP addresses and hostnames.
//!
//! The `.geoip <domain-or-ip>` command queries the ip2location.io API and replies with the
//! autonomous system (name and ASN), country, region, and city of the address — or "No location
//! data available" when the API reports none. Domains are first resolved through the bot's
//! shared DNS resolver and only the first resolved address is geolocated.
//!
//! The API key is set in `[plugins.geoip]`, falling back to the `GEOIP_API_KEY` environment
//! variable; a missing key fails plugin initialization and the plugin is skipped at startup.

use std::fmt::Display;

use argh::{ArgsInfo, FromArgs};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use url::Host;

use crate::{http, plugin::prelude::*};

const BASE_URL: &str = "https://api.ip2location.io";

/// Settings for the geoip plugin, from its `[plugins.geoip]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    /// The ip2location.io API key.
    ///
    /// Falls back to the `GEOIP_API_KEY` environment variable when unset.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// The `.geoip` command.
const GEOIP: CommandSpec = CommandSpec::with_args::<Opts>(
    ".geoip",
    "Look up the geolocation of a domain or IP",
);

/// The geoip plugin: geolocates addresses on behalf of `.geoip` commands.
pub struct GeoIp {
    /// The HTTP client used for API requests.
    pub client: reqwest::Client,
    api_key: String,
}

/// A successful geolocation lookup, formatting one reply line with the reported details.
#[must_use]
#[derive(Default)]
pub struct LookupResult(IpInfo);

/// Errors that can occur while geolocating an address.
#[derive(Debug, Error)]
pub enum Error {
    /// The API request failed, or the response was unusable.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// The domain could not be resolved to an IP address.
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] hickory_resolver::net::NetError),
    /// The domain resolved no IP addresses at all.
    #[error("domain resolved no records")]
    NoDomainRecords,
    /// The input is neither a domain nor an IP address.
    #[error("invalid input")]
    InvalidInput,
}

/// Look up the geolocation of a domain or IP address.
#[derive(FromArgs, ArgsInfo, Debug)]
pub struct Opts {
    /// the domain or IP address to look up
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
impl Plugin<Context> for GeoIp {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<GeoIp, ZetaError> {
        subscriptions.command(GEOIP);
        let api_key = resolve_secret(settings.api_key.as_deref(), "GEOIP_API_KEY")?;
        let client = http::client::builder(&ctx.config.http)
            .build()
            .map_err(plugin_err)?;

        Ok(GeoIp { client, api_key })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let Some(opts) =
            parse_args_or_usage::<Opts>(client, command, |line: &str| reply("GeoIP", line))?
        else {
            return Ok(());
        };

        match self.resolve(&opts.name).await {
            Ok(result) => {
                for line in result.to_string().lines() {
                    client.send_privmsg(command.channel(), line)?;
                }
            }
            Err(err) => {
                warn!(?err, name = %opts.name, "geoip lookup failed");

                client.send_privmsg(command.channel(), reply("GeoIP", err))?;
            }
        }

        Ok(())
    }
}

impl Display for IpInfo {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();

        if !self.asn_name.is_empty() {
            parts.push(format!("AS:{RESET} {}", self.asn_name));
        }

        if !self.asn.is_empty() {
            parts.push(format!("ASN:{RESET} {}", self.asn));
        }

        if !self.country_name.is_empty() {
            parts.push(format!("Country:{RESET} {}", self.country_name));
        }

        if !self.region_name.is_empty() {
            parts.push(format!("Region:{RESET} {}", self.region_name));
        }

        if !self.city_name.is_empty() {
            parts.push(format!("City:{RESET} {}", self.city_name));
        }

        if parts.is_empty() {
            write!(fmt, "No location data available")
        } else {
            // Every part ends in the reply color, so a plain space between parts keeps the
            // separator cyan.
            write!(fmt, "{}", parts.join(" "))
        }
    }
}

impl Display for LookupResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info = &self.0;
        let ip = &info.ip;

        write!(fmt, "{}({RESET}{ip}{COLOR}): {info}", reply_prefix("GeoIP"))
    }
}

impl GeoIp {
    async fn resolve_domain(domain: &str) -> Result<String, Error> {
        match Host::parse(domain) {
            Ok(Host::Ipv4(addr)) => Ok(addr.to_string()),
            Ok(Host::Ipv6(addr)) => Ok(addr.to_string()),
            Ok(Host::Domain(domain)) => {
                let resolver = crate::dns::resolver();
                // `dns.question.name` is the OpenTelemetry convention for the name a DNS query
                // asks for: https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
                debug!(dns.question.name = %domain, "resolving domain");

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

    /// Geolocates `name`, a domain or IP address, through the ip2location.io API.
    ///
    /// Domains are first resolved through the bot's shared DNS resolver, using the first
    /// resolved address.
    ///
    /// # Errors
    ///
    /// Returns an [`enum@Error`] when the input is neither a domain nor an IP address, the domain
    /// cannot be resolved, the request fails, or the response cannot be parsed.
    pub async fn resolve(&self, name: &str) -> Result<LookupResult, Error> {
        let ip = GeoIp::resolve_domain(name).await?;
        let params = [
            ("ip", ip.as_str()),
            ("key", &self.api_key),
            ("format", "json"),
        ];
        let request = self.client.get(BASE_URL).query(&params);
        let response = request.send().await.map_err(http::ApiError::from)?;
        let info: IpInfo = http::parse_response(response)
            .await
            .inspect_err(|error| error!(%name, %error, "error when querying for geoip"))?;
        // `dns.answers` is the OpenTelemetry convention for the addresses a DNS lookup resolved
        // to, which is what `info.ip` is — the address the queried name resolved to (the input
        // itself when it already was an IP address):
        // https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
        info!(dns.answers = %info.ip, "resolved");

        Ok(LookupResult(info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.api_key.is_none());
        }
        deserialize: {
            "api_key": "secret",
        } assert: {
            assert_eq!(settings.api_key.as_deref(), Some("secret"));
        }
    }
}
