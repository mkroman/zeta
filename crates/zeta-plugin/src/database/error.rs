//! The reply-grade database error shared by the repositories.

/// A failed database operation, naming the affected entity.
///
/// [`Display`](std::fmt::Display) is reply-grade: `could not {stage} {entity}`, a sentence a
/// channel may see because it carries no SQL or driver text. The [`sqlx::Error`] stays in
/// [`source`](std::error::Error::source) as operator-grade detail; log it through
/// [`Chain`](crate::Chain), never by interpolating the error into a reply.
#[derive(Debug, thiserror::Error)]
#[error("could not {stage} {entity}")]
pub struct DbError {
    stage: &'static str,
    entity: &'static str,
    pub(super) source: sqlx::Error,
}

impl DbError {
    /// Constructs the error for a failed load of `entity` rows.
    #[must_use]
    pub const fn load(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("load", entity, source)
    }

    /// Constructs the error for a failed insert of an `entity` row.
    #[must_use]
    pub const fn insert(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("insert", entity, source)
    }

    /// Constructs the error for a failed delete of `entity` rows.
    #[must_use]
    pub const fn delete(entity: &'static str, source: sqlx::Error) -> Self {
        Self::new("delete", entity, source)
    }

    /// Constructs the error from its parts.
    const fn new(stage: &'static str, entity: &'static str, source: sqlx::Error) -> Self {
        Self {
            stage,
            entity,
            source,
        }
    }
}
