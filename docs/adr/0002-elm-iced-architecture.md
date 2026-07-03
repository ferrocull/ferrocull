# Elm/iced architecture (TEA)

Ferrocull uses the [iced](https://iced.rs/) GUI framework with The Elm Architecture (TEA): a single `Ferrocull` state, an `enum Message` of events, a pure `update(state, msg) -> Task<Message>`, and a pure `view(state) -> Element`. State lives in one place; side effects are returned from `update` as `Task`s, not performed inline.

Considered alternatives: egui (immediate-mode, no message bus, harder to test), tauri (web stack — heavyweight and out of step with a "fast native" positioning), gtk/qt bindings (rejected for cross-platform consistency and Rust-idiomatic API).

See `docs/elm-iced-architecture-report.md` for the patterns we follow and the iced-specific best practices that flow from this choice.
