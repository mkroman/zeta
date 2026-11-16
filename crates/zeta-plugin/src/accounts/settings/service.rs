//! The cached store of per-user settings values.

use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use zeta_cache::TtlMap;

use crate::accounts::identity::UserIdentity;
use crate::accounts::repository::AccountRepository;
use crate::database::DbError;

use super::spec::UserSettingSpec;

/// How long a stored value stays cached before the next read goes to the database.
const CACHE_TTL: Duration = Duration::from_mins(5);
/// How long a cached miss (a user who never set the setting) stays cached.
///
/// Misses are cheaper to re-derive than values and the set path invalidates them anyway, so a
/// shorter TTL only bounds how long an externally deleted row can hide.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
/// The cached identity-and-setting pairs held before the closest-to-expiring entries are
/// evicted.
const CACHE_CAPACITY: usize = 4096;

/// The cache identity of one user's one setting.
///
/// The plugin and key come from a `&'static` descriptor and reduce to one hash, so the key
/// carries only the identity string besides it.
#[derive(Clone, Eq, Hash, PartialEq)]
struct CacheKey {
    identity: String,
    setting: u64,
}

impl CacheKey {
    /// Builds the cache key for `identity` and the setting `spec` addresses.
    fn new(identity: &UserIdentity, spec: &UserSettingSpec) -> Self {
        let mut hasher = DefaultHasher::new();
        spec.plugin.hash(&mut hasher);
        spec.key.hash(&mut hasher);

        Self {
            identity: identity.key().to_owned(),
            setting: hasher.finish(),
        }
    }
}

/// The read-through cached access point to the per-user settings values.
///
/// Obtained from [`Accounts`](crate::accounts::Accounts) — `ctx.accounts().settings()` — or
/// cloned out of it once at plugin construction, since cloning shares the cache and the pool.
///
/// Reads serve from the cache — positive and negative — and hit the database on a miss; writes
/// go to the database first and then drop the cache entry, so a change is visible to every
/// reader, in every plugin, immediately. Values are plain strings; the `read` and `resolved`
/// helpers canonicalize them against the descriptor, so a consumer never sees a value the
/// setting's kind would not accept.
#[derive(Clone)]
pub struct UserSettings {
    /// The account repository the reads fall back to and the writes go through.
    repository: Arc<AccountRepository>,
    /// The cached values, positive and negative, keyed by identity and setting.
    cache: Arc<TtlMap<CacheKey, String>>,
}

// Manual implementation: the pool and the cache are opaque internals.
impl fmt::Debug for UserSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserSettings").finish_non_exhaustive()
    }
}

impl UserSettings {
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

    /// Returns the raw value `identity` stored for the setting `spec`, if they ever set one.
    ///
    /// The raw stored string, as written: prefer [`UserSettings::read`] or
    /// [`UserSettings::resolved`], which canonicalize against the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails; nothing is cached in that case.
    pub async fn get(
        &self,
        identity: &UserIdentity,
        spec: &UserSettingSpec,
    ) -> Result<Option<String>, DbError> {
        let key = CacheKey::new(identity, spec);

        self.cache
            .get_or_refresh(key, || async {
                self.repository
                    .setting_value(identity.key(), spec.plugin, spec.key)
                    .await
            })
            .await
    }

    /// Returns the stored value of the setting `spec` for `identity`, canonicalized against the
    /// descriptor.
    ///
    /// [`None`] when the user never set one; also [`None`] when a stored value is no longer one
    /// of the descriptor's values (a setting whose value list shrank), which the consumer then
    /// treats exactly like unset.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails.
    pub async fn read(
        &self,
        identity: &UserIdentity,
        spec: &'static UserSettingSpec,
    ) -> Result<Option<&'static str>, DbError> {
        let stored = self.get(identity, spec).await?;

        Ok(stored.and_then(|value| spec.canonical(&value)))
    }

    /// Returns the setting `spec`'s value for `identity`, resolving the descriptor default when
    /// the user has not set one.
    ///
    /// The one call most consumers want: stored canonical value, else the descriptor's default,
    /// else [`None`] when the setting's unset behavior is not a stored value at all.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the backing query fails.
    pub async fn resolved(
        &self,
        identity: &UserIdentity,
        spec: &'static UserSettingSpec,
    ) -> Result<Option<&'static str>, DbError> {
        self.read(identity, spec)
            .await?
            .map_or_else(|| Ok(spec.default), |value| Ok(Some(value)))
    }

    /// Stores `value` for the setting `spec` and `identity`, replacing any stored value.
    ///
    /// The value must be canonical — what [`UserSettingSpec::canonical`] returns for the input
    /// the user typed — so stored values and listed values never drift apart. The cache entry is
    /// invalidated, so the next read observes the write regardless of what it cached before.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the insert fails; the cache is left alone in that case.
    pub async fn set(
        &self,
        identity: &UserIdentity,
        spec: &UserSettingSpec,
        value: &'static str,
    ) -> Result<(), DbError> {
        self.repository
            .store_setting(identity.key(), spec.plugin, spec.key, value)
            .await?;
        self.cache.remove(&CacheKey::new(identity, spec));

        Ok(())
    }

    /// Removes the stored value of the setting `spec` for `identity`, returning whether one
    /// existed, and invalidates its cache entry the same way [`UserSettings::set`] does.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] when the delete fails.
    pub async fn reset(
        &self,
        identity: &UserIdentity,
        spec: &UserSettingSpec,
    ) -> Result<bool, DbError> {
        let deleted = self
            .repository
            .delete_setting(identity.key(), spec.plugin, spec.key)
            .await?;
        self.cache.remove(&CacheKey::new(identity, spec));

        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use crate::accounts::settings::spec::{UserSettingKind, UserSettingSpec};

    use super::*;

    static ACCENT: UserSettingSpec = UserSettingSpec {
        plugin: "service-test",
        key: "accent",
        kind: UserSettingKind::Enum(&["DA", "DE"]),
        description: "the accent",
        default: Some("DE"),
    };

    /// The identity of the account `name`, connecting from `nick!user@host`.
    fn identity(account: &str) -> UserIdentity {
        UserIdentity::from_parts(account, crate::event::Sender::new("nick", "user", "host"))
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn set_read_resolved_round_trips_per_identity(db: sqlx::PgPool) {
        let settings = crate::accounts::Accounts::new(db).settings().clone();
        let alice = identity("alice");
        let bob = identity("unset");

        assert_eq!(
            settings.resolved(&alice, &ACCENT).await.unwrap(),
            Some("DE"),
            "the default"
        );
        settings.set(&alice, &ACCENT, "DA").await.unwrap();
        assert_eq!(settings.read(&alice, &ACCENT).await.unwrap(), Some("DA"));
        assert_eq!(
            settings.resolved(&alice, &ACCENT).await.unwrap(),
            Some("DA")
        );

        // Identities are independent: another identity reads its own (unset) value.
        assert_eq!(settings.resolved(&bob, &ACCENT).await.unwrap(), Some("DE"));

        assert!(settings.reset(&alice, &ACCENT).await.unwrap());
        assert!(!settings.reset(&alice, &ACCENT).await.unwrap());
        assert_eq!(
            settings.resolved(&alice, &ACCENT).await.unwrap(),
            Some("DE")
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn identities_are_case_insensitive(db: sqlx::PgPool) {
        let settings = crate::accounts::Accounts::new(db).settings().clone();

        settings
            .set(&identity("Alice"), &ACCENT, "DA")
            .await
            .unwrap();
        assert_eq!(
            settings.read(&identity("alice"), &ACCENT).await.unwrap(),
            Some("DA"),
            "account keys normalize their case, like the services that issue them"
        );

        settings
            .set(&identity("unset"), &ACCENT, "DE")
            .await
            .unwrap();
        assert_eq!(
            settings.read(&identity("unset"), &ACCENT).await.unwrap(),
            Some("DE"),
            "hostmask fallback identities normalize their case"
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn a_write_is_visible_through_the_cache_immediately(db: sqlx::PgPool) {
        let settings = crate::accounts::Accounts::new(db).settings().clone();
        let user = identity("alice");

        // Populate the cache both ways: a stored value and a cached miss.
        settings.set(&user, &ACCENT, "DA").await.unwrap();
        assert_eq!(settings.read(&user, &ACCENT).await.unwrap(), Some("DA"));
        assert_eq!(
            settings.read(&identity("unset"), &ACCENT).await.unwrap(),
            None
        );

        // The write path invalidates what the reads cached.
        settings.set(&user, &ACCENT, "DE").await.unwrap();
        assert_eq!(settings.read(&user, &ACCENT).await.unwrap(), Some("DE"));

        settings.reset(&user, &ACCENT).await.unwrap();
        assert_eq!(settings.read(&user, &ACCENT).await.unwrap(), None);

        // The previously-cached miss sees a set that happened after it.
        settings
            .set(&identity("unset"), &ACCENT, "DA")
            .await
            .unwrap();
        assert_eq!(
            settings
                .resolved(&identity("unset"), &ACCENT)
                .await
                .unwrap(),
            Some("DA")
        );
    }

    #[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]
    async fn values_and_misses_are_served_from_the_cache(db: sqlx::PgPool) {
        let settings = crate::accounts::Accounts::new(db.clone())
            .settings()
            .clone();
        let user = identity("alice");

        settings.set(&user, &ACCENT, "DA").await.unwrap();
        assert_eq!(settings.read(&user, &ACCENT).await.unwrap(), Some("DA"));

        // Take the database away: cached reads still answer, misses for unknown identities fail.
        db.close().await;

        assert_eq!(
            settings.read(&user, &ACCENT).await.unwrap(),
            Some("DA"),
            "the cached value is served without the database"
        );
        assert!(settings.read(&identity("nobody"), &ACCENT).await.is_err());
    }
}
