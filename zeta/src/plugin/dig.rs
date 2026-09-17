use std::fmt::Display;
use std::net::IpAddr;

use argh::{ArgsInfo, FromArgs};
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

use crate::plugin::prelude::*;

/// Settings for the dig plugin, from its `[plugins.dig]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Nameservers to query over UDP and TCP.
    ///
    /// Defaults to Cloudflare's public resolvers.
    #[serde(
        default = "default_nameservers",
        deserialize_with = "deserialize_nameservers"
    )]
    pub nameservers: Vec<IpAddr>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            nameservers: default_nameservers(),
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

/// Returns the default nameservers: Cloudflare's public resolvers.
fn default_nameservers() -> Vec<IpAddr> {
    vec![
        IpAddr::from([1, 1, 1, 1]),
        IpAddr::from([1, 0, 0, 1]),
        IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111]),
        IpAddr::from([0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1001]),
    ]
}

/// Look up DNS records for a domain.
#[derive(FromArgs, ArgsInfo, Debug)]
pub struct Opts {
    /// the domain to look up
    #[argh(positional)]
    name: String,
    /// the type of record to look up
    #[argh(
        positional,
        from_str_fn(record_type_from_str),
        default = "RecordType::A"
    )]
    record_type: RecordType,
}

#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] NetError),
}

/// The `.dig` command.
const DIG: PluginCommand =
    PluginCommand::with_args::<Opts>(Prefix::new(".dig"), "Look up DNS records for a domain");

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[DIG];

pub struct Dig {
    resolver: TokioResolver,
}

pub struct LookupResult(Lookup);

impl Display for LookupResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for lookup in self.0.answers() {
            // We need to convert the fields to strings for string padding to work.
            let name = lookup.name.to_string();
            let ttl = lookup.ttl.to_string();
            let dns_class = lookup.dns_class.to_string();
            let record_type = lookup.record_type().to_string();
            let data = &lookup.data;

            write!(fmt, "{}", reply_prefix("Dig"))?;
            writeln!(
                fmt,
                "{name:<25} {ttl:<7} {dns_class:<7} {record_type:<7} {data}"
            )?;
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin<Context> for Dig {
    type Settings = Settings;

    fn new(_ctx: &Context, settings: &Settings) -> Result<Dig, ZetaError> {
        let resolver = build_resolver(&settings.nameservers).map_err(ZetaError::from)?;

        Ok(Dig { resolver })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "dig".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match command.parse_args::<Opts>(args) {
            Ok(opts) => opts,
            Err(err) => {
                client.send_privmsg(channel, err.to_string())?;
                return Ok(());
            }
        };

        match self.resolve(&opts.name, opts.record_type).await {
            Ok(result) => {
                for line in result.to_string().lines() {
                    client.send_privmsg(channel, line)?;
                }
            }
            Err(err) => {
                client.send_privmsg(channel, reply("Dig", err.to_string()))?;
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
    pub async fn resolve(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<LookupResult, Error> {
        self.resolver
            .lookup(name, record_type)
            .await
            .map(LookupResult)
            .map_err(Error::Resolve)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::value::{Dict, Value};

    #[test]
    fn default_settings_build_a_resolver() {
        let settings = Settings::default();
        assert!(!settings.nameservers.is_empty());

        assert!(
            build_resolver(&settings.nameservers).is_ok(),
            "could not build a resolver from the default nameservers"
        );
    }

    #[test]
    fn configured_settings_build_a_resolver() {
        let settings: Settings = Deserialize::deserialize(&Value::from(Dict::from([(
            String::from("nameservers"),
            Value::from(&["192.0.2.53"]),
        )])))
        .expect("could not deserialize settings");

        assert_eq!(settings.nameservers, vec![IpAddr::from([192, 0, 2, 53])]);
        assert!(build_resolver(&settings.nameservers).is_ok());
    }
}
