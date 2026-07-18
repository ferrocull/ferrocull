# Product

## Register

product

## Platform

desktop

## Users

Working professionals — sports, event, and wedding photographers culling thousands of shots on deadline — are the primary audience, with serious enthusiasts as a secondary audience who benefit from the same speed. Both arrive with Photo Mechanic muscle memory (or the expectation of it) and are in a high-throughput, keyboard-driven workflow: card in, thousands of frames, decisions in seconds, ingest out.

## Product Purpose

Ferrocull is a FOSS culling tool in the Photo Mechanic tradition — it gets photographers through thousands of images as fast as possible. It is not a DAM and not an editor: the scope is culling (rating, color labels, tag/selection, compare, burst handling) and ingest (renaming, verification, multi-destination, XMP sidecars). Success looks like becoming the default answer when someone asks "is there a fast open-source culling tool?"

## Positioning

Photo Mechanic muscle memory, open source — zero relearning. Shortcuts, vocabulary, and workflow match PM conventions ([ADR-0001](docs/adr/0001-photo-mechanic-vocabulary.md)), so a PM refugee sits down and is productive immediately.

## Brand Personality

Warm, professional, invisible. The "Darkroom Editorial" aesthetic (warm neutrals, amber accents, dark-first) gives the tool a photographic warmth, but the tool's job is to disappear into the culling task — the photographs are the interface, and the chrome recedes behind them.

## Anti-references

- Legacy desktop chrome: keep PM's speed and shortcuts, not a cluttered, dialog-heavy interface.
- Trendy web-app gloss — no glassmorphism, gradient branding, or marketing-page polish inside a working tool.
- The bare GTK/Qt utility look — the generic unstyled Linux-utility aesthetic; the darkroom theme is deliberate.

## Design Principles

1. **Muscle memory is sacred.** Photo Mechanic shortcuts and vocabulary are matched verbatim; never invent a novel affordance where a PM convention exists.
2. **Optimize time-to-decision.** Every screen exists to shorten the gap between seeing a frame and rating, labeling, tagging, or rejecting it.
3. **The photograph is the interface.** Chrome stays quiet and warm-neutral so the image dominates; UI color (amber accent, label colors) marks state, never decoration.
4. **Keyboard-first, always.** The mouse is optional; a design that requires pointing has failed the primary user.
5. **Professional, not glossy.** Density, consistency, and restraint over novelty — familiarity a pro can trust.

## Accessibility & Inclusion

Full keyboard operability is a hard requirement: every action must be reachable without a mouse. This is both an accessibility commitment and the core of the product's speed claim.
