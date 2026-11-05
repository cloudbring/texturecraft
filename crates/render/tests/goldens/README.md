# Golden images: viewport renders

Reference PNGs for `crates/render/tests/headless.rs`. `render()` checks every name in its
`GOLDENS` list against `<name>.png` here and fails the test on a difference.

- **Bless** (write or update): `TC_BLESS=1 cargo test -p texturecraft-render --test headless`, then
  review the PNG diff in git. Each golden has an `.attribution` sidecar (written on first bless).
- **Failure**: the test panics with PSNR / max / p99, and writes `<name>.actual.png` and
  `<name>.diff.png` (error x8) to `target/golden-failures/`.
- **Metrics**: `craft-compare` (`max_abs_error`, `p99_abs_error`, `psnr`), over RGBA8.
- **Tolerance**: these are GPU renders, so `Tolerance::RENDER` (filmcraft's): PSNR >= 45 dB, max
  abs <= 12 levels, p99 <= 2 levels. Rendered twice on the same machine they are bit-identical
  (the tolerance is for other adapters: hardware vs software rounding). Goldens were blessed on
  Apple M6 (Metal).
- **GPU gate**: the tests skip without a GPU adapter, as before (CI has none), so the goldens are
  checked on developer machines and any GPU runner.
- **Size policy**: at most 50 KB per golden (`MAX_GOLDEN_BYTES` in `tests/common/golden.rs`). A
  larger render is box-downscaled by the smallest integer factor that fits when blessing, and
  checks downscale the render by the factor implied by the golden's size. These 480x360 renders
  all fit at full size.
