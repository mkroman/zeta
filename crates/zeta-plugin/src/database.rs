//! Database helpers shared by the DB-backed plugins.
//!
//! The host connects the pool and applies the migrations; the plugins' repositories reach it
//! through their context and persist their state through the [`DbError`] and
//! [`impl_repository_error!`](crate::impl_repository_error) helpers here.
//!
//! The module is split by concern: `retry` waits out a transient startup load failure and reads
//! the pool's namespace, `error` holds the reply-grade [`DbError`], and `repository` holds the
//! attribution and delete helpers plus the [`impl_repository!`](crate::impl_repository) and
//! [`impl_repository_error!`](crate::impl_repository_error) macros. Every public item is
//! re-exported here, so `zeta_plugin::database::{…}` stays the import path for all of them.

mod error;
mod repository;
mod retry;
#[cfg(test)]
mod tests;

pub use error::DbError;
pub use repository::{Attribution, delete_ids};
pub use retry::{LOAD_ATTEMPTS, LOAD_BACKOFF, load_with_retry, namespace};

/// Database connection pool.
pub type Database = sqlx::PgPool;
