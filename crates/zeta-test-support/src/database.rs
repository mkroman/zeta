//! The workspace migrations, embedded once for database tests.

/// The workspace migrations, embedded once for database tests.
///
/// Use it as `#[sqlx::test(migrator = "zeta_test_support::MIGRATOR")]`: the path is resolved
/// relative to this crate, so a test module needs no migrations path of its own.
///
/// Like `#[sqlx::test]`, the test needs `DATABASE_URL` to name an existing database, from which a
/// fresh database is cloned and migrated for the test.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../plugins/migrations");
