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
use std::sync::OnceLock;

use clap::Parser;
use hickory_resolver::{
    Resolver, TokioResolver,
    config::{CLOUDFLARE, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};
use url::Host;

use zeta_http as http;
use zeta_http::RequestError;
use zeta_plugin::prelude::*;

const BASE_URL: &str = "https://api.ip2location.io";

/// Settings for the geoip plugin, from its `[plugins.geoip]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The ip2location.io API key.
    ///
    /// Falls back to the `GEOIP_API_KEY` environment variable when unset.
    pub api_key: Option<String>,
}

/// The geoip plugin: geolocates addresses on behalf of `.geoip` commands.
pub struct GeoIp {
    /// The HTTP client used for API requests.
    pub client: reqwest::Client,
    api_key: String,
}

/// A successful geolocation lookup, formatting one reply line with the reported details.
#[must_use]
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
#[derive(Parser, Debug)]
pub struct Opts {
    /// the domain or IP address to look up
    name: String,
}

/// Represents geographical and network information for an IP address.
/// This struct is designed to be deserialized from a JSON response
/// providing details like location, timezone, and ASN data.
#[allow(unused)]
#[derive(Debug, Deserialize)]
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

#[plugin(name = "geoip", settings = Settings)]
impl GeoIp {
    /// Resolves the API key and builds the HTTP client the lookups go through.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "GEOIP_API_KEY")?;
        let client = http::builder(&ctx.config.http)
            .build()
            .map_err(|error| plugin_err(RequestError::from(error)))?;

        Ok(Self { client, api_key })
    }

    /// Looks up the requested domain or IP and replies with its geolocation details.
    #[command(
        ".geoip",
        "Look up the geolocation of a domain or IP",
        args = Opts,
        label = "GeoIP"
    )]
    async fn geoip(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        opts: Opts,
    ) -> Result<(), ZetaError> {
        match self.resolve(&opts.name).await {
            Ok(result) => {
                for line in result.to_string().lines() {
                    client.send_privmsg(channel, line)?;
                }
            }
            Err(err) => {
                warn!(?err, name = %opts.name, "geoip lookup failed");

                client.send_privmsg(channel, labeled_reply("GeoIP", err))?;
            }
        }

        Ok(())
    }
}

impl Display for IpInfo {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts = [
            ("AS", self.asn_name.as_str()),
            ("ASN", self.asn.as_str()),
            ("Country", self.country_name.as_str()),
            ("Region", self.region_name.as_str()),
            ("City", self.city_name.as_str()),
        ]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| format!("{label}:{RESET} {value}"))
        .collect::<Vec<_>>();

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

        write!(
            fmt,
            "{}({RESET}{ip}{COLOR}): {info}",
            labeled_reply_prefix("GeoIP")
        )
    }
}

/// Returns the process-wide DNS resolver: Cloudflare over TLS, used for the hostname
/// lookups the geolocation is built from.
///
/// # Panics
///
/// Panics if the default DNS resolver cannot be created.
fn resolver() -> &'static TokioResolver {
    static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

    RESOLVER.get_or_init(|| {
        Resolver::builder_with_config(
            ResolverConfig::tls(&CLOUDFLARE),
            TokioRuntimeProvider::default(),
        )
        .build()
        .expect("couldn't create the dns resolver")
    })
}

impl GeoIp {
    async fn resolve_domain(domain: &str) -> Result<String, Error> {
        match Host::parse(domain) {
            Ok(Host::Ipv4(addr)) => Ok(addr.to_string()),
            Ok(Host::Ipv6(addr)) => Ok(addr.to_string()),
            Ok(Host::Domain(domain)) => {
                let resolver = resolver();
                // `dns.question.name` is the OpenTelemetry convention for the name a DNS query
                // asks for: https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
                debug!(dns.question.name = %domain, "resolving domain");

                let lookup = resolver.lookup_ip(domain).await.map_err(Error::Resolve)?;

                // `dns.answers` is the OpenTelemetry convention for the addresses a lookup
                // resolved to:
                // https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
                let addresses: Vec<String> = lookup.iter().map(|ip| ip.to_string()).collect();
                if !addresses.is_empty() {
                    // The answers are a string array per convention; tracing macros have no
                    // array value type, so they go through Debug as a single string.
                    debug!(dns.answers = ?addresses, "resolved domain");
                }

                lookup
                    .iter()
                    .next()
                    .ok_or_else(|| Error::NoDomainRecords)
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
        let ip = Self::resolve_domain(name).await?;
        let params = [
            ("ip", ip.as_str()),
            ("key", &self.api_key),
            ("format", "json"),
        ];
        let request = self.client.get(BASE_URL).query(&params);
        let info: IpInfo = http::get_json(request)
            .await
            // `parse_response` already logs the status of a failed response; keeping the
            // looked-up name at debug avoids logging one failure three times over.
            .inspect_err(|error| debug!(%name, %error, "error when querying for geoip"))?;

        info!(ip = %info.ip, "resolved");

        Ok(LookupResult(info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

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
