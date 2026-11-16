//! The host-attached account service: identity tracking and per-user settings in one place.
//!
//! The host constructs one instance over the shared pool and attaches it to the context before
//! any plugin is constructed; plugins reach it through
//! [`Context::accounts`](crate::Context::accounts) and clone out the facet they need. Clones
//! share the same cache and the same pool, so a change written through the `.settings` command
//! is visible to every reader immediately.

use std::fmt;
use std::sync::Arc;

use crate::accounts::hostmasks::ObservedHostmask;
use crate::accounts::identity::UserIdentity;
use crate::accounts::repository::AccountRepository;
use crate::accounts::settings::UserSettings;
use crate::database::{Database, DbError};

/// The account subsystem's service: tracking and settings over the shared database pool.
#[derive(Clone)]
pub struct Accounts {
    /// The per-user settings facet.
    settings: Arc<UserSettings>,
    /// The repository the tracking observations go through.
    repository: Arc<AccountRepository>,
}

// Manual implementation: the pool and the caches are opaque internals.
impl fmt::Debug for Accounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

impl Accounts {
    /// Constructs the service over the shared database pool.
    ///
    /// The host calls this once at startup and attaches it with
    /// [`Context::with_accounts`](crate::Context::with_accounts).
    #[must_use]
    pub fn new(db: Database) -> Self {
        let repository = Arc::new(AccountRepository::new(db));

        Self {
            settings: Arc::new(UserSettings::new(Arc::clone(&repository))),
            repository,
        }
    }

    /// The per-user settings facet.
    ///
    /// Plugins that resolve settings per command clone the handle once at construction —
    /// cloning shares the cache and the pool.
    #[must_use]
    pub const fn settings(&self) -> &Arc<UserSettings> {
        &self.settings
    }

    /// Records that `identity` was seen from its hostmask.
    ///
    /// Observations ride along automatically on every settings read and write; call this to
    /// track an identity the settings do not touch. Best-effort: a failed observation is logged
    /// and swallowed, since tracking must never break the caller.
    pub async fn observe(&self, identity: &UserIdentity) {
        if let Err(error) = self
            .repository
            .observe_hostmask(identity.key(), identity.hostmask())
            .await
        {
            tracing::debug!(
                exception.message = %crate::Chain(&error),
                "could not record an observed hostmask"
            );
        }
    }

    /// The hostmasks `identity` has been seen from, most recently used first.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the query fails.
    pub async fn hostmasks(
        &self,
        identity: &UserIdentity,
    ) -> Result<Vec<ObservedHostmask>, DbError> {
        self.repository.hostmasks(identity.key()).await
    }
}
