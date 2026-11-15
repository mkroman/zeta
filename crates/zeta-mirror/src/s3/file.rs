//! Preparing a file for upload.
//!
//! An upload needs the file's SHA-256 hash for the signature and a content type for the object.
//! The hash is streamed in fixed-size chunks instead of reading the whole file, so a
//! multi-gigabyte upload is never held in memory.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::Error;

/// The size of the buffer used when hashing files.
///
/// Each read is an async file operation that runs on the blocking pool, so a large buffer keeps
/// hashing a multi-gigabyte upload from paying that round-trip once per 64 KiB.
const HASH_BUFFER_SIZE: usize = 1024 * 1024;

/// Returns the lowercase hex-encoded SHA-256 digest of the file at `path`.
pub(super) async fn sha256_file(path: &Path) -> Result<String, Error> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file.read(&mut buffer).await?;

        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
    }

    Ok(const_hex::encode(hasher.finalize()))
}

/// Returns the content type for the given path, based on its extension.
pub(super) fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("mp4" | "mov" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => "application/octet-stream",
    }
}
