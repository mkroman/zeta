use std::sync::OnceLock;

use hickory_resolver::{
    config::{ResolverConfig, CLOUDFLARE},
    net::runtime::TokioRuntimeProvider,
    Resolver, TokioResolver,
};

static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

/// Returns a global shared DNS resolver.
pub fn resolver() -> &'static TokioResolver {
    RESOLVER.get_or_init(new)
}

/// Creates and returns a new DNS resolver using cloudflare.
///
/// # Panics
///
/// Panics if the default DNS resolver cannot be created.
#[must_use]
pub fn new() -> TokioResolver {
    let config = ResolverConfig::tls(&CLOUDFLARE);

    Resolver::builder_with_config(config, TokioRuntimeProvider::default())
        .build()
        .expect("couldn't create default dns resolver")
}
