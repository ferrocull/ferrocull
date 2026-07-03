# Data-oriented design

Ferrocull structures its core data model around data-oriented design (DOD): separate arrays of plain values keyed by index, hot/cold field splitting, and transformations as free functions over slices — not OOP-style `MediaItem` classes with methods. The `MediaFile` struct is a transitional shape; the broader engine treats media as columnar tables, e.g. `MediaTable` / `MediaLibrary`.

The trade-off: DOD reads less naturally to programmers used to OOP, and refactoring around access patterns (rather than entities) takes more thought. The benefit is the speed target — culling thousands of RAWs in seconds — which requires cache-friendly memory layout and trivially-parallelisable transforms.

See `docs/data-oriented-design.md` for the principles and how they apply to Ferrocull specifically.
