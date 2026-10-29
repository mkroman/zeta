//! Shared context for plugins.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(feature = "http")]
use std::sync::OnceLock;

use arc_swap::ArcSwap;

use crate::config::HostConfig;

/// A registry of state published by plugins for other plugins to access.
///
/// State is keyed by its concrete type. A plugin publishes an [`Arc`] to a service (or other
/// shared state) with [`SharedState::publish`], and any other plugin can look it up with
/// [`SharedState::get`] through [`Context::shared`].
///
/// Published state is shared across plugin tasks, so it must be thread-safe and expose its
/// mutation through `&self` (for example using a lock internally).
#[derive(Default)]
pub struct SharedState {
    /// The published state, keyed by the type of the value.
    states: ArcSwap<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl SharedState {
    /// Constructs and returns a new, empty shared state registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes `state` under its concrete type.
    ///
    /// Returns the previously published state of the same type, if any.
    // `rcu` may retry its closure, so the state cannot be moved into it.
    #[allow(clippy::needless_pass_by_value)]
    pub fn publish<T: Send + Sync + 'static>(&self, state: Arc<T>) -> Option<Arc<T>> {
        let previous = self.states.rcu(|states| {
            let mut next = (**states).clone();
            next.insert(TypeId::of::<T>(), state.clone());

            Arc::new(next)
        });

        previous
            .get(&TypeId::of::<T>())
            .and_then(|published| Arc::clone(published).downcast::<T>().ok())
    }

    /// Returns the state published under type `T`, if any.
    #[must_use]
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.states
            .load()
            .get(&TypeId::of::<T>())
            .and_then(|state| Arc::clone(state).downcast::<T>().ok())
    }
}

/// Shared context for plugin invocations.
pub struct Context {
    /// The database connection pool.
    #[cfg(feature = "database")]
    pub db: crate::database::Database,
    /// The plugin-visible subset of the bot configuration.
    ///
    /// Per-plugin settings are not part of the context; each plugin receives its own settings
    /// through its constructor.
    pub config: HostConfig,
    /// State published by plugins for other plugins to access.
    pub shared: SharedState,
    /// The HTTP client shared by every plugin, built on first use.
    #[cfg(feature = "http")]
    http: OnceLock<reqwest::Client>,
}

impl Context {
    /// Creates a new context.
    #[must_use]
    pub fn new(
        #[cfg(feature = "database")] db: crate::database::Database,
        config: HostConfig,
    ) -> Self {
        Self {
            #[cfg(feature = "database")]
            db,
            config,
            shared: SharedState::new(),
            #[cfg(feature = "http")]
            http: OnceLock::new(),
        }
    }

    /// Returns the HTTP client shared by every plugin.
    ///
    /// The client is built once from the host's [`HttpConfig`](zeta_http::HttpConfig) and then
    /// reused for the process lifetime. It is deliberately anonymous: it carries no credentials,
    /// cookies, or other default headers, so plugins must attach authentication to each request —
    /// never to this client. Because `reqwest` scopes its connection pool by scheme, host, and
    /// port, pooled connections are not reused across origins.
    ///
    /// The client follows no redirects, applies the configured timeout, and sends the configured
    /// user agent (its only default header).
    ///
    /// Cloning the returned handle is cheap and shares the same connection pool, so a plugin that
    /// needs the client in helpers can keep its own `reqwest::Client` field initialized with
    /// `ctx.http().clone()`.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client fails to build.
    #[cfg(feature = "http")]
    #[must_use]
    pub fn http(&self) -> &reqwest::Client {
        self.http
            .get_or_init(|| zeta_http::build_client(&self.config.http))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_state_can_be_retrieved() {
        let shared = SharedState::new();
        let state = Arc::new(42_u32);

        assert!(shared.publish(Arc::clone(&state)).is_none());
        assert_eq!(shared.get::<u32>().as_deref(), Some(&42));
    }

    #[test]
    fn missing_state_is_not_found() {
        let shared = SharedState::new();

        assert!(shared.get::<u32>().is_none());
    }

    #[test]
    fn publishing_replaces_state_of_the_same_type() {
        let shared = SharedState::new();

        shared.publish(Arc::new(1_u32));
        let previous = shared.publish(Arc::new(2_u32));

        assert_eq!(previous.as_deref(), Some(&1));
        assert_eq!(shared.get::<u32>().as_deref(), Some(&2));
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn http_client_is_built_once_and_shared() {
        #[cfg(feature = "database")]
        let db = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://invalid/zeta_test")
            .expect("lazy database pool");
        let context = Context::new(
            #[cfg(feature = "database")]
            db,
            HostConfig::default(),
        );

        assert!(std::ptr::eq(context.http(), context.http()));
    }
}
