use std::str::FromStr;
use std::{fmt::Display, net::IpAddr};

use argh::FromArgs;
use async_trait::async_trait;
use hickory_resolver::{
    ResolveError, Resolver, TokioResolver,
    config::{NameServerConfigGroup, ResolveHosts, ResolverConfig, ResolverOpts},
    lookup::Lookup,
    name_server::TokioConnectionProvider,
    proto::rr::RecordType,
};
use irc::client::Client;
use irc::proto::{Command, Message};
use miette::Diagnostic;
use serde::Deserialize;
use thiserror::Error;
use tracing::debug;

use super::{Author, Version};
use crate::{Error as ZetaError, plugin::NewPlugin};

/// DNS lookup utility
#[derive(FromArgs, Debug)]
pub struct Opts {
    /// the name of the domain to look to look up
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
    #[error("could not parse arguments")]
    ParseArguments,
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] ResolveError),
}

pub struct Dig {
    resolver: TokioResolver,
}

#[derive(Deserialize)]
pub struct DigConfig {
    /// List of nameserver addresses to use.
    pub name_servers: Vec<IpAddr>,
}

pub struct LookupResult(Lookup);

impl Display for LookupResult {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for lookup in self.0.record_iter() {
            // We need to convert the fields to strings for string padding to work.
            let name = lookup.name().to_string();
            let ttl = lookup.ttl().to_string();
            let dns_class = lookup.dns_class().to_string();
            let record_type = lookup.record_type().to_string();
            let data = lookup.data();

            write!(fmt, "\x0310>\x0f\x02 Dig:\x02\x0310 ")?;
            writeln!(
                fmt,
                "{name:<25} {ttl:<7} {dns_class:<7} {record_type:<7} {data}"
            )?;
        }

        Ok(())
    }
}

#[async_trait]
impl NewPlugin for Dig {
    const NAME: &str = "dig";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = DigConfig;

    fn with_config(config: &Self::Config) -> Dig {
        debug!(name_servers = ?config.name_servers, "using nameservers");
        let domain = None;
        let search = vec![];
        let ns_group = NameServerConfigGroup::from_ips_clear(&config.name_servers, 53, true);
        let config = ResolverConfig::from_parts(domain, search, ns_group);
        let mut opts = ResolverOpts::default();
        opts.use_hosts_file = ResolveHosts::Never;
        let resolver = Resolver::builder_with_config(config, TokioConnectionProvider::default())
            .with_options(opts)
            .build();

        Dig { resolver }
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command
            && let Some(args) = message.strip_prefix(".dig ")
        {
            let sub_args = shlex::split(args)
                .ok_or_else(|| ZetaError::PluginError(Box::new(Error::ParseArguments)))?;
            let sub_args_ref = sub_args.iter().map(String::as_ref).collect::<Vec<_>>();

            match Opts::from_args(&[".dig"], &sub_args_ref) {
                Ok(opts) => match self.resolve(&opts.name, opts.record_type).await {
                    Ok(result) => {
                        for line in result.to_string().lines() {
                            client
                                .send_privmsg(channel, line)
                                .map_err(ZetaError::IrcClientError)?;
                        }
                    }
                    Err(err) => {
                        client
                            .send_privmsg(channel, format!("\x0310>\x03\x02 Dig:\x02\x0310 {err}"))
                            .map_err(ZetaError::IrcClientError)?;
                    }
                },
                Err(err) => {
                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 Dig:\x02\x0310 {}", err.output),
                        )
                        .map_err(ZetaError::IrcClientError)?;
                }
            }
        }

        Ok(())
    }
}

fn record_type_from_str(s: &str) -> Result<RecordType, String> {
    let record = s.to_uppercase();
    RecordType::from_str(&record).map_err(|_| format!("Invalid record type `{record}`"))
}

impl Dig {
    pub async fn resolve(
        &self,
        name: &str,
        record_type: RecordType,
    ) -> Result<LookupResult, Error> {
        let result = self.resolver.lookup(name, record_type).await;

        match result {
            Ok(result) => Ok(LookupResult(result)),
            Err(err) => Err(Error::Resolve(err)),
        }
    }
}
