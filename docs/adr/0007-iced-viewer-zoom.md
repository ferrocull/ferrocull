# Use iced's `Viewer` widget for preview zoom and pan

The full-screen preview uses iced's built-in `Viewer` widget for continuous wheel-zoom (25%–800%) and drag-pan. The widget gives us smooth, flicker-free native rendering with zero custom code.

The trade-offs are real and worth recording so they aren't re-litigated: `Viewer` has no discrete zoom levels (no `1` for fit, `2` for 100%), no `+`/`-` keyboard zoom, no `Z`-key loupe toggle, and — most importantly — **no synchronised zoom/pan across two panes in compare mode**. Photo Mechanic does have synced zoom; getting it would require forking iced's `Viewer` widget.

We accepted the trade-off because the alternative (a custom zoom widget) is a multi-week project and the current behaviour covers ~90% of culling. Synced zoom moves to "future enhancement" rather than blocking compare mode shipping.
