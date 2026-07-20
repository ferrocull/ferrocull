# Ferrocull Project Rules

## Design Context

Strategic product context (users, positioning, design principles) lives in [`PRODUCT.md`](PRODUCT.md); the visual design system ("The Darkroom": tokens, color roles, component vocabulary) lives in [`DESIGN.md`](DESIGN.md). Read both before any UI/UX work.

## Alpha Version

This is an alpha version. Do not add backwards compatibility code, cache versioning, or migration logic. Users can clear caches manually if needed.

## Clippy Compliance

Prefer fixing code as clippy suggests. Suppress a lint only when the suggested fix hurts readability or correctness — use `#[expect(clippy::...)]` rather than `#[allow(clippy::...)]` (a stale `#[expect]` warns when the lint no longer fires), always with a justifying comment.

## Toolchain

Rust **nightly** (pinned via `rust-toolchain.toml`), edition 2024 — nightly features are fair game.

## Commands

- Build: `cargo build`
- Run the app: `cargo run`
- Test: `cargo test --workspace`
- Pre-push gate: `cargo fmt` then `cargo clippy --workspace --all-targets` — resolve all lints

## Workspace Layout

- `crates/ferrocull` — binary entry point
- `crates/ferrocull-core` — culling engine: ingest, copy/verify, caches, metadata store
- `crates/ferrocull-media` — media file type definitions and extension categorization
- `crates/ferrocull-devices` — device/volume discovery and scanning (per-platform backends)
- `crates/ferrocull-ui` — iced UI (TEA)

## Rust Style

- **No `get_` prefix** — Use `foo()` not `get_foo()`. The getter convention is noise.
- **No `mod.rs`** — Use `foo.rs` instead of `foo/mod.rs`. Modern Rust module style.
- **RFC 0356 naming** — Never prefix a type with its module name. Use `copy::Error` not `copy::CopyError`. Use qualified paths (`module::Type`) at call sites instead of `use ... as` aliases.
- **No parameter bag structs** — Don't create a struct solely to bundle parameters for a single function call. If a function needs too many params, split it into smaller functions, use the builder pattern, or rethink the design. A struct is justified only when it has independent domain meaning — not when it's just a carrier for one call site.
- **Name by intent, not by container or type** — `user_count`, not `vec_users_len`. Full words, no abbreviations: `version`, not `ver`.
- **Iterators over manual loops** — `.collect()` over a push loop, `.map()` over `if let Some` just to transform.
- **Pattern matching over `if-else` chains**; let-chains (`if let ... && ...`) over nested `if let`.
- **Shadowing over `mut` when rebinding.**
- **Borrow over `.clone()` by default** — clone is fine for cheap types or when the alternative is lifetime contortion. `&T` over `Arc`/`Rc` when ownership isn't shared; pass `&Arc<T>` when not taking ownership.
- **`#[derive(...)]` over manual impls** when the behavior is standard.
- **Named struct fields over tuples** once positions become non-obvious (typically 3+ fields).
- **Enum variants encode state** — avoid enum + separate bool for related state.
- **Functions over macros** — macros hurt IDE support and error messages.
- **Safe code over `unsafe`** — justify every unsafe block with a comment stating why its invariants hold.
- **Extract, don't section-comment** — Once a function grows past ~30–50 lines, extract its logical steps into named functions. A `// validate input` comment over a block is the signal to pull it out into `validate_input(...)`, not to fence it off.
- **Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)** — general conventions, not just public APIs.

## Errors and Logging

- **`Result<T, E>` over `Option<T>` when absence is an error condition** — `None` carries no context.
- **Typed error enums (`thiserror`), not raw strings** — model domain errors as types the caller can match on.
- **Wrap I/O errors with context** — attach the file path; a bare "Permission denied" without it is undebuggable.
- **Never silently swallow errors** — no bare `let _ =` on fallible operations; justify any ignored result with a comment.
- **No `print!`/`println!`/`dbg!` in app code** — log via `tracing` instead.

## Async

- **Never hold a lock across an `.await`** — scope the guard so it drops before the await point.
- **Async I/O in async contexts** — `tokio::fs`, not `std::fs`, inside async code; keep `block_on` at the edges.

## Defensive Programming

Crash on invariant violations — never silently default. Handle runtime errors with `Result`.

This deliberately inverts the common Rust advice to avoid `expect()`: a broken invariant is a bug the software cannot handle, and continuing would run the app in an undefined state. Crash with context, get the report, fix the bug — do not "recover" from what has no correct recovery. Do not rewrite invariant `expect()`s into `?` propagation.

- **`expect()` for broken invariants** — When `None`/`Err` means a bug, use `.expect("reason")` to crash with context. Silent defaults (`unwrap_or_default()`) hide broken invariants.
- **`expect` messages describe what failed, not why it shouldn't** — `.expect("index out of bounds")`, not `.expect("guard ensures valid index")`. The message appears in the panic; make it useful for debugging.
- **`Result` + `?` for runtime errors** — When failure is a legitimate possibility (I/O, user input, external data), propagate with `?`. Don't `expect()` what can reasonably fail at runtime.
- **Trust upstream validation** — If data was already validated (parsed, filtered, resolved from an index), don't re-validate it downstream. One check at the boundary, then trust the type/value.
- **Trust append-only collections** — `items` is append-only and `item_index` maps paths to valid indices. After `item_index.get()` succeeds, use `items[idx]` directly — never `items.get(idx)` with a silent fallback.
- **No guards for impossible conditions** — If the data flow guarantees a condition (e.g., a scanned file always has a parent directory and a known extension), don't add a guard. Dead branches are misleading.
- **`map` over `filter_map` when all items pass** — If the pipeline guarantees every element satisfies the condition, use `map` + `expect`, not `filter_map` which silently drops "failures" that can't happen.
- **No internal parameter validation** — Functions trust their inputs. The caller validates before calling. No redundant checks, no silent coercion, no hidden fallbacks. If input is invalid, crash — don't mask the caller's bug.
- **Trust the database after startup** — `MediaDatabase::open()` validates the DB (opens connection, sets WAL mode, creates tables). If `open()` succeeds, the DB is a validated resource for the session lifetime. Mid-session query failures mean something catastrophic (disk yanked, filesystem corruption) — that's a broken invariant, not a runtime error. Use `expect()` on DB calls, not silent `status_message` fallbacks that would let the app continue in an invalid state.

## Keyboard shortcuts and vocabulary

Match Photo Mechanic for both shortcuts and user-facing terminology. Rationale: [ADR-0001](docs/adr/0001-photo-mechanic-vocabulary.md). Canonical terms: [`CONTEXT.md`](CONTEXT.md).

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
