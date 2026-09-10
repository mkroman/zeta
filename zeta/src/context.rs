//! Shared context for plugins.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use hickory_resolver::TokioResolver;

use crate::Config;
#[cfg(feature = "database")]
use crate::database::Database;

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
    states: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
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
    pub fn publish<T: Send + Sync + 'static>(&self, state: Arc<T>) -> Option<Arc<T>> {
        self.states
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(TypeId::of::<T>(), state)
            .and_then(|previous| previous.downcast::<T>().ok())
    }

    /// Returns the state published under type `T`, if any.
    #[must_use]
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.states
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&TypeId::of::<T>())
            .and_then(|state| Arc::clone(state).downcast::<T>().ok())
    }
}

/// Shared context for plugin invocations.
pub struct Context {
    /// The database connection pool.
    #[cfg(feature = "database")]
    pub db: Database,
    /// The DNS resolver.
    pub dns: TokioResolver,
    /// The bot configuration.
    pub config: Config,
    /// State published by plugins for other plugins to access.
    pub shared: SharedState,
}

impl Context {
    /// Creates a new context.
    #[must_use]
    pub fn new(
        #[cfg(feature = "database")] db: Database,
        dns: TokioResolver,
        config: Config,
    ) -> Self {
        Self {
            #[cfg(feature = "database")]
            db,
            dns,
            config,
            shared: SharedState::new(),
        }
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
}
