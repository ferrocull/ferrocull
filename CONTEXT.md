# Ferrocull

A FOSS culling tool for photographers — gets you through thousands of images as fast as possible, in the Photo Mechanic tradition. Not a DAM, not an editor. The vocabulary in this glossary tracks Photo Mechanic conventions wherever a term is shared.

## Language

**Ingest**:
The operation of copying media files from a source (memory card, camera, directory) to one or more destinations, with renaming, verification, and post-copy hooks. The central workflow alongside culling.
_Avoid_: Download, Import, Copy.

**Color label**:
A single named color (Red, Yellow, Green, Blue, Purple, Orange, Gray) attached to a media file. Stored in XMP as `xmp:Label`. A file either has one label or none — "no label" is the absence of a value, not an 8th label.
_Avoid_: Color class, Color tag.

**Focus**:
The single media file currently under the keyboard cursor. At most one item is focused at a time. Set by clicking a thumbnail or moving with arrow keys; shown with a blue border. Rating, color-label, tag, and reject keystrokes act on the focused item.
_Avoid_: Active item, Current item.

**Tag** (verb) / **Tagged** (state):
The Photo Mechanic action of marking a file as part of the working set. Toggled by the `T` key (or `+`/`-` to add/remove). A file is either tagged or not.
_Avoid_: Pick, Star (star means rating), Mark.

**Selection**:
The set of currently-tagged files across the loaded view. Batch operations (ingest selected, rate selected, etc.) act on this set. Distinct from [[focus]] — focus is a single cursor position, selection is a set.
_Avoid_: Pickset, Selected files.

**Paired file**:
A sibling media file with the same basename as another (the canonical case is RAW+JPEG shot by the same camera press). Both files are first-class media; the pair is a UI grouping concern, not a metadata one.
_Avoid_: Sibling, RAW+JPEG (when speaking generically).

**Sidecar**:
A non-media auxiliary file sharing a basename with a media file. Recognised extensions: `.xmp`, `.thm`, `.wav`, `.mp3`. Distinct from a [[paired-file]] — a sidecar is not itself a media item.
_Avoid_: Companion file, Auxiliary file.

**XMP sidecar**:
The specific `.xmp` [[sidecar]] that carries XMP metadata Ferrocull cares about — rating, color label, and (in future) IPTC fields. The only sidecar Ferrocull reads from and writes to. Tracked separately on `MediaFile` because the other sidecar types are opaque to the app.
_Avoid_: XMP file, Metadata file.

**Burst**:
A run of **3 or more** consecutive media files whose capture times are each within 1 second of the previous one. Two shots within a second do not form a burst — the minimum is three. Bursts are detected from EXIF `DateTimeOriginal` + `SubSecTimeOriginal` and can be visually collapsed in the grid; rating/labelling/tagging a burst member applies to all members. Collapse/expand is toggled by clicking the burst count badge or pressing `B` on the focused item (a Ferrocull binding — Photo Mechanic has no equivalent).
_Avoid_: Sequence, Series.

**Source**:
A thing Ferrocull can scan and ingest from. One of three subtypes:
- **Storage source** — a block device (SD card, USB stick, internal disk) with a mount point.
- **Camera source** — a gphoto2-controlled camera, accessed over PTP, no mount point.
- **Directory source** — a folder the user explicitly added, not auto-detected.
_Avoid_: Device, Origin, Input.

**Destination**:
A folder on local storage where ingested files are written. An [[ingest]] has one primary destination plus zero or more backup destinations, all written in the same operation.
_Avoid_: Target, Output folder.

**Rating**:
A signed integer in `[-1, 5]` attached to a media file, matching the XMP `xmp:Rating` wire format:
- `-1` — **Rejected** (the file is thrown out; `X` key)
- `0` — **Unrated** (the default; no `xmp:Rating` written)
- `1`..`5` — star rating (`1` through `5`)
Rejection and star rating are not orthogonal — a file is in exactly one of these states at a time.
Binding unmodified digits to star ratings is a deliberate divergence from Photo Mechanic, where digits set the color class.
_Avoid_: Rank, Score. Don't speak of "rejected" as a separate field — it's the `-1` rating.

### Compare mode

**Select** (noun, singular):
In compare mode, the photo currently being kept — the reigning champion of the comparison. Distinct from [[selection]] (the grid-mode set of tagged files). When in doubt, prefer "compare select" to disambiguate.
_Avoid_: Winner, Champion, Pick.

**Candidate**:
In compare mode, the challenger photo being shown alongside the [[select]]. Pressing `G` promotes the candidate.
_Avoid_: Challenger, Other.

**Promote**:
The compare-mode action (`G` key) where the [[candidate]] beats the [[select]]: the candidate becomes the new select, and the next file in the list becomes the new candidate.
_Avoid_: Choose, Pick.

## Flagged ambiguities

**"Select"**: in compare mode, a singular noun (the chosen photo). In grid context, "selection" is the *set* of tagged files. Same root, two different cardinalities — be explicit when speaking outside the immediate context.

## Example dialogue

> **Dev:** I'm adding a filter so the grid can hide everything that's been [[rating|rated]] below a threshold.
>
> **Photographer:** Make sure rejected files still show separately — I don't want them mixed with unrated.
>
> **Dev:** Right, rejected is `xmp:Rating = -1` — it's its own bucket, not "below 1 star".
>
> **Photographer:** Good. And when I'm in compare mode and I [[promote]] a [[candidate]], the new [[select]] should keep its [[color-label|color label]], not inherit from the loser.
>
> **Dev:** Color labels are per-file, so yes — the file's label stays with the file.
>
> **Photographer:** One more — if I [[tag]] a photo that's in a [[burst]], do the others in the burst get tagged too?
>
> **Dev:** Yes. Tagging, rating, and labelling all apply to every burst member, same as RAW+JPEG [[paired-file|pairs]].
