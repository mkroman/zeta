//! Filter service, persisting filters in the database and keeping them indexed in memory.

use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::{debug, instrument};
use url::Url;

use super::{
    error::Error,
    index::FilterIndex,
    model::{Criteria, Filter, NewFilter},
    repository::FilterRepository,
};
use zeta_plugin::database::Database;
use zeta_plugin::{Sender, UrlFilter};

/// Stores filters in the database and mirrors them in an in-memory index.
///
/// The database is the source of truth; the index is rebuilt from it on [`load`](Self::load) and
/// kept in sync by [`add`](Self::add) and [`delete_ids`](Self::delete_ids). The hot lookup path
/// ([`is_filtered`](Self::is_filtered)) only touches the index and never the database, so filters are
/// loaded once at launch and the service is shared through `SharedState`.
pub struct FilterService {
    /// The filter repository.
    repo: FilterRepository,
    /// The in-memory index of all filters.
    index: ArcSwap<FilterIndex>,
}

impl FilterService {
    /// Creates a new filter service backed by the given database pool.
    #[must_use]
    pub fn new(db: Database) -> Self {
        Self {
            repo: FilterRepository::new(db),
            index: ArcSwap::from_pointee(FilterIndex::default()),
        }
    }

    /// Loads all filters from the database into the index, replacing its contents.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filters could not be fetched from the database.
    #[instrument(name = "filter.load", skip_all, err)]
    pub async fn load(&self) -> Result<(), Error> {
        let mut index = FilterIndex::default();

        for filter in self.repo.list().await? {
            index.insert(filter);
        }

        debug!(filter.count = index.len(), "loaded filters into index");

        self.index.store(Arc::new(index));

        Ok(())
    }

    /// Inserts `filter` into the database and the index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filter could not be inserted into the database.
    #[instrument(name = "filter.add", skip_all, err)]
    pub async fn add(&self, filter: NewFilter) -> Result<Filter, Error> {
        let filter = self.repo.insert(filter).await?;

        self.index.rcu(|index| {
            let mut next = (**index).clone();
            next.insert(filter.clone());

            Arc::new(next)
        });

        Ok(filter)
    }

    /// Deletes the filters with the given `ids` from the database and the index, returning the
    /// number of filters removed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filters could not be deleted from the database.
    #[instrument(name = "filter.delete", skip_all, err)]
    pub async fn delete_ids(&self, ids: &[i32]) -> Result<u64, Error> {
        let removed = self.repo.delete_all(ids).await?;

        self.index.rcu(|index| {
            let mut next = (**index).clone();
            next.remove_ids(ids);

            Arc::new(next)
        });

        Ok(removed)
    }

    /// Returns a snapshot of all filters, in id order.
    #[must_use]
    pub fn list(&self) -> Vec<Filter> {
        self.index.load().filters()
    }

    /// Returns a snapshot of the filters matching `criteria`, in id order.
    #[must_use]
    pub fn select(&self, criteria: &Criteria) -> Vec<Filter> {
        // Compile once for the whole listing, not once per filter.
        let criteria = criteria.compile();

        self.list()
            .into_iter()
            .filter(|filter| criteria.matches(filter))
            .collect()
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter.
    #[must_use]
    pub fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.index.load().matches(channel, sender, url)
    }

    /// Whether messages from `sender` in `channel` are ignored entirely: the index holds a
    /// filter without URL criteria whose sender criteria match.
    #[must_use]
    pub fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.index.load().ignores(channel, sender)
    }
}

/// Exposes the service as the shared URL filter: the plugin publishes it wrapped in
/// [`Filters`](zeta_plugin::Filters) at construction, and the host consults it through the
/// same facade before delivering URL events and messages from ignored senders.
impl UrlFilter for FilterService {
    fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.is_filtered(channel, sender, url)
    }

    fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.ignores(channel, sender)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filter fixture. The `created_by` column is part of the model; with per-test databases
    /// the value no longer doubles as a cleanup marker.
    fn new_filter(channel: Option<&str>, host: Option<&str>, username: Option<&str>) -> NewFilter {
        NewFilter {
            channel: channel.map(String::from),
            host: host.map(String::from),
            path: None,
            nickname: None,
            username: username.map(String::from),
            hostname: None,
            created_by: "test".into(),
        }
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn added_filters_are_listed_with_their_fields(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        let added = service
            .add(new_filter(
                Some("#smoke"),
                Some("example.com"),
                Some("*other"),
            ))
            .await
            .unwrap();

        assert_eq!(added.channel.as_deref(), Some("#smoke"));
        assert_eq!(added.host.as_deref(), Some("example.com"));
        assert_eq!(added.username.as_deref(), Some("*other"));
        assert!(service.list().iter().any(|f| f.id == added.id));
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn deleted_filters_leave_the_database_and_the_index(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        let added = service
            .add(new_filter(
                Some("#smoke"),
                Some("example.com"),
                Some("*other"),
            ))
            .await
            .unwrap();

        let removed = service.delete_ids(&[added.id]).await.unwrap();

        assert_eq!(removed, 1);

        assert!(!service.is_filtered(
            "#smoke",
            Some(Sender::new("someone", "~cliother", "host.example")),
            &"https://example.com/page".parse().unwrap()
        ));

        // The database is the source of truth: a fresh load restores the index without the
        // deleted filter.
        service.load().await.unwrap();

        assert!(!service.list().iter().any(|f| f.id == added.id));
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn wildcard_filters_match_any_channel(db: sqlx::PgPool) {
        let service = FilterService::new(db);

        service
            .add(new_filter(None, Some("*.com"), None))
            .await
            .unwrap();

        assert!(service.is_filtered(
            "#anywhere",
            Some(Sender::new("nick", "user", "host.example")),
            &"https://reuters.com/article".parse().unwrap()
        ));
    }

    #[sqlx::test(migrations = "../plugins/migrations")]
    async fn matches_and_ignores_through_the_url_filter_trait(db: sqlx::PgPool) {
        let url: Url = "https://imdb.com/title/tt0111161".parse().unwrap();
        let matching = Some(Sender::new("nick", "someuser", "host.example"));
        let other = Some(Sender::new("nick", "other", "host.example"));

        let url_only = FilterService::new(db.clone());

        url_only
            .add(new_filter(None, Some("imdb.com"), Some("someuser")))
            .await
            .unwrap();

        assert!(UrlFilter::is_filtered(&url_only, "#chan", matching, &url));
        assert!(!UrlFilter::is_filtered(&url_only, "#chan", other, &url));
        // A filter with URL criteria filters those URLs without ignoring the sender entirely.
        assert!(!UrlFilter::ignores(&url_only, "#chan", matching));

        let sender_only = FilterService::new(db);

        sender_only
            .add(new_filter(None, None, Some("someuser")))
            .await
            .unwrap();

        assert!(UrlFilter::ignores(&sender_only, "#chan", matching));
        assert!(!UrlFilter::ignores(&sender_only, "#chan", other));
    }
}
