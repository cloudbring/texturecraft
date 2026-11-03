# Compose Gaussian migration (#2)

Measured on November 3, 2026, on `algo/2-texturecraft-craft-blur`, based on
`algo/36-fast-area-reduce`. The consumer uses `craft-blur` from
`cloudbring/craft-libs`, branch `algo/2-craft-blur-midband`, locked at
`d98d0605bb63a0862330f3885605ec1def323643`. The manifest TODO tracks repointing
to an upstream revision after that branch lands.

## Units and variance

The UI's blur intensity/radius is measured in texels at 2K. For a positive UI
radius `R`, the Gaussian sigma in texture texels is
`max(0.5, R * resolution / 2048)`, capped at the atlas's larger dimension for
hostile inputs. Block-scaled atlases use `max(1, round(resolution * scale))`
in place of resolution. Zero/negative radius still bypasses Blur. Bevel keeps
its existing half-width conversion; Sharpen keeps its 1.5-at-2K radius and
0.75-texel minimum.

Previously this texel radius was rounded to an integer `r >= 1` and used for
three identical box passes per axis. Their ideal variance is `r * (r + 1)`,
not `r²`; fractional radii also suffered integer quantization. Centered impulse
measurements before migration, normalized by output mass, were:

| Texel radius / intended sigma | Old variance | Old relative error |
| --- | --- | --- |
| 1 | 2.000000007 | +100.000001% |
| 2 | 6.000000000 | +50.000000% |
| 4 | 19.999999910 | +24.999999% |
| 8 | 71.999999968 | +12.500000% |
| 16 | 271.999997357 | +6.249999% |
| 32 | 1055.999834805 | +3.124984% |
| 64 | 4160.011676185 | +1.562785% |

The old fixed-point implementation required a high-amplitude impulse (`1e6`)
to avoid quantizing away the signal when measuring one axis in a one-texel-high
plane. The measurements divide by mass; amplitude does not change variance.
The new regression uses unit impulses along both axes, at sigma
0.5/0.75/1/1.25/2/4/7.99/8/16/32/64/128/256, with boundaries at least 32 sigma
away. Every relative variance error must be at most 0.5%; the observed maximum
after migration is below 0.00001% across all 26 axis/sigma cases.

## Semantics and scheduling

- All Gaussian-backed operations (Blur, Sharpen, High Pass, Bevel, Glow) use
  `Quality::Recursive`. The library selects its variance-calibrated sampled
  kernel below sigma 8, including fractional sigmas.
- Effective edge policy remains zero extension. An explicit one-texel zero
  border enables the library's optimized Clamp implementation without
  extending any nonzero original boundary value. Each complete axis therefore
  sees the same input extension as `Edge::Zero`.
- Mask values and UV coverage are filtered separately, then normalized by
  filtered UV coverage. Content's premultiplied values and painted coverage
  are filtered with the same kernel before unpremultiplication. Opacity,
  signed-channel midpoints, and Bevel/Glow coverage handling are unchanged.
- Sampled output bands retain the kernel's full support. Recursive passes
  use the full atlas extent, never independently initialized tile halos;
  otherwise tile grouping would change their infinite-support response.
  This can increase field-evaluation work for sparse dirty-tile requests at
  recursive sigmas, even though the blur primitive is faster.
- Parallel scheduling uses four workers for the benchmark, reusable per-worker
  scratch, sampled bands or complete recursive rows/disjoint column stripes,
  and no full-image transpose. Nonfinite input remains zeroed and finite HDR
  input remains capped at ±1e6 before filtering.
- Directional/slope blurs remain directional sample gathers, not Gaussians.
  The compose/texel audit found no other independent Gaussian implementation.
  The 57-line fixed-point/three-box/transpose block was removed entirely.

## Existing expectations and output changes

No existing assertions or golden expectations were loosened or changed.
`blur_filter_softens_mask_edges`, `content_blur_softens_fill_edges`, and the
existing cache/anchor/grouping tests retain their original expectations.

A temporary before/after audit rendered ten 64² checker fixtures through the
real compositor: the five Gaussian-backed filters applied to masks and to
roughness content. Blur/High Pass/Glow use UI radius 64, Bevel width 128, and
Sharpen amount 0.5; Glow amount is 0.5. Eight of ten outputs change as expected
with the corrected kernel. Maximum normalized output deltas after u16 storage:

| Filter | Mask max delta | Content max delta |
| --- | --- | --- |
| Blur | 0.040955186 | 0.081910446 |
| Sharpen | 0 | 0 |
| High Pass | 0.040955186 | 0.081910431 |
| Bevel | 0.081910431 | 0.163836092 |
| Glow | 0.020477593 | 0.040955223 |

New tests cover fractional radius mapping, variance on both axes, degenerate
dimensions, nonfinite/HDR sanitation, agreement with a direct `Edge::Zero`
reference (relative/absolute tolerance 1e-6), sampled-band boundaries,
premultiplied edge coverage, and bit-identical final u16 composites across
tile grouping and cached evaluation, including recursive sigmas 8 and 16.

## 4096² single-channel benchmark

Release build, rustup stable 1.99.0, four Rayon workers, 32-texel checker cells.
These are alternating-order, same-binary before/after medians of seven runs
after two warmups. Timing includes the blur's internal allocations, sanitation,
and padding, but excludes cloning the source, field evaluation, and coverage
normalization. The legacy implementation was present only in the temporary
measurement harness, not retained in production or tests. Host load varied;
these are observations, not CI timing thresholds.

| Sigma in texture texels | Before (ms) | After (ms) |
| --- | --- | --- |
| 1 | 97.889 | 14.262 |
| 4 | 98.051 | 45.447 |
| 16 | 90.121 | 26.958 |
| 64 | 93.878 | 25.395 |

The retained ignored benchmark can be rerun with:

```sh
export PATH="$(rustup run stable rustc --print sysroot)/bin:$PATH"
export RUSTC="$(rustup which --toolchain stable rustc)"
export RUSTDOC="$(rustup which --toolchain stable rustdoc)"
export CARGO_TARGET_DIR="$PWD/target-2"
rustup run stable cargo test -p texturecraft-compose --release \
  bench_mask_blur_4096 -- --ignored --nocapture
```

Validation uses `cargo test -p texturecraft-compose` in debug and release,
workspace/all-target clippy with only `chunks_exact_to_as_chunks`,
`manual_slice_fill`, and `deprecated` allowed, `cargo fmt --all -- --check`,
`cargo xtask layers`, and `cargo xtask wasm`. Explicit rustup compiler paths
avoid accidentally using Homebrew rustc, whose sysroot lacks the wasm target.

Results: debug 44 passed / 0 failed, release 44 passed / 0 failed, one ignored
benchmark explicitly passed, clippy and fmt passed, layering 15 crates / 0
violations, wasm 11 packages / 0 failures. No application commands changed.
