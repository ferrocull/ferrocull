# Research: icon approach and set for iced 0.14

Issue: [#36](https://github.com/ferrocull/ferrocull/issues/36) (part of the #34 wayfinder map).

## Question

What is the best way to render consistent icons in iced 0.14: an icon font with
codepoint constants, the `svg` feature widget, or curated Unicode with guaranteed
font coverage? And which openly licensed icon set should Ferrocull adopt?

## Requirements

From [`PRODUCT.md`](../../PRODUCT.md) and [`DESIGN.md`](../../DESIGN.md):

- Chrome is quiet and warm; icon buttons are boxless and tint the glyph amber on
  hover/press ("the color change is the entire affordance"). Per-state tinting of
  icons is therefore a hard requirement.
- The working type range is 9 to 16px at desktop DPI, so icons must stay crisp at
  12 to 16px.
- Binary size matters; the app is a single distributable binary.
- The app is Apache-2.0, so bundled icon assets must be redistributable.

## Current state

Icons today are raw Unicode glyphs inside `text` widgets, rendered by whatever
system font the fallback chain picks. That is the source of the cross-platform
inconsistency. Inventory of icon-role glyphs in `crates/ferrocull-ui`:

| Role | Glyphs | Call sites |
|---|---|---|
| Disclosure / chevrons | `▾ ▸` (U+25BE/25B8), `▼ ▶` (U+25BC/25B6) | `views/section.rs`, `views/date_tree.rs`, `views/rename.rs` |
| Sort direction | `↑ ↓` (U+2191/2193) | `views/date_tree.rs`, `views/filters.rs` |
| Star rating | `★ ☆` (U+2605/2606), `○` (U+25CB, unrated) | `views/rating.rs`, `views/filters.rs`, `views/thumbnails.rs`, `app.rs` |
| Close | `✕` (U+2715) | `views/preview.rs`, `views/compare.rs` |
| Reject (ballot X) | `✗` (U+2717) | `views/status.rs`, `views/filters.rs`, `app.rs` |
| Tag check | `✓` (U+2713) | `views/status.rs` |
| Ingested | `⤓` (U+2913) | `views/status.rs`, plus the thumbnail badge |
| Preview zoom | `⊕` (U+2295) | `views/thumbnails.rs` (comment there already notes an emoji-presentation hazard) |
| Settings | `⚙` (U+2699) | `app.rs` |
| Burst | `▣` (U+25A3) | `views/burst.rs` |
| Undo | `↩` (U+21A9) | `app.rs` |

Two of these (`⊕`, `⚙`) sit in emoji-presentation territory, where some platform
fallback chains substitute a colored emoji glyph.

## Option 1: curated Unicode with guaranteed coverage

"Guaranteed coverage" would mean restricting glyph choice to codepoints present in
every platform's default UI font, or bundling a general text font (the `fira-sans`
feature is already enabled) and hoping it covers the symbols. Neither holds up:

- No cross-platform default font covers the arrows/dingbats blocks consistently;
  `⤓` and `⊕` are exactly the glyphs that fall back inconsistently today.
- Even where coverage exists, metrics and weights differ per platform, so icons
  cannot be optically aligned or sized as a set.
- Emoji-presentation codepoints can render as colored emoji, which breaks amber
  tinting outright.

This option is the status quo and is the problem, not a solution. Rejected.

## Option 2: the `svg` feature widget

Mechanics, verified against iced 0.14 docs:

- `iced::widget::svg::Style` has exactly one field, `color: Option<Color>`: "The
  Color filter of an Svg. Useful for coloring a symbolic icon. None keeps the
  original color" ([docs.rs](https://docs.rs/iced/0.14.0/iced/widget/svg/struct.Style.html)).
  So per-state amber tinting works, via a style function per widget.
- The `svg` feature "enables the svg widget" and pulls the resvg/tiny-skia stack
  into the binary (iced `Cargo.toml` lists `resvg 0.45` on master; 0.14 uses the
  same stack) ([iced Cargo.toml](https://github.com/iced-rs/iced/blob/master/Cargo.toml)).
  `ferrocull-ui` does not currently enable `svg`, so this is a new dependency
  subtree (resvg, usvg, tiny-skia) taken on solely for icons.
- Each icon is a separate embedded asset with a `svg::Handle`; there is no
  codepoint/constant story, and SVGs are rasterized by resvg at the laid-out size
  rather than going through the text pipeline.

Workable, but it buys nothing over an icon font for monochrome symbolic icons,
and costs a rasterizer dependency, per-asset plumbing, and compile time. Rejected
as the primary mechanism (it remains the right tool if a multicolor asset ever
appears; a logo, for example).

## Option 3: icon font with codepoint constants

Mechanics, verified against iced 0.14 docs:

- Fonts load either at boot via `Settings { fonts: Vec<Cow<'static, [u8]>>, .. }`
  ([docs.rs](https://docs.rs/iced/0.14.0/iced/struct.Settings.html)) or at runtime
  via `font::load(bytes: impl Into<Cow<'static, [u8]>>) -> Task<Result<(), Error>>`
  ([docs.rs](https://docs.rs/iced/0.14.0/iced/font/fn.load.html)). Embedding with
  `include_bytes!` and listing the bytes in `Settings.fonts` is the simple path.
- A glyph is just `text("\u{F588}").font(Font::with_name("bootstrap-icons"))`;
  `Font::with_name` is a `const fn` ([docs.rs](https://docs.rs/iced/0.14.0/iced/struct.Font.html)).
- Tinting is ordinary text color styling, the exact mechanism the icon buttons
  already use for their amber hover today. No new code path, no new dependency.
- Icons render through the same cosmic-text pipeline as all other UI text, so
  crispness at 12 to 16px matches the rest of the chrome.

Packaging for iced 0.14:

- [`iced_fonts` 0.3.0](https://docs.rs/iced_fonts) (MIT, released 2025-12-08,
  depends on `iced ^0.14`) bundles eight icon fonts behind feature flags:
  Bootstrap, Lucide, Font Awesome, Nerd Font, Codicon, Devicon, Octicons,
  Pomicons. Per set it exposes the `Font` constant, the raw `FONT_BYTES` for
  `Settings.fonts`, and one function per icon returning a ready `text` widget
  (with advanced-shaping variants for glyphs that need `Shaping::Advanced`).
  Enabling a feature embeds that set's full font file
  ([repo](https://github.com/Redhawk18/iced_fonts)): `bootstrap.ttf` is 449,648
  bytes, `lucide.ttf` is 678,864 bytes.
- [`iced_fontello`](https://github.com/hecrj/iced_fontello) (by iced's author)
  generates a type-safe, subsetted icon font at compile time, but only from the
  sets Fontello hosts, which exclude every set on our shortlist.
- Independent of any crate, a vendored subset is always available: fonttools'
  `pyftsubset` can strip any of these TTFs to the roughly 20 glyphs Ferrocull
  needs, landing in the single-digit-KB range, with our own codepoint constants.

This is the recommended mechanism.

## Icon set evaluation

License texts fetched from each repo's LICENSE file. All permit redistribution
inside an Apache-2.0 binary; the MIT/ISC sets require carrying the copyright and
license notice (a THIRD-PARTY notice file or About-screen credit satisfies this).

| Set | License | Official font? | Grid | Filled + outline star? | Full-font size | Verdict |
|---|---|---|---|---|---|---|
| [Bootstrap Icons](https://github.com/twbs/icons) | MIT ([LICENSE](https://github.com/twbs/icons/blob/main/LICENSE)) | Yes, with JSON codepoint map ([font/](https://github.com/twbs/icons/tree/main/font)) | 16x16, `currentColor` fill | Yes: `star`, `star-fill`, `star-half` | 134KB woff2; 449KB ttf via iced_fonts | Recommended |
| [Lucide](https://github.com/lucide-icons/lucide) | ISC, Feather-derived icons MIT ([LICENSE](https://github.com/lucide-icons/lucide/blob/main/LICENSE)) | Yes (lucide-static; packaged in iced_fonts) | 24x24, 2px stroke | No: "Fills are not officially supported" ([docs](https://lucide.dev/guide/lucide/advanced/filled-icons)); web fill tricks don't transfer to a font glyph | 679KB ttf | Runner-up |
| [Phosphor](https://github.com/phosphor-icons/core) | MIT ([LICENSE](https://github.com/phosphor-icons/core/blob/main/LICENSE)) | Yes, one font per weight ([web repo](https://github.com/phosphor-icons/web)) | 256 grid, 6 weights | Yes, but the fill weight is a second font file (Phosphor-Fill.ttf, 449KB, plus regular) | ~450KB ttf per weight | Viable, heavier |
| [Tabler Icons](https://github.com/tabler/tabler-icons) | MIT ([LICENSE](https://github.com/tabler/tabler-icons/blob/main/LICENSE)) | Yes (@tabler/icons-webfont) | 24x24, 2px stroke, separate filled set | Yes (`star` + `star-filled`) | multi-MB webfont (5900+ icons); not packaged for iced | Viable, unpackaged |
| [Material Symbols](https://github.com/google/material-design-icons) | Apache-2.0 ([LICENSE](https://github.com/google/material-design-icons/blob/master/LICENSE)) | Variable font with FILL/wght/GRAD/opsz axes | 24x24 | Only via the FILL variation axis | multi-MB | Rejected |

Material Symbols is rejected on mechanics, not license: iced's `Font` exposes
only `family`, `weight`, `stretch`, `style`
([docs.rs](https://docs.rs/iced/0.14.0/iced/struct.Font.html)), so the FILL axis
that produces the filled star cannot be set, and the variable font is by far the
largest of the five.

Lucide's gap is decisive for a culling tool: the rating row is star-heavy
(`★★★☆☆` at 12px), and Lucide has no filled star as a distinct glyph; its
filled look relies on the SVG `fill` attribute, which does not exist in a font
glyph. Its 24-grid 2px strokes also thin out below 16px, while Bootstrap's
16-grid shapes are drawn for exactly our working sizes.

### Coverage map (Bootstrap Icons)

Every needed role resolves, verified against the official
[codepoint map](https://github.com/twbs/icons/blob/main/font/bootstrap-icons.json)
(~1,775 icons):

| Role | Current glyph | Bootstrap icon |
|---|---|---|
| Disclosure open/closed | `▾ ▸ ▼ ▶` | `chevron-down` / `chevron-right` |
| Sort asc/desc | `↑ ↓` | `sort-up` / `sort-down` (or `arrow-down-up` for a toggle) |
| Star filled / outline / half | `★ ☆` | `star-fill` / `star` / `star-half` |
| Unrated | `○` | `slash-circle` |
| Close | `✕` | `x-lg` |
| Reject | `✗` | `x-octagon`-family or `x-circle`; plain `x` where inline |
| Tag check | `✓` | `check-lg` |
| Ingest / ingested badge | `⤓` | `download` or `box-arrow-in-down` |
| Add | `⊕` (zoom affordance uses this too) | `plus-lg`; zoom maps to `zoom-in` |
| Compare | (text today) | `layout-split` |
| Settings | `⚙` | `gear` (or `sliders`) |

## Recommendation

**Approach: icon font with codepoint constants. Set: Bootstrap Icons (MIT).**

An embedded icon font rides the text pipeline the UI already lives on: per-state
amber tinting is the same text-color styling the icon buttons use now, glyphs are
hinted and shaped like every other 12px label, and the only cost is font bytes
(no resvg subtree, no per-asset handles). Bootstrap Icons wins on every axis that
matters here: it is the only shortlisted set drawn on a 16px grid, so it is
natively crisp at Ferrocull's 12 to 16px working sizes; it is the only single-file
font covering the full needed vocabulary including the `star`/`star-fill`/
`star-half` trio the rating UI depends on; its MIT license bundles cleanly into
an Apache-2.0 binary with a notice file; and it is packaged for iced 0.14 today
by `iced_fonts` 0.3 (`bootstrap` feature: font bytes for `Settings.fonts`, a
`Font` constant, and per-icon helpers), which makes adoption a dependency bump
plus mechanical glyph swaps. The 449KB embedded TTF is acceptable at alpha; if
binary size ever matters, a `pyftsubset` subset of the ~20 used glyphs drops it
to a few KB without changing any call sites.

**Runner-up: Lucide (ISC), via the same `iced_fonts` crate.** It is the closest
aesthetic fit for a warm, quiet chrome (round caps, even 2px strokes) and equally
easy to integrate, but it has no filled variants by design, so the filled rating
star cannot come from the font, and its 24-grid strokes read thinner at 12px.
If the rating row ever moves to a non-font rendering, Lucide becomes worth
revisiting; until then Bootstrap Icons is the set that covers the product.
