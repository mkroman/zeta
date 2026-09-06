use std::fmt::Display;

use argh::FromArgs;
use hickory_resolver::{
    Resolver, TokioResolver,
    config::{CLOUDFLARE, LookupIpStrategy, ResolveHosts, ResolverConfig, ResolverOpts},
    lookup::Lookup,
    net::{NetError, runtime::TokioRuntimeProvider},
    proto::{rr::RecordType, serialize::binary::DecodeError},
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
    #[error("could not resolve domain: {0}")]
    Resolve(#[source] NetError),
}

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

        opts.attempts = 5;
        opts.ip_strategy = LookupIpStrategy::Ipv6thenIpv4;
        opts.use_hosts_file = ResolveHosts::Never;

        let resolver = Resolver::builder_with_config(config, TokioRuntimeProvider::default())
            .with_options(opts)
            .build()
            .map_err(plugin_err)?;

        Ok(Dig { resolver })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "dig".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        const { &[Prefix::new(".dig")] }
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
                client.send_privmsg(channel, formatted(&err.to_string()))?;
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

fn formatted(message: &str) -> String {
    format!("\x0310>\x03\x02 Dig:\x02\x0310 {message}")
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
