use std::sync::OnceLock;

use hickory_resolver::{
    config::ResolverConfig, name_server::TokioConnectionProvider, Resolver, TokioResolver,
};

static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

pub fn resolver() -> &'static TokioResolver {
    RESOLVER.get_or_init(|| {
        let config = ResolverConfig::cloudflare();

        Resolver::builder_with_config(config, TokioConnectionProvider::default()).build()
    })
}
