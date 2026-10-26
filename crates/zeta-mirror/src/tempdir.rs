//! Temporary download directories.
//!
//! Downloads happen inside temporary directories that are owned by the download manager and
//! removed when the download finishes; directories left behind by a killed or crashed process
//! are removed on startup.

use std::time::Duration;

use tracing::warn;

/// The filename prefix for our temporary download directories.
pub const TEMP_DIR_PREFIX: &str = "zeta-mirror-";

/// The maximum age of a download directory before it is considered stale during the sweep of the
/// shared system temporary directory.
///
/// The system temporary directory may hold the active download directories of other running
/// processes (including other instances of the bot), so only directories that have not been
/// touched recently are removed there.
pub const STALE_DOWNLOAD_MAX_AGE: Duration = Duration::from_hours(24);

/// Returns a builder for our temporary download directories.
///
/// Directories are private to the current user on unix, since downloaded media may be sensitive.
#[must_use]
pub fn tempdir_builder() -> tempfile::Builder<'static, 'static> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = tempfile::Builder::new();
    builder.prefix(TEMP_DIR_PREFIX);

    #[cfg(unix)]
    builder.permissions(std::fs::Permissions::from_mode(0o700));

    builder
}

/// Removes any temporary download directories left behind by a previous run.
///
/// Returns the number of directories removed. When `max_age` is given, only directories whose
/// modification time is older are removed; use it when sweeping a directory that may hold the
/// active download directories of other running processes (the system temporary directory). Left
/// `None` for directories owned by this process.
pub fn cleanup_stale_downloads(base: &std::path::Path, max_age: Option<Duration>) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };

    let mut removed = 0;

    for entry in entries.flatten() {
        if !is_temp_download_dir(&entry) {
            continue;
        }

        if let Some(max_age) = max_age
            && !is_older_than(&entry, max_age)
        {
            continue;
        }

        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => removed += 1,
            Err(err) => warn!(
                path = %entry.path().display(),
                error = %err,
                "could not remove stale download directory"
            ),
        }
    }

    removed
}

/// Whether `entry` is a temporary download directory left by a mirror download.
pub fn is_temp_download_dir(entry: &std::fs::DirEntry) -> bool {
    entry
        .file_name()
        .to_string_lossy()
        .starts_with(TEMP_DIR_PREFIX)
        && entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

/// Returns whether the entry was last modified longer than `age` ago.
///
/// Entries whose age cannot be determined (including ones with a modification time in the future)
/// are not considered old.
fn is_older_than(entry: &std::fs::DirEntry, age: Duration) -> bool {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleanup_stale_downloads_respects_age() {
        let base = tempfile::tempdir().unwrap();

        let fresh = base.path().join(format!("{TEMP_DIR_PREFIX}1234"));
        std::fs::create_dir(&fresh).unwrap();

        // A directory that was touched recently is not stale while a maximum age is given, but
        // is removed unconditionally when no age is given.
        cleanup_stale_downloads(base.path(), Some(STALE_DOWNLOAD_MAX_AGE));
        assert!(fresh.exists());

        cleanup_stale_downloads(base.path(), None);
        assert!(!fresh.exists());
    }
}
