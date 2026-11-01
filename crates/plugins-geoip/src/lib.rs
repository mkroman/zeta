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

use clap::Parser;
use hickory_resolver::{
    Resolver, TokioResolver,
    config::{CLOUDFLARE, ResolverConfig},
    net::runtime::TokioRuntimeProvider,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{Instrument, debug, warn};
use url::Host;

use zeta_http as http;
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
    client: reqwest::Client,
    api_key: String,
    /// The resolver used to turn domains into addresses.
    resolver: TokioResolver,
}

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
    /// Resolves the API key and takes a handle to the shared HTTP client the lookups go through.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "GEOIP_API_KEY")?;
        let client = ctx.http().clone();
        let resolver = Resolver::builder_with_config(
            ResolverConfig::tls(&CLOUDFLARE),
            TokioRuntimeProvider::default(),
        )
        .build()
        .map_err(plugin_err)?;

        Ok(Self {
            client,
            api_key,
            resolver,
        })
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
            Ok(info) => {
                for line in reply(&info).lines() {
                    client.send_privmsg(channel, line)?;
                }
            }
            Err(err) => {
                warn!(
                    exception.message = ?err,
                    geoip.lookup.name = %opts.name,
                    "geoip lookup failed"
                );

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
        ];

        // Written part by part: collecting formatted strings to join them afterwards would
        // allocate once per part for a display that is built for every reply.
        let mut wrote = false;

        for (label, value) in parts.into_iter().filter(|(_, value)| !value.is_empty()) {
            if wrote {
                fmt.write_str(" ")?;
            }

            // Every part ends in the reply color, so a plain space between parts keeps the
            // separator cyan.
            write!(fmt, "{label}:{RESET} {}", unstyle_text(value))?;
            wrote = true;
        }

        if !wrote {
            write!(fmt, "No location data available")?;
        }

        Ok(())
    }
}

/// Renders `info` as the labeled reply line the `.geoip` command sends.
fn reply(info: &IpInfo) -> String {
    format!(
        "{}({RESET}{}{COLOR}): {info}",
        labeled_reply_prefix("GeoIP"),
        unstyle_text(&info.ip)
    )
}

impl GeoIp {
    /// Resolves `domain` to its first address through the plugin's Cloudflare-over-TLS
    /// resolver. IP address literals are returned as they are.
    async fn resolve_domain(&self, domain: &str) -> Result<String, Error> {
        match Host::parse(domain) {
            Ok(Host::Ipv4(addr)) => Ok(addr.to_string()),
            Ok(Host::Ipv6(addr)) => Ok(addr.to_string()),
            Ok(Host::Domain(domain)) => {
                // `dns.question.name` is the OpenTelemetry convention for the name a DNS query
                // asks for: https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
                debug!(dns.question.name = %domain, "resolving domain");

                let span = tracing::info_span!("dns.lookup", dns.question.name = %domain);

                let lookup = self
                    .resolver
                    .lookup_ip(domain)
                    .instrument(span)
                    .await
                    .map_err(Error::Resolve)?;

                // `dns.answers` is the OpenTelemetry convention for the addresses a lookup
                // resolved to:
                // https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
                // The string array is built only when debug logging is on: the collection
                // itself would run for every lookup even when the event is filtered out.
                if tracing::enabled!(tracing::Level::DEBUG) {
                    let addresses: Vec<String> = lookup.iter().map(|ip| ip.to_string()).collect();

                    if !addresses.is_empty() {
                        // The answers are a string array per convention; tracing macros have no
                        // array value type, so they go through Debug as a single string.
                        debug!(dns.answers = ?addresses, "resolved domain");
                    }
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
    pub async fn resolve(&self, name: &str) -> Result<IpInfo, Error> {
        let ip = self.resolve_domain(name).await?;
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
            .inspect_err(|error| {
                debug!(
                    geoip.lookup.name = %name,
                    exception.message = %error,
                    "error when querying for geoip"
                );
            })?;

        debug!(geoip.lookup.address = %info.ip, "resolved");

        Ok(info)
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

    /// The details the API reported for a test address, with each formatted part overridable.
    fn ip_info(
        asn_name: &str,
        asn: &str,
        country_name: &str,
        region_name: &str,
        city_name: &str,
    ) -> IpInfo {
        IpInfo {
            ip: "203.0.113.1".to_owned(),
            country_code: "DK".to_owned(),
            country_name: country_name.to_owned(),
            region_name: region_name.to_owned(),
            city_name: city_name.to_owned(),
            latitude: 55.6761,
            longitude: 12.5683,
            zip_code: "1000".to_owned(),
            time_zone: "+01:00".to_owned(),
            asn: asn.to_owned(),
            asn_name: asn_name.to_owned(),
            is_proxy: false,
        }
    }

    #[test]
    fn display_writes_each_location_part_once() {
        let info = ip_info(
            "Cloudflare",
            "13335",
            "Denmark",
            "Capital Region",
            "Copenhagen",
        );

        // The parts are separated by a single space and end in `RESET`, so the separator keeps
        // the reply color.
        assert_eq!(
            info.to_string(),
            format!(
                "AS:{RESET} Cloudflare ASN:{RESET} 13335 Country:{RESET} Denmark \
                 Region:{RESET} Capital Region City:{RESET} Copenhagen"
            )
        );
    }

    #[test]
    fn display_strips_formatting_from_api_text() {
        let info = ip_info(
            "\x0310Fake\nASN",
            "13335",
            "De\x02nmark",
            "Capital Region",
            "Copenhagen",
        );

        let rendered = info.to_string();
        let rendered = unstyle_text(&rendered);

        assert!(
            rendered.contains("AS: Fake ASN ASN: 13335 Country: Denmark"),
            "{rendered}"
        );
        assert!(!rendered.contains('\n'), "{rendered}");
    }

    #[test]
    fn display_skips_empty_location_parts() {
        let info = ip_info("", "", "Denmark", "", "Copenhagen");

        assert_eq!(
            info.to_string(),
            format!("Country:{RESET} Denmark City:{RESET} Copenhagen")
        );
    }

    #[test]
    fn display_falls_back_when_no_part_is_known() {
        let info = ip_info("", "", "", "", "");

        assert_eq!(info.to_string(), "No location data available");
    }

    #[test]
    fn reply_wraps_the_location_in_a_labeled_reply() {
        let info = ip_info("Cloudflare", "13335", "Denmark", "", "");

        assert_eq!(
            reply(&info),
            format!(
                "{}({RESET}203.0.113.1{COLOR}): AS:{RESET} Cloudflare ASN:{RESET} 13335 \
                 Country:{RESET} Denmark",
                labeled_reply_prefix("GeoIP")
            )
        );
    }
}
