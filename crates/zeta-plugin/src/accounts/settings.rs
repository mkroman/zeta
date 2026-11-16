//! The per-user settings of the plugins: descriptors, catalog, and store.

pub mod catalog;
#[cfg(feature = "database")]
pub mod service;
pub mod spec;

pub use catalog::UserSettingsCatalog;
#[cfg(feature = "database")]
pub use service::UserSettings;
pub use spec::{UserSettingKind, UserSettingSpec};
