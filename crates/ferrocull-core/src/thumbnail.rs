//! Thumbnail generation for images and RAW files.
//!
//! Strategy:
//! - JPEG/PNG: Load with image crate, resize with `fast_image_resize` (SIMD, RGB/U8x3)
//! - RAW files: Extract embedded JPEG preview, same decode/resize path
//!
//! All public functions return JPEG bytes (`Vec<u8>`), suitable for both
//! disk caching and `iced::widget::image::Handle::from_bytes()`.

use std::{
    cmp::Reverse,
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use chrono::TimeZone;
use fast_image_resize::{PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
use ferrocull_media::{FileCategory, categorize_extension};
use image::{DynamicImage, ImageEncoder, ImageError, RgbImage};

use crate::media::CaptureTime;

/// Result of thumbnail generation: JPEG bytes and optional capture time from EXIF.
#[derive(Debug, Clone)]
pub struct Output {
    pub jpeg: Vec<u8>,
    pub capture_time: Option<CaptureTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error reading {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("image processing error: {0}")]
    Image(#[from] ImageError),
    #[error("resize failed: {0}")]
    Resize(String),
    #[error("unsupported file format: {}", path.display())]
    UnsupportedFormat { path: PathBuf },
    #[error("no embedded preview in RAW file: {}", path.display())]
    NoEmbeddedPreview { path: PathBuf },
}

/// Extract the largest embedded JPEG from a file for full-screen preview.
///
/// Returns JPEG bytes. For JPEG/PNG with orientation == 1, returns the original
/// file bytes directly (zero decode). Otherwise decodes, orients, and re-encodes.
///
/// For RAW files, extracts the largest embedded JPEG preview.
pub fn extract_largest_preview(path: &Path) -> Result<Vec<u8>, Error> {
    let ext = path.extension().and_then(OsStr::to_str);
    let category = ext
        .and_then(categorize_extension)
        .ok_or_else(|| Error::UnsupportedFormat {
            path: path.to_path_buf(),
        })?;

    let data = fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let (orientation, _) = parse_exif_fields(&data);

    match category {
        FileCategory::Photo => {
            if orientation <= 1 {
                return Ok(data);
            }
            let img = image::load_from_memory(&data)?;
            let oriented = apply_orientation_transform(img, orientation);
            encode_jpeg(&oriented.to_rgb8(), PREVIEW_JPEG_QUALITY)
        }
        FileCategory::Raw => {
            let spans = find_jpeg_spans(&data);
            let &(start, len) =
                spans
                    .iter()
                    .max_by_key(|(_, len)| *len)
                    .ok_or(Error::NoEmbeddedPreview {
                        path: path.to_path_buf(),
                    })?;
            if orientation <= 1 {
                return Ok(data[start..start + len].to_vec());
            }
            let img = image::load_from_memory(&data[start..start + len])?;
            let oriented = apply_orientation_transform(img, orientation);
            encode_jpeg(&oriented.to_rgb8(), PREVIEW_JPEG_QUALITY)
        }
        FileCategory::Video | FileCategory::Sidecar => Err(Error::UnsupportedFormat {
            path: path.to_path_buf(),
        }),
    }
}

/// JPEG quality for thumbnails (small images, size matters more than quality).
const THUMBNAIL_JPEG_QUALITY: u8 = 80;

/// JPEG quality for full-screen previews (quality matters).
const PREVIEW_JPEG_QUALITY: u8 = 92;

/// Parse both orientation and capture time from EXIF bytes in a single parse.
fn parse_exif_fields(data: &[u8]) -> (u32, Option<CaptureTime>) {
    let Ok(exif) = exif::Reader::new().read_from_container(&mut io::Cursor::new(data)) else {
        return (1, None);
    };
    let orientation = exif
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1);
    let capture_time = parse_capture_time_from_exif(&exif);
    (orientation, capture_time)
}

/// Shared EXIF capture time parsing from parsed Exif data.
fn parse_capture_time_from_exif(exif: &exif::Exif) -> Option<CaptureTime> {
    let datetime_str = exif
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .or_else(|| exif.get_field(exif::Tag::DateTime, exif::In::PRIMARY))?
        .display_value()
        .to_string();

    let subsec_str = exif
        .get_field(exif::Tag::SubSecTimeOriginal, exif::In::PRIMARY)
        .and_then(|f| match &f.value {
            exif::Value::Ascii(v) if !v.is_empty() => {
                std::str::from_utf8(&v[0]).ok().map(str::to_owned)
            }
            _ => None,
        });

    let ndt = chrono::NaiveDateTime::parse_from_str(&datetime_str, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&datetime_str, "%Y:%m:%d %H:%M:%S"))
        .ok()?;

    // Parse subseconds - normalize to nanoseconds (max 9 digits)
    let nanos = subsec_str
        .and_then(|s| {
            let s = s.trim();
            let digits: u32 = s.len().try_into().expect("subsec string length fits u32");
            s.parse::<u32>()
                .ok()
                .map(|v| v * 10u32.pow(9u32.saturating_sub(digits)))
        })
        .unwrap_or(0);

    let local_dt = chrono::Local.from_local_datetime(&ndt);
    let second = local_dt
        .single()
        .or_else(|| local_dt.earliest())
        .or_else(|| local_dt.latest())?
        .with_timezone(&chrono::Utc);

    Some(CaptureTime::new(second, nanos))
}

/// Encode an RGB image as JPEG bytes.
fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(buf)
}

/// Resize image using `fast_image_resize` (SIMD accelerated, RGB/U8x3).
fn resize_fast(img: &DynamicImage, target_size: u32) -> Result<RgbImage, Error> {
    let src = img.to_rgb8();
    let (src_w, src_h) = (src.width(), src.height());

    let (dst_w, dst_h) = scale_dimensions(src_w, src_h, target_size);

    let src_image = Image::from_vec_u8(src_w, src_h, src.into_raw(), PixelType::U8x3)
        .map_err(|e| Error::Resize(e.to_string()))?;

    let mut dst_image = Image::new(dst_w, dst_h, PixelType::U8x3);

    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(
        fast_image_resize::FilterType::Bilinear,
    ));
    let mut resizer = Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, &options)
        .map_err(|e| Error::Resize(e.to_string()))?;

    Ok(RgbImage::from_raw(dst_w, dst_h, dst_image.into_vec())
        .expect("resize preserves buffer dimensions"))
}

/// Calculate scaled dimensions that fit within `target_size`, preserving aspect ratio.
#[expect(
    clippy::integer_division,
    reason = "pixel dimensions are integers, truncation is correct"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "result <= target (u32): dividing by the larger dimension guarantees the quotient fits"
)]
fn scale_dimensions(src_w: u32, src_h: u32, target: u32) -> (u32, u32) {
    if src_w > src_h {
        let h = u64::from(target) * u64::from(src_h) / u64::from(src_w);
        (target, (h as u32).max(1))
    } else {
        let w = u64::from(target) * u64::from(src_w) / u64::from(src_h);
        ((w as u32).max(1), target)
    }
}

/// Apply orientation transform based on EXIF orientation value.
fn apply_orientation_transform(img: DynamicImage, orientation: u32) -> DynamicImage {
    match orientation {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

fn is_valid_jpeg(data: &[u8]) -> bool {
    data.len() >= 10
        && data[0] == 0xFF
        && data[1] == 0xD8
        && data[2] == 0xFF
        && matches!(data[3], 0xE0..=0xEF | 0xDB | 0xC0..=0xCF)
}

/// Parse EXIF capture time from file bytes (only reads header portion).
#[must_use]
pub fn parse_exif_from_bytes(data: &[u8]) -> Option<CaptureTime> {
    let mut cursor = io::Cursor::new(data);
    let exif = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    parse_capture_time_from_exif(&exif)
}

/// Generate thumbnail JPEG bytes from file bytes. Parses EXIF once for both
/// orientation and capture time.
pub fn generate_thumbnail_from_bytes(data: &[u8], path: &Path, size: u32) -> Result<Output, Error> {
    let ext = path.extension().and_then(OsStr::to_str);
    let category = ext.and_then(categorize_extension);

    let (orientation, capture_time) = parse_exif_fields(data);

    let jpeg = match category {
        Some(FileCategory::Photo) => {
            let img = image::load_from_memory(data)?;
            let oriented = apply_orientation_transform(img, orientation);
            let resized = resize_fast(&oriented, size)?;
            encode_jpeg(&resized, THUMBNAIL_JPEG_QUALITY)?
        }
        Some(FileCategory::Raw) => {
            generate_from_raw_bytes_with_orientation(data, size, orientation, path)?
        }
        _ => {
            return Err(Error::UnsupportedFormat {
                path: path.to_path_buf(),
            });
        }
    };

    Ok(Output { jpeg, capture_time })
}

fn generate_from_raw_bytes_with_orientation(
    data: &[u8],
    size: u32,
    orientation: u32,
    path: &Path,
) -> Result<Vec<u8>, Error> {
    let spans = find_jpeg_spans(data);

    for (start, len) in spans {
        if let Ok(img) =
            image::load_from_memory_with_format(&data[start..start + len], image::ImageFormat::Jpeg)
        {
            let oriented = apply_orientation_transform(img, orientation);
            let resized = resize_fast(&oriented, size)?;
            return encode_jpeg(&resized, THUMBNAIL_JPEG_QUALITY);
        }
    }

    Err(Error::NoEmbeddedPreview {
        path: path.to_path_buf(),
    })
}

/// Minimum preview size to consider "good enough" for early termination.
const MIN_PREVIEW_BYTES: usize = 50_000;

/// Scan data for JPEG spans (SOI to EOI), return sorted by preference.
fn find_jpeg_spans(data: &[u8]) -> Vec<(usize, usize)> {
    let mut scanner = JpegScanner::new();
    scanner.scan(data);
    scanner.into_sorted_spans()
}

/// Incremental JPEG span scanner. Tracks open SOI markers across multiple `scan()` calls
/// so that `generate_raw_with_preread` only scans newly-read bytes each iteration.
struct JpegScanner {
    /// Byte offset to resume scanning from.
    offset: usize,
    /// SOI positions that haven't found their EOI yet.
    open_sois: Vec<usize>,
    /// Completed (start, length) spans.
    spans: Vec<(usize, usize)>,
}

impl JpegScanner {
    const fn new() -> Self {
        Self {
            offset: 0,
            open_sois: Vec::new(),
            spans: Vec::new(),
        }
    }

    /// Scan `data[self.offset..]` for new JPEG markers, updating internal state.
    fn scan(&mut self, data: &[u8]) {
        let mut i = self.offset;
        while i < data.len().saturating_sub(1) {
            if data[i] == 0xFF && data[i + 1] == 0xD8 {
                self.open_sois.push(i);
            } else if data[i] == 0xFF && data[i + 1] == 0xD9 {
                let eoi_end = i + 2;
                if let Some(soi) = self.open_sois.pop() {
                    let len = eoi_end - soi;
                    if len > 100 && is_valid_jpeg(&data[soi..eoi_end]) {
                        self.spans.push((soi, len));
                    }
                }
            }
            i += 1;
        }
        self.offset = i;
    }

    /// Return the first completed span >= `MIN_PREVIEW_BYTES` as (start, len).
    fn good_preview_span(&self) -> Option<(usize, usize)> {
        self.spans
            .iter()
            .find(|(_, len)| *len >= MIN_PREVIEW_BYTES)
            .copied()
    }

    /// Consume the scanner, returning spans sorted by preference:
    /// "good enough" (>= `MIN_PREVIEW_BYTES`) first, largest first within each group.
    fn into_sorted_spans(mut self) -> Vec<(usize, usize)> {
        self.spans
            .sort_by_key(|(_, len)| (*len < MIN_PREVIEW_BYTES, Reverse(*len)));
        self.spans
    }
}

/// Chunk size for incremental file reading (2MB).
const READ_CHUNK_SIZE: usize = 2 * 1024 * 1024;

/// Decode JPEG bytes, apply orientation, resize, and re-encode as JPEG.
///
/// Callers chain multiple candidate JPEGs and try the next one on `Err`, so
/// per-candidate failures aren't fatal — but the LAST attempt's error is what
/// they surface to the user, which is more informative than a bare "no preview".
fn try_decode_resize_encode(jpeg: &[u8], orientation: u32, size: u32) -> Result<Vec<u8>, Error> {
    let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)?;
    let oriented = apply_orientation_transform(img, orientation);
    let resized = resize_fast(&oriented, size)?;
    encode_jpeg(&resized, THUMBNAIL_JPEG_QUALITY)
}

/// Extract preview from RAW data, continuing to read from file if preview not found.
/// Uses incremental JPEG scanning to avoid re-scanning already-processed bytes.
pub fn generate_raw_with_preread(
    mut data: Vec<u8>,
    file: &mut File,
    size: u32,
    path: &Path,
) -> Result<Vec<u8>, Error> {
    let file_len = file
        .metadata()
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let (orientation, _) = parse_exif_fields(&data);
    let mut scanner = JpegScanner::new();
    scanner.scan(&data);

    if let Some((start, len)) = scanner.good_preview_span()
        && let Ok(result) = try_decode_resize_encode(&data[start..start + len], orientation, size)
    {
        return Ok(result);
    }

    let mut buf = vec![0u8; READ_CHUNK_SIZE];
    while (data.len() as u64) < file_len {
        let n = file.read(&mut buf).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        scanner.scan(&data);

        if let Some((start, len)) = scanner.good_preview_span()
            && let Ok(result) =
                try_decode_resize_encode(&data[start..start + len], orientation, size)
        {
            return Ok(result);
        }
    }

    // Final pass: try all found spans sorted by preference, keep the last error
    // so the user sees what actually went wrong if every candidate failed.
    let mut last_err = None;
    for (start, len) in scanner.into_sorted_spans() {
        let jpeg = &data[start..start + len];
        match try_decode_resize_encode(jpeg, orientation, size) {
            Ok(result) => return Ok(result),
            Err(e) => last_err = Some(e),
        }
    }

    Err(last_err.unwrap_or(Error::NoEmbeddedPreview {
        path: path.to_path_buf(),
    }))
}
