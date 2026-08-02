//! Bundled typefaces: IBM Plex Sans for UI text, IBM Plex Mono for content
//! the user reads character-by-character (rename patterns, filename previews,
//! key caps). Static instances, registered at boot in `run()`.
//!
//! Plex ships tabular figures and the mono's marked zero as font defaults,
//! which is required here: iced 0.14 passes no OpenType features to the
//! shaper, so only default behavior is reachable.

use iced::Font;
use iced::font::Weight;

pub(crate) const SANS: Font = Font::with_name("IBM Plex Sans");
pub(crate) const SANS_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..SANS
};
pub(crate) const MONO: Font = Font::with_name("IBM Plex Mono");

pub(crate) const SANS_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
pub(crate) const SANS_SEMIBOLD_BYTES: &[u8] =
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf");
pub(crate) const MONO_REGULAR_BYTES: &[u8] =
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");
pub(crate) const MONO_SEMIBOLD_BYTES: &[u8] =
    include_bytes!("../assets/fonts/IBMPlexMono-SemiBold.ttf");
