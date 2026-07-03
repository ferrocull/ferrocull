# XMP sidecars are written at ingest time, not on every edit

When a file is ingested with a rating or color label, Ferrocull writes the XMP sidecar once, atomically, as part of the copy. Subsequent rating changes during the same session update in-memory state and the SQLite history DB; they do **not** re-write the XMP. Re-ingest or an explicit "write XMP" action is what flushes session changes to sidecars.

Considered alternative: continuous XMP writes on every rating/label change, the Lightroom model. Rejected because it (a) creates write contention with darktable/Lightroom if a user is running them concurrently, (b) multiplies disk writes during fast culling (one keystroke = one fsync), and (c) makes the "ratings travel with files on copy" guarantee harder to reason about.

The consequence is that ratings made *after* ingest aren't visible to other apps until explicitly re-written. That's acceptable for the culling workflow — the use case is "cull, then ingest the keepers" — but a future "sync XMP" action will be needed if users want to edit ratings post-ingest.
