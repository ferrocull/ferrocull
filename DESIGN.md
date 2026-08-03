---
name: Ferrocull
description: A FOSS culling tool for photographers — Darkroom Editorial, warm and invisible
colors:
  safelight-amber: "#D98E48"
  safelight-amber-hover: "#E59E58"
  safelight-amber-muted: "#A06A35"
  safelight-amber-pressed: "#C67F3D"
  focus-blue-dark: "#5A9BD9"
  focus-blue-light: "#3D6FA6"
  sage-success: "#5CB87A"
  gold-warning: "#E5A853"
  warm-danger: "#D95353"
  warm-danger-rest: "#CB4747"
  warm-danger-hover: "#C43E3E"
  warm-danger-pressed: "#B94747"
  dark-bg: "#181716"
  dark-ink: "#F0EBE5"
  light-bg: "#FAF8F5"
  light-ink: "#2A2622"
  taupe-muted: "#8C7B6A"
  taupe-muted-hover: "#A89682"
  rejected-wash: "#3D1C1C"
  label-red: "#D94F4F"
  label-gold: "#E5C04D"
  label-green: "#5CB87A"
  label-blue: "#5A9BD9"
  label-purple: "#9B6BC9"
  label-orange: "#E56A2E"
  label-gray: "#ADA69C"
typography:
  headline:
    fontFamily: "IBM Plex Sans"
    fontSize: "24px"
    fontWeight: 400
  title:
    fontFamily: "IBM Plex Sans"
    fontSize: "14px"
    fontWeight: 600
  body:
    fontFamily: "IBM Plex Sans"
    fontSize: "12px"
    fontWeight: 400
  label:
    fontFamily: "IBM Plex Sans"
    fontSize: "10px"
    fontWeight: 400
  mono:
    fontFamily: "IBM Plex Mono"
    fontSize: "12px"
    fontWeight: 400
rounded:
  xs: "2px"
  sm: "4px"
  md: "6px"
  lg: "10px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
components:
  button-primary:
    backgroundColor: "{colors.safelight-amber}"
    textColor: "{colors.light-ink}"
    rounded: "{rounded.sm}"
  button-primary-hover:
    backgroundColor: "{colors.safelight-amber-hover}"
    textColor: "{colors.light-ink}"
    rounded: "{rounded.sm}"
  button-danger:
    backgroundColor: "{colors.warm-danger-rest}"
    textColor: "#FFFFFF"
    rounded: "{rounded.sm}"
  filter-pill-selected:
    backgroundColor: "{colors.safelight-amber-muted}"
    textColor: "#FFFFFF"
    rounded: "{rounded.lg}"
---

# Design System: Ferrocull

## 1. Overview

**Creative North Star: "The Darkroom"**

Ferrocull's interface is a darkroom: a warm, dim workspace where the print is the only bright thing. Surfaces are warm near-blacks (dark-first, with a warm-white light theme resolved from OS preference), text is warm off-white, and a single amber accent — the safelight — marks selection, focus, and primary actions. The photographs carry all the visual weight; the chrome is quiet, flat, and recedes behind them. This is a professional culling tool in the Photo Mechanic tradition, and the design's job is to disappear into the task: every visual decision optimizes time-to-decision for a photographer moving through thousands of frames by keyboard.

The system explicitly rejects cluttered, dialog-heavy legacy chrome, trendy web-app gloss (glassmorphism, gradients, marketing polish), and the bare unstyled GTK/Qt utility look. Warmth is deliberate and photographic, never decorative.

**Key Characteristics:**
- Dark-first, warm-neutral surfaces; the image is the brightest element on screen
- One amber accent ("Safelight Amber") for selection, focus, and primary actions — never decoration
- Quiet-until-needed components: transparent at rest, amber on interaction
- Dense, keyboard-first layouts; small type sizes (9–14px) are the working range
- Flat panels with 1px borders; shadows reserved for elements that genuinely float

## 2. Colors

A warm-neutral base with one amber voice and a fixed semantic vocabulary; the seven XMP color-label hues are data colors, not brand colors.

### Primary
- **Safelight Amber** (#D98E48): The single accent. Primary action buttons, current selection (date tree, settings rail, filter pills), focused/active icon tints, progress bars. Hover brightens to Safelight Amber Hover (#E59E48-family, #E59E58); pressed button fills dim to Safelight Amber Pressed (#C67F3D); background tints use Safelight Amber Muted (#A06A35) at 0.28–0.38 alpha for selected-row washes. On light surfaces, selected text on the amber tint uses the dark amber ink #6E4419.
- **Focus Blue** (#5A9BD9 dark / #3D6FA6 light): The keyboard cursor — the border on the focused thumbnail, and the system's only cool hue. Blue keeps the cursor distinguishable from amber selection at a glance.

### Neutral
- **Dark Bg** (#181716): Warm near-black body surface of the dark theme; the grid and preview canvas.
- **Dark Ink** (#F0EBE5): Warm white text on dark surfaces.
- **Light Bg** (#FAF8F5) / **Light Ink** (#2A2622): The light-theme pair, same warmth, resolved from OS preference or explicit user choice.
- **Taupe Muted** (#8C7B6A): Secondary text. The warm-gray workhorse. The burst badge pill uses a deeper step (#6B5D4E, hover #746353) so its warm-white count stays legible.
- Panel and border steps come from iced's generated extended palette (weakest/weak/neutral/strong steps off the base background) — sidebars sit on the `weakest` step with 1px `weaker` borders.

### Semantic
- **Sage Success** (#5CB87A): Confirmations, verified-ingest states. Doubles as color label 3 (Green).
- **Gold Warning** (#E5A853): Warnings; deliberately close to the accent family so alarm stays warm.
- **Warm Danger** (#D95353): Destructive actions and rejection, and the ink for danger/error *text*. Danger *button* fills use a slightly deepened rest step (#CB4747) so white text clears 4.5:1 (the base #D95353 is only 3.95:1 with white); hover #C43E3E and pressed #B94747 already pass. Rejected thumbnails get the **Rejected Wash** (#3D1C1C) dark-red background.

### Color Labels (data, indexed 1–7 to match XMP)
The seven XMP labels, in order: Red #D94F4F · Gold (XMP "Yellow") #E5C04D · Green #5CB87A · Blue #5A9BD9 · Purple #9B6BC9 · Orange #E56A2E · Gray #ADA69C. Each hue must correspond to its XMP name (a unit test pins the dark palette to per-name hue windows so a swap can't recur): Orange is a true orange (~20° hue, redder than the amber accent and the gold label), and Gray is a low-saturation warm neutral, distinct from the taupe chrome. These mark user metadata on thumbnails and filter pills; they are never used as UI chrome. Thumbnail label bars in the light theme use darkened variants (Gold #A8862B, Green #3D8E58, Blue #3A78B5, Orange #C05A1E, Gray #7D7468) so every hue reads ≥3:1 on warm-white while staying recognizable.

### Named Rules
**The Safelight Rule.** Amber is the only light that doesn't spoil the print: it appears on state (selection, focus, primary action, progress) and nowhere else. If amber is decorating rather than indicating, remove it. The one sanctioned exception is Focus Blue — the keyboard cursor must stay distinguishable from amber selection.

**The Print-Is-Brightest Rule.** No chrome element may compete in luminance or saturation with the photographs. Overlays on images use warm-black semi-transparent badges (rgba ~0.12/0.11/0.10 at 0.88), never bright fills.

## 3. Typography

**UI Font:** IBM Plex Sans, bundled static Regular (400) and SemiBold (600), the app's default font
**Mono Font:** IBM Plex Mono, bundled static Regular (400) and SemiBold (600): rename patterns, filename previews, shortcut key caps, anything the user must read character-by-character

**Character:** One quiet sans at small, dense sizes. No display font, no pairing games — the type is labeling, not performing. Plex's slightly squared, engineering-rooted voice stays workmanlike and unshowy.

Why Plex, and why statics (decided in [the typography wayfinder](https://github.com/ferrocull/ferrocull/issues/34)): the renderer decides. iced 0.14 forwards no OpenType features to the shaper, so only a font's *default* behavior is reachable. Plex Sans ships tabular figures by default (counts, ratings, and the status bar never jitter), and Plex Mono's dotted zero and distinct `1`/`l`/`I` are defaults rather than opt-in features. Sans and Mono are one designed superfamily with identical vertical metrics, so mixed sans/mono lines (status bar, rename preview) share one optical size. Variable fonts register only their default weight in this stack and silently fall back to a system font at any other, so the SemiBold statics are load-bearing, not a packaging choice. The OFL-1.1 license text ships alongside the font files.

### Hierarchy
- **Headline** (400, 24px): Rare; dialog/settings headers and empty-state titles only.
- **Title** (600, 14–16px): Section headers, panel titles.
- **Body** (400, 12–13px): The working size for controls, lists, and settings prose.
- **Label** (400, 10–11px): Badges, thumbnail info bars, status-bar detail.
- **Caption** (400, 9px): Finest print — overlay metadata where space is scarce.

### Named Rules
**The Density Rule.** 12px is the default voice. Sizes above 16px must justify themselves; this is a pro tool viewed at desktop DPI, not a marketing page.

### Iconography

**Set:** Bootstrap Icons (MIT), embedded as an icon font and drawn on a 16px grid, so glyphs are natively crisp at the 10–16px sizes this UI uses. The MIT notice ships in the third-party notices.

- **Icons are text.** Glyphs render through the same text pipeline as every label: sizing is the text size, and tinting is text color styling. State color follows the Safelight Rule exactly as it would on a word.
- **Roles, not glyphs.** Call sites go through the intent-named vocabulary (`icons.rs`: `reject()`, `star_filled()`, `chevron_expanded()`, ...). The glyph behind a role lives in one place, so a remap or a set swap touches one file, and one role always renders one glyph everywhere.
- **Size to the neighboring text.** Badges 9–10px, inline controls and marks 10–12px, close/zoom affordances 14px, the settings gear 16px.
- **Never inside strings.** An icon-font glyph cannot ride in a format string; icon widgets sit beside text in a row. Unicode symbols in UI strings are reserved for glyphs the bundled text font covers (`·`, `—`, `…`).

| Role | Glyph |
|---|---|
| Disclosure expanded / collapsed | `chevron-down` / `chevron-right` |
| Nav previous / next | `chevron-left` / `chevron-right` |
| Dropdown handle | `chevron-down` (shared deliberately; a menu strip and a disclosure indicator never share a control) |
| Sort ascending / descending | `arrow-up` / `arrow-down` |
| Rating star filled / outline | `star-fill` / `star` |
| Unrated (rating filter pill) | `slash-circle` |
| Scroll locked / unlocked (compare panes) | `lock` / `unlock` |
| Close and Reject | `x-lg` (shared deliberately; a close affordance and a reject mark never share a context) |
| Tag check | `check-lg` |
| Ingested | `download` |
| Preview zoom | `zoom-in` |
| Settings | `gear` |
| Undo | `arrow-counterclockwise` |
| Burst | `stack` |
| Storage source / Camera source / Directory source | `sd-card` / `camera` / `folder` |

Binary-size stance: the alpha ships the full font files (four Plex statics plus the icon TTF, ~1.2 MB total). Subsetting is a deferred optimization, and it must start from the `iced_fonts`-vendored TTF: upstream Bootstrap Icons distributes WOFF2 only, which this rendering stack cannot read.

## 4. Elevation

Flat by default; a shadow means the element floats. Panels, sidebars, and the grid are flat surfaces separated by 1px borders and background-lightness steps. Shadows appear only on things genuinely above the workspace: the settings card (0 8px 32px at 0.45 black over a warm-black 0.62-alpha scrim), the status bar (0 −2px 8px, lighter in light theme), and primary/danger buttons (a whisper: 0 1px 2px at ≤0.2 alpha).

### Shadow Vocabulary
- **Floating card** (`0 8px 32px rgba(0,0,0,0.45)`): Modal-grade surfaces over a scrim.
- **Edge lift** (`0 -2px 8px rgba(0,0,0,0.3)` dark / `0.1` light): The status bar separating itself from the grid.
- **Button whisper** (`0 1px 2px rgba(0,0,0,0.15–0.2)`): Primary buttons only; disabled buttons drop it.

### Named Rules
**The Float Rule.** If it doesn't float above the workspace, it doesn't cast a shadow. Panels and headers earn separation with borders and tone, never shadow.

## 5. Components

Quiet until needed: transparent or tonal at rest, amber on interaction or selection. Color is the affordance.

### Buttons
- **Shape:** Softly squared (4px radius); filter pills round further (10px).
- **Primary:** Safelight Amber fill, dark warm-ink text (#2A2622), button-whisper shadow. Hover brightens, press dims to Safelight Amber Pressed.
- **Secondary:** Tonal — weak-step background with a 1px strong-step border; no accent.
- **Danger:** Warm Danger fill, white text; reserved for destructive actions.
- **Ghost / Icon:** No box at all. Ghost buttons gain a neutral tonal wash on hover; icon buttons stay boxless and tint the glyph amber on hover/press — the color change is the entire affordance.
- **Disabled:** Tonal grays from the palette steps, shadow removed.

### Filter Pills
- **Style:** 10px-radius pills; unselected are transparent with a 1px weak border and muted text, selected fill with muted amber, white text, and a full-amber 1px border.

### Panels / Containers
- **Corner Style:** Square (0) for structural panels; 4–10px for floating or inset elements.
- **Background:** `weakest` palette step for sidebars, base background for the grid and preview.
- **Border:** 1px in the `weaker` step; this is the primary separator vocabulary.
- **Internal Padding:** the 4/8/12/16px spacing scale.

### Inputs
- **Style:** iced defaults tuned to theme; the merged pattern control squares the adjoining corners of its picker + input halves so they read as one control (2px outer radius).
- **Mono content:** rename-pattern inputs and previews render monospace.

### Selection & Focus (signature)
The focused thumbnail carries a blue-family border (Focus Blue: #5A9BD9 dark / #3D6FA6 light); on a rejected card in the light theme the border switches to the dark-theme blue (#5A9BD9), since the darkened light blue reads only 2.91:1 on the Rejected Wash (the brighter blue reads 5.16:1). Tagged/selected rows and rail items carry amber washes (muted amber at 0.28–0.38 alpha) with amber text (dark amber ink in the light theme). Tagged thumbnails pair an amber wash with an amber check badge — the badge is the guaranteed mark over any photo. Rejected thumbnails sit on the dark-red Rejected Wash with a red badge. Burst membership shows as a deep warm-taupe pill badge. Already-ingested frames carry a taupe download-arrow badge (#A89682 on the warm-black badge fill, 5.94:1) — deliberately not amber, since "already copied" is completed history and the Safelight Rule reserves amber for active state. The full-screen views (preview, compare) carry rejected/tagged/ingested as the same top-left badges and never as washes: at full-screen scale a wash shades the photograph being judged. These state layers are the interface's real vocabulary — they must stay instantly distinguishable at a glance and at speed.

### Thumbnail Overlays
Warm-black semi-transparent badges (4px radius) for info bars, pair badges, and rating marks; a 0.55-alpha warm-black wash marks already-ingested items, paired with the ingested badge — the wash is the fast-scan cue, the badge the guaranteed mark over any photo. Overlays never use opaque or bright fills.

## 6. Do's and Don'ts

### Do:
- **Do** route every accent use through the Safelight Rule: amber marks selection, focus, primary action, or progress — nothing else.
- **Do** keep panels flat with 1px borders; reserve shadows for the floating tier (settings card, status bar, buttons at ≤0.2 alpha).
- **Do** use the 4/8/12/16px spacing scale and 2/4/6/10px radius scale; new values need a reason.
- **Do** keep every interactive state pair defined — hover, pressed, disabled — matching the existing quiet-until-needed pattern (transparent rest, tonal hover, amber active).
- **Do** design keyboard-first: every new action gets a shortcut and must be reachable without a mouse, matching Photo Mechanic conventions where one exists.

### Don't:
- **Don't** recreate legacy desktop chrome: no dialog-heavy flows, no visual clutter, even where the workflow matches.
- **Don't** add trendy web-app gloss: no glassmorphism, no gradients (in branding or controls), no marketing-page polish inside the tool.
- **Don't** ship the bare GTK/Qt utility look — unstyled defaults, cold grays, or mismatched control shapes; the warm darkroom theme is deliberate.
- **Don't** let chrome outshine the photograph: no bright fills or saturated colors on persistent UI, no opaque overlays on thumbnails.
- **Don't** use the seven color-label hues as UI chrome; they are user data.
- **Don't** use cool grays anywhere — every neutral in the system is warm (taupe, warm black, warm white).
