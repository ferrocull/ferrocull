# Research: UI sans + mono font pairing

Issue: [#35](https://github.com/ferrocull/ferrocull/issues/35), part of #34.
Date: 2026-08-02.

## Question

Which openly licensed (OFL or Apache-compatible) font pairing, one UI sans and one
monospace, best fits Ferrocull's Darkroom Editorial system for dense 9-14px desktop
UI, bundled into an iced 0.14 app?

## Constraints (from PRODUCT.md / DESIGN.md)

- One quiet sans at small, dense sizes: "the type is labeling, not performing".
- The Density Rule: 12px is the default voice; working range 9-14px at desktop DPI.
- Weights 400 and 600 minimum (Body/Label vs Title).
- Tabular figures for counts, star ratings, and the status bar.
- The mono renders rename patterns and filename previews, so 0/O and 1/l/I must be
  distinguishable and text must render literally (no code ligatures).
- The app is Apache-2.0; the fonts ship inside the binary, so they must be
  redistributable (OFL-1.1 is fine; OFL only asks that its license text and
  copyright notice accompany the font).
- Brand is warm, professional, invisible; a font with a loud third-party brand
  identity works against "invisible".

## Rendering stack findings (iced 0.14 / cosmic-text)

- Bundling: `iced::Settings { fonts: Vec<Cow<'static, [u8]>>, default_font: Font, .. }`
  loads font bytes at boot; selection is by family name via `Font::with_name` and
  `Font::MONOSPACE` resolves to the monospace fallback family.
  Source: [docs.rs iced 0.14 `Settings`](https://docs.rs/iced/0.14.0/iced/struct.Settings.html).
- **iced 0.14 exposes no OpenType feature control.** `iced_graphics::text::to_attributes`
  builds `cosmic_text::Attrs` from family, weight, stretch, and style only; no
  feature tags are ever passed to the shaper
  ([iced 0.14 `graphics/src/text.rs`](https://github.com/iced-rs/iced/blob/0.14/graphics/src/text.rs)).
  cosmic-text gained `Attrs::font_features` on its main branch
  ([`src/shape.rs`](https://github.com/pop-os/cosmic-text/blob/main/src/shape.rs)
  converts them to shaper features), but iced 0.14 pins cosmic-text 0.15 and never
  sets any. Consequence: **only a font's default behavior is reachable**. `tnum`,
  `zero`, and stylistic sets cannot be enabled; a sans must have tabular figures
  by default, and a mono must have its disambiguation baked into the defaults.
- `iced::Font` carries no variation-axis API, so bundle static per-weight
  instances rather than variable fonts.
- cosmic-text falls back to system fonts for glyphs the bundled font lacks, so
  Latin-focused bundles are acceptable.

## Method

License texts read from each project repository. File sizes measured from the
official release archives downloaded locally (`ls -l`, bytes, converted to KB) or
from the GitHub contents API (marked API). Figure style, x-height, zero shape,
and feature tags inspected with fontTools 4.63 on the release binaries:
"tabular by default" means all ten digit glyphs have identical advance widths in
the shipped Regular; "marked zero" means the `0` outline has a third contour
(dot or enclosed slash) that `O` lacks.

## Sans candidates

| Font | Version | License | 400 / 600 TTF (KB) | x-height | Default figures | Notes |
|---|---|---|---|---|---|---|
| Inter | 4.1 | OFL-1.1 | 412 / 420 (OTF 610 / 630) | 0.546 | Proportional | `tnum`/`zero`/ss01-ss08 exist but are unreachable in iced 0.14 |
| IBM Plex Sans | 3.005 | OFL-1.1 | 200 / 203 (OTF 136 / 143) | 0.516 | **Tabular** | Digits monospaced by design, no feature needed |
| Source Sans 3 | 3.052 | OFL-1.1 | 431 / 426 | 0.486 | **Tabular** | `pnum` is the opt-out; huge coverage (2478 glyphs) |
| Public Sans | 2.001 | OFL-1.1 | 85 / 84 (API) | 0.517 | Proportional | Tabular only via `tnum`; 648 glyphs |
| Geist | 1.800 (v1.7.2 release) | OFL-1.1 | 126 / 128 | 0.530 | Proportional | Tabular only via `tnum` |

- **Inter** ([rsms/inter](https://github.com/rsms/inter), [rsms.me/inter](https://rsms.me/inter/)):
  designed "from detailed user interfaces to marketing & signage", text optical
  size has "a tall x-height to aid in legibility"; weights 100-900. Best pure
  small-size sans of the set, but its default figures are proportional (nine
  distinct digit widths measured), so numeric UI jitters in iced 0.14, and it is
  the heaviest candidate. No designed mono sibling.
- **IBM Plex Sans** ([IBM/plex](https://github.com/IBM/plex)): "designed to work
  well in user interface (UI) environments"; Sans, Serif, Mono, and Condensed are
  one designed family. Digits are tabular by default (measured: every digit 600
  units). Grotesque with humanist warmth; neutral, workmanlike voice.
- **Source Sans 3** ([adobe-fonts/source-sans](https://github.com/adobe-fonts/source-sans)):
  "designed to work well in user interface (UI) environments", Paul D. Hunt,
  Adobe's first open-source family. Tabular by default (measured: 497 units),
  but the lowest x-height of the set (0.486), which costs legibility at 9-10px,
  and the largest files.
- **Public Sans** ([uswds/public-sans](https://github.com/uswds/public-sans)):
  USWDS fork of Libre Franklin, "strong, neutral, principles-driven". Its README
  lists tabular figures as a feature, but they sit behind `tnum` (measured
  default: ten distinct widths), so they are unreachable here. Tiny files, small
  glyph set. The GitHub license API reports NOASSERTION because LICENSE.md wraps
  the OFL; the text itself is SIL OFL-1.1.
- **Geist** ([vercel/geist-font](https://github.com/vercel/geist-font)):
  "modern, geometric typeface... principles of classic Swiss typography", made
  by Vercel as its brand face. Proportional default figures, and its strong
  association with the Vercel identity works against an "invisible" brand.

## Mono candidates

All monos have inherently tabular figures. All five have a marked default zero
(measured: `0` has 3 contours, `O` has 2).

| Font | Version | License | 400 / 600 TTF (KB) | x-height | Default substitutions | Notes |
|---|---|---|---|---|---|---|
| JetBrains Mono | 2.304 | OFL-1.1 | 274 / 277 (NL: 209 / 210) | 0.550 | Code ligatures via `calt` | Dotted zero; "1, l, I easily distinguishable" |
| IBM Plex Mono | 2.005 | OFL-1.1 | 173 / 175 (OTF 89 / 93) | 0.516 | None | Metrics identical to Plex Sans |
| Commit Mono | 1.143 | OFL-1.1 (font); MIT covers the site tooling | 275 (OTF, wt 400) / no 600 | 0.540 | "Smart kerning" via `calt` | Standard build ships 400 and 700 only |
| Geist Mono | 1.700 (v1.7.2 release) | OFL-1.1 | 149 / 150 | 0.530 | None | Metrics identical to Geist |
| Source Code Pro | 2.042 | OFL-1.1 | 210 / 207 | 0.486 | None | Metrics identical to Source Sans 3 |

- **JetBrains Mono** ([JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono),
  [specimen](https://www.jetbrains.com/lp/mono/)): "the zero has a dot inside,
  the letter O does not"; tallest x-height. Its code ligatures live in `calt`,
  which shapers apply by default, so filename previews would not render
  literally; the shipped NL ("No Ligatures") variant fixes that and is the form
  to bundle if chosen.
- **IBM Plex Mono** ([IBM/plex](https://github.com/IBM/plex)): part of the Plex
  superfamily; measured vertical metrics match Plex Sans exactly (x-height 516,
  cap height 698 per 1000 UPM), so mixed sans/mono lines share one optical size.
  No default-on substitutions: literal rendering out of the box.
- **Commit Mono** ([commitmono.com](https://commitmono.com/),
  [repo](https://github.com/eigilnikolajsen/commit-mono)): "anonymous and
  neutral" by design, which fits the brand, but the standard download ships only
  400 and 700; a 600 exists only through the site's bespoke customization build,
  which is a poor story for reproducible packaging. Its "smart kerning" also
  rides on `calt`, subtly variable spacing where literal fidelity is wanted.
- **Geist Mono** ([vercel/geist-font](https://github.com/vercel/geist-font)):
  "crafted to be the perfect partner to Geist Sans"; clean and small, no default
  substitutions, but it pairs with a sans that fails the tabular requirement.
- **Source Code Pro** ([adobe-fonts/source-code-pro](https://github.com/adobe-fonts/source-code-pro)):
  companion to Source Sans (identical measured metrics); no default
  substitutions; shares Source Sans 3's low x-height.

## Superfamily pairings

Three candidate pairs are designed as one family, confirmed by identical
measured vertical metrics: IBM Plex Sans + IBM Plex Mono (516/698), Geist +
Geist Mono (530/710), Source Sans 3 + Source Code Pro (486/660). Inter has no
mono sibling; Inter pairings are conventional, not designed.

## Ranked shortlist

1. **IBM Plex Sans + IBM Plex Mono** (recommended). The only pairing that
   clears every hard constraint: tabular figures by default in the sans (the
   requirement iced 0.14 makes non-negotiable), static 400 and 600 instances,
   marked zero and literal no-ligature rendering in the mono, one designed
   superfamily with identical metrics for the status bar and rename dialog
   where sans labels meet mono values, and moderate weight: about 750 KB total
   for four TTFs (about 460 KB if the CFF OTFs prove to render well). Plex's
   slightly squared, engineering-rooted voice is professional and unshowy,
   which is exactly "labeling, not performing".
2. **Source Sans 3 + Source Code Pro**. Same superfamily logic, tabular by
   default, designed-for-UI pedigree. Loses on the set's lowest x-height
   (0.486), which is the wrong direction for 9-10px labels, and on the largest
   sans files (about 860 KB for the pair's four TTFs).
3. **Inter + JetBrains Mono NL**. The strongest individual faces for small-size
   legibility, but Inter's proportional default figures cannot be fixed from
   iced 0.14, the pairing is not designed (x-heights 0.546 vs 0.550 do sit
   close), and it is the heaviest sans. Worth revisiting only if iced exposes
   cosmic-text font features (then `tnum`, `zero`, and cv05/cv08 become
   available and Inter leapfrogs the field).

## Recommendation

**Bundle IBM Plex Sans (Regular + SemiBold) as the UI sans and IBM Plex Mono
(Regular + SemiBold) as the mono**, via `Settings::fonts`, with
`default_font: Font::with_name("IBM Plex Sans")` and an explicit
`Font::with_name("IBM Plex Mono")` replacing `Font::MONOSPACE` at mono call
sites. The deciding fact is the renderer: iced 0.14 passes no OpenType features
to cosmic-text, so tabular figures and glyph disambiguation must be defaults,
and Plex is the pairing that ships all of them as defaults in a single designed
superfamily at a reasonable binary cost. Ship the OFL-1.1 license text and
copyright notices alongside the app's licenses.
