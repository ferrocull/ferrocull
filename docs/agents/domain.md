# Domain Docs

How engineering skills should consume this repo's domain documentation.

## Before exploring, read these

Ferrocull is single-context: one `CONTEXT.md` (the domain glossary) and one `docs/adr/` tree, both at the repo root. Read `CONTEXT.md` first, then any ADRs that touch the area you're about to work in.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/grill-with-docs`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0003 (data-oriented design) — but worth reopening because…_
