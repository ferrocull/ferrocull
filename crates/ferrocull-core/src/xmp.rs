//! XMP sidecar file generation and reading for photo metadata.
//!
//! Generates and parses XMP sidecars compatible with darktable, Lightroom, and other
//! photo management applications.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Datelike, Utc};
use quick_xml::{
    events::{BytesEnd, BytesPI, BytesStart, Event},
    reader::Reader,
    writer::Writer,
};

use crate::media::ColorLabel;

/// Metadata for XMP sidecar generation and reading.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    /// XMP rating in `[-1, 5]`: `-1` rejected, `0` unrated, `1..=5` star rating.
    pub rating: i8,
    /// XMP color label, or `None` if unclassified.
    pub color_label: Option<ColorLabel>,
    pub original_filename: Option<String>,
    pub capture_date: Option<DateTime<Utc>>,
}

/// Generates XMP sidecar content.
///
/// # Panics
/// Panics if the XML writer fails on an in-memory `Vec` (should never happen).
#[must_use]
pub fn generate_xmp(metadata: &Metadata) -> Vec<u8> {
    let mut writer = Writer::new_with_indent(io::Cursor::new(Vec::new()), b' ', 1);

    writer
        .write_event(Event::PI(BytesPI::new(
            "xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"",
        )))
        .expect("write to Vec");

    let mut xmpmeta = BytesStart::new("x:xmpmeta");
    xmpmeta.push_attribute(("xmlns:x", "adobe:ns:meta/"));
    writer
        .write_event(Event::Start(xmpmeta))
        .expect("write to Vec");

    let mut rdf = BytesStart::new("rdf:RDF");
    rdf.push_attribute(("xmlns:rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"));
    writer.write_event(Event::Start(rdf)).expect("write to Vec");

    let mut desc = BytesStart::new("rdf:Description");
    desc.push_attribute(("rdf:about", ""));
    desc.push_attribute(("xmlns:xmp", "http://ns.adobe.com/xap/1.0/"));

    let rating_str = metadata.rating.to_string();
    desc.push_attribute(("xmp:Rating", rating_str.as_str()));

    if let Some(label) = metadata.color_label {
        desc.push_attribute(("xmp:Label", label.xmp_str()));
    }

    if let Some(ref filename) = metadata.original_filename {
        desc.push_attribute(("xmp:Nickname", filename.as_str()));
    }

    if let Some(date) = metadata.capture_date {
        let date_str = format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day());
        desc.push_attribute(("xmp:CreateDate", date_str.as_str()));
    }

    writer
        .write_event(Event::Empty(desc))
        .expect("write to Vec");

    writer
        .write_event(Event::End(BytesEnd::new("rdf:RDF")))
        .expect("write to Vec");
    writer
        .write_event(Event::End(BytesEnd::new("x:xmpmeta")))
        .expect("write to Vec");

    writer
        .write_event(Event::PI(BytesPI::new("xpacket end=\"w\"")))
        .expect("write to Vec");

    let mut output = writer.into_inner().into_inner();
    output.push(b'\n');
    output
}

/// Parses XMP content and extracts rating and color label metadata.
///
/// Returns `None` if the content is not valid XMP or contains no `rdf:Description`.
#[must_use]
pub fn parse_xmp(content: &[u8]) -> Option<Metadata> {
    let content_str = std::str::from_utf8(content).ok()?;
    let mut reader = Reader::from_str(content_str);
    reader.config_mut().trim_text(true);

    let mut found_description = false;
    let mut rating_value: Option<i32> = None;
    let mut label: Option<ColorLabel> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e))
                if e.name().local_name().as_ref() == b"Description" =>
            {
                found_description = true;
                for attr in e.attributes().flatten() {
                    match attr.key.local_name().as_ref() {
                        b"Rating" => {
                            rating_value = std::str::from_utf8(&attr.value)
                                .ok()
                                .and_then(|v| v.parse().ok());
                        }
                        b"Label" => {
                            label = std::str::from_utf8(&attr.value)
                                .ok()
                                .and_then(ColorLabel::from_xmp_str);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }

    if !found_description {
        return None;
    }

    let rating = match rating_value {
        Some(r @ -1..=5) => {
            #[expect(clippy::cast_possible_truncation, reason = "range -1..=5 fits i8")]
            let r = r as i8;
            r
        }
        _ => 0,
    };

    Some(Metadata {
        rating,
        color_label: label,
        ..Default::default()
    })
}

/// Reads and parses an XMP sidecar file.
pub fn read_sidecar(path: &Path) -> io::Result<Metadata> {
    let content = fs::read(path)?;
    parse_xmp(&content).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: no XMP metadata found", path.display()),
        )
    })
}

/// Writes an XMP sidecar file alongside the given image path.
///
/// Creates `image.ext.xmp` (darktable style) next to the original file.
pub fn write_sidecar(image_path: &Path, metadata: &Metadata) -> io::Result<()> {
    let sidecar_path = sidecar_path_for(image_path);
    let xmp_content = generate_xmp(metadata);
    fs::write(&sidecar_path, xmp_content)
}

/// Returns the XMP sidecar path for a given image path.
///
/// Uses darktable convention: `image.cr2.xmp` (appends `.xmp`).
#[must_use]
pub fn sidecar_path_for(image_path: &Path) -> PathBuf {
    let mut sidecar = image_path.as_os_str().to_owned();
    sidecar.push(".xmp");
    PathBuf::from(sidecar)
}
