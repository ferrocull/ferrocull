//! Per-file scan pipeline: EXIF extraction then thumbnail generation, fanned
//! out across a rayon pool.
//!
//! Ordering contract, per file: [`Event::ExifLoaded`] is emitted before
//! [`Event::ThumbnailReady`]. EXIF drives item creation upstream, so it must
//! land first.
//!
//! Cache hits ([`cache::cache_key_from_canonical`] + [`cache::ThumbnailCache`])
//! skip generation entirely.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
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
fn preread(path: &Path) -> io::Result<(Vec<u8>, File)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let initial = usize::try_from(len).unwrap_or(usize::MAX).min(INITIAL_READ);
    let mut data = vec![0u8; initial];
    file.read_exact(&mut data)?;
    Ok((data, file))
}

/// Threads reading media file bodies concurrently. Cold-cache scans are
/// I/O-bound and storage throughput collapses when many reads interleave, so a
/// few near-sequential streams beat a full rayon fan-out — more so on SD cards,
/// where interleaving costs most.
const READ_CONCURRENCY: usize = 2;

/// Read-ahead budget between the reader pool and the decode pool: how many
/// pre-read files may sit in the channel awaiting decode. Sized to absorb
/// scheduling jitter without holding tens of full photo bodies in memory.
const READ_QUEUE_DEPTH: usize = 8;

/// A pre-read media file awaiting thumbnail generation on the decode pool:
/// the full body for photos, the head (plus the open handle to read more) for
/// RAW files.
struct ReadFile {
    path: PathBuf,
    category: FileCategory,
    data: Vec<u8>,
    handle: File,
    capture_time: CaptureTime,
    key: Option<String>,
}

/// Scan `files` as a two-stage pipeline. A small reader pool
/// ([`READ_CONCURRENCY`] threads) walks the list in order, resolves the cache,
/// and reads media bodies; the rayon pool decodes, resizes, encodes, and
/// caches. Splitting the stages keeps disk access limited to a few
/// near-sequential streams while decode still uses every core; `on_event`
/// fires from both pools. Blocks until every file is processed.
pub fn run<T, F>(files: Vec<T>, thumbnail_size: u32, cache: Option<&ThumbnailCache>, on_event: F)
where
    T: Input + Send,
    F: Fn(Event<T>) + Sync + Send,
{
    use rayon::prelude::*;

    let queue = Mutex::new(files.into_iter());
    let (tx, rx) = mpsc::sync_channel::<ReadFile>(READ_QUEUE_DEPTH);

    std::thread::scope(|scope| {
        for _ in 0..READ_CONCURRENCY {
            let tx = tx.clone();
            let queue = &queue;
            let on_event = &on_event;
            scope.spawn(move || {
                loop {
                    let next = queue.lock().expect("reader queue lock poisoned").next();
                    let Some(file) = next else { break };
                    if let Some(read) = read_stage(file, cache, on_event) {
                        tx.send(read).expect("decode pool hung up");
                    }
                }
            });
        }
        // Readers own the remaining senders; the channel closes when the last
        // reader finishes, ending the decode loop below.
        drop(tx);

        rx.into_iter().par_bridge().for_each(|read| {
            decode_stage(read, thumbnail_size, cache, &on_event);
        });
    });
}

/// Reader-pool stage: resolve the cache (hit emits both events and ends the
/// file's pipeline), otherwise read the media body and hand it to the decode
/// pool. Emits [`Event::ExifLoaded`]; a file that fails to read its head is
/// logged and dropped without emitting any event.
fn read_stage<T, F>(file: T, cache: Option<&ThumbnailCache>, on_event: &F) -> Option<ReadFile>
where
    T: Input,
    F: Fn(Event<T>),
{
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
        return None;
    }

    let (mut data, mut handle) = match preread(&path) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "failed to read file, skipping");
            return None;
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

    // A photo's body IS the image, so the whole read belongs on the reader
    // pool. RAW files usually decode from the pre-read head; the rare
    // continuation reads happen on the decode pool instead.
    if category == FileCategory::Photo
        && let Err(e) = handle.read_to_end(&mut data)
    {
        on_event(Event::ThumbnailReady {
            path,
            result: Err(e.to_string()),
        });
        return None;
    }

    Some(ReadFile {
        path,
        category,
        data,
        handle,
        capture_time,
        key,
    })
}

/// Decode-pool stage: generate the thumbnail from the pre-read bytes, cache
/// it, and emit [`Event::ThumbnailReady`].
fn decode_stage<T, F>(read: ReadFile, thumbnail_size: u32, cache: Option<&ThumbnailCache>, on_event: &F)
where
    F: Fn(Event<T>),
{
    let ReadFile {
        path,
        category,
        data,
        mut handle,
        capture_time,
        key,
    } = read;

    let thumb_result = match category {
        FileCategory::Photo => generate_thumbnail_from_bytes(&data, &path, thumbnail_size)
            .map(|r| r.jpeg)
            .map_err(|e| e.to_string()),
        FileCategory::Raw => generate_raw_with_preread(data, &mut handle, thumbnail_size, &path)
            .map_err(|e| e.to_string()),
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
        File::open(&path)
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
