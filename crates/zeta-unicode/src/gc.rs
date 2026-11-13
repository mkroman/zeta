//! The Unicode general category property (UAX #44).
//!
//! [`GeneralCategory`] and its id and name tables are the shared contract in
//! [`zeta_unicode_format`]; this module re-exports them and wraps the Nomina section lookup.

pub use zeta_unicode_format::GeneralCategory;

/// Returns the general category of `c`; unassigned points answer with
/// [`GeneralCategory::Unassigned`].
#[must_use]
pub fn general_category(c: char) -> GeneralCategory {
    crate::gc_table()
        .general_category(u32::from(c))
        .and_then(|id| GeneralCategory::from_id(usize::from(id)))
        .unwrap_or(GeneralCategory::Unassigned)
}
