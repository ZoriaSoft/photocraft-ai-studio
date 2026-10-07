# Creative Studio MVP format baseline

The Creative Studio MVP supports the four primary interchange formats used by the product workflow.

| Format | Open | Export / Save | Notes |
|---|---|---|---|
| PNG | Yes | Yes | Lossless flat raster; alpha supported. |
| JPEG | Yes | Yes | Lossy flat raster; JPEG quality is configurable in Export As. |
| WebP | Yes | Yes | Lossless WebP encoding in the current pure-Rust codec; lossy WebP export is intentionally not advertised. |
| PSD | Yes | Yes | Layered import/export. Export As preserves layers and transparency; Save As also supports PSD/PSB. |

The desktop Open dialog exposes PNG, JPEG (`.jpg` / `.jpeg`), WebP and PSD directly. The Save As dialog exposes the same MVP set, while File > Export > Export As offers PNG, JPEG, lossless WebP and layered PSD copies.

These claims are backed by IO round-trip tests plus UI/service tests that keep the file-dialog format lists aligned with the MVP roadmap. PSD fidelity is broader than this baseline and is covered by the repository's dedicated PSD round-trip and corpus tests.
