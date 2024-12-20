use std::fmt::Display;
use std::str::FromStr;

use argh::FromArgs;
use async_trait::async_trait;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::error::ResolveError;
use hickory_resolver::lookup::Lookup;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::TokioAsyncResolver;
use irc::client::Client;
use irc::proto::{Command, Message};
use miette::Diagnostic;
use thiserror::Error;

use crate::Error as ZetaError;

use super::{Author, Name, Plugin, Version};

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
    #[error("Unsupported record type: {0}")]
    InvalidRecordType(#[from] hickory_resolver::proto::error::ProtoError),
    #[error("No records found")]
    NoRecordsFound,
    #[error("Could not parse arguments")]
    ParseArguments,
    #[error("Could not resolve domain")]
    Resolve(#[source] ResolveError),
}

pub struct Dig {
    resolver: TokioAsyncResolver,
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

            if let Some(data) = data {
                write!(fmt, "\x0310>\x0f\x02 Dig:\x02\x0310 ")?;
                writeln!(
                    fmt,
                    "{name:<25} {ttl:<7} {dns_class:<7} {record_type:<7} {data}"
                )?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin for Dig {
    fn new() -> Dig {
        // TODO: Use TLS/DoH
        let config = ResolverConfig::cloudflare();

        let mut opts = ResolverOpts::default();
        opts.use_hosts_file = false;

        let resolver = TokioAsyncResolver::tokio(config, opts);

        Dig { resolver }
    }

    fn name() -> Name {
        Name("dig")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref message) = message.command {
            if let Some(args) = message.strip_prefix(".dig ") {
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
                                .send_privmsg(
                                    channel,
                                    format!("\x0310>\x03\x02 Dig:\x02\x0310 {err}"),
                                )
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
            Err(err) => {
                if let hickory_resolver::error::ResolveErrorKind::NoRecordsFound { .. } = err.kind()
                {
                    Err(Error::NoRecordsFound)
                } else {
                    Err(Error::Resolve(err))
                }
            }
        }
    }
}
