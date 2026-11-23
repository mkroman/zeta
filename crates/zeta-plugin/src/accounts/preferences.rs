//! The per-user preferences of the plugins: descriptors and store.

#[cfg(feature = "database")]
pub mod service;
pub mod spec;

#[cfg(feature = "database")]
pub use service::Preferences;
pub use spec::{PreferenceKind, PreferenceSet, PreferenceSpec, PreferenceValue};
