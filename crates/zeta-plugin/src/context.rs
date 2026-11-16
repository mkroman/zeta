//! Shared context for plugins.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
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
    #[expect(clippy::needless_pass_by_value)]
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

// Manual implementation: the published values are type-erased and may hold anything, so only
// their number is shown.
impl fmt::Debug for SharedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedState")
            .field("published", &self.states.load().len())
            .finish_non_exhaustive()
    }
}

/// Shared context for plugin invocations.
///
/// The context carries the resources every plugin shares: the database pool, the plugin-visible
/// [`HostConfig`], the type-keyed [`SharedState`] registry, and a lazily built HTTP client.
/// The state is private and read-only through [`Context::db`], [`Context::config`],
/// [`Context::shared`] and [`Context::http`]; the context is immutable after construction, so a
/// plugin cannot replace what another plugin observes.
///
/// The host constructs the context with [`Context::new`] and attaches the resources a build
/// feature enables — the database pool through [`Context::with_database`] — before any plugin is
/// constructed, so a plugin always finds them in place.
///
/// Per-plugin settings are not part of the context: each plugin receives its own settings
/// through [`Registered::new`](crate::Registered::new), not through the context.
#[non_exhaustive]
pub struct Context {
    /// The database connection pool, exposed through [`Context::db`].
    ///
    /// Always `Some` by the time a plugin runs: the host attaches the pool with
    /// [`Context::with_database`] before constructing the plugins.
    #[cfg(feature = "database")]
    db: Option<crate::database::Database>,
    /// The account subsystem service, exposed through [`Context::accounts`].
    ///
    /// Always `Some` by the time a plugin runs: the host attaches the service with
    /// [`Context::with_accounts`] before constructing the plugins.
    #[cfg(feature = "database")]
    accounts: Option<Arc<crate::accounts::Accounts>>,
    /// The plugin-visible subset of the bot configuration, exposed through [`Context::config`].
    config: HostConfig,
    /// State published by plugins for other plugins to access, exposed through
    /// [`Context::shared`].
    shared: SharedState,
    /// The HTTP client shared by every plugin, built on first use, exposed through
    /// [`Context::http`].
    #[cfg(feature = "http")]
    http: OnceLock<reqwest::Client>,
}

impl Context {
    /// Creates a new context with the plugin-visible configuration.
    ///
    /// The context starts without the resources that are attached after construction: the host
    /// calls [`Context::new`] once at startup and then attaches the database pool with
    /// [`Context::with_database`] before any plugin is constructed.
    #[must_use]
    pub fn new(config: HostConfig) -> Self {
        Self {
            #[cfg(feature = "database")]
            db: None,
            #[cfg(feature = "database")]
            accounts: None,
            config,
            shared: SharedState::new(),
            #[cfg(feature = "http")]
            http: OnceLock::new(),
        }
    }

    /// Attaches the database connection pool.
    ///
    /// The host calls this immediately after [`Context::new`], before any plugin is constructed
    /// and therefore before [`Context::db`] can be reached.
    #[cfg(feature = "database")]
    #[must_use]
    pub fn with_database(mut self, db: crate::database::Database) -> Self {
        self.db = Some(db);
        self
    }

    /// Attaches the account subsystem service.
    ///
    /// The host calls this after [`Context::with_database`], before any plugin is constructed
    /// and therefore before [`Context::accounts`] can be reached.
    #[cfg(feature = "database")]
    #[must_use]
    pub fn with_accounts(mut self, accounts: Arc<crate::accounts::Accounts>) -> Self {
        self.accounts = Some(accounts);
        self
    }

    /// Returns the account subsystem service.
    ///
    /// The service is shared by every plugin: reads and writes go through one cache, so a value
    /// changed through the `.settings` command is visible to the next reader. Plugins resolve a
    /// user's settings through
    /// [`Accounts::settings`](crate::accounts::Accounts::settings) by their own descriptors —
    /// typically cloned once at construction — and users change the values through the
    /// `.settings` command.
    ///
    /// # Panics
    ///
    /// Panics if the host did not attach a service with [`Context::with_accounts`] before
    /// plugins were constructed, which is a host bug.
    #[cfg(feature = "database")]
    #[must_use]
    pub const fn accounts(&self) -> &Arc<crate::accounts::Accounts> {
        self.accounts
            .as_ref()
            .expect("the host attaches the account service before constructing plugins")
    }

    /// Returns the database connection pool.
    ///
    /// The pool is shared by every plugin and is connected by the host before the plugins are
    /// constructed. Repositories clone the handle and keep it for their own queries.
    ///
    /// # Panics
    ///
    /// Panics if the host did not attach a pool with [`Context::with_database`] before plugins
    /// were constructed, which is a host bug.
    #[cfg(feature = "database")]
    #[must_use]
    pub const fn db(&self) -> &crate::database::Database {
        self.db
            .as_ref()
            .expect("the host attaches the database pool before constructing plugins")
    }

    /// Returns the plugin-visible subset of the bot configuration.
    ///
    /// Per-plugin settings are not part of the context; each plugin receives its own settings
    /// through its constructor.
    #[must_use]
    pub const fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Returns the registry of state published by plugins for other plugins to access.
    #[must_use]
    pub const fn shared(&self) -> &SharedState {
        &self.shared
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
            .get_or_init(|| zeta_http::build_client(self.config.http()))
    }
}

// Manual implementation: the database pool and HTTP client are opaque resources, so their
// presence is shown instead of the values themselves.
impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Context");
        debug
            .field("config", &self.config)
            .field("shared", &self.shared);

        #[cfg(feature = "database")]
        debug.field("database", &self.db.is_some());
        #[cfg(feature = "database")]
        debug.field("accounts", &self.accounts.is_some());
        #[cfg(feature = "http")]
        debug.field("http", &self.http.get().is_some());

        debug.finish_non_exhaustive()
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
        let context = Context::new(HostConfig::default());
        #[cfg(feature = "database")]
        let context = context.with_database(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgresql://invalid/zeta_test")
                .expect("lazy database pool"),
        );

        assert!(std::ptr::eq(context.http(), context.http()));
    }
}
