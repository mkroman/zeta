//! Filter service, persisting filters in the database and keeping them indexed in memory.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use tracing::{debug, instrument};
use url::Url;

use super::{
    error::Error,
    index::FilterIndex,
    model::{Criteria, FilterRule, NewFilterRule},
    repository::FilterRepository,
};
use crate::database::Database;
use zeta_plugin::{FilterPolicy, Sender};

/// Stores filters in the database and mirrors them in an in-memory index.
///
/// The database is the source of truth; the index is rebuilt from it on [`load`](Self::load) and
/// kept in sync by [`create`](Self::create) and [`remove_by_ids`](Self::remove_by_ids). The hot
/// lookup path ([`FilterPolicy::blocks_url`]) only touches the index and never the database, so
/// filters are loaded once at launch and the service is shared through `SharedState`.
pub struct FilterService {
    /// The filter repository.
    repo: FilterRepository,
    /// The in-memory index of all filters.
    index: RwLock<FilterIndex>,
}

impl FilterService {
    /// Creates a new filter service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            repo: FilterRepository::new(db),
            index: RwLock::new(FilterIndex::default()),
        }
    }

    /// Loads all filters from the database into the index, replacing its contents.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filters could not be fetched from the database.
    #[instrument(skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        let index: FilterIndex = self.repo.list().await?.into_iter().collect();

        debug!(count = index.len(), "loaded filters into index");

        *self.lock_index_mut() = index;

        Ok(())
    }

    /// Inserts `filter` into the database and the index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filter could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn create(&self, filter: NewFilterRule) -> Result<FilterRule, Error> {
        let filter = self.repo.insert(filter).await?;

        self.lock_index_mut().insert(filter.clone());

        Ok(filter)
    }

    /// Deletes the filters with the given `ids` from the database and the index, returning the
    /// number of filters removed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filters could not be deleted from the database.
    #[instrument(skip_all, err)]
    pub async fn remove_by_ids(&self, ids: &[i32]) -> Result<u64, Error> {
        let removed = self.repo.delete_by_ids(ids).await?;

        self.lock_index_mut().remove_ids(ids);

        Ok(removed)
    }

    /// Whether no filters are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock_index().is_empty()
    }

    /// Returns a snapshot of all filters, in id order.
    #[must_use]
    pub fn all(&self) -> Vec<FilterRule> {
        self.lock_index().filters()
    }

    /// Returns the filter with `id`, if any.
    #[must_use]
    pub fn get(&self, id: i32) -> Option<FilterRule> {
        self.lock_index().get(id)
    }

    /// Returns a snapshot of the filters matching `criteria`, in id order.
    #[must_use]
    pub fn matching(&self, criteria: &Criteria) -> Vec<FilterRule> {
        self.lock_index().select(&criteria.compile())
    }

    /// Locks the index for reading, continuing on poisoning.
    fn lock_index(&self) -> RwLockReadGuard<'_, FilterIndex> {
        zeta_utils::sync::read(&self.index)
    }

    /// Locks the index for writing, continuing on poisoning.
    fn lock_index_mut(&self) -> RwLockWriteGuard<'_, FilterIndex> {
        zeta_utils::sync::write(&self.index)
    }
}

/// Exposes the service as the shared URL filter: the plugin publishes it wrapped in
/// [`FilterHandle`](zeta_plugin::FilterHandle) at construction, and the host consults it
/// through the [`FilterGate`](zeta_plugin::FilterGate) facade before delivering URL events and
/// messages from ignored senders.
impl FilterPolicy for FilterService {
    fn blocks_url(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.lock_index().matches(channel, sender, url)
    }

    fn ignores_sender(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.lock_index().ignores(channel, sender)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filter fixture. The `created_by` column is part of the model; with per-test databases
    /// the value no longer doubles as a cleanup marker.
    fn new_filter(
        channel: Option<&str>,
        host: Option<&str>,
        username: Option<&str>,
    ) -> NewFilterRule {
        NewFilterRule {
            channel: channel.map(String::from),
            host: host.map(String::from),
            path: None,
            nickname: None,
            username: username.map(String::from),
            hostname: None,
            created_by: "test".into(),
        }
    }

    #[sqlx::test]
    async fn added_filters_are_listed_with_their_fields(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        let added = service
            .create(new_filter(
                Some("#smoke"),
                Some("example.com"),
                Some("*other"),
            ))
            .await
            .unwrap();

        assert_eq!(added.channel.as_deref(), Some("#smoke"));
        assert_eq!(added.host.as_deref(), Some("example.com"));
        assert_eq!(added.username.as_deref(), Some("*other"));
        assert!(service.all().iter().any(|f| f.id == added.id));
        assert_eq!(service.get(added.id), Some(added));
    }

    #[sqlx::test]
    async fn deleted_filters_leave_the_database_and_the_index(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        let added = service
            .create(new_filter(
                Some("#smoke"),
                Some("example.com"),
                Some("*other"),
            ))
            .await
            .unwrap();

        let removed = service.remove_by_ids(&[added.id]).await.unwrap();

        assert_eq!(removed, 1);
        assert!(service.get(added.id).is_none());

        assert!(!FilterPolicy::blocks_url(
            &service,
            "#smoke",
            Some(Sender::new("someone", "~cliother", "host.example")),
            &"https://example.com/page".parse().unwrap()
        ));

        // The database is the source of truth: a fresh load restores the index without the
        // deleted filter.
        service.load().await.unwrap();

        assert!(service.is_empty());
        assert!(!service.all().iter().any(|f| f.id == added.id));
    }

    #[sqlx::test]
    async fn wildcard_filters_match_any_channel(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        service
            .create(new_filter(None, Some("*.com"), None))
            .await
            .unwrap();

        assert!(FilterPolicy::blocks_url(
            &service,
            "#anywhere",
            Some(Sender::new("nick", "user", "host.example")),
            &"https://reuters.com/article".parse().unwrap()
        ));
    }

    #[sqlx::test]
    async fn matches_urls_through_the_filter_policy(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        service
            .create(new_filter(None, Some("imdb.com"), Some("someuser")))
            .await
            .unwrap();

        let url: Url = "https://imdb.com/title/tt0111161".parse().unwrap();

        assert!(FilterPolicy::blocks_url(
            &service,
            "#chan",
            Some(Sender::new("nick", "someuser", "host.example")),
            &url
        ));
        assert!(!FilterPolicy::blocks_url(
            &service,
            "#chan",
            Some(Sender::new("nick", "other", "host.example")),
            &url
        ));
    }

    #[sqlx::test]
    async fn ignores_senders_through_the_filter_policy(db: sqlx::PgPool) {
        let service = FilterService::new(db.clone());

        service
            .create(new_filter(None, None, Some("someuser")))
            .await
            .unwrap();

        assert!(FilterPolicy::ignores_sender(
            &service,
            "#chan",
            Some(Sender::new("nick", "someuser", "host.example"))
        ));
        assert!(!FilterPolicy::ignores_sender(
            &service,
            "#chan",
            Some(Sender::new("nick", "other", "host.example"))
        ));

        // A filter with URL criteria filters those URLs without ignoring the sender entirely.
        let url_only = FilterService::new(db.clone());

        url_only
            .create(new_filter(None, Some("imdb.com"), Some("someuser")))
            .await
            .unwrap();

        assert!(!FilterPolicy::ignores_sender(
            &url_only,
            "#chan",
            Some(Sender::new("nick", "someuser", "host.example"))
        ));
    }
}
