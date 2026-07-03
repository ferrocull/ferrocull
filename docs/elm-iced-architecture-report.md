# Elm Architecture and Iced Best Practices

A comprehensive guide for architectural decisions in iced (Rust GUI framework) applications.

## 1. The Elm Architecture (TEA) Fundamentals

### Model-Update-View Cycle

TEA structures applications around four interconnected concepts:

1. **Model (State)**: The underlying data of the application - a single source of truth
2. **Messages**: Events that have occurred, represented as pure data
3. **Update**: How messages change the state
4. **View**: How the state dictates the widgets displayed

The cycle flows as:
```
User interacts with UI -> Widget produces Message -> Update handles Message
-> State changes -> View renders new widgets -> Repeat
```

### State Structure Principles

**Single Source of Truth**: Store state in one place. Components don't "own" state - they operate on pieces of the main application state. You cannot get out of sync when there is only one state.

**Flat vs Nested State**:
- **Flat**: Easier to reason about, prevents sync issues, but update function can become unwieldy
- **Nested**: Better modularity, smaller update functions, but communication between nested components is harder

**Rule of thumb**: Keep state flat when possible. When nesting is required, don't go more than one level deep. A super common beginner mistake is to split out separate state ownership prematurely.

**Cross-cutting state**: When multiple message handlers write to the same state (e.g., a sorted view updated by filters, ratings, devices, bursts), flat state avoids synchronization issues that plague nested architectures.

**Placement guideline**: State should live in the lowest model that is still common to all views that need to access it. Pass it down as props from there.

### Why Flat State Works for Single-Screen Apps

A single-screen application with cross-cutting concerns (filters, ratings, devices, burst grouping all affecting a shared sorted view) should keep state flat. The arguments for nesting — "it's a lot of fields," "it would be more organized" — don't survive contact with reality:

1. **Cross-cutting state resists splitting.** If `rebuild_sorted_view()` needs filter state, rating state, device state, and grouping state, those fields must be accessible together. Splitting them into sub-structs means either passing all sub-structs as arguments (pointless indirection) or reaching into children from the parent (leaky abstraction).

2. **Grouping by view creates arbitrary boundaries.** Does `ascending` belong to `FilterState` or `GridState`? Does `selected` belong to `GridState` or `PreviewState`? These questions have no good answers because the fields are cross-cutting.

3. **"Organization" is not a type-level concern.** Adding `self.filters.hide_rejected` instead of `self.hide_rejected` encodes no invariant and prevents no bugs. It's namespacing for aesthetics.

**Real-world validation**: Jeroen Engels (Elm Radio) maintained a 6,000-line Elm module with flat state successfully. NoRedInk (largest production Elm codebase) uses flat state within each page. The Elm guide explicitly says: "Keep growing those modules longer and longer."

### Making Impossible States Impossible (Not the Same as Splitting State)

Richard Feldman's "Making Impossible States Impossible" (elm-conf 2016) is about **type design**, not state organization. When a group of fields only makes sense together and only during a specific mode, wrap them:

```rust
// Before: 6 fields that are meaningless when not in compare mode
struct App {
    compare_left: Option<usize>,
    compare_right: Option<usize>,
    compare_zoom: f32,
    compare_sync_pan: bool,
    // ... invalid combinations are representable
}

// After: impossible states are impossible
struct App {
    compare: Option<CompareState>,  // None = not comparing, period
    // ...
}
```

This is not "splitting the model." The state still lives on the top-level struct. You're reducing the cardinality of your type to match only valid states. Apply this when:
- A group of fields is only meaningful during a specific UI mode
- Having some but not all of them set is invalid
- `None` means "this entire mode is inactive"

Do **not** apply this for always-present fields that happen to relate to the same feature. Wrapping `filter_mode`, `sort_key`, `ascending` into a `FilterConfig` struct encodes no new invariant — those fields are always valid and always present.

### Narrowing Update Helpers

Richard Feldman's technique for managing large flat state: write helpers that take only the fields they need, not `&mut self`. In Rust, this means free functions or methods that borrow specific fields:

```rust
// Instead of a method that takes &mut self and touches 2 of 50 fields:
fn apply_sort(items: &mut [MediaItem], key: SortKey, ascending: bool) {
    // Only has access to what it needs
}

// Called from update:
apply_sort(&mut self.sorted_view, self.sort_key, self.ascending);
```

This gives isolation benefits (narrower bug surface, easier to test) without the cost of nesting (message-passing boilerplate, sync problems, artificial boundaries).

### When and How State Should Be Updated

1. **Updates must be pure**: The update function takes current state + message, returns new state. No side effects allowed in the update function itself.

2. **Side effects via Commands**: All I/O (HTTP, disk, random numbers) happens through Commands/Tasks returned from update. The runtime executes these and feeds results back as messages.

3. **Immutability**: Never mutate state directly. Return a new state value.

```rust
fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::IncrementPressed => {
            self.count += 1;
            Task::none()
        }
        Message::FetchData => {
            Task::perform(fetch_data(), Message::DataFetched)
        }
        Message::DataFetched(data) => {
            self.data = Some(data);
            Task::none()
        }
    }
}
```

### Message Design Patterns

**Messages should be pure data**: They represent "what happened" not "what to do." Design messages around events, not commands.

```rust
// Good: describes what happened
enum Message {
    ButtonPressed,
    DataLoaded(Vec<Item>),
    SearchQueryChanged(String),
}

// Bad: describes what to do
enum Message {
    LoadData,           // "Load" is an action
    UpdateSearchQuery,  // "Update" is an action
}
```

**Keep message data minimal**: Don't include entire new state in messages. Pass only the data needed for the update function to compute the new state.

**Flat vs Nested Messages**: The choice depends on application complexity.

- **Flat messages**: Simpler for small to medium apps. No mapping overhead, direct message handling. Appropriate when the `update` match statement is manageable (under ~30 variants).

- **Nested messages** (message-only decomposition): For complex apps, nest messages while keeping state flat. This provides organizational benefits without ownership complexity. The [iced todos example](https://github.com/iced-rs/iced/blob/master/examples/todos/src/main.rs) uses nested `TaskMessage` for per-item operations.

- **Full decomposition** (nested state + messages): Only when components are genuinely isolated with minimal cross-cutting concerns. Introduces ownership complexity and requires careful state synchronization.

Use `Element::map` and `Task::map` to compose nested messages back to the parent type. Use `Function::with()` (from `iced::Function`) for partial application of binary enum constructors in `.map()` calls (e.g., `.map(Event::Item.with(path))` instead of `.map(move |e| Event::Item(path.clone(), e))`).

---

## 2. Iced-Specific Patterns

### How Iced Implements TEA

Iced follows TEA closely but adapts it to Rust's ownership model:

```rust
struct App {
    count: i32,
    data: Option<Vec<Item>>,
}

#[derive(Debug, Clone)]
enum Message {
    Increment,
    Decrement,
    DataLoaded(Vec<Item>),
}

impl App {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Increment => self.count += 1,
            Message::Decrement => self.count -= 1,
            Message::DataLoaded(data) => self.data = Some(data),
        }
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        column![
            text(self.count),
            button("Increment").on_press(Message::Increment),
            button("Decrement").on_press(Message::Decrement),
        ]
        .into()
    }
}
```

### Task vs Subscription

| Aspect | Task | Subscription |
|--------|------|--------------|
| Purpose | One-time async operations | Continuous event streams |
| Lifecycle | Runs once, completes | Runs until stopped |
| Triggered by | User action / message | Application state |
| Examples | HTTP request, file load, computation | Timer, keyboard events, WebSocket |

**Task** (formerly Command):
```rust
// One-shot async operation
fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::FetchWeather => {
            Task::perform(fetch_weather(), Message::WeatherFetched)
        }
        Message::WeatherFetched(weather) => {
            self.weather = Some(weather);
            Task::none()
        }
    }
}
```

**Subscription**:
```rust
// Continuous event stream
fn subscription(&self) -> Subscription<Message> {
    if self.timer_enabled {
        time::every(Duration::from_secs(1)).map(Message::Tick)
    } else {
        Subscription::none()
    }
}
```

Key Task methods:
- `Task::none()` - No operation
- `Task::done(value)` - Instantly produce a value
- `Task::perform(future, mapper)` - Run async operation, map result to message
- `Task::batch(tasks)` - Run multiple tasks in parallel
- `Task::chain(task)` - Sequence tasks

Key Subscription methods:
- `Subscription::none()` - No subscription
- `Subscription::run(builder)` - Create from stream builder
- `Subscription::batch(subs)` - Combine multiple subscriptions

### Sipper Pattern

The `Sipper` trait (requires `sipper` feature) combines `Stream` and `Future` - it produces progress updates while working toward a final output:

```rust
// A Sipper produces progress AND a final result
Task::sip(
    download_file(url),
    |progress| Message::DownloadProgress(progress),  // Progress updates
    |result| Message::DownloadComplete(result),       // Final result
)
```

Use Sipper when you need:
- Progress reporting (file downloads, long computations)
- Intermediate updates before completion
- Both streaming progress and a final result

---

## 3. State Management Best Practices

### Derived Data: View vs Model

**Default approach: Compute in view()**

Keep the model simple and compute derived values in `view()`. The Elm/iced virtual DOM is optimized to handle this efficiently.

```rust
// Good: compute in view
fn view(&self) -> Element<Message> {
    let filtered_items: Vec<_> = self.items
        .iter()
        .filter(|i| i.matches(&self.filter))
        .collect();

    let count = filtered_items.len();

    column![
        text(format!("{} items", count)),
        // render filtered_items...
    ]
}
```

**Rationale**:
1. Single source of truth - no risk of derived state getting out of sync
2. Simpler model, fewer fields to maintain
3. Virtual DOM diffing is cheap; DOM operations are expensive
4. Most "expensive" computations are actually negligible compared to DOM work

### When Caching IS Appropriate

Cache computed values in the model only when you have **measured** a performance problem. Signs you might need caching:

1. Computation takes measurable time (profile first!)
2. The same expensive computation runs repeatedly with identical inputs
3. You've tried `lazy` widget and still have issues

```rust
// When caching is warranted
struct App {
    items: Vec<Item>,
    filter: Filter,
    // Cache only after profiling shows this is a bottleneck
    filtered_items_cache: Option<(Filter, Vec<Item>)>,
}

fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::FilterChanged(filter) => {
            self.filter = filter;
            // Invalidate cache
            self.filtered_items_cache = None;
        }
        // ...
    }
}

fn get_filtered_items(&mut self) -> &[Item] {
    if self.filtered_items_cache.as_ref().map(|(f, _)| f) != Some(&self.filter) {
        let filtered = self.items
            .iter()
            .filter(|i| i.matches(&self.filter))
            .cloned()
            .collect();
        self.filtered_items_cache = Some((self.filter.clone(), filtered));
    }
    &self.filtered_items_cache.as_ref().unwrap().1
}
```

### Incremental Updates vs Full Recomputation

**Prefer full recomputation unless proven necessary**:
- Simpler code, fewer bugs
- Easier to reason about
- Virtual DOM handles most "waste" efficiently

**Use incremental updates when**:
- Working with large datasets (10,000+ items)
- Real-time updates where latency matters
- Profiling shows recomputation is the bottleneck

```rust
// Incremental update example - only when necessary
Message::ItemAdded(item) => {
    self.items.push(item.clone());
    // Update cached stats incrementally
    if let Some(ref mut stats) = self.stats_cache {
        stats.total_count += 1;
        stats.total_value += item.value;
    }
}
```

---

## 4. Performance Patterns

### Avoiding Expensive View Computations

**The DOM is the bottleneck, not your view function**. Profiling Elm apps consistently shows:
- Better data structures? Negligible impact
- Caching computations in model? Negligible impact and worse code
- Only thing that matters: `Html.Lazy` / `lazy` widget to reduce DOM operations

**Pre-optimization checklist** (in order of effectiveness):
1. Use `lazy` widget at natural visual boundaries
2. Use `lazy` for long, stable lists
3. Cache images/heavy resources in state (decode once)
4. Only after profiling: cache expensive computations

### The `lazy` Widget

`lazy` only rebuilds its contents when the dependency changes:

```rust
use iced::widget::lazy;

fn view(&self) -> Element<Message> {
    column![
        // Header rarely changes
        lazy(&self.user, |user| {
            header_view(user)
        }),

        // Main content
        lazy(&self.items, |items| {
            items_list_view(items)
        }),

        // Footer is static
        lazy(&(), |_| footer_view()),
    ]
}
```

**When to use `lazy`**:
- Root level application - natural visual boundaries (header, sidebar, main content)
- Long lists where items rarely change
- Expensive-to-render components

**How it works**:
- Stores reference to function + dependency
- Compares dependency by hash
- Skips rebuilding if dependency unchanged

**Best practices**:
- Place at natural UI boundaries
- Higher placement = more computation avoided, but risk of over-caching
- Lower placement = tighter cache control, but less benefit
- The lazy widget has minimal overhead even if suboptimal

### Memoization in Elm/Iced

**Elm**: Only `Html.Lazy` provides memoization. No general-purpose function memoization.

**Iced**: The `lazy` widget provides similar functionality. Relies on dependency hashing.

**Key insight**: Memoization in both frameworks is specifically for view output, not arbitrary functions. This is intentional - the view-to-DOM step is where optimization matters most.

### What's Acceptable in View

**Always OK in view()**:
- Simple iteration and filtering
- String formatting
- Conditional rendering
- Mapping data to widgets

**Avoid in view()**:
- Network/disk I/O (use Commands/Tasks)
- Heavy computation on every render (use `lazy`)
- State mutation (views must be pure)
- Expensive allocations in hot paths

---

## 5. Anti-Patterns to Avoid

### State Structure Anti-Patterns

**Anti-pattern: Duplicated/derived state**
```rust
// Bad: storing what can be computed
struct App {
    items: Vec<Item>,
    item_count: usize,      // Derived from items.len()
    filtered_items: Vec<Item>, // Derived from items + filter
}
```

**Anti-pattern: Component-owned state when unnecessary**
```rust
// Bad: unnecessary nesting
struct App {
    header: HeaderComponent,  // Has its own state
    sidebar: SidebarComponent, // Has its own state
    main: MainComponent,      // Has its own state
}
// These components just need view functions, not state
```

**Anti-pattern: Remote data without loading state**
```rust
// Bad: impossible states are possible
struct App {
    loading: bool,
    data: Vec<Item>,  // Empty when loading, but also empty when "no results"
}

// Good: use enum to make impossible states impossible
enum DataState {
    Loading,
    Loaded(Vec<Item>),
    Error(String),
}
```

### View Function Anti-Patterns

**Anti-pattern: Side effects in view**
```rust
// Bad: view should never do this
fn view(&self) -> Element<Message> {
    println!("Rendering...");  // Side effect!
    log_to_file("view called"); // I/O in view!
    // ...
}
```

**Anti-pattern: Expensive inline computation without lazy**
```rust
// Bad: recomputes on every render
fn view(&self) -> Element<Message> {
    let sorted_and_grouped = self.compute_expensive_grouping(); // Runs every time
    // ...
}

// Good: use lazy when computation is expensive
fn view(&self) -> Element<Message> {
    lazy(&self.items, |items| {
        let sorted_and_grouped = compute_expensive_grouping(items);
        render_grouped(sorted_and_grouped)
    })
}
```

**Anti-pattern: Anonymous closures breaking lazy caching**
```rust
// Bad: anonymous closure defeats lazy caching
fn view(&self) -> Element<Message> {
    lazy(&self.items, |items| {
        let helper = |i: &Item| i.render(); // New function every time!
        // ...
    })
}
```

### Message Handling Anti-Patterns

**Anti-pattern: Messages that carry entire new state**
```rust
// Bad: bypasses update logic
enum Message {
    SetEntireModel(Model),  // Just replacing state
}
```

**Anti-pattern: Overly granular messages**
```rust
// Bad: message per field
enum Message {
    SetUserName(String),
    SetUserEmail(String),
    SetUserAge(u32),
    // ... 50 more SetUserX variants
}

// Better: one message for user form changes
enum Message {
    UserFormUpdated(UserFormField, String),
}
```

**Anti-pattern: Blocking in update**
```rust
// Bad: blocks UI
fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::LoadData => {
            self.data = blocking_fetch();  // Freezes everything!
            Task::none()
        }
    }
}

// Good: async via Task
fn update(&mut self, message: Message) -> Task<Message> {
    match message {
        Message::LoadData => {
            Task::perform(async_fetch(), Message::DataLoaded)
        }
    }
}
```

### Module Organization Anti-Patterns

**Anti-pattern: Splitting one abstraction across layer files**

The Elm guide and iced maintainers warn against splitting a single application into `model.rs` / `update.rs` / `view.rs` / `messages.rs`. These four elements are tightly coupled parts of one abstraction — splitting them creates boundaries that don't encapsulate anything. You can't work on any feature without opening all four files.

```rust
// Bad: one abstraction split across four files by layer
mod model;    // THE model struct
mod update;   // THE update function
mod view;     // THE view function
mod messages; // THE message enum
```

hecrj (iced maintainer) in [Discussion #1572](https://github.com/iced-rs/iced/discussions/1572): "every module in Rust is like a small library, and it makes no sense to have a module that only exposes an incomplete abstraction."

**NOT the same: Feature-aligned sub-modules within layer directories**

Having `messages/grid.rs` alongside `views/grid.rs` is a different structure. Each file is a complete sub-abstraction: `GridMessage` is a self-contained enum, `view_grid()` is a self-contained function. This is feature-aligned organization that happens to use layer directories.

```rust
// Fine: each file is a complete sub-abstraction
mod messages {
    mod grid;       // GridMessage enum — complete vocabulary for grid interactions
    mod compare;    // CompareMessage enum — complete vocabulary for compare mode
}
mod views {
    mod thumbnails; // grid view function — complete rendering of the grid
    mod compare;    // compare view function — complete rendering of compare mode
}
mod app;            // Flat state + update routing + root view
```

This structure emerges naturally in flat-state apps where view functions need `&App`. You can't put `view_grid()` in a `grid.rs` feature module without creating a circular dependency (app.rs uses grid.rs, grid.rs uses App). The layer directories are a practical solution to Rust's module visibility rules.

**The real test**: "When working on a task, how many different modules must you jump between?" If the answer is 2-3 (message enum + view function + update arm), that's fine. If it's 6+, the organization is wrong.

**Ideal for multi-page apps: Organize by feature/type**

When pages have isolated state (each page owns its model), pure feature modules work cleanly:

```rust
mod app;              // Root application, page routing
mod pages {
    mod home;         // HomePage state + update + view
    mod settings;     // SettingsPage state + update + view
}
mod components {
    mod user_card;    // Stateless view helper
}
```

Richard Feldman originally had `Views/` and `Data/` top-level folders in elm-spa-example but [later removed them](https://dev.to/rtfeldman/tour-of-an-open-source-elm-spa), saying they "inadvertently encouraged sloppy module boundaries." His final structure organized around types and pages. However, his app used per-page state isolation — each `Page.X` owned its model — which made pure feature modules possible.

---

## 6. Real-World Examples

### Official Iced Examples

**Counter** - Basic TEA:
```rust
struct Counter {
    value: i32,
}

enum Message {
    Increment,
    Decrement,
}

impl Counter {
    fn update(&mut self, message: Message) {
        match message {
            Message::Increment => self.value += 1,
            Message::Decrement => self.value -= 1,
        }
    }

    fn view(&self) -> Element<Message> {
        column![
            button("+").on_press(Message::Increment),
            text(self.value),
            button("-").on_press(Message::Decrement),
        ]
        .into()
    }
}
```

**Todos** - Async, dynamic lists, persistence, [source](https://github.com/iced-rs/iced/blob/master/examples/todos/src/main.rs):
- Dynamic layout with scrollable
- Text input handling
- Checkbox state
- Background auto-save via Subscription
- Filter state (all/active/completed)
- **Nested `TaskMessage`** for per-item operations (demonstrates message-only decomposition)

**Download Progress** - Tasks and progress reporting:
- Custom Subscription for download progress
- Progress bar updates
- Async file operations

### Multi-Page Application Pattern

```rust
#[derive(Debug)]
enum App {
    Loading,
    Home(HomePage),
    Settings(SettingsPage),
}

#[derive(Debug, Clone)]
enum Message {
    HomeMessage(home::Message),
    SettingsMessage(settings::Message),
    NavigateTo(Page),
}

impl App {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NavigateTo(Page::Home) => {
                *self = App::Home(HomePage::new());
                Task::none()
            }
            Message::NavigateTo(Page::Settings) => {
                *self = App::Settings(SettingsPage::new());
                Task::none()
            }
            Message::HomeMessage(msg) => {
                if let App::Home(page) = self {
                    page.update(msg).map(Message::HomeMessage)
                } else {
                    Task::none()
                }
            }
            Message::SettingsMessage(msg) => {
                if let App::Settings(page) = self {
                    page.update(msg).map(Message::SettingsMessage)
                } else {
                    Task::none()
                }
            }
        }
    }

    fn view(&self) -> Element<Message> {
        match self {
            App::Loading => text("Loading...").into(),
            App::Home(page) => page.view().map(Message::HomeMessage),
            App::Settings(page) => page.view().map(Message::SettingsMessage),
        }
    }
}
```

### Community Best Practices Summary

1. **Start flat, nest strategically**: Begin with flat state and messages. When the update function becomes unwieldy (30+ variants), consider message-only decomposition (nested messages, flat state). Only nest state when components are genuinely isolated.

2. **Avoid the "component mindset"**: Richard Feldman (Elm core contributor): "The 'component' mindset leads to worse Elm code. It's overcomplicated, bloated with unnecessary wiring, and more time-consuming to work with."

3. **Use `lazy` strategically**: Place at visual boundaries (header, sidebar, content areas) and for long stable lists.

4. **Profile before optimizing**: The only performance optimization that consistently matters is reducing DOM operations via `lazy`. Everything else is usually negligible.

5. **Keep modules cohesive**: Don't split one abstraction across `model.rs` / `update.rs` / `view.rs` / `messages.rs`. Feature-aligned sub-modules within layer directories (e.g., `messages/grid.rs` + `views/grid.rs`) are fine when flat state forces view functions to depend on the app struct.

6. **Design messages around events**: Messages describe "what happened," not "what to do."

7. **Let the runtime handle side effects**: Never perform I/O in update or view. Return Tasks/Commands and let the runtime execute them.

---

## Sources

### Elm Documentation
- [The Elm Architecture](https://guide.elm-lang.org/architecture/)
- [Html.lazy Optimization](https://guide.elm-lang.org/optimization/lazy.html)
- [Elm Patterns: Nested TEA](https://sporto.github.io/elm-patterns/architecture/nested-tea.html)
- [Elm Patterns: Effects](https://sporto.github.io/elm-patterns/architecture/effects.html)

### Iced Documentation
- [Iced Architecture](https://book.iced.rs/architecture.html)
- [Iced First Steps](https://book.iced.rs/first-steps.html)
- [Iced Task Documentation](https://docs.rs/iced/latest/iced/struct.Task.html)
- [Iced Subscription Documentation](https://docs.rs/iced/latest/iced/struct.Subscription.html)
- [Iced Lazy Widget](https://docs.rs/iced/latest/iced/widget/struct.Lazy.html)
- [Iced Examples](https://github.com/iced-rs/iced/blob/master/examples/README.md)
- [Iced Todos Example](https://github.com/iced-rs/iced/blob/master/examples/todos/src/main.rs) - nested `TaskMessage` pattern
- [Module Organization Discussion](https://github.com/iced-rs/iced/discussions/1572) - maintainer guidance on module organization

### Community Resources
- [Elm Radio: Performance Episode](https://elm-radio.com/episode/performance/)
- [Elm Radio: Life of a File](https://elm-radio.com/episode/life-of-a-file/)
- [Elm Radio: Scaling Elm Apps](https://elm-radio.com/episode/scaling-elm-apps/)
- [Elm Radio: Make Impossible States Impossible](https://elm-radio.com/episode/impossible-states/)
- [Html.Lazy Performance Analysis](https://juliu.is/performant-elm-html-lazy/)
- [Caching Behind Elm Lazy](https://jfmengels.net/caching-behind-elm-lazy/)
- [Building Large Elm Applications](https://www.huy.rocks/everyday/03-14-2022-elm-building-large-elm-applications)
- [Multi-page Iced Tutorial](https://github.com/max-ishere/howto-iced-multipage)
- [State-Driven Subscriptions in Iced](https://d34dl0ck.me/rust-bites-iced-subscriptions/index.html)
- [How Elm Slays a UI Antipattern](https://blog.jenkster.com/2016/06/how-elm-slays-a-ui-antipattern.html)
- [Richard Feldman: Tour of an Open-Source Elm SPA](https://dev.to/rtfeldman/tour-of-an-open-source-elm-spa) - evolved away from layer directories
- [Richard Feldman: Making Impossible States Impossible](https://incrementalelm.com/make-impossible-states-impossible/)
- [Evan Czaplicki: Architecture Guidelines](https://gist.github.com/evancz/2b2ba366cae1887fe621) - single source of truth
- [Elm Shared State Pattern](https://github.com/ohanhi/elm-shared-state) - cross-cutting state in nested architectures
- [Halloy IRC Client](https://github.com/squidowl/halloy) - largest real-world iced application
- [Iced Discussion #1364](https://github.com/iced-rs/iced/discussions/1364) - module design with events
- [Learning Elm by Porting from React](https://benhoyt.com/writings/learning-elm/) - practical flat vs nested decisions
