# Elm Architecture and Iced Best Practices

Architectural guidance for Ferrocull's iced UI. ADR-0002 commits the UI to The Elm Architecture (TEA): a single state model, messages as pure data describing what happened, a pure `update` that returns side effects as `Task`s, and a `view` derived from state.

**Iced version.** This doc describes iced 0.14 (trust `Cargo.toml` over this line if they diverge). Iced's API broke sharply across versions: `Command` became `Task` in 0.13, and the `Sandbox`/`Application` traits were replaced by the functional `iced::application(...)` builder. Most tutorial code in circulation — including LLM training data — predates this; when in doubt, check docs.rs for the pinned version rather than remembered patterns.

**Scope.** TEA governs the UI crate. The core data model follows data-oriented design (ADR-0003, [data-oriented-design.md](data-oriented-design.md)) — the two compose rather than conflict: `update` mutating app state is TEA-correct, not a DOD violation.

## 1. State Structure

### Flat state is the decision for this app

Ferrocull is a single-screen app with cross-cutting concerns — filters, ratings, sources, burst grouping all feed one shared sorted view. Keep state flat on the top-level struct. The arguments for nesting don't survive contact with reality:

1. **Cross-cutting state resists splitting.** If `rebuild_sorted_view()` needs filter, rating, source, and grouping state, those fields must be accessible together. Sub-structs mean either passing them all as arguments (pointless indirection) or reaching into children from the parent (leaky abstraction).
2. **Grouping by view creates arbitrary boundaries.** Does `ascending` belong to `FilterState` or `GridState`? The question has no good answer because the fields are cross-cutting.
3. **"Organization" is not a type-level concern.** `self.filters.hide_rejected` over `self.hide_rejected` encodes no invariant and prevents no bugs — it's namespacing for aesthetics.

Real-world validation: the official [Elm guide](https://guide.elm-lang.org/webapps/structure) advises "keep growing those modules longer and longer" rather than splitting preemptively, and NoRedInk (one of the largest production Elm codebases, ~212k lines) nests a child Model/Msg only for genuinely stateful components, keeping stateless ones as plain view functions.

### Making impossible states impossible ≠ splitting state

Richard Feldman's "Making Impossible States Impossible" is about **type design**, not state organization. When a group of fields is only meaningful during a specific UI mode and having some-but-not-all set is invalid, wrap them:

```rust
// Before: fields that are meaningless outside compare mode,
// invalid combinations representable
compare_left: Option<usize>,
compare_right: Option<usize>,
compare_zoom: f32,

// After: None = not comparing, period
compare: Option<CompareState>,
```

The state still lives on the top-level struct; you're reducing the type's cardinality to valid states only. Do **not** apply this to always-present fields that merely relate to the same feature — wrapping `filter_mode`, `sort_key`, `ascending` into a `FilterConfig` encodes no new invariant.

### Narrowing update helpers

Manage large flat state with helpers that borrow only the fields they need, not `&mut self`:

```rust
fn apply_sort(items: &mut [MediaId], key: SortKey, ascending: bool) { ... }
// from update:
apply_sort(&mut self.sorted_view, self.sort_key, self.ascending);
```

This gives isolation (narrow bug surface, testable) without nesting's costs (wiring boilerplate, sync problems, artificial boundaries).

### Message design

- Messages describe **what happened** (`ButtonPressed`, `DataLoaded(items)`), not what to do (`LoadData`).
- Keep message payloads minimal — the data `update` needs, never a whole new state.
- Flat messages are fine up to roughly 30 variants. Beyond that, use **message-only decomposition**: nested message enums (`GridMessage`, `CompareMessage`) routed from the top, with state still flat. Full decomposition (nested state + messages) only for genuinely isolated components.
- Compose nested messages with `Element::map` / `Task::map`. `iced::Function::with()` partially applies binary enum constructors: `.map(Event::Item.with(path))` instead of `.map(move |e| Event::Item(path.clone(), e))`.

## 2. Side Effects: Task, Subscription, Sipper

| | `Task` | `Subscription` |
|---|---|---|
| Purpose | One-shot async operation | Continuous event stream |
| Lifecycle | Runs once, completes | Runs while `subscription()` returns it |
| Examples | File load, computation | Timer, keyboard events, device watch |

Key `Task` constructors: `none()`, `done(value)`, `perform(future, mapper)`, `batch(tasks)`, `.chain(task)`. Subscriptions are recreated from state each cycle — return `Subscription::none()` to stop one.

**Sipper** (`sipper` feature): a stream + future hybrid for operations that report progress *and* produce a final result — `Task::sip(work, Message::Progress, Message::Done)`. Use for ingest-style long operations with progress bars.

Never block in `update` and never do I/O in `update` or `view` — return a `Task` and let the runtime feed the result back as a message.

## 3. Derived Data and Performance

**Default: compute derived values in `view()`.** One source of truth, no cache-invalidation bugs, and the widget-diffing step is cheap. Profiling Elm apps consistently shows better data structures and model-side caches have negligible impact — the only optimization that reliably matters is reducing rebuild work via `lazy`.

**`lazy` widget** (`lazy` feature): rebuilds its contents only when the (hashed) dependency changes. Place it at natural visual boundaries (header, sidebar, main content) and around long, stable lists. Higher placement avoids more work but risks over-caching; the widget's own overhead is minimal either way. Don't define closures inside a `lazy` body that defeat its caching.

**Cache in the model only after profiling** shows a specific computation is a bottleneck and `lazy` didn't solve it. Same for incremental updates vs full recomputation: prefer recomputing (simpler, fewer bugs) unless datasets are large (10k+ items) and profiling says otherwise.

Fine in `view()`: iteration, filtering, formatting, conditional rendering. Never in `view()`: I/O, state mutation, heavy computation outside `lazy`.

## 4. Anti-Patterns

- **Duplicated/derived state in the model** (`item_count` next to `items`) — compute it in view.
- **Component-owned state for things that only need view functions** — Feldman's "Scaling Elm Apps" point: wiring a mini-TEA (own model/message/update) through every self-contained piece is a JS-component habit that produces overcomplicated Elm code bloated with unnecessary wiring.
- **Remote data as `loading: bool` + empty `Vec`** — use an enum (`Loading` / `Loaded(items)` / `Error(e)`) so impossible states are unrepresentable.
- **Messages that carry an entire new model**, or one message variant per form field — group related updates (`UserFormUpdated(field, value)`).

### Module organization

**Don't split one abstraction across layer files.** A single app split into `model.rs` / `update.rs` / `view.rs` / `messages.rs` creates boundaries that encapsulate nothing — every task opens all four files. hecrj (iced maintainer, [Discussion #1572](https://github.com/iced-rs/iced/discussions/1572)): "Every module in Rust is like a small library. It makes no sense to have a module that only exposes an incomplete abstraction!"

**Feature-aligned sub-modules are a different thing, and fine** — a message enum covering one feature, a view function rendering one region, an update helper for one mode are each complete sub-abstractions, wherever they live. In a flat-state app, view and update functions need the app struct, so pure per-feature modules can create circular dependencies with the root; grouping complete sub-abstractions under layer or root directories is a legitimate answer to Rust's visibility rules, not layer-splitting. Follow the layout the codebase already uses rather than any tree sketched in a doc.

**The real test:** how many modules must you jump between for one task? 2–3 (message enum + view function + update arm) is fine; 6+ means the organization is wrong.

(Pure per-feature modules — `pages/home.rs` owning its state + update + view — work only in multi-page apps with isolated per-page state. Ferrocull is single-screen; that pattern doesn't apply.)

## Sources

- [The Elm Architecture](https://guide.elm-lang.org/architecture/) · [Html.lazy optimization](https://guide.elm-lang.org/optimization/lazy.html)
- [Iced architecture](https://book.iced.rs/architecture.html) · [Task](https://docs.rs/iced/latest/iced/struct.Task.html) · [Subscription](https://docs.rs/iced/latest/iced/struct.Subscription.html) · [Lazy](https://docs.rs/iced/latest/iced/widget/struct.Lazy.html)
- [Iced todos example](https://github.com/iced-rs/iced/blob/master/examples/todos/src/main.rs) — nested `TaskMessage` (message-only decomposition)
- [Iced Discussion #1572](https://github.com/iced-rs/iced/discussions/1572) — maintainer guidance on module organization
- [Elm Radio: Performance](https://elm-radio.com/episode/performance/) · [Scaling Elm Apps](https://elm-radio.com/episode/scaling-elm-apps/) · [Impossible States](https://elm-radio.com/episode/impossible-states/)
- [Elm guide: Structure](https://guide.elm-lang.org/webapps/structure) — "keep growing those modules longer and longer"
- [Elm at NoRedInk](https://juliu.is/elm-at-noredink/) — nesting reserved for stateful children in a ~212k-line codebase
- [Halloy IRC client](https://github.com/squidowl/halloy) — prominent real-world iced application
