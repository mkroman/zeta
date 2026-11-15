//! Mirroring of media to S3.
//!
//! Downloads media with `yt-dlp`, uploads it to an S3-compatible bucket and links to the public
//! copy. The service is shared by every plugin that mirrors media (such as the tiktok and reddit
//! plugins) and is configured through the top-level `[mirror]` configuration section or the
//! `S3_*` environment variables; when the configuration is incomplete, plugins degrade to their
//! non-mirroring behavior.
//!
//! The actual mirroring is managed by a long-running download manager task that downloads media
//! (streaming progress status back), uploads it to the bucket in a bounded upload pipeline and
//! notifies the requester with the public link. Downloads happen inside temporary directories
//! that are owned by the manager and removed when the download finishes; directories left behind
//! by a killed or crashed process are removed on startup.
//!
//! Plugins access the shared mirror through a [`MirrorHandle`],
//! which carries the key prefix and the public URL base used for their links, and is resolved
//! from their own configuration.

mod config;
mod handle;
mod links;
mod manager;
mod media_id;
mod mirror;
mod s3;
mod tempdir;
mod ytdlp;

pub use config::MirrorConfig;
pub use handle::MirrorHandle;
pub(crate) use links::{object_key, public_url_for};
pub use media_id::{InvalidMediaId, MediaId};
pub use mirror::Mirror;
pub use s3::Error as MirrorError;
