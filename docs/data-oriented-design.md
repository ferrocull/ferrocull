# Data-Oriented Design Principles for Ferrocull

Based on [Data-Oriented Design](https://www.dataorienteddesign.com/dodbook/) by Richard Fabian. ADR-0003 commits the core data model to these principles.

**Scope.** These principles pay off in hot paths iterating thousands of media items; in cold paths and small collections, prefer the clearest structure — "design for the measured case" cuts both ways. The UI follows The Elm Architecture (ADR-0002, [elm-iced-architecture-report.md](elm-iced-architecture-report.md)); TEA's `update` mutating app state is not a DOD violation.

## Core Philosophy

**Data is not the problem domain.** OOP models the problem domain in code structure; DOD treats data as raw facts to be transformed. A capture time is just an `i64` — sorting, grouping, and filtering give it meaning, not the struct it lives in. Bundling every field into one `MediaItem` class locks in a single interpretation and hides the real statistics (most items share a capture date, thumbnails are sparse during loading, paths are rarely needed for display).

**Design around measured data patterns**, not hypothetical extensibility: access frequency, read/write ratio, item count at runtime, and how sparsely optional fields are populated.

## Principles

### 1. Separate hot and cold data

Hot data is touched every frame (thumbnail handles, capture times, ratings); cold data only on updates or specific operations (file paths, pairing info). Keep them in separate parallel arrays so display iteration never drags cold bytes through the cache:

```rust
struct HotData {
    thumbnails: Vec<Option<ThumbnailHandle>>,
    capture_times: Vec<DateTime<Utc>>,
    ratings: Vec<i8>,
}

struct ColdData {
    paths: Vec<PathBuf>,
}
```

### 2. Indices, not pointers

`struct MediaId(u32)` — 4 bytes, trivially copyable, no lifetime complexity, works across all parallel arrays, keeps storage contiguous.

### 3. State as table membership

Encode state through presence in a collection, not boolean flags:

```rust
// Bad: is_tagged / is_rejected flags on every item
// Good: membership IS state
tagged: HashSet<MediaId>,
rejected: HashSet<MediaId>,
```

Iterating "all tagged items" is O(tagged) instead of O(all), with no per-item branch and no None checks.

### 4. Process in homogeneous batches

Batch by operation (`update_thumbnails(...)`, `update_ratings(...)`) instead of dispatching per item — no vtable lookups, predictable branches, sequential memory access.

### 5. Transforms over mutations

View operations as `data in → transform → data out`. Pure functions over slices beat methods that mutate an item and touch caches as a side effect.

### 6. Existence-based processing

Structure data so iteration *is* the condition: keep a collection of items needing work and iterate it, instead of scanning everything and testing a flag per item.

## Cache Efficiency

A cache miss costs ~100 cycles. Favor contiguous memory, minimal wasted bytes per cache line, and predictable linear iteration.

**SoA vs AoS**: use array-of-structs when fields are always accessed together (`positions: Vec<(f32, f32, f32)>`); use struct-of-arrays when access patterns differ per field — sorting touches only the times array, display only ratings, file ops only paths.

## Anti-Patterns

| Anti-pattern | Why it's bad | DOD alternative |
|--------------|--------------|-----------------|
| `Arc<MediaItem>` | Pointer chasing, scattered memory | Indices into contiguous arrays |
| `HashMap<Path, MediaItem>` in hot path | ~100 cycles per lookup | Vec + separate HashMap index |
| `Option<T>` for sparse data | Branches + wasted cache space | Separate sparse table |
| Boolean flags | Branches in tight loops | Set membership |
| Virtual dispatch | Unpredictable branches, cache misses | Process by type in batches |

## Summary

| OOP assumption | DOD reality |
|----------------|-------------|
| Model the domain | Transform the data |
| Objects have identity | Data has shape and statistics |
| Encapsulate state | Separate by access pattern |
| Polymorphism via vtables | Process homogeneous batches |
| Optional fields are free | Every Option is a branch and cache waste |
| Design for extensibility | Design for the measured case |
