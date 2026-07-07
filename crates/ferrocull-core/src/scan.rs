//! Per-file scan pipeline: EXIF extraction then thumbnail generation, fanned
//! out across a rayon pool.
//!
//! Ordering contract, per file: [`Event::ExifLoaded`] is emitted before
//! [`Event::ThumbnailReady`]. EXIF drives item creation upstream, so it must
//! land first.
//!
//! Cache hits ([`cache::cache_key_from_disk`] + [`cache::ThumbnailCache`])
//! skip generation entirely.

use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use ferrocull_media::FileCategory;

use crate::{
    cache::{self, ThumbnailCache},
    media::CaptureTime,
    thumbnail::{generate_raw_with_preread, generate_thumbnail_from_bytes, parse_exif_from_bytes},
    xmp,
};

/// Bytes pre-read from the head of each file, reused for EXIF and thumbnail.
const INITIAL_READ: usize = 2 * 1024 * 1024;

/// A file to scan. The concrete value is echoed back unchanged in
/// [`Event::ExifLoaded`] so the caller can build its own item from it.
pub trait Input {
    fn path(&self) -> &Path;
    fn category(&self) -> FileCategory;
    fn xmp_sidecar(&self) -> Option<&Path>;
}

/// Progress events emitted per file, in order (EXIF before thumbnail).
///
/// A file that fails to open or read its head is logged and dropped silently —
/// no event is emitted for it.
pub enum Event<T> {
    /// Capture time (persisted from cache, EXIF, or mtime fallback) and XMP
    /// sidecar are available; the input file is handed back for item
    /// construction. `canonical_path` is the file's canonicalized path (or the
    /// raw path when canonicalization fails), resolved here so the caller does
    /// not repeat that I/O on its update loop.
    ExifLoaded {
        file: T,
        canonical_path: PathBuf,
        capture_time: CaptureTime,
        xmp: Option<xmp::Metadata>,
    },
    /// The thumbnail is cached (freshly generated or already present), or its
    /// generation failed. Carries no pixel data.
    ThumbnailReady {
        path: PathBuf,
        result: Result<(), String>,
    },
}

/// Read the head of a file for EXIF/thumbnail, returning the buffer and the
/// still-open handle positioned right after it.
fn preread(path: &Path) -> io::Result<(Vec<u8>, std::fs::File)> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let initial = usize::try_from(len).unwrap_or(usize::MAX).min(INITIAL_READ);
    let mut data = vec![0u8; initial];
    file.read_exact(&mut data)?;
    Ok((data, file))
}

/// Scan `files` in parallel, invoking `on_event` from rayon worker threads as
/// each file produces EXIF then thumbnail results. Blocks until every file is
/// processed.
pub fn run<T, F>(files: Vec<T>, thumbnail_size: u32, cache: Option<&ThumbnailCache>, on_event: F)
where
    T: Input + Send,
    F: Fn(Event<T>) + Sync + Send,
{
    use rayon::prelude::*;

    files.into_par_iter().for_each(|file| {
        let path = file.path().to_path_buf();
        let category = file.category();

        // The XMP sidecar is a tiny separate file read on both the hit and miss
        // paths, since external ratings can change between sessions.
        let xmp = file
            .xmp_sidecar()
            .and_then(|sidecar| xmp::read_sidecar(sidecar).ok());

        // Canonicalize once here (metadata I/O, not a body read): the result
        // feeds both the cache key and the event, so the caller needn't repeat
        // it on its update loop. On failure, fall back to the raw path and
        // bypass the cache.
        let (canonical_path, key) = match path.canonicalize() {
            Ok(canonical) => {
                let key = match cache::cache_key_from_canonical(&canonical) {
                    Ok(k) => Some(k),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cache key derivation failed, bypassing cache");
                        None
                    }
                };
                (canonical, key)
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "canonicalize failed, bypassing cache");
                (path.clone(), None)
            }
        };

        // Cache hit: the thumbnail and its persisted capture time are both
        // present, so we skip every read of the media body.
        if let Some(ref k) = key
            && let Some(c) = cache
            && let Ok(Some((_, capture_time))) = c.load(k)
        {
            on_event(Event::ExifLoaded {
                file,
                canonical_path,
                capture_time,
                xmp,
            });
            on_event(Event::ThumbnailReady {
                path,
                result: Ok(()),
            });
            return;
        }

        let (data, mut handle) = match preread(&path) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "failed to read file, skipping");
                return;
            }
        };

        // Fall back to file modification time if EXIF carries no date.
        let capture_time = parse_exif_from_bytes(&data).unwrap_or_else(|| {
            let mtime = handle
                .metadata()
                .expect("file already opened")
                .modified()
                .expect("modification time available");
            CaptureTime::new(DateTime::<Utc>::from(mtime), 0)
        });

        on_event(Event::ExifLoaded {
            file,
            canonical_path,
            capture_time,
            xmp,
        });

        let mut data = data;
        let thumb_result = match category {
            FileCategory::Photo => {
                if let Err(e) = handle.read_to_end(&mut data) {
                    Err(e.to_string())
                } else {
                    generate_thumbnail_from_bytes(&data, &path, thumbnail_size)
                        .map(|r| r.jpeg)
                        .map_err(|e| e.to_string())
                }
            }
            FileCategory::Raw => {
                generate_raw_with_preread(data, &mut handle, thumbnail_size, &path)
                    .map_err(|e| e.to_string())
            }
            _ => Err("unsupported format".to_owned()),
        };

        if let Ok(ref img) = thumb_result
            && let Some(ref k) = key
            && let Some(c) = cache
        {
            drop(c.put(k, img, capture_time));
        }

        on_event(Event::ThumbnailReady {
            path,
            result: thumb_result.map(|_| ()),
        });
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct TestFile {
        path: PathBuf,
    }

    impl Input for TestFile {
        fn path(&self) -> &Path {
            &self.path
        }

        fn category(&self) -> FileCategory {
            FileCategory::Photo
        }

        fn xmp_sidecar(&self) -> Option<&Path> {
            None
        }
    }

    /// A recorded event, flattened so it can be collected without the generic payload.
    enum Rec {
        Exif(CaptureTime),
        Thumbnail(Result<(), String>),
    }

    /// Encode a solid-color JPEG (no EXIF, so capture time falls back to mtime).
    fn write_jpeg(path: &Path) {
        let img = image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 60]));
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut buf = Vec::new();
        dynamic
            .write_to(&mut io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .expect("encode test jpeg");
        std::fs::write(path, &buf).expect("write test jpeg");
    }

    fn scan_recording(files: Vec<TestFile>, cache: Option<&ThumbnailCache>) -> Vec<Rec> {
        let recorded = Mutex::new(Vec::new());
        run(files, 256, cache, |event| {
            let rec = match event {
                Event::ExifLoaded { capture_time, .. } => Rec::Exif(capture_time),
                Event::ThumbnailReady { result, .. } => Rec::Thumbnail(result),
            };
            recorded.lock().expect("lock recorder").push(rec);
        });
        recorded.into_inner().expect("unwrap recorder")
    }

    #[test]
    fn emits_exif_before_thumbnail() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);

        let recs = scan_recording(vec![TestFile { path }], None);

        assert_eq!(recs.len(), 2, "one exif and one thumbnail event");
        assert!(matches!(recs[0], Rec::Exif(_)), "exif comes first");
        assert!(
            matches!(recs[1], Rec::Thumbnail(Ok(()))),
            "thumbnail comes second and succeeds"
        );
    }

    #[test]
    fn falls_back_to_mtime_without_exif() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);

        let mtime = std::fs::metadata(&path)
            .expect("stat test jpeg")
            .modified()
            .expect("mtime available");
        let expected = DateTime::<Utc>::from(mtime);

        let recs = scan_recording(vec![TestFile { path }], None);

        let Rec::Exif(capture_time) = &recs[0] else {
            panic!("first event is exif");
        };
        assert_eq!(
            capture_time.second, expected,
            "capture time is the file mtime"
        );
        assert_eq!(
            capture_time.subsec_nanos, 0,
            "mtime fallback has no subseconds"
        );
    }

    #[test]
    fn cache_hit_skips_regeneration() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cache_dir = dir.path().join("cache");
        let cache = ThumbnailCache::open_at(cache_dir).expect("open cache");

        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);
        let mtime = std::fs::metadata(&path)
            .expect("stat test jpeg")
            .modified()
            .expect("mtime available");
        let len = std::fs::metadata(&path).expect("stat test jpeg").len();

        let first = scan_recording(vec![TestFile { path: path.clone() }], Some(&cache));
        assert!(
            matches!(first[1], Rec::Thumbnail(Ok(()))),
            "first run generates and caches the thumbnail"
        );

        // Corrupt the pixels but preserve length + mtime so the cache key is
        // identical: a cache miss would try to decode this and fail, so a
        // succeeding second run proves the cached thumbnail was reused.
        let corrupt = vec![0u8; usize::try_from(len).expect("length fits usize")];
        std::fs::write(&path, &corrupt).expect("overwrite pixels");
        std::fs::File::open(&path)
            .expect("reopen for mtime")
            .set_modified(mtime)
            .expect("restore mtime");

        let second = scan_recording(vec![TestFile { path }], Some(&cache));
        assert!(
            matches!(second[1], Rec::Thumbnail(Ok(()))),
            "second run yields the cached thumbnail without decoding"
        );

        // The second run recovers capture time from the persisted sidecar,
        // never reading the now-corrupt body.
        let (Rec::Exif(first_time), Rec::Exif(second_time)) = (&first[0], &second[0]) else {
            panic!("each run emits exif first");
        };
        assert_eq!(
            first_time, second_time,
            "capture time is recovered from cache metadata, matching the first run"
        );
    }
}
