# Creative Studio MVP core editing baseline

The Creative Studio MVP reuses PhotoCraft's live document model rather than flattening edits into generated pixels. The core baseline is intentionally concrete:

| Area | MVP operations | Editing model |
|---|---|---|
| Layers | Create, duplicate, delete, rename/properties, visibility/opacity/blend, reorder, group/ungroup and clipping masks | Live document layers with undo/redo |
| Layer masks | Reveal/hide masks, selection-derived masks, enable/disable, link/unlink, apply and delete | Separate editable grayscale mask attached to the layer |
| Text | Create point or paragraph text, edit content, change font/size/color/paragraph styling, transform and rasterize when explicitly requested | Live type layers until rasterized |

The product-level smoke test in `crates/engine/tests/creative_studio_core.rs` exercises representative layer, mask and type edits through the public `Session` command boundary. The engine and UI crates already contain deeper command-specific tests for grouping, mask thumbnails/targets, typography, paragraph text, font handling, transforms and undo behavior.

This milestone does not mean every Photoshop layer, mask or typography feature is complete. It means the common Creative Studio editing path is present, exposed through the same command system used by the UI and AI Studio, and remains editable after each operation.
