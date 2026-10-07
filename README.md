# PhotoCraft AI Studio

> **Working title** — an experimental, AI-first layered image editor built on top of [PhotoCraft](https://github.com/storytold/photocraft).

PhotoCraft AI Studio explores a simpler way to edit real layered documents: keep the professional document engine, PSD compatibility, masks, text, vectors, filters and GPU compositing — but put natural-language commands, reusable workflows and automation at the center of the product.

## Project status

This repository is at the **foundation / early prototype** stage. The current codebase is intentionally close to upstream PhotoCraft while we separate product identity, preserve compatibility, and build the first AI-native workflow layer.

## Product direction

The first target is a focused desktop creative studio with four primary surfaces:

- **Canvas** — native GPU-rendered document editing.
- **Layers** — real editable layers, masks, groups, text and adjustments.
- **Properties** — direct manual control when precision matters.
- **AI / Commands** — translate natural-language intent into deterministic, undoable editor commands.

The goal is not to generate a new flattened image for every request. AI operations should map to normal editor actions whenever possible, leaving the document editable and exportable.

Example workflow:

```text
"Prepare this product photo for an online store"
        ↓
select subject
        ↓
create / refine mask
        ↓
create clean background layer
        ↓
center + scale subject
        ↓
add soft shadow
        ↓
apply tonal adjustment
        ↓
export requested sizes
```

## Planned milestones

### Foundation

- [x] Start from the PhotoCraft codebase and preserve upstream history.
- [x] Keep upstream licenses and attribution.
- [x] Remove upstream ArtCraft brand assets from the modified distribution.
- [ ] Establish the final product name and visual identity.
- [x] Add a dedicated AI / command panel shell.

### AI workflow MVP

- [x] Natural-language command planner foundation (local + optional model provider).
- [x] Expand the reviewed AI command vocabulary for common layer and tonal edits.

Model planner setup: [`docs/ai-studio.md`](docs/ai-studio.md)
- [x] Safe command allowlist with command-specific parameter validation.
- [x] Human-readable plan review before execution, with atomic Undo semantics visible in the UI.
- [x] Atomic multi-step AI actions: one Undo, automatic rollback on failure.
- [x] Save a validated command sequence as a reusable workflow.

### Creative Studio MVP

- [ ] Open PNG, JPEG, WebP and PSD documents.
- [ ] Core layer / mask / text operations.
- [ ] Product-photo workflow presets.
- [ ] Batch workflow execution.
- [ ] Export PNG, JPEG, WebP and PSD where supported by the engine.

## Architecture strategy

We intend to reuse PhotoCraft's modular Rust engine rather than rewrite the expensive foundations of an image editor. The important upstream building blocks include the document model, raster/color stack, PSD support, CPU/GPU compositing, text/vector systems, IO, automation and command engine.

The product-specific layer will gradually live above those foundations:

```text
AI planner / workflow engine
            ↓
validated editor commands
            ↓
PhotoCraft engine + document model
            ↓
CPU / GPU compositor
            ↓
editable layered document
```

We deliberately avoid a large-scale crate rename at this stage. Internal `photocraft-*` crate names remain intact so upstream synchronization stays practical while the new product layer is still being established.

## Upstream and attribution

This project is based on **PhotoCraft** by the ArtCraft team and PhotoCraft contributors:

- Upstream: https://github.com/storytold/photocraft
- Original copyright and license notices are retained in this repository.
- Source code remains available under the upstream **MIT OR Apache-2.0** licensing terms, subject to the notices and third-party licenses included in the repository.

This modified project is **not an official ArtCraft product and is not sponsored or endorsed by the ArtCraft team**.

ArtCraft brand assets are not open source. They are intentionally not included as branding for this fork. See `NOTICE`, `LICENSE-MIT`, `LICENSE-APACHE`, `ATTRIBUTION.md`, and the upstream repository for details.

## Upstream synchronization

The recommended Git remote layout is:

```text
origin    -> this fork
upstream  -> https://github.com/storytold/photocraft.git
```

To bring in upstream changes:

```bash
git fetch upstream
git switch main
git merge upstream/main
```

As product-specific changes grow, upstream updates should be integrated deliberately and validated with the existing test suite.

## Contributing

The project is currently experimental. Contributions should prefer small, reviewable changes that keep the editor engine separable from AI/product-specific code.
