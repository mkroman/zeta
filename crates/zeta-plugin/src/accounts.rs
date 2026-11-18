//! The account subsystem: who a message's sender is, and what they configured.
//!
//! Accounts are the framework's answer to "which user is this?", stable beyond a nick: the
//! identity is the IRCv3 services account, resolved from the `account` tag the host negotiates
//! onto incoming messages (`account-tag`, plus `account-notify` and `extended-join` so
//! authentication changes reach the bot as `ACCOUNT` notifications and accounted joins). A
//! user who has not identified has no identity — per-user state simply does not apply to them.
//! Every event carries the resolution: [`UserIdentity`] through the generated `identity()`
//! accessor, [`None`] for unidentified senders.
//!
//! The host attaches one [`Accounts`] service to the context before the plugins are
//! constructed, feeds it every incoming message through [`Accounts::track`], and spawns
//! [`Accounts::run_flush`](Accounts::run_flush) beside its other tasks. The facets:
//!
//! - **Tracking** — the tracker maintains the nick → (account, hostmask) sessions in memory
//!   from those notifications; a flush moves the sessions marked dirty — an account or hostmask
//!   that changed — to the `user_hostmasks` table in one batched upsert, so the hosts an
//!   account has been used from stay auditable without a write per message.
//! - **Settings** — [`UserSettings`], the cached store of the per-user values plugins declared
//!   through their `#[plugin(user_settings = …)]` descriptors. Users change them with the
//!   `.set` command; plugins read them with the typed `read`/`resolved` helpers, which
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
