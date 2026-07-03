//! Thumbnail caching.
//!
//! Caches thumbnails as JPEG files to platform-appropriate locations:
//! - Linux: `~/.cache/ferrocull/thumbnails/`
//! - macOS: `~/Library/Caches/Ferrocull/thumbnails/`
//! - Windows: `%LOCALAPPDATA%\Ferrocull\thumbnails\`
//!
//! Files are keyed by a hash of canonical path, file size, and mtime,
//! so cached thumbnails are invalidated when file content changes at the same path.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use twox_hash::XxHash3_128;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cache I/O error at {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("could not determine cache directory")]
    NoCacheDir,
}

pub struct ThumbnailCache {
    cache_dir: PathBuf,
}

impl ThumbnailCache {
    /// Opens or creates the thumbnail cache in the platform-appropriate location.
    ///
    /// # Errors
    /// Returns `Error::NoCacheDir` if no cache directory can be determined,
    /// or `Error::Io` if directory creation fails.
    pub fn open() -> Result<Self, Error> {
        let cache_dir = cache_dir().ok_or(Error::NoCacheDir)?;
        fs::create_dir_all(&cache_dir).map_err(|source| Error::Io {
            path: cache_dir.clone(),
            source,
        })?;
        tracing::debug!(?cache_dir, "ThumbnailCache opened");
        Ok(Self { cache_dir })
    }

    /// Loads cached JPEG bytes by content key.
    ///
    /// # Errors
    /// Returns `Error::Io` if reading fails.
    pub fn load(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let cached_path = self.cache_dir.join(format!("{key}.jpg"));

        match fs::read(&cached_path) {
            Ok(data) => {
                tracing::trace!(?key, "Cache hit");
                Ok(Some(data))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tracing::trace!(?key, "Cache miss");
                Ok(None)
            }
            Err(source) => Err(Error::Io {
                path: cached_path,
                source,
            }),
        }
    }

    /// Stores JPEG bytes in the cache by content key.
    ///
    /// # Errors
    /// Returns `Error::Io` if writing fails.
    pub fn put(&self, key: &str, jpeg: &[u8]) -> Result<PathBuf, Error> {
        let cached_path = self.cache_dir.join(format!("{key}.jpg"));
        tracing::trace!(?key, "Saving to cache");

        fs::write(&cached_path, jpeg).map_err(|source| Error::Io {
            path: cached_path.clone(),
            source,
        })?;
        Ok(cached_path)
    }
}

/// Cache key from file path, size, and mtime.
///
/// Canonicalizes the path (filesystem I/O), stats the file for size and mtime,
/// then hashes all three into a single key. This ensures cached thumbnails are
/// invalidated when file content changes at the same path (e.g. reformatted SD card).
///
/// # Errors
/// Returns the underlying `io::Error` if the path cannot be canonicalized,
/// its metadata cannot be read, or its mtime is unavailable.
pub fn cache_key_from_disk(path: &Path) -> io::Result<String> {
    let canonical = path.canonicalize()?;
    let meta = fs::metadata(&canonical)?;
    let mtime = meta.modified()?;
    let duration = mtime
        .duration_since(std::time::UNIX_EPOCH)
        .expect("file mtime after UNIX_EPOCH");

    let mut key_bytes = canonical.as_os_str().as_encoded_bytes().to_vec();
    key_bytes.extend_from_slice(&meta.len().to_le_bytes());
    key_bytes.extend_from_slice(&duration.as_secs().to_le_bytes());
    key_bytes.extend_from_slice(&duration.subsec_nanos().to_le_bytes());

    let hash = XxHash3_128::oneshot(&key_bytes);
    Ok(format!("{hash:032x}"))
}

fn cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let subdir = "ferrocull/thumbnails";

    #[cfg(not(target_os = "linux"))]
    let subdir = "Ferrocull/thumbnails";

    dirs::cache_dir().map(|p| p.join(subdir))
}
