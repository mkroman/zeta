//! Filter service, persisting filters in the database and keeping them indexed in memory.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use tracing::{debug, instrument};
use url::Url;

use super::{
    error::Error,
    index::FilterIndex,
    model::{Filter, NewFilter},
    repository::FilterRepository,
};
use zeta_plugin::Sender;
use crate::database::Database;

/// Stores filters in the database and mirrors them in an in-memory index.
///
/// The database is the source of truth; the index is rebuilt from it on [`load`](Self::load) and
/// kept in sync by [`add`](Self::add) and [`delete_ids`](Self::delete_ids). The hot lookup path
/// ([`matches`](Self::matches)) only touches the index and never the database, so filters are
/// loaded once at launch and shared with the URL-handling plugins through `SharedState`.
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
        let filters = self.repo.list().await?;
        let mut index = FilterIndex::default();

        for filter in filters {
            index.insert(filter);
        }

        let count = index.filters().len();
        debug!(count, "loaded filters into index");

        *self.lock_index_mut() = index;

        Ok(())
    }

    /// Inserts `filter` into the database and the index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Database`] if the filter could not be inserted into the database.
    #[instrument(skip_all, err)]
    pub async fn add(&self, filter: NewFilter) -> Result<Filter, Error> {
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
    pub async fn delete_ids(&self, ids: &[i32]) -> Result<u64, Error> {
        let removed = self.repo.delete_all(ids).await?;

        self.lock_index_mut().remove_ids(ids);

        Ok(removed)
    }

    /// Returns a snapshot of all filters, in id order.
    #[must_use]
    pub fn list(&self) -> Vec<Filter> {
        self.lock_index().filters()
    }

    /// Returns a snapshot of the filters matching `criteria`, in id order.
    #[must_use]
    pub fn select(&self, criteria: &Criteria) -> Vec<Filter> {
        self.list()
            .into_iter()
            .filter(|filter| criteria.matches(filter))
            .collect()
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter.
    #[must_use]
    pub fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.lock_index().matches(channel, sender, url)
    }

    /// Locks the index for reading, continuing on poisoning.
    fn lock_index(&self) -> RwLockReadGuard<'_, FilterIndex> {
        crate::sync::read(&self.index)
    }

    /// Locks the index for writing, continuing on poisoning.
    fn lock_index_mut(&self) -> RwLockWriteGuard<'_, FilterIndex> {
        crate::sync::write(&self.index)
    }
}

/// A set of filter criteria for selecting filters in listings and deletions.
///
/// Every criterion is optional; a filter is selected when all of the set criteria match its
/// corresponding field. Wildcards are supported in every criterion; paths are compared
/// case-sensitively, everything else case-insensitively.
#[derive(Debug, Default, Clone)]
pub struct Criteria {
    /// The channel pattern, or `None` for any channel.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
}

impl Criteria {
    /// Whether no criterion is set.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.channel.is_none()
            && self.host.is_none()
            && self.path.is_none()
            && self.nickname.is_none()
            && self.username.is_none()
            && self.hostname.is_none()
    }

    /// Whether the criteria select `filter`.
    #[must_use]
    pub fn matches(&self, filter: &Filter) -> bool {
        Self::field(self.channel.as_deref(), filter.channel.as_deref(), false)
            && Self::field(self.host.as_deref(), filter.host.as_deref(), false)
            && Self::field(self.path.as_deref(), filter.path.as_deref(), true)
            && Self::field(self.nickname.as_deref(), filter.nickname.as_deref(), false)
            && Self::field(self.username.as_deref(), filter.username.as_deref(), false)
            && Self::field(self.hostname.as_deref(), filter.hostname.as_deref(), false)
    }

    /// Whether `pattern` matches `value`, or is unset.
    fn field(pattern: Option<&str>, value: Option<&str>, case_sensitive: bool) -> bool {
        let Some(pattern) = pattern.map(str::trim).filter(|pattern| !pattern.is_empty()) else {
            return true;
        };

        let Some(value) = value else {
            return false;
        };

        if case_sensitive {
            wildmatch::WildMatch::new(pattern).matches(value)
        } else {
            wildmatch::WildMatch::new(&pattern.to_lowercase()).matches(&value.to_lowercase())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(host: Option<&str>, path: Option<&str>) -> Filter {
        Filter {
            id: 1,
            channel: Some("#foo".into()),
            host: host.map(String::from),
            path: path.map(String::from),
            nickname: None,
            username: None,
            hostname: None,
        }
    }

    #[test]
    fn criteria_match_selectively() {
        let criteria = Criteria {
            host: Some("*.com".into()),
            ..Criteria::default()
        };

        assert!(criteria.matches(&filter(Some("reuters.com"), None)));
        assert!(!criteria.matches(&filter(Some("dr.dk"), None)));
    }

    #[test]
    fn empty_criteria_match_everything() {
        let criteria = Criteria::default();

        assert!(criteria.matches(&filter(None, None)));
    }

    #[test]
    fn set_criteria_do_not_match_unset_fields() {
        let criteria = Criteria {
            host: Some("dr.dk".into()),
            ..Criteria::default()
        };

        assert!(!criteria.matches(&filter(None, None)));
    }

    #[test]
    fn path_criteria_are_case_sensitive() {
        let criteria = Criteria {
            path: Some("/Title/*".into()),
            ..Criteria::default()
        };

        assert!(!criteria.matches(&filter(None, Some("/title/*"))));

        let criteria = Criteria {
            path: Some("/title/*".into()),
            ..Criteria::default()
        };

        assert!(criteria.matches(&filter(None, Some("/title/*"))));
    }

    #[test]
    fn host_criteria_are_case_insensitive() {
        let criteria = Criteria {
            host: Some("DR.DK".into()),
            ..Criteria::default()
        };

        assert!(criteria.matches(&filter(Some("dr.dk"), None)));
    }

    /// Skips the test if a test database has not been configured. The `filters` table must
    /// exist — start the bot once against the test database to apply the migrations.
    async fn test_service() -> Option<FilterService> {
        let db = crate::database::connect_for_tests().await?;

        Some(FilterService::new(db))
    }

    #[tokio::test]
    async fn add_delete_and_load_roundtrip() {
        let Some(service) = test_service().await else {
            return;
        };

        let added = service
            .add(NewFilter {
                channel: Some("#smoke".into()),
                host: Some("example.com".into()),
                path: None,
                nickname: None,
                username: Some("*other".into()),
                hostname: None,
                created_by: "smoke".into(),
            })
            .await
            .unwrap();

        assert_eq!(added.channel.as_deref(), Some("#smoke"));
        assert_eq!(added.username.as_deref(), Some("*other"));
        assert!(service.list().iter().any(|f| f.id == added.id));

        let removed = service.delete_ids(&[added.id]).await.unwrap();

        assert_eq!(removed, 1);
        assert!(!service.list().iter().any(|f| f.id == added.id));

        // The database is the source of truth: a fresh load restores the index without the
        // deleted filter.
        service.load().await.unwrap();

        assert!(!service.list().iter().any(|f| f.id == added.id));
        assert!(!service.matches(
            "#smoke",
            Some(Sender::new("someone", "~cliother", "host.example")),
            &"https://example.com/page".parse().unwrap()
        ));

        let re_added = service
            .add(NewFilter {
                channel: None,
                host: Some("*.com".into()),
                path: None,
                nickname: None,
                username: None,
                hostname: None,
                created_by: "smoke".into(),
            })
            .await
            .unwrap();

        assert!(service.matches(
            "#anywhere",
            Some(Sender::new("nick", "user", "host.example")),
            &"https://reuters.com/article".parse().unwrap()
        ));

        service.delete_ids(&[re_added.id]).await.unwrap();
    }
}
