//! Resolves DNS records through hickory-resolver.
//!
//! The `.dig <domain> [record-type]` command looks up `domain` through the configured
//! nameservers — Cloudflare's public resolvers by default — and replies with one line per
//! record: name, TTL, class, type, and data, in `dig` output format. The record type defaults
//! to `A`, parses case-insensitively (`.dig example.com AAAA`), and resolves against the
//! nameservers configured in `[plugins.dig]` (an empty list is rejected at startup).
//!
//! A resolver is built once at plugin initialization, querying the nameservers over both UDP
//! and TCP; the host file is never consulted.

use std::fmt::Display;
use std::net::IpAddr;

use clap::Parser;
use hickory_resolver::{
    Resolver, TokioResolver,
    config::{LookupIpStrategy, NameServerConfig, ResolveHosts, ResolverConfig, ResolverOpts},
    lookup::Lookup,
    net::{NetError, runtime::TokioRuntimeProvider},
    proto::{rr::RecordType, serialize::binary::DecodeError},
};
use miette::Diagnostic;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use tracing::{Instrument, debug};

use zeta_plugin::prelude::*;
use zeta_utils::irc::unstyle_text;

/// Settings for the dig plugin, from its `[plugins.dig]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Nameservers to query over UDP and TCP.
    ///
    /// Defaults to Cloudflare's public resolvers.
    #[serde(deserialize_with = "deserialize_nameservers")]
    pub nameservers: Vec<IpAddr>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            nameservers: vec![
                IpAddr::from([1, 1, 1, 1]),
                IpAddr::from([1, 0, 0, 1]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
                IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1001]),
            ],
        }
    }
}

/// Deserializes `nameservers`, rejecting an empty list.
fn deserialize_nameservers<'de, D>(deserializer: D) -> Result<Vec<IpAddr>, D::Error>
where
    D: Deserializer<'de>,
{
    let nameservers = Vec::<IpAddr>::deserialize(deserializer)?;

    if nameservers.is_empty() {
        return Err(serde::de::Error::custom("`nameservers` must not be empty"));
    }

    Ok(nameservers)
}

/// Look up DNS records for a domain.
#[derive(Parser, Debug)]
pub struct Opts {
    /// the domain to look up
    name: String,
    /// the type of record to look up
    #[arg(value_parser = record_type_from_str, default_value = "A")]
    record_type: RecordType,
}

/// Errors that can occur while resolving DNS records.
#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    /// The domain could not be resolved through the configured nameservers.
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] NetError),
}

/// The dig plugin: resolves DNS records on behalf of `.dig` commands.
pub struct Dig {
    resolver: TokioResolver,
}

/// A successful DNS lookup, formatting one `dig`-style reply line per record.
#[must_use]
pub struct LookupResult(Lookup);

impl Display for LookupResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for lookup in self.0.answers() {
            // We need to convert the fields to strings for string padding to work.
            let name = unstyle_text(&lookup.name.to_string());
            let ttl = lookup.ttl.to_string();
            let dns_class = lookup.dns_class.to_string();
            let record_type = lookup.record_type().to_string();
            let data = unstyle_text(&lookup.data.to_string());

            write!(fmt, "{}", labeled_reply_prefix("Dig"))?;
            writeln!(
                fmt,
                "{name:<25} {ttl:<7} {dns_class:<7} {record_type:<7} {data}"
            )?;
        }

        Ok(())
    }
}

#[plugin(name = "dig", settings = Settings)]
impl Dig {
    /// Builds the resolver the plugin queries nameservers through.
    fn new(settings: &Settings) -> Result<Self, ZetaError> {
        let resolver = build_resolver(&settings.nameservers).map_err(ZetaError::from)?;

        Ok(Self { resolver })
    }

    /// Looks up the requested records and replies with one `dig`-style line per record.
    #[command(".dig", "Look up DNS records for a domain", args = Opts, label = "Dig")]
    async fn dig(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        opts: Opts,
    ) -> Result<(), ZetaError> {
        match self.resolve(&opts.name, opts.record_type).await {
            Ok(result) => {
                for line in result.to_string().lines() {
                    client.send_privmsg(channel, line)?;
                }
            }
            Err(err) => {
                client.send_privmsg(channel, labeled_reply("Dig", err.to_string()))?;
            }
        }

        Ok(())
    }
}

fn record_type_from_str(s: &str) -> Result<RecordType, String> {
    s.to_uppercase()
        .parse()
        .map_err(|err: DecodeError| err.to_string())
}

/// Builds a resolver that queries `nameservers` over UDP and TCP.
///
/// # Errors
///
/// Returns an error if the resolver cannot be built.
fn build_resolver(nameservers: &[IpAddr]) -> Result<TokioResolver, BoxError> {
    let mut config = ResolverConfig::from_parts(None, Vec::new(), Vec::new());

    for ip in nameservers {
        config.add_name_server(NameServerConfig::udp_and_tcp(*ip));
    }

    let mut opts = ResolverOpts::default();
    opts.attempts = 5;
    opts.ip_strategy = LookupIpStrategy::Ipv6thenIpv4;
    opts.use_hosts_file = ResolveHosts::Never;

    Resolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_options(opts)
        .build()
        .map_err(Into::into)
}

impl Dig {
    /// Looks up `record_type` records for `name` through the configured nameservers.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Resolve`] if the lookup fails, e.g. when the domain has no records of
    /// the requested type or the nameservers are unreachable.
    pub async fn resolve(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<LookupResult, Error> {
        // `dns.question.name` is the OpenTelemetry convention for the name a DNS query asks
        // for: https://opentelemetry.io/docs/specs/semconv/registry/attributes/dns/
        debug!(dns.question.name = %name, "resolving dns records");

        let lookup = self
            .resolver
            .lookup(name, record_type)
            .instrument(tracing::info_span!(
                "dns.lookup",
                dns.question.name = %name
            ))
            .await
            .map_err(Error::Resolve)?;

        // `dns.answers` is the OpenTelemetry convention for the addresses a lookup resolved
        // to, so only address records go through it — the answers of any other record type
        // are not addresses and stay in the reply to the channel. The string array is built
        // only when debug logging is on: the collection itself would run for every lookup
        // even when the event is filtered out.
        if tracing::enabled!(tracing::Level::DEBUG) {
            let addresses: Vec<String> = lookup
                .answers()
                .iter()
                .filter(|record| matches!(record.record_type(), RecordType::A | RecordType::AAAA))
                .map(|record| record.data.to_string())
                .collect();

            if !addresses.is_empty() {
                // `dns.answers` is a string array per convention; tracing macros have no array
                // value type, so it goes through Debug as a single string.
                debug!(dns.answers = ?addresses, "resolved dns records");
            }
        }

        Ok(LookupResult(lookup))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use zeta_test_support::settings_tests;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

    #[test]
    fn parses_record_types_case_insensitively() {
        assert_eq!(record_type_from_str("mx").unwrap(), RecordType::MX);
        assert!(record_type_from_str("nope").is_err());
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_ne!(settings.nameservers, Vec::<IpAddr>::new());
            assert!(
                build_resolver(&settings.nameservers).is_ok(),
                "could not build a resolver from the default nameservers"
            );
        }
        deserialize: {
            "nameservers": ["192.0.2.53"],
        } assert: {
            assert_eq!(settings.nameservers, vec![IpAddr::from([192, 0, 2, 53])]);
            assert!(build_resolver(&settings.nameservers).is_ok());
        }
    }
}
