//! The data model of the filter plugin.
//!
//! The module is split by concern: `filter` holds the persisted row and the model used to insert
//! one, `pattern` the wildcard matcher compiled once for repeated matching, and `criteria` the
//! listing and deletion criteria built on it. The items the rest of the plugin names are
//! re-exported here, so `crate::model::{…}` stays their import path.

mod criteria;
mod filter;
mod pattern;
#[cfg(test)]
mod tests;

pub use self::criteria::Criteria;
pub use self::filter::{Filter, NewFilter};
pub(crate) use self::pattern::{Case, Pattern, is_wildcard};
