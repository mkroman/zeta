//! Database helpers shared by the DB-backed plugins.
//!
//! The host connects the pool and applies the migrations; the plugins' repositories reach it
//! through their context and persist their state through the [`DbError`] and
//! [`database_error!`](crate::database_error) helpers here.

/// Database connection pool.
pub type Database = sqlx::PgPool;

/// Returns the name of the database the pool connects to, for `db.namespace` span fields.
#[must_use]
pub fn namespace(db: &Database) -> String {
    db.connect_options()
        .get_database()
        .unwrap_or_default()
        .to_owned()
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

/// A failed database operation, naming the affected entity.
#[derive(Debug)]
pub struct DbError {
    stage: Stage,
    entity: &'static str,
    source: sqlx::Error,
}

/// The stage of a database operation that failed.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Loading rows from the database.
    Load,
    /// Inserting a row into the database.
    Insert,
    /// Deleting rows from the database.
    Delete,
}

impl DbError {
    /// Constructs the error for a failed load of `entity` rows.
    #[must_use]
    pub const fn load(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Load, entity, source)
    }

    /// Constructs the error for a failed insert of an `entity` row.
    #[must_use]
    pub const fn insert(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Insert, entity, source)
    }

    /// Constructs the error for a failed delete of `entity` rows.
    #[must_use]
    pub const fn delete(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new(Stage::Delete, entity, source)
    }

    /// Constructs the error from its parts.
    const fn new(stage: Stage, entity: &'static str, source: sqlx::Error) -> Self {
        Self {
            stage,
            entity,
            source,
        }
    }
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.stage {
            Stage::Load => write!(f, "could not load {}: {}", self.entity, self.source),
            Stage::Insert => write!(f, "could not insert {}: {}", self.entity, self.source),
            Stage::Delete => write!(f, "could not delete {}: {}", self.entity, self.source),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
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
/// [`DbError`] naming `$entity`, wrapped into `$error::Database` — the variant every repository
/// error enum carries. The macro keeps that wiring in one place instead of three near-identical
/// constructor blocks per plugin.
#[macro_export]
macro_rules! database_error {
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
