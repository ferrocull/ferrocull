//! Disk caching for generated JPEGs, in two namespaces under the
//! platform-appropriate cache root (`ferrocull/` on Linux, `Ferrocull/`
//! elsewhere):
//! - [`ThumbnailCache`] — grid thumbnails, in `thumbnails/`
//! - [`PreviewCache`] — full-screen previews, in `previews/`
//!
//! Both are keyed by a hash of canonical path, file size, and mtime
//! ([`cache_key_from_disk`]), so cached entries are invalidated when file
//! content changes at the same path.
//!
//! Each thumbnail `{key}.jpg` is paired with a `{key}.meta` sidecar holding the
//! item's [`CaptureTime`], so a cache hit recovers the capture time without
//! re-reading the media body. A thumbnail whose sidecar is missing or
//! unparseable is treated as a miss. Previews carry no sidecar — they are pure
//! JPEG bytes.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use twox_hash::XxHash3_128;

use crate::media::CaptureTime;

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
        let root = default_cache_root().ok_or(Error::NoCacheDir)?;
        Self::open_in_root(&root)
    }

    /// Opens or creates the thumbnail cache under `root` (in its `thumbnails/`
    /// namespace).
    ///
    /// # Errors
    /// Returns `Error::Io` if directory creation fails.
    pub fn open_in_root(root: &Path) -> Result<Self, Error> {
        Self::open_at(root.join(THUMBNAIL_NAMESPACE))
    }

    /// Opens or creates a thumbnail cache at the exact namespace directory
    /// `cache_dir`.
    ///
    /// # Errors
    /// Returns `Error::Io` if directory creation fails.
    pub fn open_at(cache_dir: PathBuf) -> Result<Self, Error> {
        Ok(Self {
            cache_dir: open_namespace_dir(cache_dir)?,
        })
    }

    /// Deletes every cached thumbnail and sidecar, leaving an empty namespace
    /// directory. Used when the thumbnail resolution changes: the cache key
    /// carries no resolution, so stale entries must be dropped before
    /// regenerating at the new size.
    ///
    /// # Errors
    /// Returns `Error::Io` if removing or recreating the directory fails.
    pub fn clear(&self) -> Result<(), Error> {
        match fs::remove_dir_all(&self.cache_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(Error::Io {
                    path: self.cache_dir.clone(),
                    source,
                });
            }
        }
        open_namespace_dir(self.cache_dir.clone())?;
        Ok(())
    }

    /// Loads a cached thumbnail: its JPEG bytes and persisted [`CaptureTime`].
    ///
    /// A hit requires both the JPEG and a parseable `{key}.meta` sidecar. A
    /// missing JPEG, a missing sidecar, or an unparseable sidecar is a miss
    /// (`Ok(None)`), forcing regeneration.
    ///
    /// # Errors
    /// Returns `Error::Io` if a read fails for a reason other than the file
    /// being absent.
    pub fn load(&self, key: &str) -> Result<Option<(Vec<u8>, CaptureTime)>, Error> {
        let Some(jpeg) = read_if_present(self.cache_dir.join(format!("{key}.jpg")))? else {
            tracing::trace!(?key, "Cache miss");
            return Ok(None);
        };

        let Some(meta) = read_if_present(self.cache_dir.join(format!("{key}.meta")))? else {
            tracing::trace!(?key, "Cache miss: thumbnail without capture-time sidecar");
            return Ok(None);
        };

        let Some(capture_time) = String::from_utf8(meta).ok().and_then(|s| parse_meta(&s)) else {
            tracing::trace!(?key, "Cache miss: unparseable capture-time sidecar");
            return Ok(None);
        };
        tracing::trace!(?key, "Cache hit");
        Ok(Some((jpeg, capture_time)))
    }

    /// Stores a thumbnail's JPEG bytes and capture time by content key.
    ///
    /// # Errors
    /// Returns `Error::Io` if writing fails.
    pub fn put(&self, key: &str, jpeg: &[u8], capture_time: CaptureTime) -> Result<PathBuf, Error> {
        tracing::trace!(?key, "Saving to cache");
        let jpeg_path = write_entry(self.cache_dir.join(format!("{key}.jpg")), jpeg)?;

        // Sidecar written after the JPEG: a crash in between leaves a
        // metadata-less thumbnail, which `load` reads back as a miss.
        write_entry(
            self.cache_dir.join(format!("{key}.meta")),
            format_meta(capture_time).as_bytes(),
        )?;
        Ok(jpeg_path)
    }
}

/// Disk cache for extracted full-screen preview JPEGs, in a namespace separate
/// from [`ThumbnailCache`]. Previews carry no capture-time sidecar: they are
/// pure JPEG bytes keyed by content hash ([`cache_key_from_disk`]).
pub struct PreviewCache {
    cache_dir: PathBuf,
}

impl PreviewCache {
    /// Opens or creates the preview cache in the platform-appropriate location.
    ///
    /// # Errors
    /// Returns `Error::NoCacheDir` if no cache directory can be determined,
    /// or `Error::Io` if directory creation fails.
    pub fn open() -> Result<Self, Error> {
        let root = default_cache_root().ok_or(Error::NoCacheDir)?;
        Self::open_in_root(&root)
    }

    /// Opens or creates the preview cache under `root` (in its `previews/`
    /// namespace).
    ///
    /// # Errors
    /// Returns `Error::Io` if directory creation fails.
    pub fn open_in_root(root: &Path) -> Result<Self, Error> {
        Self::open_at(root.join(PREVIEW_NAMESPACE))
    }

    /// Opens or creates a preview cache at the exact namespace directory
    /// `cache_dir`.
    ///
    /// # Errors
    /// Returns `Error::Io` if directory creation fails.
    pub fn open_at(cache_dir: PathBuf) -> Result<Self, Error> {
        Ok(Self {
            cache_dir: open_namespace_dir(cache_dir)?,
        })
    }

    /// Loads a cached preview's JPEG bytes, or `Ok(None)` on a miss.
    ///
    /// # Errors
    /// Returns `Error::Io` if a read fails for a reason other than the file
    /// being absent.
    pub fn load(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        let jpeg = read_if_present(self.cache_dir.join(format!("{key}.jpg")))?;
        if jpeg.is_some() {
            tracing::trace!(?key, "Preview cache hit");
        } else {
            tracing::trace!(?key, "Preview cache miss");
        }
        Ok(jpeg)
    }

    /// Stores a preview's JPEG bytes by content key.
    ///
    /// # Errors
    /// Returns `Error::Io` if writing fails.
    pub fn put(&self, key: &str, jpeg: &[u8]) -> Result<(), Error> {
        tracing::trace!(?key, "Saving preview to cache");
        write_entry(self.cache_dir.join(format!("{key}.jpg")), jpeg)?;
        Ok(())
    }
}

/// Creates a cache namespace directory, returning it ready for use.
fn open_namespace_dir(cache_dir: PathBuf) -> Result<PathBuf, Error> {
    fs::create_dir_all(&cache_dir).map_err(|source| Error::Io {
        path: cache_dir.clone(),
        source,
    })?;
    tracing::debug!(?cache_dir, "cache namespace opened");
    Ok(cache_dir)
}

/// Reads a cache entry, mapping a missing file to `Ok(None)`.
fn read_if_present(path: PathBuf) -> Result<Option<Vec<u8>>, Error> {
    match fs::read(&path) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io { path, source }),
    }
}

/// Writes a cache entry, returning its path.
fn write_entry(path: PathBuf, bytes: &[u8]) -> Result<PathBuf, Error> {
    match fs::write(&path, bytes) {
        Ok(()) => Ok(path),
        Err(source) => Err(Error::Io { path, source }),
    }
}

/// Serializes a capture time for its `{key}.meta` sidecar: three integer lines
/// (whole seconds, the timestamp's subsecond nanos, then the capture subsecond
/// nanos) — a lossless round-trip of both [`CaptureTime`] fields.
fn format_meta(capture_time: CaptureTime) -> String {
    format!(
        "{}\n{}\n{}\n",
        capture_time.second.timestamp(),
        capture_time.second.timestamp_subsec_nanos(),
        capture_time.subsec_nanos,
    )
}

/// Parses a `{key}.meta` sidecar. Any malformed content reconstructs to `None`,
/// which the caller treats as a cache miss.
fn parse_meta(contents: &str) -> Option<CaptureTime> {
    let mut lines = contents.lines();
    let secs: i64 = lines.next()?.parse().ok()?;
    let timestamp_subsec_nanos: u32 = lines.next()?.parse().ok()?;
    let subsec_nanos: u32 = lines.next()?.parse().ok()?;
    let second = DateTime::<Utc>::from_timestamp(secs, timestamp_subsec_nanos)?;
    Some(CaptureTime::new(second, subsec_nanos))
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
    cache_key_from_canonical(&path.canonicalize()?)
}

/// [`cache_key_from_disk`] for a path the caller has already canonicalized,
/// skipping the redundant `canonicalize` syscall.
///
/// # Errors
/// Returns the underlying `io::Error` if the file's metadata cannot be read or
/// its mtime is unavailable.
pub fn cache_key_from_canonical(canonical: &Path) -> io::Result<String> {
    let meta = fs::metadata(canonical)?;
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

/// Namespace directory names under a cache root.
const THUMBNAIL_NAMESPACE: &str = "thumbnails";
const PREVIEW_NAMESPACE: &str = "previews";

/// The default cache root — the parent of the `thumbnails/` and `previews/`
/// namespace directories — under the platform-appropriate cache location
/// (`ferrocull/` on Linux, `Ferrocull/` elsewhere). `None` when no platform
/// cache directory can be determined.
#[must_use]
pub fn default_cache_root() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let root = "ferrocull";

    #[cfg(not(target_os = "linux"))]
    let root = "Ferrocull";

    dirs::cache_dir().map(|p| p.join(root))
}

/// Moves the thumbnail and preview cache namespaces from `old_root` to
/// `new_root`. Prefers `fs::rename`, falling back to copy+delete per file when
/// the move crosses filesystems. A namespace with nothing cached yet is skipped.
///
/// # Errors
/// Returns `Error::Io` if reading, creating, copying, or removing an entry fails.
pub fn relocate(old_root: &Path, new_root: &Path) -> Result<(), Error> {
    move_namespace(
        &old_root.join(THUMBNAIL_NAMESPACE),
        &new_root.join(THUMBNAIL_NAMESPACE),
    )?;
    move_namespace(
        &old_root.join(PREVIEW_NAMESPACE),
        &new_root.join(PREVIEW_NAMESPACE),
    )
}

/// Moves every file in `from` into `to`, creating `to` first. Renames each
/// entry, falling back to copy+delete across filesystems, then drops the emptied
/// source directory (best effort). Cache namespaces are flat (only `{key}.jpg`
/// and `{key}.meta` files), so a single-level walk suffices.
fn move_namespace(from: &Path, to: &Path) -> Result<(), Error> {
    if !from.is_dir() {
        return Ok(());
    }
    let io_err = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    fs::create_dir_all(to).map_err(io_err(to))?;

    for entry in fs::read_dir(from).map_err(io_err(from))? {
        let entry = entry.map_err(io_err(from))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        match fs::rename(&src, &dst) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                fs::copy(&src, &dst).map_err(io_err(&src))?;
                fs::remove_file(&src).map_err(io_err(&src))?;
            }
            Err(e) => return Err(io_err(&src)(e)),
        }
    }

    drop(fs::remove_dir(from));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_cache_round_trips_without_sidecar() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cache = PreviewCache::open_at(dir.path().join("previews")).expect("open preview cache");

        assert!(
            cache.load("abc").expect("load miss").is_none(),
            "empty cache misses"
        );

        cache.put("abc", b"jpeg-bytes").expect("put preview");
        assert_eq!(
            cache.load("abc").expect("load hit").as_deref(),
            Some(&b"jpeg-bytes"[..]),
            "put value round-trips"
        );

        // Previews carry no `.meta` sidecar, unlike thumbnails.
        let has_meta = fs::read_dir(dir.path().join("previews"))
            .expect("read previews dir")
            .filter_map(Result::ok)
            .any(|e| e.path().extension().is_some_and(|ext| ext == "meta"));
        assert!(!has_meta, "preview cache writes no metadata sidecar");
    }

    #[test]
    fn clear_drops_entries_but_keeps_cache_usable() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cache = ThumbnailCache::open_at(dir.path().join("thumbnails")).expect("open cache");

        let capture = CaptureTime::new(DateTime::<Utc>::from_timestamp(1, 0).expect("valid timestamp"), 0);
        cache.put("key", b"jpeg", capture).expect("put");
        assert!(cache.load("key").expect("load").is_some(), "entry present");

        cache.clear().expect("clear");
        assert!(
            cache.load("key").expect("load after clear").is_none(),
            "clear drops the entry"
        );

        cache.put("key2", b"jpeg", capture).expect("put after clear");
        assert!(
            cache.load("key2").expect("load new").is_some(),
            "cache still usable after clear"
        );
    }

    #[test]
    fn relocate_moves_both_namespaces() {
        let old = tempfile::tempdir().expect("old root");
        let new = tempfile::tempdir().expect("new root");
        let capture = CaptureTime::new(DateTime::<Utc>::from_timestamp(1, 0).expect("valid timestamp"), 0);

        let thumbs = ThumbnailCache::open_in_root(old.path()).expect("open thumbs");
        let previews = PreviewCache::open_in_root(old.path()).expect("open previews");
        thumbs.put("t", b"thumb", capture).expect("put thumb");
        previews.put("p", b"preview").expect("put preview");
        drop((thumbs, previews));

        relocate(old.path(), new.path()).expect("relocate");

        let moved_thumbs = ThumbnailCache::open_in_root(new.path()).expect("reopen thumbs");
        let moved_previews = PreviewCache::open_in_root(new.path()).expect("reopen previews");
        assert!(
            moved_thumbs.load("t").expect("load thumb").is_some(),
            "thumbnail followed the move"
        );
        assert!(
            moved_previews.load("p").expect("load preview").is_some(),
            "preview followed the move"
        );

        assert!(
            !old.path().join(THUMBNAIL_NAMESPACE).join("t.jpg").exists(),
            "old thumbnail entry is gone"
        );
    }

    #[test]
    fn relocate_skips_namespace_with_nothing_cached() {
        let old = tempfile::tempdir().expect("old root");
        let new = tempfile::tempdir().expect("new root");
        // Neither namespace exists under `old` yet — relocate must not error.
        relocate(old.path(), new.path()).expect("relocate with empty source");
    }

    #[test]
    fn preview_and_thumbnail_namespaces_are_independent() {
        // Real cache roots put the two namespaces in sibling directories.
        let root = default_cache_root();
        let thumbs = root.as_ref().map(|r| r.join(THUMBNAIL_NAMESPACE));
        let previews = root.as_ref().map(|r| r.join(PREVIEW_NAMESPACE));
        assert_ne!(thumbs, previews, "namespaces resolve to different dirs");

        // A key stored in one namespace is invisible to the other.
        let dir = tempfile::tempdir().expect("create tempdir");
        let preview_cache =
            PreviewCache::open_at(dir.path().join("previews")).expect("open preview cache");
        let thumb_cache =
            ThumbnailCache::open_at(dir.path().join("thumbnails")).expect("open thumbnail cache");

        preview_cache.put("shared", b"preview").expect("put preview");
        assert!(
            thumb_cache.load("shared").expect("thumb load").is_none(),
            "a preview key does not leak into the thumbnail cache"
        );
    }
}
