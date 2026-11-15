//! Repository scaffolding shared by the DB-backed plugins.
//!
//! [`Attribution`] groups the user and channel columns every insert records, [`delete_ids`]
//! removes rows by primary key, and [`impl_repository!`](crate::impl_repository) and
//! [`impl_repository_error!`](crate::impl_repository_error) generate the repository struct and
//! its error constructors.

use crate::event::Sender;

use super::Database;

/// The attribution of an inserted row to the user and channel it came from.
///
/// The `(nickname, username, hostname, channel)` columns are repeated by every plugin that
/// records who posted a link or created a row, so the `New*` insert models carry them as one
/// field. Row models keep the four columns as their own fields: they derive `FromRow` from the
/// queried column order, which the attribution does not change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    /// The nickname of the user.
    pub nickname: String,
    /// The username (ident) of the user.
    pub username: String,
    /// The hostname of the user.
    pub hostname: String,
    /// The channel the user acted in.
    pub channel: String,
}

impl Attribution {
    /// Constructs an attribution from its parts.
    #[must_use]
    pub fn new(nickname: &str, username: &str, hostname: &str, channel: &str) -> Self {
        Self {
            nickname: nickname.to_owned(),
            username: username.to_owned(),
            hostname: hostname.to_owned(),
            channel: channel.to_owned(),
        }
    }

    /// Attributes `channel` to `sender`.
    #[must_use]
    pub fn from_sender(sender: Sender<'_>, channel: &str) -> Self {
        Self::new(sender.nick, sender.username, sender.hostname, channel)
    }
}

/// Deletes the rows with the given `ids` from `table`, returning the number of deleted rows.
///
/// `table` must be a table known at compile time; it is never sourced from user input.
///
/// # Errors
///
/// Returns the `sqlx::Error` if the query fails.
pub async fn delete_ids(
    db: &Database,
    table: &'static str,
    ids: &[i32],
) -> Result<u64, sqlx::Error> {
    // The table name is a compile-time constant from the calling module, never user input.
    let query = sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE id = ANY($1)"));

    sqlx::query(query)
        .bind(ids)
        .execute(db)
        .await
        .map(|result| result.rows_affected())
}

/// Generates a repository struct backed by the shared database pool.
///
/// ```ignore
/// zeta_plugin::impl_repository! {
///     /// Repository for storing and retrieving alerts in the database.
///     AlertRepository
/// }
/// ```
///
/// The generated struct has a private `db` field and a `pub const fn new(db: Database)`, so the
/// repository's own `impl` block contains only queries.
#[macro_export]
macro_rules! impl_repository {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name {
            /// The database connection pool.
            db: $crate::database::Database,
        }

        impl $name {
            /// Creates a new repository backed by the given database pool.
            #[must_use]
            pub const fn new(db: $crate::database::Database) -> Self {
                Self { db }
            }
        }
    };
}

/// Generates the database error constructors for a repository error enum.
///
/// The generated `load`, `insert` and `delete` constructors wrap a [`sqlx::Error`] into a
/// [`DbError`](super::DbError) naming `$entity`, wrapped into `$error::Database` — the variant
/// every repository error enum carries. The macro keeps that wiring in one place instead of
/// three near-identical constructor blocks per plugin.
#[macro_export]
macro_rules! impl_repository_error {
    ($error:ty, $entity:literal) => {
        impl $error {
            /// Constructs the error for a failed load from the database.
            pub(crate) const fn load(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::load($entity, source))
            }

            /// Constructs the error for a failed insert into the database.
            pub(crate) const fn insert(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::insert($entity, source))
            }

            /// Constructs the error for a failed delete from the database.
            pub(crate) const fn delete(source: $crate::sqlx::Error) -> Self {
                Self::Database($crate::database::DbError::delete($entity, source))
            }
        }
    };
}
