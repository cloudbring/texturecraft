# Golden images: whole-window snapshots

Reference PNGs for `crates/ui-egui/tests/ui_snapshots.rs` (`ui-<name>.png`; 28 scenes). The test
compares each scene and fails on a difference.

- **Bless**: `TC_BLESS=1 cargo test -p texturecraft-ui-egui --test ui_snapshots`, then review the
  PNGs in git. Each has an `.attribution` sidecar.
- **Failure**: PSNR / max / p99 in the panic message; `<name>.actual.png` and `<name>.diff.png`
  (error x8) in `target/golden-failures/`.
- **Metrics**: `craft-compare`, same helper as the render goldens (`crates/render/tests/common/golden.rs`).
- **Tolerance**: `Tolerance::RENDER` (PSNR >= 45 dB, max <= 12, p99 <= 2) on GPU output.
- **Settling**: the test steps the app until the picture is unchanged for three frames (tile
  compositing and GPU uploads are paced by wall-clock time) before it compares.
- **Status bar**: the bottom 22 points are blanked in the golden because they show wall-clock UI
  frame time and job progress. `TC_UI_SNAPSHOTS=<dir>` still writes the full pictures.
- **GPU gate**: skipped without a GPU adapter (CI has none).
- **Size policy**: at most 50 KB per golden. The 1600x960 windows do not fit at full size, so
  each is stored box-downscaled by the smallest integer factor that fits (1/3 to 1/7, recorded in
  its `.attribution` note); checks apply the same factor to the render. Averaging makes the check
  less sensitive to changes of a few pixels (a one-pixel glyph shift is mostly averaged away), so
  these goldens catch layout, colour and missing-element changes, not sub-pixel text changes.
