# Photo Mechanic vocabulary and keyboard shortcuts

Ferrocull is a FOSS culling tool with Photo Mechanic-compatible shortcuts and vocabulary. We commit to matching Photo Mechanic's keyboard shortcuts and user-facing vocabulary (see `CONTEXT.md`) wherever a term or shortcut is shared — `T` for tag, `X` for reject, `G` for promote, "ingest" rather than "download/import", "tagged" rather than "picked", and so on. One deliberate divergence: unmodified digits `1..5` set star ratings, where Photo Mechanic uses them for color class. One Ferrocull-only addition: `B` toggles burst collapse/expand, a concept Photo Mechanic does not have.

The cost is alienating users coming from other tools (Lightroom, darktable, digiKam) who expect different conventions. The benefit is that the target audience — photographers who want these conventions in an open-source tool — can pick up Ferrocull without retraining muscle memory. That compatibility is the value proposition; without it, Ferrocull is just another viewer.

This decision constrains every future UX choice: new features adopt PM's terminology and shortcuts first, and only invent new ones when PM has no equivalent.
