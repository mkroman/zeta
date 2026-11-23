//! The cached store of per-user preference values.

use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use zeta_cache::TtlMap;

use crate::accounts::identity::UserIdentity;
use crate::accounts::repository::AccountRepository;
use crate::database::DbError;

use super::spec::PreferenceSpec;

/// How long a stored value stays cached before the next read goes to the database.
const CACHE_TTL: Duration = Duration::from_mins(5);
/// How long a cached miss (a user who never set the preference) stays cached.
///
/// Misses are cheaper to re-derive than values and the set path invalidates them anyway, so a
/// shorter TTL only bounds how long an externally deleted row can hide.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
/// The cached identity-and-preference pairs held before the closest-to-expiring entries are
/// evicted.
const CACHE_CAPACITY: usize = 4096;

/// The cache identity of one user's one preference.
///
/// The plugin and key come from the descriptor and reduce to one hash, so the key carries only
/// the identity string besides it.
#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey {
    identity: String,
    preference: u64,
}

impl CacheKey {
    /// Builds the cache key for `identity` and the preference `spec` addresses.
    fn new(identity: &UserIdentity, spec: &PreferenceSpec) -> Self {
        let mut hasher = DefaultHasher::new();
        spec.plugin.hash(&mut hasher);
        spec.key.hash(&mut hasher);

        Self {
            identity: identity.key().to_owned(),
            preference: hasher.finish(),
        }
    }
}

/// The read-through cached access point to the per-user preference values.
///
/// Obtained from [`Accounts`](crate::accounts::Accounts) — `ctx.accounts().preferences()` — or
/// cloned out of it once at plugin construction, since cloning shares the cache and the pool.
///
/// Reads serve from the cache — positive and negative — and hit the database on a miss; writes
/// go to the database first and then drop the cache entry, so a change is visible to every
/// reader, in every plugin, immediately. Values are plain strings; the [`value`](Self::value)
/// and [`value_or_default`](Self::value_or_default) helpers canonicalize them against the
/// descriptor, so a consumer never sees a value the preference's kind would not accept.
#[derive(Clone)]
pub struct Preferences {
    /// The account repository the reads fall back to and the writes go through.
    repository: Arc<AccountRepository>,
    /// The cached values, positive and negative, keyed by identity and preference.
    cache: Arc<TtlMap<CacheKey, String>>,
}

// Manual implementation: the pool and the cache are opaque internals.
impl fmt::Debug for Preferences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Preferences").finish_non_exhaustive()
    }
}

impl Preferences {
    /// Constructs the store over the shared account repository.
    pub(crate) fn new(repository: Arc<AccountRepository>) -> Self {
        Self {
            repository,
            cache: Arc::new(TtlMap::new_with_negative_ttl(
                CACHE_TTL,
                NEGATIVE_CACHE_TTL,
                CACHE_CAPACITY,
            )),
        }
    }

    /// Returns the raw value `identity` stored for the preference `spec`, if they ever set one.
    ///
    /// The raw stored string, as written: prefer [`Preferences::value`] or
    /// [`Preferences::value_or_default`], which canonicalize against the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails; nothing is cached in that case.
    pub async fn raw(
        &self,
        identity: &UserIdentity,
        spec: &PreferenceSpec,
    ) -> Result<Option<String>, DbError> {
        let key = CacheKey::new(identity, spec);

        self.cache
            .get_or_refresh(key, || async {
                self.repository
                    .preference_value(identity.key(), spec.plugin, spec.key)
                    .await
            })
            .await
    }

    /// Returns the stored value of the preference `spec` for `identity`, canonicalized against
    /// the descriptor.
    ///
    /// [`None`] when the user never set one; also [`None`] when a stored value is no longer one
    /// of the descriptor's values (a preference whose value list shrank), which the consumer
    /// then treats exactly like unset.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails.
    pub async fn value(
        &self,
        identity: &UserIdentity,
        spec: &PreferenceSpec,
    ) -> Result<Option<&'static str>, DbError> {
        let stored = self.raw(identity, spec).await?;

        Ok(stored.and_then(|value| spec.canonical(&value)))
    }

    /// Returns the preference `spec`'s value for `identity`, resolving the descriptor default
    /// when the user has not set one.
    ///
    /// The one call most consumers want: stored canonical value, else the descriptor's default,
    /// else [`None`] when the preference's unset behavior is not a stored value at all.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails.
    pub async fn value_or_default(
        &self,
        identity: &UserIdentity,
        spec: &PreferenceSpec,
    ) -> Result<Option<&'static str>, DbError> {
        self.value(identity, spec)
            .await?
            .map_or_else(|| Ok(spec.default), |value| Ok(Some(value)))
    }

    /// Stores `value` for the preference `spec` and `identity`, replacing any stored value.
    ///
    /// The value must be canonical — what [`PreferenceSpec::canonical`] returns for the input
    /// the user typed — so stored values and listed values never drift apart. The cache entry is
    /// invalidated, so the next read observes the write regardless of what it cached before.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the insert fails; the cache is left alone in that case.
    pub async fn set(
        &self,
        identity: &UserIdentity,
        spec: &PreferenceSpec,
        value: &'static str,
    ) -> Result<(), DbError> {
        self.repository
            .store_preference(identity.key(), spec.plugin, spec.key, value)
            .await?;
        self.cache.remove(&CacheKey::new(identity, spec));

        Ok(())
    }

    /// Removes the stored value of the preference `spec` for `identity`, returning whether one
    /// existed, and invalidates its cache entry the same way [`Preferences::set`] does.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the delete fails.
    pub async fn reset(
        &self,
        identity: &UserIdentity,
        spec: &PreferenceSpec,
    ) -> Result<bool, DbError> {
        let deleted = self
            .repository
            .delete_preference(identity.key(), spec.plugin, spec.key)
            .await?;
        self.cache.remove(&CacheKey::new(identity, spec));

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use crate::accounts::preferences::spec::{PreferenceKind, PreferenceSpec};

    use super::*;

    static ACCENT: PreferenceSpec = PreferenceSpec {
        plugin: "service-test",
        key: "accent",
        kind: PreferenceKind::Enum(&["DA", "DE"]),
        description: "the accent",
        default: Some("DE"),
    };

    /// The identity of the account `name`, connecting from `nick!user@host`.
    fn identity(account: &str) -> UserIdentity {
        UserIdentity::from_parts(account, crate::event::Sender::new("nick", "user", "host"))
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn set_value_and_default_round_trip_per_identity(db: sqlx::PgPool) {
        let preferences = crate::accounts::Accounts::new(db).preferences().clone();
        let alice = identity("alice");
        let bob = identity("unset");

        assert_eq!(
            preferences.value_or_default(&alice, &ACCENT).await.unwrap(),
            Some("DE"),
            "the default"
        );
        preferences.set(&alice, &ACCENT, "DA").await.unwrap();
        assert_eq!(
            preferences.value(&alice, &ACCENT).await.unwrap(),
            Some("DA")
        );
        assert_eq!(
            preferences.value_or_default(&alice, &ACCENT).await.unwrap(),
            Some("DA")
        );

        // Identities are independent: another identity reads its own (unset) value.
        assert_eq!(
            preferences.value_or_default(&bob, &ACCENT).await.unwrap(),
            Some("DE")
        );

        assert!(preferences.reset(&alice, &ACCENT).await.unwrap());
        assert!(!preferences.reset(&alice, &ACCENT).await.unwrap());
        assert_eq!(
            preferences.value_or_default(&alice, &ACCENT).await.unwrap(),
            Some("DE")
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn identities_are_case_insensitive(db: sqlx::PgPool) {
        let preferences = crate::accounts::Accounts::new(db).preferences().clone();

        preferences
            .set(&identity("Alice"), &ACCENT, "DA")
            .await
            .unwrap();
        assert_eq!(
            preferences
                .value(&identity("alice"), &ACCENT)
                .await
                .unwrap(),
            Some("DA"),
            "account keys normalize their case, like the services that issue them"
        );

        preferences
            .set(&identity("unset"), &ACCENT, "DE")
            .await
            .unwrap();
        assert_eq!(
            preferences
                .value(&identity("unset"), &ACCENT)
                .await
                .unwrap(),
            Some("DE"),
            "every identity normalizes its case"
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn a_write_is_visible_through_the_cache_immediately(db: sqlx::PgPool) {
        let preferences = crate::accounts::Accounts::new(db).preferences().clone();
        let user = identity("alice");

        // Populate the cache both ways: a stored value and a cached miss.
        preferences.set(&user, &ACCENT, "DA").await.unwrap();
        assert_eq!(preferences.value(&user, &ACCENT).await.unwrap(), Some("DA"));
        assert_eq!(
            preferences
                .value(&identity("unset"), &ACCENT)
                .await
                .unwrap(),
            None
        );

        // The write path invalidates what the reads cached.
        preferences.set(&user, &ACCENT, "DE").await.unwrap();
        assert_eq!(preferences.value(&user, &ACCENT).await.unwrap(), Some("DE"));

        preferences.reset(&user, &ACCENT).await.unwrap();
        assert_eq!(preferences.value(&user, &ACCENT).await.unwrap(), None);

        // The previously-cached miss sees a set that happened after it.
        preferences
            .set(&identity("unset"), &ACCENT, "DA")
            .await
            .unwrap();
        assert_eq!(
            preferences
                .value_or_default(&identity("unset"), &ACCENT)
                .await
                .unwrap(),
            Some("DA")
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn values_and_misses_are_served_from_the_cache(db: sqlx::PgPool) {
        let preferences = crate::accounts::Accounts::new(db.clone())
            .preferences()
            .clone();
        let user = identity("alice");

        preferences.set(&user, &ACCENT, "DA").await.unwrap();
        assert_eq!(preferences.value(&user, &ACCENT).await.unwrap(), Some("DA"));

        // Take the database away: cached reads still answer, misses for unknown identities fail.
        db.close().await;

        assert_eq!(
            preferences.value(&user, &ACCENT).await.unwrap(),
            Some("DA"),
            "the cached value is served without the database"
        );
        assert!(
            preferences
                .value(&identity("nobody"), &ACCENT)
                .await
                .is_err()
        );
    }
}
