//! The `user_settings` and `user_hostmasks` queries.
//!
//! One repository for both tables of the account subsystem: the settings facet reads and writes
//! values, and the hostmask tracking records where identities have been seen from.

use crate::accounts::hostmasks::ObservedHostmask;
use crate::database::DbError;
use crate::impl_repository;
use zeta_plugin_macros::instrument_database;

impl_repository! {
    /// Repository for the account subsystem: settings values and observed hostmasks.
    AccountRepository
}

impl AccountRepository {
    /// Returns the stored value of `plugin.key` for `identity`, if it was ever set.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the query fails.
    #[instrument_database(operation = "SELECT", collection = "user_settings")]
    pub(crate) async fn setting_value(
        &self,
        identity: &str,
        plugin: &str,
        key: &str,
    ) -> Result<Option<String>, DbError> {
        sqlx::query_scalar(
            "SELECT value
             FROM user_settings
             WHERE identity = $1 AND plugin = $2 AND key = $3",
        )
        .bind(identity)
        .bind(plugin)
        .bind(key)
        .fetch_optional(&self.db)
        .await
        .map_err(|error| DbError::load("user setting", error))
    }

    /// Stores `value` for `plugin.key` and `identity`, replacing any stored value.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the insert fails.
    #[instrument_database(operation = "INSERT", collection = "user_settings")]
    pub(crate) async fn store_setting(
        &self,
        identity: &str,
        plugin: &str,
        key: &str,
        value: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO user_settings (identity, plugin, key, value, updated_at)
             VALUES ($1, $2, $3, $4, now())
             ON CONFLICT (identity, plugin, key)
             DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(identity)
        .bind(plugin)
        .bind(key)
        .bind(value)
        .execute(&self.db)
        .await
        .map_err(|error| DbError::insert("user setting", error))?;

        Ok(())
    }

    /// Removes the stored value of `plugin.key` for `identity`, returning whether one existed.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the delete fails.
    #[instrument_database(operation = "DELETE", collection = "user_settings")]
    pub(crate) async fn delete_setting(
        &self,
        identity: &str,
        plugin: &str,
        key: &str,
    ) -> Result<bool, DbError> {
        let deleted = sqlx::query(
            "DELETE FROM user_settings
             WHERE identity = $1 AND plugin = $2 AND key = $3",
        )
        .bind(identity)
        .bind(plugin)
        .bind(key)
        .execute(&self.db)
        .await
        .map_err(|error| DbError::delete("user setting", error))?;

        Ok(deleted.rows_affected() > 0)
    }

    /// Records that `identity` was seen from `hostmask`, refreshing `last_seen`.
    ///
    /// `first_seen` is set on the first observation and never moves.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the upsert fails.
    #[instrument_database(operation = "INSERT", collection = "user_hostmasks")]
    pub(crate) async fn observe_hostmask(
        &self,
        identity: &str,
        hostmask: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO user_hostmasks (identity, hostmask)
             VALUES ($1, $2)
             ON CONFLICT (identity, hostmask)
             DO UPDATE SET last_seen = now()",
        )
        .bind(identity)
        .bind(hostmask)
        .execute(&self.db)
        .await
        .map_err(|error| DbError::insert("observed hostmask", error))?;

        Ok(())
    }

    /// Records `batch` of `(identity, hostmask)` observations in one statement, refreshing
    /// `last_seen` per row.
    ///
    /// `first_seen` is set on the first observation and never moves.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the upsert fails; the caller keeps the batch dirty and retries it
    /// whole.
    #[instrument_database(operation = "INSERT", collection = "user_hostmasks")]
    pub(crate) async fn observe_hostmasks(
        &self,
        batch: &[(String, String)],
    ) -> Result<(), DbError> {
        let (identities, hostmasks): (Vec<&str>, Vec<&str>) = batch
            .iter()
            .map(|(identity, hostmask)| (identity.as_str(), hostmask.as_str()))
            .unzip();

        sqlx::query(
            "INSERT INTO user_hostmasks (identity, hostmask)
             SELECT identity, hostmask
             FROM unnest($1::text[], $2::text[]) AS observed(identity, hostmask)
             ON CONFLICT (identity, hostmask)
             DO UPDATE SET last_seen = now()",
        )
        .bind(&identities)
        .bind(&hostmasks)
        .execute(&self.db)
        .await
        .map_err(|error| DbError::insert("observed hostmask", error))?;

        Ok(())
    }

    /// Returns the hostmasks `identity` has been seen from, most recently used first.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if the query fails.
    #[instrument_database(operation = "SELECT", collection = "user_hostmasks")]
    pub(crate) async fn hostmasks(&self, identity: &str) -> Result<Vec<ObservedHostmask>, DbError> {
        sqlx::query_as(
            "SELECT hostmask, first_seen, last_seen
             FROM user_hostmasks
             WHERE identity = $1
             ORDER BY last_seen DESC",
        )
        .bind(identity)
        .fetch_all(&self.db)
        .await
        .map_err(|error| DbError::load("observed hostmask", error))
    }
}
