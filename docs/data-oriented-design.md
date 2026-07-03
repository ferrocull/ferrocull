# Data-Oriented Design Principles for Ferrocull

Based on [Data-Oriented Design](https://www.dataorienteddesign.com/dodbook/) by Richard Fabian.

## Core Philosophy

### Data is Not the Problem Domain

OOP models the *problem domain* in code structure. DOD treats data as raw facts to be transformed.

**OOP trap**: Creating a `MediaItem` class with all fields locks you into one interpretation. You can't easily see that most items share the same capture date, that thumbnails are sparse during loading, or that paths are rarely needed for display.

**DOD approach**: Data exists independently of its interpretation. A capture time is just an `i64`—it could be for sorting, grouping, or filtering. The transformation code gives meaning, not the data structure.

### Data Statistics Matter

Design around *actual data patterns*: frequency, quantity, access patterns.

- How often is this field accessed?
- What's the read/write ratio?
- How many items exist at runtime—100? 10,000?
- What percentage of items have this optional field populated?

## Key Principles for Ferrocull

### 1. Separate Hot and Cold Data

**Hot data**: Accessed every frame during display
- Thumbnail handles
- Capture times (for sorting/grouping)
- Ratings (for display)

**Cold data**: Accessed only on updates or specific operations
- File paths (only for ThumbnailLoaded updates)
- Flags (only when filtering)
- JPEG pair info (only during download)

```rust
// DOD: Separate by access pattern
struct HotData {
    thumbnails: Vec<Option<ThumbnailHandle>>,
    capture_times: Vec<DateTime<Utc>>,
    ratings: Vec<u8>,
}

struct ColdData {
    paths: Vec<PathBuf>,
    jpeg_pairs: Vec<Option<PathBuf>>,
}
```

### 2. Use Indices, Not Pointers

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct MediaId(u32);  // 4 bytes, trivially copyable
```

Benefits:
- 4 bytes vs 8 bytes for pointers
- No lifetime complexity
- Arrays stay contiguous
- Indices work across all parallel arrays

### 3. State as Table Membership

Instead of boolean flags, encode state through presence in a collection:

```rust
// BAD: Boolean flags
struct MediaItem {
    is_selected: bool,
    is_downloaded: bool,
    is_rejected: bool,
}

// GOOD: Membership IS state
selected_items: HashSet<MediaId>,
downloaded_items: HashSet<MediaId>,
rejected_items: HashSet<MediaId>,
```

Benefits:
- Iteration over "all selected items" is O(selected), not O(all)
- No branch misprediction from `if item.is_selected`
- No null/None checks—presence implies state

### 4. Process by Type, Not by Instance

```rust
// BAD: Virtual dispatch per item
for item in items {
    item.process();  // vtable lookup, cache miss
}

// GOOD: Batch by operation
update_thumbnails(&mut thumbnails, &loaded_data);
update_ratings(&mut ratings, &user_input);
```

### 5. Transforms Over Mutations

View operations as data transforms: `data in -> transform -> data out`

```rust
// BAD: Mutate in place with side effects
fn process_item(&mut self, item: &mut MediaItem) {
    if item.needs_thumbnail {
        item.thumbnail = self.load_thumbnail(&item.path);
        self.cache.insert(&item.path, &item.thumbnail);
    }
}

// GOOD: Pure transform
fn load_thumbnails(paths: &[PathBuf]) -> Vec<ThumbnailResult> {
    paths.iter().map(|p| load_thumbnail(p)).collect()
}
```

### 6. Existence-Based Processing

Instead of checking conditions, structure data so iteration IS the condition:

```rust
// BAD: Check every item
for item in &items {
    if item.needs_thumbnail.is_some() {
        process(item);
    }
}

// GOOD: Only iterate items that need processing
for id in &items_needing_thumbnails {
    process(&items[id]);
}
```

## Cache Efficiency

A cache miss costs ~100 CPU cycles. Design for:

1. **Contiguous memory**: Sequential access enables prefetching
2. **Minimal wasted bytes**: Don't load unused fields into cache lines
3. **Predictable patterns**: Linear iteration beats random access

### Struct of Arrays (SoA) vs Array of Structs (AoS)

**Use AoS when fields are always accessed together:**
```rust
// Good AoS: x,y,z always used as unit
positions: Vec<(f32, f32, f32)>
```

**Use SoA when access patterns vary:**
```rust
// Good SoA: Different operations need different fields
struct MediaTable {
    ids: Vec<MediaId>,        // Lookup operations
    times: Vec<DateTime>,     // Sorting/filtering
    ratings: Vec<u8>,         // Display
    paths: Vec<PathBuf>,      // File operations (rare)
}

// Sorting: touches only times array
// Display: touches only ratings array
// File ops: touches only paths array
```

## Applying to Ferrocull Architecture

### Recommended Structure

```rust
pub struct MediaLibrary {
    // === HOT DATA (display iteration) ===
    thumbnails: Vec<Option<ThumbnailHandle>>,
    capture_times: Vec<DateTime<Utc>>,
    ratings: Vec<u8>,

    // === COLD DATA (updates, detail views) ===
    paths: Vec<PathBuf>,
    jpeg_pairs: Vec<Option<PathBuf>>,

    // === INDICES ===
    path_to_id: HashMap<PathBuf, MediaId>,

    // === STATE AS MEMBERSHIP ===
    selected: HashSet<MediaId>,
    downloaded: HashSet<MediaId>,
    rejected: HashSet<MediaId>,

    // === SORTED VIEWS ===
    sorted_view: BTreeMap<SortKey, MediaId>,
}
```

### Display Loop (Hot Path)

```rust
for (_, &id) in &self.sorted_view {
    let thumb = &self.thumbnails[id.0 as usize];
    let rating = self.ratings[id.0 as usize];
    let is_selected = self.selected.contains(&id);
    render_thumbnail(thumb, rating, is_selected);
}
```

### Update Path (Cold Path)

```rust
fn handle_thumbnail_loaded(&mut self, path: &PathBuf, thumb: ThumbnailHandle) {
    if let Some(&id) = self.path_to_id.get(path) {
        self.thumbnails[id.0 as usize] = Some(thumb);
    }
}
```

## Anti-Patterns to Avoid

| Anti-Pattern | Why It's Bad | DOD Alternative |
|--------------|--------------|-----------------|
| `Arc<MediaItem>` | Pointer chasing, scattered memory | Indices into contiguous arrays |
| `HashMap<Path, MediaItem>` | ~100 cycles per lookup in hot path | Vec + separate HashMap index |
| `Option<T>` for sparse data | Branches + wasted cache space | Separate sparse table |
| Boolean flags | Branches in tight loops | Set membership |
| Virtual dispatch | Unpredictable branches, cache misses | Process by type in batches |

## Summary Table

| OOP Assumption | DOD Reality |
|----------------|-------------|
| Model the domain | Transform the data |
| Objects have identity | Data has shape and statistics |
| Encapsulate state | Separate by access pattern |
| Polymorphism via vtables | Process homogeneous batches |
| Optional fields are free | Every Option is a branch and cache waste |
| Design for extensibility | Design for the measured case |
