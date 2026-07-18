# Ferrocull Project Rules

## Design Context

Strategic product context (users, positioning, design principles) lives in [`PRODUCT.md`](PRODUCT.md); the visual design system ("The Darkroom": tokens, color roles, component vocabulary) lives in [`DESIGN.md`](DESIGN.md). Read both before any UI/UX work.

## Alpha Version

This is an alpha version. Do not add backwards compatibility code, cache versioning, or migration logic. Users can clear caches manually if needed.

## Clippy Compliance

Prefer fixing code as clippy suggests. Use `#[allow(clippy::...)]` only when the suggested fix hurts readability or correctness — always include a justifying comment.

## Toolchain

Using **Rust nightly** — nightly features are fair game.

## Rust Style

- **No `get_` prefix** — Use `foo()` not `get_foo()`. The getter convention is noise.
- **No `mod.rs`** — Use `foo.rs` instead of `foo/mod.rs`. Modern Rust module style.
- **RFC 0356 naming** — Never prefix a type with its module name. Use `copy::Error` not `copy::CopyError`. Use qualified paths (`module::Type`) at call sites instead of `use ... as` aliases.
- **No parameter bag structs** — Don't create a struct solely to bundle parameters for a single function call. If a function needs too many params, split it into smaller functions, use the builder pattern, or rethink the design. A struct is justified only when it has independent domain meaning — not when it's just a carrier for one call site.

## Defensive Programming

Crash on invariant violations — never silently default. Handle runtime errors with `Result`.

- **`expect()` for broken invariants** — When `None`/`Err` means a bug, use `.expect("reason")` to crash with context. Silent defaults (`unwrap_or_default()`) hide broken invariants.
- **`Result` + `?` for runtime errors** — When failure is a legitimate possibility (I/O, user input, external data), propagate with `?`. Don't `expect()` what can reasonably fail at runtime.
- **Trust upstream validation** — If data was already validated (parsed, filtered, resolved from an index), don't re-validate it downstream. One check at the boundary, then trust the type/value.
- **Trust append-only collections** — `items` is append-only and `item_index` maps paths to valid indices. After `item_index.get()` succeeds, use `items[idx]` directly — never `items.get(idx)` with a silent fallback.
- **No guards for impossible conditions** — If the data flow guarantees a condition (e.g., a scanned file always has a parent directory and a known extension), don't add a guard. Dead branches are misleading.
- **`map` over `filter_map` when all items pass** — If the pipeline guarantees every element satisfies the condition, use `map` + `expect`, not `filter_map` which silently drops "failures" that can't happen.
- **No internal parameter validation** — Functions trust their inputs. The caller validates before calling. No redundant checks, no silent coercion, no hidden fallbacks. If input is invalid, crash — don't mask the caller's bug.
- **Trust the database after startup** — `MediaDatabase::open()` validates the DB (opens connection, sets WAL mode, creates tables). If `open()` succeeds, the DB is a validated resource for the session lifetime. Mid-session query failures mean something catastrophic (disk yanked, filesystem corruption) — that's a broken invariant, not a runtime error. Use `expect()` on DB calls, not silent `status_message` fallbacks that would let the app continue in an invalid state.

## Keyboard shortcuts and vocabulary

Match Photo Mechanic for both shortcuts and user-facing terminology. Rationale: [ADR-0001](docs/adr/0001-photo-mechanic-vocabulary.md). Canonical terms: [`CONTEXT.md`](CONTEXT.md).

## Research Over Guessing

**Never guess.** If uncertain about APIs, library behavior, conventions, or any technical detail, do a web search first. Wrong assumptions waste time.

## Architecture

- UI follows The Elm Architecture (TEA) on iced — [ADR-0002](docs/adr/0002-elm-iced-architecture.md), patterns in [`docs/elm-iced-architecture-report.md`](docs/elm-iced-architecture-report.md).
- Core data model follows data-oriented design — [ADR-0003](docs/adr/0003-data-oriented-design.md), principles in [`docs/data-oriented-design.md`](docs/data-oriented-design.md).
- Architectural decisions are recorded in [`docs/adr/`](docs/adr/).

## Agent skills

### Issue tracker

GitHub Issues on `remigastaldi/ferrocull` via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Canonical five-role triage vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
