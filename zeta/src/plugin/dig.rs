use std::fmt::Display;
use std::str::FromStr;

use argh::FromArgs;
use hickory_resolver::{
    Resolver, TokioResolver,
    config::{CLOUDFLARE, LookupIpStrategy, ResolveHosts, ResolverConfig, ResolverOpts},
    lookup::Lookup,
    net::{NetError, runtime::TokioRuntimeProvider},
    proto::rr::RecordType,
};
use miette::Diagnostic;
use thiserror::Error;

use crate::plugin::prelude::*;

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
    Resolve(#[source] NetError),
}

pub struct Dig {
    command: Prefix,
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
impl Plugin<Context> for Dig {
    fn new(_ctx: &Context) -> Result<Dig, ZetaError> {
        let config = ResolverConfig::udp_and_tcp(&CLOUDFLARE);
        let mut opts = ResolverOpts::default();
        opts.use_hosts_file = ResolveHosts::Never;
        opts.attempts = 5;
        opts.ip_strategy = LookupIpStrategy::Ipv6thenIpv4;
        let resolver = Resolver::builder_with_config(config, TokioRuntimeProvider::default())
            .with_options(opts)
            .build().map_err(plugin_err)?;
        let command = Prefix::new(".dig");

        Ok(Dig { command, resolver })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "dig".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(args) = self.command.parse(user_message)
        {
            let sub_args = shlex::split(args)
                .ok_or_else(|| ZetaError::Plugin(Box::new(Error::ParseArguments)))?;
            let sub_args_ref = sub_args.iter().map(String::as_ref).collect::<Vec<_>>();

            match Opts::from_args(&[".dig"], &sub_args_ref) {
                Ok(opts) => match self.resolve(&opts.name, opts.record_type).await {
                    Ok(result) => {
                        for line in result.to_string().lines() {
                            client.send_privmsg(channel, line)?;
                        }
                    }
                    Err(err) => {
                        client.send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 Dig:\x02\x0310 {err}"),
                        )?;
                    }
                },
                Err(err) => {
                    client.send_privmsg(
                        channel,
                        format!("\x0310>\x03\x02 Dig:\x02\x0310 {}", err.output),
                    )?;
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
