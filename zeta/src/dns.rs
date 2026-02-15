use std::sync::OnceLock;

use hickory_resolver::{
    Resolver, TokioResolver, config::ResolverConfig, name_server::TokioConnectionProvider,
};

static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

/// Returns a global shared DNS resolver.
pub fn resolver() -> &'static TokioResolver {
    RESOLVER.get_or_init(new)
}

/// Creates and returns a new DNS resolver using cloudflare.
#[must_use]
pub fn new() -> TokioResolver {
    let config = ResolverConfig::cloudflare();

    Resolver::builder_with_config(config, TokioConnectionProvider::default()).build()
}
