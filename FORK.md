# Fork maintenance notes

This repository is a modified derivative of `storytold/photocraft`.

## Principles

1. Preserve upstream Git history.
2. Keep `upstream` configured separately from this fork's `origin`.
3. Retain required copyright, license, NOTICE and third-party attribution files.
4. Do not reintroduce ArtCraft trademark assets into the modified product branding.
5. Keep upstream engine changes separable from product-specific AI/workflow features when practical.

## Branches

- `main`: publishable project baseline.
- `ai-studio-foundation`: initial fork/product-foundation work.
- `feature/*`: focused implementation branches.

## Initial product boundary

For the first MVP, avoid rewriting the editor engine. Build the AI/workflow layer around existing commands and only change lower-level crates when a concrete product requirement demands it.
