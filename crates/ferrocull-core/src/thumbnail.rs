//! Thumbnail generation for images and RAW files.
//!
//! Strategy:
//! - JPEG: Downscale during decode with libjpeg-turbo, then resize the rest of
//!   the way with `fast_image_resize` (SIMD, RGB/U8x3)
//! - PNG: Load with the image crate, same `fast_image_resize` path
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
    #[error("JPEG decode error: {0}")]
    JpegDecode(String),
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

    match category {
        FileCategory::Photo => {
            let data = fs::read(path).map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let (orientation, _) = parse_exif_fields(&data);
            if orientation <= 1 {
                return Ok(data);
            }
            let img = image::load_from_memory(&data)?;
            let oriented = apply_orientation_transform(img, orientation);
            encode_jpeg(&oriented.to_rgb8(), PREVIEW_JPEG_QUALITY)
        }
        FileCategory::Raw => extract_raw_largest_preview(path),
        FileCategory::Video | FileCategory::Sidecar => Err(Error::UnsupportedFormat {
            path: path.to_path_buf(),
        }),
    }
}

/// Extract the highest-resolution embedded JPEG preview from a RAW file for
/// full-screen display. Reads the whole file and picks the span with the most
/// pixels, parsed from each candidate's SOF frame header. Byte size is not a
/// reliable proxy for resolution — a more-compressed preview can be larger in
/// pixels yet smaller in bytes — so the scan runs to EOF rather than stopping at
/// the first full-size-class span.
fn extract_raw_largest_preview(path: &Path) -> Result<Vec<u8>, Error> {
    let data = fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut scanner = JpegScanner::new();
    scanner.scan(&data);

    let (start, len) = scanner
        .largest_by_pixels(&data)
        .ok_or(Error::NoEmbeddedPreview {
            path: path.to_path_buf(),
        })?;
    let (orientation, _) = parse_exif_fields(&data);
    oriented_preview(&data[start..start + len], orientation)
}

/// Return embedded preview JPEG bytes, applying EXIF orientation only if needed.
/// Orients at full resolution: previews keep their native size for full-screen
/// display.
fn oriented_preview(jpeg: &[u8], orientation: u32) -> Result<Vec<u8>, Error> {
    if orientation <= 1 {
        return Ok(jpeg.to_vec());
    }
    let img = image::load_from_memory(jpeg)?;
    let oriented = apply_orientation_transform(img, orientation);
    encode_jpeg(&oriented.to_rgb8(), PREVIEW_JPEG_QUALITY)
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

/// True if `data` starts with the JPEG SOI + marker prefix (`FF D8 FF`).
fn is_jpeg_magic(data: &[u8]) -> bool {
    matches!(data, [0xFF, 0xD8, 0xFF, ..])
}

/// Decode a Photo (JPEG or PNG) for thumbnailing. JPEG takes the scaled
/// libjpeg-turbo path; PNG (and anything else) falls back to the `image` crate.
fn decode_photo_scaled(data: &[u8], target: u32) -> Result<DynamicImage, Error> {
    if is_jpeg_magic(data) {
        decode_jpeg_scaled(data, target)
    } else {
        Ok(image::load_from_memory(data)?)
    }
}

/// Decode a JPEG with libjpeg-turbo, downscaling during decode to the smallest
/// DCT scaling factor whose output still covers `target` on its longer edge.
///
/// This avoids fully decoding a 45MP JPEG to build a 256px thumbnail; the
/// remaining fractional resize is left to `resize_fast`. Returns an
/// `ImageRgb8`, so downstream `into_rgb8` is a no-op move.
fn decode_jpeg_scaled(jpeg: &[u8], target: u32) -> Result<DynamicImage, Error> {
    let mut decompressor =
        turbojpeg::Decompressor::new().map_err(|e| Error::JpegDecode(e.to_string()))?;
    let header = decompressor
        .read_header(jpeg)
        .map_err(|e| Error::JpegDecode(e.to_string()))?;

    let target = target as usize;
    // Factors ascending, so the first match is the largest downscale (smallest
    // output) that still covers the target; fall back to 1:1 for tiny images.
    let factor = [
        turbojpeg::ScalingFactor::ONE_EIGHTH,
        turbojpeg::ScalingFactor::ONE_QUARTER,
        turbojpeg::ScalingFactor::ONE_HALF,
        turbojpeg::ScalingFactor::ONE,
    ]
    .into_iter()
    .find(|f| {
        let s = header.scaled(*f);
        s.width.max(s.height) >= target
    })
    .unwrap_or(turbojpeg::ScalingFactor::ONE);

    decompressor
        .set_scaling_factor(factor)
        .map_err(|e| Error::JpegDecode(e.to_string()))?;

    let scaled = header.scaled(factor);
    let mut output = turbojpeg::Image {
        pixels: vec![0u8; scaled.width * scaled.height * 3],
        width: scaled.width,
        pitch: scaled.width * 3,
        height: scaled.height,
        format: turbojpeg::PixelFormat::RGB,
    };
    decompressor
        .decompress(jpeg, output.as_deref_mut())
        .map_err(|e| Error::JpegDecode(e.to_string()))?;

    let rgb = RgbImage::from_raw(
        u32::try_from(scaled.width).expect("scaled width fits u32"),
        u32::try_from(scaled.height).expect("scaled height fits u32"),
        output.pixels,
    )
    .expect("turbojpeg buffer matches scaled dimensions");
    Ok(DynamicImage::ImageRgb8(rgb))
}

/// Resize image using `fast_image_resize` (SIMD accelerated, RGB/U8x3).
///
/// Takes ownership so an already-`ImageRgb8` decode (the common JPEG case) is
/// unwrapped with `into_rgb8` instead of cloned via `to_rgb8`.
fn resize_fast(img: DynamicImage, target_size: u32) -> Result<RgbImage, Error> {
    let src = img.into_rgb8();
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

/// True if the span is a JPEG our decode pipeline can display: a well-formed
/// marker stream whose frame header is Huffman baseline/extended/progressive
/// (SOF0/1/2). Expects `data` to start with SOI, which the scanner guarantees
/// by construction.
///
/// RAW files embed their sensor data as JPEG-framed streams too (lossless
/// SOF3, arithmetic-coded variants), and stray `FFD8`/`FFD9` pairs inside
/// compressed data produce garbage spans. The full-screen preview path
/// returns span bytes without decoding them, so the scanner is the only gate.
fn is_displayable_jpeg(data: &[u8]) -> bool {
    displayable_dimensions(data).is_some()
}

/// Walk the JPEG marker stream and return the frame's `(width, height)` in
/// pixels if it is a displayable Huffman baseline/extended/progressive frame
/// (SOF0/1/2), else `None`. The SOF segment is validated the way a conformant
/// decoder does (length against component count, precision, component count),
/// so garbage spans are rejected here rather than at decode time. See
/// [`is_displayable_jpeg`] for which frames count as displayable and why.
/// Expects `data` to start with SOI.
fn displayable_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut pos = 2; // past SOI
    loop {
        // Skip fill bytes (0xFF padding before a marker).
        while pos < data.len() && data[pos] == 0xFF && data.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let (Some(&0xFF), Some(&marker)) = (data.get(pos), data.get(pos + 1)) else {
            return None;
        };
        match marker {
            // SOF layout after the marker: length(2), precision(1),
            // height(2), width(2), component_count(1), then 3 bytes per
            // component. A truncated header yields None, rejecting the span.
            //
            // Validate the segment as a conformant decoder would before
            // trusting the dimensions: length must be the fixed 8-byte header
            // plus 3 bytes per component, samples must be 8-bit, and the
            // component count must be displayable (1 gray, 3 YCbCr/RGB, 4
            // CMYK/YCCK). Kept strict on purpose — a stray FFD8..FFD9 span
            // formed inside a RAW's binary data can pass a bare marker walk yet
            // declare garbage dimensions that outscore the real preview, then
            // fail in the renderer. Do not relax.
            0xC0..=0xC2 => {
                let length = u16::from_be_bytes([*data.get(pos + 2)?, *data.get(pos + 3)?]);
                let precision = *data.get(pos + 4)?;
                let height = u16::from_be_bytes([*data.get(pos + 5)?, *data.get(pos + 6)?]);
                let width = u16::from_be_bytes([*data.get(pos + 7)?, *data.get(pos + 8)?]);
                let components = *data.get(pos + 9)?;
                if precision != 8
                    || !matches!(components, 1 | 3 | 4)
                    || usize::from(length) != 8 + 3 * usize::from(components)
                {
                    return None;
                }
                return Some((u32::from(width), u32::from(height)));
            }
            // Lossless (C3, C7, CF), hierarchical (C5, C6, CD, CE),
            // arithmetic (C9..CB), and DAC (CC) frames are undecodable
            // downstream; SOS or EOI before any SOF is malformed. C4 (DHT)
            // and C8 (reserved) sit inside the C0-CF block but are ordinary
            // length segments, not frame markers.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCF | 0xDA | 0xD9 => return None,
            // Standalone markers (no length segment).
            0x01 | 0xD0..=0xD8 => pos += 2,
            _ => {
                let seg = data.get(pos + 2..pos + 4)?;
                pos += 2 + usize::from(u16::from_be_bytes([seg[0], seg[1]]));
            }
        }
    }
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
            let img = decode_photo_scaled(data, size)?;
            let resized = resize_fast(img, size)?;
            let oriented =
                apply_orientation_transform(DynamicImage::ImageRgb8(resized), orientation);
            encode_jpeg(&oriented.into_rgb8(), THUMBNAIL_JPEG_QUALITY)?
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
        if let Ok(result) = try_decode_resize_encode(&data[start..start + len], orientation, size) {
            return Ok(result);
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
    ///
    /// `data` is a growing buffer fed across calls; scanning stops one byte
    /// short of the end (`end = len - 1`) and resumes there next call, so a
    /// marker pair straddling a chunk boundary (`0xFF` last, `0xD8`/`0xD9`
    /// first) is caught once the following byte arrives.
    fn scan(&mut self, data: &[u8]) {
        let end = data.len().saturating_sub(1);
        for i in memchr::memchr_iter(0xFF, &data[self.offset..end]) {
            let pos = self.offset + i;
            match data[pos + 1] {
                0xD8 => self.open_sois.push(pos),
                0xD9 => {
                    let eoi_end = pos + 2;
                    if let Some(soi) = self.open_sois.pop() {
                        let len = eoi_end - soi;
                        if len > 100 && is_displayable_jpeg(&data[soi..eoi_end]) {
                            self.spans.push((soi, len));
                        }
                    }
                }
                _ => {}
            }
        }
        self.offset = end;
    }

    /// Return the first completed span >= `MIN_PREVIEW_BYTES` as (start, len).
    fn good_preview_span(&self) -> Option<(usize, usize)> {
        self.spans
            .iter()
            .find(|(_, len)| *len >= MIN_PREVIEW_BYTES)
            .copied()
    }

    /// Return the completed span with the most pixels as (start, len), reading
    /// dimensions from each span's SOF header. `data` is the buffer the spans
    /// index into.
    fn largest_by_pixels(&self, data: &[u8]) -> Option<(usize, usize)> {
        self.spans
            .iter()
            .max_by_key(|(start, len)| {
                // Every stored span passed the scanner's SOF gate, so its
                // dimensions parse.
                let (w, h) = displayable_dimensions(&data[*start..*start + *len])
                    .expect("scanner only stores spans with a displayable SOF header");
                u64::from(w) * u64::from(h)
            })
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
    let img = decode_jpeg_scaled(jpeg, size)?;
    let resized = resize_fast(img, size)?;
    let oriented = apply_orientation_transform(DynamicImage::ImageRgb8(resized), orientation);
    encode_jpeg(&oriented.into_rgb8(), THUMBNAIL_JPEG_QUALITY)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal SOF0 frame header (baseline, 8-bit, one component) carrying
    /// `width` x `height`. `displayable_dimensions` reads the dimensions from
    /// this segment.
    fn sof0(width: u16, height: u16) -> Vec<u8> {
        let mut seg = vec![0xFF, 0xC0, 0x00, 0x0B, 0x08];
        seg.extend_from_slice(&height.to_be_bytes());
        seg.extend_from_slice(&width.to_be_bytes());
        seg.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]);
        seg
    }

    /// A SOF0 marker + header with caller-chosen field values, for driving the
    /// displayability gate against malformed frames. A conformant frame has
    /// `length == 8 + 3 * components`, `precision == 8`, and `components` in
    /// {1, 3, 4}. The gate inspects only these header bytes, so no per-component
    /// descriptors are appended.
    fn sof_header(length: u16, precision: u8, width: u16, height: u16, components: u8) -> Vec<u8> {
        let mut seg = vec![0xFF, 0xC0];
        seg.extend_from_slice(&length.to_be_bytes());
        seg.push(precision);
        seg.extend_from_slice(&height.to_be_bytes());
        seg.extend_from_slice(&width.to_be_bytes());
        seg.push(components);
        seg
    }

    /// Byte sequence that passes the scanner's filters: SOI, an APP0 padding
    /// segment of `pad` zero bytes (no stray 0xFF), an SOF0 frame header
    /// carrying `width` x `height`, then EOI.
    fn jpeg_with_dimensions(width: u16, height: u16, pad: usize) -> Vec<u8> {
        let app0_len = u16::try_from(pad + 2).expect("padding fits a JPEG segment length");
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0];
        data.extend_from_slice(&app0_len.to_be_bytes());
        data.extend(std::iter::repeat_n(0x00, pad));
        data.extend(sof0(width, height));
        data.extend_from_slice(&[0xFF, 0xD9]);
        data
    }

    /// A well-formed span comfortably over the scanner's 100-byte floor.
    fn minimal_jpeg() -> Vec<u8> {
        jpeg_with_dimensions(64, 48, 0x60)
    }

    #[test]
    fn scan_finds_span_in_single_pass() {
        let jpeg = minimal_jpeg();
        let mut scanner = JpegScanner::new();
        scanner.scan(&jpeg);
        assert_eq!(scanner.spans, vec![(0, jpeg.len())]);
    }

    #[test]
    fn scan_catches_eoi_split_across_chunk_boundary() {
        let jpeg = minimal_jpeg();
        let split = jpeg.len() - 1; // 0xFF of EOI lands at the chunk edge.

        let mut scanner = JpegScanner::new();
        // First chunk withholds the trailing 0xD9; scan leaves the final 0xFF
        // unscanned, so no span completes yet.
        scanner.scan(&jpeg[..split]);
        assert!(scanner.spans.is_empty());
        assert_eq!(scanner.open_sois, vec![0]);

        // Growing buffer delivers the 0xD9; the split EOI is now recognized.
        scanner.scan(&jpeg);
        assert_eq!(scanner.spans, vec![(0, jpeg.len())]);
    }

    #[test]
    fn scan_rejects_span_with_undecodable_frame_header() {
        // A false span as found inside NEF raw sensor data: a stray SOI
        // immediately followed by a DAC marker (0xCC), closed by a stray EOI.
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xCC];
        data.extend(std::iter::repeat_n(0x00, 100));
        data.extend_from_slice(&[0xFF, 0xD9]);

        let mut scanner = JpegScanner::new();
        scanner.scan(&data);
        assert!(scanner.spans.is_empty());
    }

    #[test]
    fn scan_accepts_span_with_dht_before_frame_header() {
        // C4 (DHT) sits inside the C0-CF block but is a length segment, not a
        // frame marker; the walk must skip it and accept the SOF0 behind it.
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x62];
        data.extend(std::iter::repeat_n(0x00, 0x60));
        data.extend(sof0(64, 48));
        data.extend_from_slice(&[0xFF, 0xD9]);

        let mut scanner = JpegScanner::new();
        scanner.scan(&data);
        assert_eq!(scanner.spans, vec![(0, data.len())]);
    }

    #[test]
    fn displayable_dimensions_reads_sof_frame_size() {
        let jpeg = jpeg_with_dimensions(1920, 1280, 0x60);
        assert_eq!(displayable_dimensions(&jpeg), Some((1920, 1280)));
    }

    #[test]
    fn largest_by_pixels_prefers_resolution_over_byte_size() {
        // A byte-heavy low-res preview followed by a byte-light high-res one.
        // Selection must go by pixel count, not span length.
        let low_res = jpeg_with_dimensions(160, 120, 4000);
        let high_res = jpeg_with_dimensions(2000, 1500, 0x60);
        assert!(
            high_res.len() < low_res.len(),
            "high-res span must be smaller in bytes for this test to be meaningful"
        );

        let mut data = low_res;
        let high_start = data.len();
        data.extend_from_slice(&high_res);

        let mut scanner = JpegScanner::new();
        scanner.scan(&data);
        assert_eq!(scanner.spans.len(), 2);
        assert_eq!(
            scanner.largest_by_pixels(&data),
            Some((high_start, high_res.len()))
        );
    }

    #[test]
    fn displayable_dimensions_rejects_sof_length_inconsistent_with_components() {
        // The reported NEF failure class: a stray SOF whose length field (here
        // 12) doesn't equal 8 + 3 * component_count (8 + 3*1 = 11). A real
        // decoder rejects it, so the gate must too — otherwise the bogus span
        // outscores the genuine preview on its garbage dimensions.
        let data = [vec![0xFF, 0xD8], sof_header(12, 8, 100, 100, 1)].concat();
        assert_eq!(displayable_dimensions(&data), None);
    }

    #[test]
    fn displayable_dimensions_rejects_bad_precision_and_component_count() {
        // 12-bit precision: length 8 + 3*1 = 11 is self-consistent, but the
        // decode pipeline only handles 8-bit samples.
        let bad_precision = [vec![0xFF, 0xD8], sof_header(11, 12, 100, 100, 1)].concat();
        assert_eq!(displayable_dimensions(&bad_precision), None);

        // 189 components (0xBD) — the garbage count from the reported error;
        // outside the displayable set {1, 3, 4}. Length is consistent
        // (8 + 3*189 = 575) so only the component count is at fault.
        let bad_components = [vec![0xFF, 0xD8], sof_header(575, 8, 100, 100, 189)].concat();
        assert_eq!(displayable_dimensions(&bad_components), None);
    }

    #[test]
    fn displayable_dimensions_accepts_three_component_sof() {
        // A well-formed YCbCr frame (3 components, length 8 + 3*3 = 17, 8-bit):
        // the gate must still accept it, guarding against over-tightening.
        let data = [vec![0xFF, 0xD8], sof_header(17, 8, 1920, 1280, 3)].concat();
        assert_eq!(displayable_dimensions(&data), Some((1920, 1280)));
    }

    #[test]
    fn scan_excludes_bogus_span_so_selector_returns_genuine_preview() {
        // End-to-end reproduction of the reported bug: a genuine embedded
        // preview plus a bogus span whose fake SOF declares enormous
        // dimensions. Before the gate validated the SOF segment, the bogus
        // span was admitted and outscored the real preview on pixel count;
        // now it never enters the candidate set.
        let genuine = jpeg_with_dimensions(1024, 768, 0x60);

        // Bogus span: SOI, a malformed SOF claiming 9000x9000 with the reported
        // garbage length/component count, zero padding to clear the 100-byte
        // floor, then EOI.
        let mut bogus = vec![0xFF, 0xD8];
        bogus.extend(sof_header(0xBDC8, 8, 9000, 9000, 0xBD));
        bogus.extend(std::iter::repeat_n(0x00, 100));
        bogus.extend_from_slice(&[0xFF, 0xD9]);

        let mut data = bogus;
        let genuine_start = data.len();
        data.extend_from_slice(&genuine);

        let mut scanner = JpegScanner::new();
        scanner.scan(&data);
        assert_eq!(
            scanner.spans,
            vec![(genuine_start, genuine.len())],
            "only the genuine preview should be admitted as a candidate"
        );
        assert_eq!(
            scanner.largest_by_pixels(&data),
            Some((genuine_start, genuine.len()))
        );
    }

    #[test]
    fn orientation_applied_after_resize_swaps_dimensions() {
        // 400x200 landscape -> resized to a 100px box -> 100x50.
        let img = DynamicImage::ImageRgb8(RgbImage::new(400, 200));
        let resized = resize_fast(img, 100).expect("resize succeeds");
        assert_eq!((resized.width(), resized.height()), (100, 50));

        // Orientation 6 (rotate 90) on the small image swaps to portrait 50x100.
        let oriented = apply_orientation_transform(DynamicImage::ImageRgb8(resized), 6);
        assert_eq!((oriented.width(), oriented.height()), (50, 100));
    }
}
