//! The cryptocurrency listings capability shared between plugins.
//!
//! The coinmarketcap plugin owns the keyed API client; the rink plugin needs the ranked
//! listings for its live cryptocurrency units. Publishing the client itself would make rink
//! depend on the client crate for a runtime-optional capability, so the coinmarketcap plugin
//! publishes this neutral source through [`Context::shared`](zeta_plugin::Context) instead and
//! keeps the client private.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use zeta_plugin::Error;

/// A cryptocurrency listing shared between plugins.
///
/// This is the neutral subset of an API listing: the ticker symbol, the display name, and the
/// current price in US dollars. The market-cap rank is carried by the position in the returned
/// [`Vec`], highest first.
#[derive(Clone, Debug, PartialEq)]
pub struct CryptoListing {
    /// The ticker symbol (e.g. `ETH`).
    pub symbol: String,
    /// The display name (e.g. `Ethereum`).
    pub name: String,
    /// The current price in US dollars, when the source provides one.
    ///
    /// A listing without a USD quote is still returned, so the consumer decides whether to
    /// skip it.
    pub usd_price: Option<f64>,
}

/// A source of ranked cryptocurrency listings.
///
/// The listings are returned in market-cap rank order, highest first, so a consumer can prefer
/// the highest-ranked coin when several share a ticker.
///
/// The returned future is boxed because the trait has to stay object safe: the coinmarketcap
/// plugin publishes it as an `Arc<dyn CryptoSource>` in shared state, and the consumer stores
/// that handle without naming the concrete adapter or the client type behind it. The box
/// allocates once per call, and the call happens on the currency refresh interval (10
/// minutes), never on the message path, so the cost is negligible.
pub trait CryptoSource: Send + Sync {
    /// Fetches the ranked cryptocurrency listings.
    ///
    /// # Errors
    ///
    /// Returns a [`zeta_plugin::Error`] when the listings cannot be fetched.
    fn listings<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CryptoListing>, Error>> + Send + 'a>>;
}

/// A shared handle to a [`CryptoSource`].
///
/// Shared state keys published values by their concrete type, which excludes unsized trait
/// objects, so the coinmarketcap plugin publishes this sized handle wrapping an
/// `Arc<dyn CryptoSource>` instead of the trait object itself. Consumers that want the bare
/// source clone it out of the handle with [`CryptoSourceHandle::source`].
#[derive(Clone)]
pub struct CryptoSourceHandle {
    /// The shared listings source.
    source: Arc<dyn CryptoSource>,
}

impl CryptoSourceHandle {
    /// Wraps `source` in a handle that can be published in shared state.
    #[must_use]
    pub fn new(source: Arc<dyn CryptoSource>) -> Self {
        Self { source }
    }

    /// Returns the shared listings source behind the handle.
    ///
    /// Cloning the [`Arc`] is cheap; consumers that only fetch listings can keep the handle and
    /// call [`CryptoSource::listings`] on it instead.
    #[must_use]
    pub fn source(&self) -> Arc<dyn CryptoSource> {
        Arc::clone(&self.source)
    }
}

impl CryptoSource for CryptoSourceHandle {
    fn listings<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CryptoListing>, Error>> + Send + 'a>> {
        self.source.listings()
    }
}
