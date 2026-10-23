# Ferrocull

A FOSS culling tool for photographers in the Photo Mechanic tradition: fast triage of thousands of images, not a DAM and not an editor. Glossary terms track Photo Mechanic conventions wherever a term is shared.

## Language

**Ingest**:
The operation of copying media files from a source to one or more destinations, with renaming, verification, and post-copy hooks. The central workflow alongside culling.
_Avoid_: Download, Import, Copy.

**Color label**:
A single named color (Red, Yellow, Green, Blue, Purple, Orange, Gray) attached to a media file. A file has one label or none; "no label" is the absence of a value, not an 8th label.
_Avoid_: Color class, Color tag.

**Focus**:
The single media file currently under the keyboard cursor; at most one item is focused at a time. Rating, color-label, tag, and reject actions apply to the focused item.
_Avoid_: Active item, Current item.

**Tag** (verb) / **Tagged** (state):
The action of marking a file as part of the working set. A file is either tagged or not. A tag is durable: it persists until the file is [[ingest|ingested]].
_Avoid_: Pick, Star (star means rating), Mark.

**Selection**:
The set of currently [[tag|tagged]] files across the loaded view, acted on by batch operations. Filters change what is visible, not what is tagged: a hidden file stays in the selection. Distinct from [[focus]]: focus is a single cursor position, selection is a set.
_Avoid_: Pickset, Selected files.

**Arrivals window**:
The period after Tag All or Untag All pressed while a scan is still streaming frames in, during which every frame the scan produces is [[tag|tagged]] (or untagged) as it arrives. Both commands mean "the whole scan", not "the frames loaded at press time", and the active filter does not narrow that. The window closes when the scan settles, and only one direction is open at a time: the most recent press wins. Arrivals record no undo entry, so an undo restores the press-time members only.
_Avoid_: Pending tag, Deferred tag.

**Paired file**:
A sibling media file with the same basename as another (the canonical case is RAW+JPEG shot by the same camera press). Both files are first-class media; the pair is a display grouping, not a metadata one.
_Avoid_: Sibling, RAW+JPEG (when speaking generically).

**Sidecar**:
A non-media auxiliary file sharing a basename with a media file (recognised: `.xmp`, `.thm`, `.wav`, `.mp3`). Distinct from a [[paired-file]]: a sidecar is not itself a media item.
_Avoid_: Companion file, Auxiliary file.

**XMP sidecar**:
The specific `.xmp` [[sidecar]] carrying the metadata Ferrocull reads and writes: rating, color label, and (in future) IPTC fields. The only sidecar type that is not opaque to the app.
_Avoid_: XMP file, Metadata file.

**Burst**:
A run of **3 or more** consecutive media files whose capture times are each within 1 second of the previous one; two shots within a second do not form a burst. A collapsed burst behaves as one virtual photo across the whole display sequence, [[preview]] included, shown as its [[representative]], and rating/labelling/tagging any member applies to all members. `B` or the burst badge collapses and expands it, from the grid and the preview alike, onto one shared expansion state, whose default is the durable "Expand" preference in the filter bar rather than always-collapsed.
_Avoid_: Sequence, Series.

**Representative**:
The first (earliest-captured) member of a [[burst]], which stands in for the whole burst while it is collapsed.
_Avoid_: Cover, Stack top.

**Preview**:
The full-size single-photo view opened from the grid, navigating the same display sequence the grid shows.
_Avoid_: Loupe, Viewer.

**Thumbnail size**:
The edge length of a grid cell, chosen by the user. Each step of the control changes the number of columns by one; the chosen size is kept across window sizes and maps to the nearest column count. It applies to the grid only. It is not the resolution of the cached thumbnail image, which is a storage preference.
_Avoid_: Grid zoom, Cell size, Zoom (zoom means the [[preview]] zoom).

**Source**:
A thing Ferrocull can scan and ingest from. One of three subtypes:

- **Storage source** — a block device (SD card, USB stick, internal disk) with a mount point.
- **Camera source** — a tethered camera accessed directly, with no mount point.
- **Directory source** — a folder the user explicitly added, not auto-detected.
  _Avoid_: Device, Origin, Input.

**Destination**:
A folder on local storage where ingested files are written. An [[ingest]] has one primary destination plus zero or more backup destinations, all written in the same operation.
_Avoid_: Target, Output folder.

**Rating**:
A signed integer in `[-1, 5]` attached to a media file: `-1` is **Rejected**, `0` is **Unrated** (the default), `1`..`5` is the star rating. Rejection and star rating are not orthogonal; a file is in exactly one of these states at a time.
_Avoid_: Rank, Score. Don't speak of "rejected" as a separate field — it's the `-1` rating.

### Compare mode

**Select** (noun, singular):
In compare mode, the photo currently being kept — the reigning champion of the comparison. Distinct from [[selection]] (the grid-mode set of tagged files); when in doubt, prefer "compare select" to disambiguate.
_Avoid_: Winner, Champion, Pick.

**Candidate**:
In compare mode, the challenger photo shown alongside the [[select]].
_Avoid_: Challenger, Other.

**Promote**:
The compare-mode action where the [[candidate]] beats the [[select]]: the candidate becomes the new select, and the next file in the list becomes the new candidate.
_Avoid_: Choose, Pick.

**Info strip**:
A retractable readout beneath a photo showing its capture settings (shutter, aperture, ISO, focal length, capture time), present in compare mode and the [[preview]]. Named "info" rather than "exposure" because capture time and focal length are not exposure data.
_Avoid_: Exposure strip, EXIF panel, Metadata bar.
