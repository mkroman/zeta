//! The account subsystem: who a message's sender is, and what they configured.
//!
//! Accounts are the framework's answer to "which user is this?", stable beyond a nick: the
//! identity prefers the IRCv3 [`account` tag](crate::event::account) — the services account the
//! sender is logged into, which survives reconnects, nick changes and new hosts — and falls
//! back to the sender's `user@host` when the network does not report accounts. Every event
//! carries the resolution: [`UserIdentity`] through the generated `identity()` accessor.
//!
//! The host attaches one [`Accounts`] service to the context before the plugins are
//! constructed. Its facets:
//!
//! - **Tracking** — the hostmasks each identity has been seen from, recorded in the
//!   `user_hostmasks` table so operators can audit which hosts used an account. Observations
//!   ride along whenever settings are read or written; [`Accounts::observe`] records explicitly.
//! - **Settings** — [`UserSettings`], the cached store of the per-user values plugins declared
//!   through their `#[plugin(user_settings = …)]` descriptors. Users change them with the
//!   `.settings` command; plugins read them with the typed `read`/`resolved` helpers, which
//!   canonicalize stored values against the descriptor so consumers never see a value the
//!   descriptor would not accept.

pub mod identity;

#[cfg(feature = "database")]
pub mod hostmasks;
#[cfg(feature = "database")]
pub mod repository;
#[cfg(feature = "database")]
pub mod service;
pub mod settings;

#[cfg(feature = "database")]
pub use hostmasks::ObservedHostmask;
pub use identity::UserIdentity;
#[cfg(feature = "database")]
pub use service::Accounts;
#[cfg(feature = "database")]
pub use settings::UserSettings;
pub use settings::{UserSettingKind, UserSettingSpec, UserSettingsCatalog};
