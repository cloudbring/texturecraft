//! Sparse, copy-on-write tiled images of `u16` components.

use std::sync::Arc;

/// Tile edge in texels.
pub const TILE: u32 = 64;
/// Texels per tile.
pub const TILE_AREA: usize = (TILE * TILE) as usize;

/// A tile's texel data: `TILE_AREA × comps` interleaved components.
pub type TileData = Arc<Vec<u16>>;

/// Interpretation of texels when resizing. Coverage is stored in the last component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResamplePolicy {
    /// Independent continuous components (including one-component coverage masks).
    Continuous,
    /// Image values with coverage: Lanczos for fractional reductions, premultiplied filtering.
    Color,
    /// Continuous mask and coverage: nonnegative area/triangle weights, premultiplied filtering.
    Coverage,
    /// Premultiplied normal XYZ and coverage; normalize after filtering (Lanczos when shrinking).
    Normal,
    /// Categorical IDs or binary labels: never invent intermediate values.
    Nearest,
}

struct AxisTaps {
    first: u32,
    weights: Vec<f64>,
}

fn axis_taps(index: u32, source: u32, destination: u32, policy: ResamplePolicy) -> AxisTaps {
    let ratio = source as f64 / destination as f64;
    let center = (index as f64 + 0.5) * ratio - 0.5;
    if policy == ResamplePolicy::Nearest {
        return AxisTaps { first: (center + 0.5).floor().clamp(0.0, (source - 1) as f64) as u32, weights: vec![1.0] };
    }
    let integer = source.is_multiple_of(destination) && source / destination >= 2;
    let triangle = policy == ResamplePolicy::Coverage;
    let radius = if integer {
        ratio / 2.0
    } else if ratio > 1.0 {
        if triangle { ratio } else { 3.0 * ratio }
    } else {
        1.0
    };
    let mut start = (center - radius).floor().max(0.0) as u32;
    let end = (center + radius).ceil().min((source - 1) as f64) as u32;
    let mut weights = Vec::with_capacity((end - start + 1) as usize);
    for sample in start..=end {
        let distance = (sample as f64 - center).abs();
        let weight = if integer {
            (ratio / 2.0 - distance + 0.5).clamp(0.0, 1.0)
        } else if ratio > 1.0 && triangle {
            (1.0 - distance / ratio).max(0.0)
        } else if ratio > 1.0 {
            let phase = distance / ratio;
            if phase >= 3.0 {
                0.0
            } else if phase < 1e-12 {
                1.0
            } else {
                let angle = std::f64::consts::PI * phase;
                3.0 * angle.sin() * (angle / 3.0).sin() / (angle * angle)
            }
        } else {
            (1.0 - distance).max(0.0)
        };
        weights.push(weight);
    }
    while weights.len() > 1 && weights.first() == Some(&0.0) {
        weights.remove(0);
        start += 1;
    }
    while weights.len() > 1 && weights.last() == Some(&0.0) {
        weights.pop();
    }
    let sum: f64 = weights.iter().sum();
    if sum.abs() > 1e-12 {
        for weight in &mut weights {
            *weight /= sum;
        }
    }
    AxisTaps { first: start, weights }
}

/// An image split into 64×64 tiles; unallocated tiles read as zero. Cloning shares every tile,
/// so snapshots for undo cost one `Arc` clone per allocated tile.
#[derive(Clone, Debug, PartialEq)]
pub struct TileImage {
    width: u32,
    height: u32,
    comps: usize,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Option<TileData>>,
}

impl TileImage {
    /// `comps` is clamped to 1..=4; sizes to at least 1.
    pub fn new(width: u32, height: u32, comps: usize) -> TileImage {
        let (width, height) = (width.max(1), height.max(1));
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        TileImage { width, height, comps: comps.clamp(1, 4), tiles_x, tiles_y, tiles: vec![None; (tiles_x * tiles_y) as usize] }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn comps(&self) -> usize {
        self.comps
    }
    pub fn tiles_x(&self) -> u32 {
        self.tiles_x
    }
    pub fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    fn tile_index(&self, tx: u32, ty: u32) -> Option<usize> {
        (tx < self.tiles_x && ty < self.tiles_y).then(|| (ty * self.tiles_x + tx) as usize)
    }

    pub fn tile(&self, tx: u32, ty: u32) -> Option<&[u16]> {
        self.tile_index(tx, ty).and_then(|i| self.tiles.get(i)).and_then(|t| t.as_deref().map(Vec::as_slice))
    }

    /// The shared tile handle (for undo snapshots and change fingerprints).
    pub fn tile_handle(&self, tx: u32, ty: u32) -> Option<&TileData> {
        self.tile_index(tx, ty).and_then(|i| self.tiles.get(i)).and_then(Option::as_ref)
    }

    /// Replace a tile wholesale (None frees it).
    pub fn set_tile(&mut self, tx: u32, ty: u32, data: Option<TileData>) {
        let len = TILE_AREA * self.comps;
        let data = data.filter(|d| d.len() == len);
        if let Some(i) = self.tile_index(tx, ty)
            && let Some(slot) = self.tiles.get_mut(i)
        {
            *slot = data;
        }
    }

    /// Mutable texels of a tile, allocating it filled with `fill` (one value per component)
    /// when absent and un-sharing it when shared.
    pub fn tile_mut(&mut self, tx: u32, ty: u32, fill: &[u16]) -> Option<&mut [u16]> {
        let comps = self.comps;
        let i = self.tile_index(tx, ty)?;
        let slot = self.tiles.get_mut(i)?;
        let tile = slot.get_or_insert_with(|| {
            let mut v = vec![0u16; TILE_AREA * comps];
            if fill.iter().any(|f| *f != 0) {
                for px in v.chunks_exact_mut(comps) {
                    for (d, s) in px.iter_mut().zip(fill) {
                        *d = *s;
                    }
                }
            }
            Arc::new(v)
        });
        Some(Arc::make_mut(tile).as_mut_slice())
    }

    pub fn is_allocated(&self, tx: u32, ty: u32) -> bool {
        self.tile_handle(tx, ty).is_some()
    }

    /// Allocated tile coordinates.
    pub fn allocated(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.tiles.iter().enumerate().filter(|(_, t)| t.is_some()).map(|(i, _)| (i as u32 % self.tiles_x, i as u32 / self.tiles_x))
    }

    pub fn allocated_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count()
    }

    /// Components of the texel at (x, y); zeros when out of range or unallocated.
    pub fn get(&self, x: u32, y: u32) -> [u16; 4] {
        let mut out = [0u16; 4];
        if x >= self.width || y >= self.height {
            return out;
        }
        if let Some(t) = self.tile(x / TILE, y / TILE) {
            let o = (((y % TILE) * TILE + x % TILE) as usize) * self.comps;
            for (c, v) in out.iter_mut().take(self.comps).enumerate() {
                *v = t.get(o + c).copied().unwrap_or(0);
            }
        }
        out
    }

    /// Set one texel (allocating its tile with zeros).
    pub fn set(&mut self, x: u32, y: u32, v: &[u16]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let comps = self.comps;
        if let Some(t) = self.tile_mut(x / TILE, y / TILE, &[]) {
            let o = (((y % TILE) * TILE + x % TILE) as usize) * comps;
            for (c, s) in v.iter().take(comps).enumerate() {
                if let Some(d) = t.get_mut(o + c) {
                    *d = *s;
                }
            }
        }
    }

    /// Bytes held by allocated tiles (memory estimate for the status bar).
    pub fn bytes(&self) -> usize {
        self.allocated_count() * TILE_AREA * self.comps * 2
    }

    /// Drop tiles whose every texel is zero.
    pub fn prune_empty(&mut self) {
        for t in &mut self.tiles {
            if t.as_ref().is_some_and(|d| d.iter().all(|v| *v == 0)) {
                *t = None;
            }
        }
    }
}

/// 0..1 → u16.
#[inline]
pub fn to_u16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

/// u16 → 0..1.
#[inline]
pub fn from_u16(v: u16) -> f32 {
    v as f32 * (1.0 / 65535.0)
}

/// -1..1 → u16.
#[inline]
pub fn to_u16_signed(v: f32) -> u16 {
    to_u16(v * 0.5 + 0.5)
}

/// u16 → -1..1.
#[inline]
pub fn from_u16_signed(v: u16) -> f32 {
    from_u16(v) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unallocated_tiles_read_zero_and_writes_allocate() {
        let mut img = TileImage::new(200, 130, 2);
        assert_eq!(img.tiles_x(), 4);
        assert_eq!(img.tiles_y(), 3);
        assert_eq!(img.get(10, 10), [0; 4]);
        img.set(199, 129, &[7, 9]);
        assert_eq!(img.get(199, 129), [7, 9, 0, 0]);
        assert_eq!(img.allocated_count(), 1);
        assert_eq!(img.allocated().collect::<Vec<_>>(), vec![(3, 2)]);
        // Out of range is ignored, not a panic.
        img.set(500, 500, &[1, 1]);
        assert_eq!(img.get(500, 500), [0; 4]);
    }

    #[test]
    fn clones_share_tiles_until_written() {
        let mut a = TileImage::new(64, 64, 1);
        a.set(0, 0, &[5]);
        let b = a.clone();
        assert!(Arc::ptr_eq(a.tile_handle(0, 0).unwrap(), b.tile_handle(0, 0).unwrap()));
        a.set(1, 0, &[6]);
        assert!(!Arc::ptr_eq(a.tile_handle(0, 0).unwrap(), b.tile_handle(0, 0).unwrap()));
        assert_eq!(b.get(1, 0)[0], 0);
        assert_eq!(a.get(1, 0)[0], 6);
    }

    #[test]
    fn tile_mut_fills_new_tiles() {
        let mut img = TileImage::new(64, 64, 2);
        let t = img.tile_mut(0, 0, &[100, 200]).unwrap();
        assert_eq!(&t[..4], &[100, 200, 100, 200]);
        img.prune_empty();
        assert_eq!(img.allocated_count(), 1);
        img.set_tile(0, 0, Some(Arc::new(vec![0; TILE_AREA * 2])));
        img.prune_empty();
        assert_eq!(img.allocated_count(), 0);
    }

    #[test]
    fn unorm_round_trips() {
        for v in [0.0, 0.25, 0.5, 1.0] {
            assert!((from_u16(to_u16(v)) - v).abs() < 1e-4);
            assert!((from_u16_signed(to_u16_signed(v * 2.0 - 1.0)) - (v * 2.0 - 1.0)).abs() < 1e-4);
        }
        assert_eq!(to_u16(f32::NAN), 0);
    }

    #[test]
    fn checkerboard_integer_reductions_average_all_source_texels() {
        for ratio in [2, 4, 8] {
            let mut image = TileImage::new(64, 64, 1);
            for y in 0..64 {
                for x in 0..64 {
                    image.set(x, y, &[if x % ratio == 0 && y % ratio == 0 { 65472 } else { 0 }]);
                }
            }
            let reduced = image.resampled(64 / ratio, 64 / ratio, &[0]);
            assert_eq!(reduced.get(0, 0)[0], 65472 / (ratio * ratio) as u16, "ratio {ratio}");
        }
    }

    #[test]
    fn constant_and_sparse_defaults_are_preserved() {
        let mut image = TileImage::new(143, 145, 2);
        for y in 0..145 {
            for x in 0..143 {
                image.set(x, y, &[12345, 23456]);
            }
        }
        for (width, height) in [(71, 69), (16, 18), (286, 290)] {
            let reduced = image.resampled_with(width, height, &[0, 0], ResamplePolicy::Continuous);
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(&reduced.get(x, y)[..2], &[12345, 23456]);
                }
            }
        }
        let mut sparse = TileImage::new(256, 256, 1);
        sparse.set(0, 0, &[100]);
        assert!(!sparse.resampled(128, 128, &[42]).is_allocated(1, 1));
    }

    #[test]
    fn fractional_impulse_has_scaled_footprint() {
        let mut image = TileImage::new(60, 1, 1);
        image.set(28, 0, &[60000]);
        let reduced = image.resampled_with(23, 1, &[0], ResamplePolicy::Continuous);
        let values: Vec<_> = (0..23).map(|x| reduced.get(x, 0)[0]).collect();
        assert!(values.iter().filter(|value| **value > 0).count() >= 2, "{values:?}");
        assert!(values[10] > 0 && values[11] > 0, "{values:?}");
    }

    #[test]
    fn coverage_is_premultiplied_and_masks_keep_their_kind() {
        let mut image = TileImage::new(2, 1, 4);
        image.set(0, 0, &[65535, 0, 0, 32768]);
        image.set(1, 0, &[0, 0, 0, 0]);
        assert_eq!(image.resampled_with(1, 1, &[], ResamplePolicy::Color).get(0, 0), [65535, 0, 0, 16384]);

        let mut mask = TileImage::new(8, 1, 1);
        mask.set(0, 0, &[65535]);
        mask.set(1, 0, &[65535]);
        let categorical = mask.resampled_with(3, 1, &[], ResamplePolicy::Nearest);
        assert_eq!(categorical.get(0, 0)[0], 65535);
        assert!((0..3).all(|x| [0, 65535].contains(&categorical.get(x, 0)[0])));
        let coverage = mask.resampled_with(3, 1, &[], ResamplePolicy::Coverage);
        assert!((0..3).any(|x| (1..65535).contains(&coverage.get(x, 0)[0])));
        for x in 0..3 {
            let taps = axis_taps(x, 8, 3, ResamplePolicy::Coverage);
            assert!(taps.weights.iter().all(|weight| *weight >= 0.0));
            assert!((taps.weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn normals_are_renormalized_and_tile_results_match_whole_image() {
        let mut image = TileImage::new(260, 130, 4);
        for y in 0..130 {
            for x in 0..260 {
                let normal = if x % 2 == 0 { [65535, 32768, 32768, 65535] } else { [32768, 65535, 32768, 65535] };
                image.set(x, y, &normal);
            }
        }
        let reduced = image.resampled_with(139, 70, &[32768, 32768, 65535, 0], ResamplePolicy::Normal);
        for y in 0..70 {
            for x in 0..139 {
                let pixel = reduced.get(x, y);
                let length = pixel[..3].iter().map(|part| (f64::from(*part) / 32767.5 - 1.0).powi(2)).sum::<f64>().sqrt();
                assert!((length - 1.0).abs() < 0.0001, "{pixel:?}: {length}");
            }
        }
        for ty in 0..reduced.tiles_y() {
            for tx in 0..reduced.tiles_x() {
                let separate = image.resampled_tile_with(139, 70, tx, ty, &[], ResamplePolicy::Normal);
                assert_eq!(separate.as_deref(), reduced.tile(tx, ty));
            }
        }
        let mut rgb = TileImage::new(2, 1, 3);
        rgb.set(0, 0, &[65535, 32768, 32768]);
        rgb.set(1, 0, &[32768, 65535, 32768]);
        let pixel = rgb.resampled_with(1, 1, &[], ResamplePolicy::Normal).get(0, 0);
        assert!((pixel[0] as i32 - pixel[1] as i32).abs() <= 1);
        assert!(pixel[0] > 45000 && pixel[2] < 33000);
    }

    #[test]
    #[ignore = "manual 8192² -> 1024² timing"]
    fn benchmark_large_reduction() {
        let mut image = TileImage::new(8192, 8192, 1);
        for y in 0..image.tiles_y() {
            for x in 0..image.tiles_x() {
                let tile = image.tile_mut(x, y, &[]).unwrap();
                tile.fill(32768);
            }
        }
        let start = std::time::Instant::now();
        let result = image.resampled(1024, 1024, &[0]);
        println!("8192² -> 1024²: {:.1} ms; sample {}", start.elapsed().as_secs_f64() * 1000.0, result.get(0, 0)[0]);
    }
}

impl TileImage {
    /// A scale-aware resample to `width × height` (same component count). Unallocated source
    /// texels read as `empty` (one value per component); destination tiles whose source
    /// footprint is entirely unallocated stay unallocated.
    pub fn resampled(&self, width: u32, height: u32, empty: &[u16]) -> TileImage {
        let policy = if self.comps > 1 { ResamplePolicy::Color } else { ResamplePolicy::Continuous };
        self.resampled_with(width, height, empty, policy)
    }

    /// Resize using an explicit interpretation of coverage, normals or categorical IDs.
    pub fn resampled_with(&self, width: u32, height: u32, empty: &[u16], policy: ResamplePolicy) -> TileImage {
        let mut out = TileImage::new(width, height, self.comps);
        if self.allocated_count() == 0 {
            return out;
        }
        for ty in 0..out.tiles_y {
            for tx in 0..out.tiles_x {
                if let Some(data) = self.resampled_tile_with(width, height, tx, ty, empty, policy) {
                    out.set_tile(tx, ty, Some(Arc::new(data)));
                }
            }
        }
        out
    }

    /// Tile (tx, ty) of [`TileImage::resampled`] alone (None when its source footprint is
    /// entirely unallocated).
    pub fn resampled_tile(&self, width: u32, height: u32, tx: u32, ty: u32, empty: &[u16]) -> Option<Vec<u16>> {
        let policy = if self.comps > 1 { ResamplePolicy::Color } else { ResamplePolicy::Continuous };
        self.resampled_tile_with(width, height, tx, ty, empty, policy)
    }

    /// Independently resample one destination tile, using the global image coordinates and
    /// complete source footprint so tile boundaries match a whole-image reduction.
    pub fn resampled_tile_with(&self, width: u32, height: u32, tx: u32, ty: u32, empty: &[u16], policy: ResamplePolicy) -> Option<Vec<u16>> {
        let (dw, dh) = (width.max(1), height.max(1));
        if tx >= dw.div_ceil(TILE) || ty >= dh.div_ceil(TILE) || self.allocated_count() == 0 {
            return None;
        }
        let comps = self.comps;
        let x_taps: Vec<_> = (tx * TILE..((tx + 1) * TILE).min(dw)).map(|x| axis_taps(x, self.width, dw, policy)).collect();
        let y_taps: Vec<_> = (ty * TILE..((ty + 1) * TILE).min(dh)).map(|y| axis_taps(y, self.height, dh, policy)).collect();
        let x_start = x_taps.first()?.first;
        let x_end = x_taps.last().map(|tap| tap.first + tap.weights.len() as u32 - 1)?;
        let y_start = y_taps.first()?.first;
        let y_end = y_taps.last().map(|tap| tap.first + tap.weights.len() as u32 - 1)?;
        let any = (y_start / TILE..=y_end / TILE).any(|sy| (x_start / TILE..=x_end / TILE).any(|sx| self.is_allocated(sx, sy)));
        if !any {
            return None;
        }
        let mut data = vec![0u16; TILE_AREA * comps];
        let coverage = matches!(policy, ResamplePolicy::Color | ResamplePolicy::Coverage | ResamplePolicy::Normal) && (comps == 2 || comps == 4);
        let mut rows = vec![[0.0f64; 4]; (y_end - y_start + 1) as usize * x_taps.len()];
        for sy in y_start..=y_end {
            for (lx, taps) in x_taps.iter().enumerate() {
                let result = &mut rows[(sy - y_start) as usize * x_taps.len() + lx];
                for (offset, weight) in taps.weights.iter().enumerate() {
                    if *weight == 0.0 {
                        continue;
                    }
                    let sx = taps.first + offset as u32;
                    let pixel = if self.is_allocated(sx / TILE, sy / TILE) {
                        self.get(sx, sy)
                    } else {
                        let mut value = [0; 4];
                        for (component, input) in value.iter_mut().zip(empty) {
                            *component = *input;
                        }
                        value
                    };
                    let alpha = if coverage { pixel[comps - 1] as f64 / 65535.0 } else { 1.0 };
                    for component in 0..comps {
                        result[component] += pixel[component] as f64 * weight * if coverage && component != comps - 1 { alpha } else { 1.0 };
                    }
                }
            }
        }
        for (ly, taps) in y_taps.iter().enumerate() {
            for lx in 0..x_taps.len() {
                let mut value = [0.0f64; 4];
                for (offset, weight) in taps.weights.iter().enumerate() {
                    let row = &rows[(taps.first + offset as u32 - y_start) as usize * x_taps.len() + lx];
                    for component in 0..comps {
                        value[component] += row[component] * weight;
                    }
                }
                if coverage {
                    let alpha = value[comps - 1].clamp(0.0, 65535.0) / 65535.0;
                    for component in 0..comps - 1 {
                        value[component] = if alpha > 1e-12 { value[component] / alpha } else { 0.0 };
                    }
                }
                if policy == ResamplePolicy::Normal && comps >= 3 && (!coverage || value[comps - 1] > 0.0) {
                    let vector = [value[0], value[1], value[2]].map(|part| part / 32767.5 - 1.0);
                    let length = vector.iter().map(|part| part * part).sum::<f64>().sqrt();
                    let normal = if length > 1e-12 { vector.map(|part| part / length) } else { [0.0, 0.0, 1.0] };
                    for component in 0..3 {
                        value[component] = (normal[component] + 1.0) * 32767.5;
                    }
                }
                let base = (ly * TILE as usize + lx) * comps;
                for component in 0..comps {
                    data[base + component] = value[component].round().clamp(0.0, 65535.0) as u16;
                }
            }
        }
        Some(data)
    }

    /// Bilinear sample at `(fx, fy)` in texels (texel centers at +0.5, clamped to the image)
    /// into `out` (one value per component); unallocated texels read as `empty`. Returns
    /// whether any of the four texels is allocated.
    fn bilinear(&self, fx: f32, fy: f32, empty: &[u16], out: &mut [u16]) -> bool {
        let (fx, fy) = ((fx - 0.5).max(0.0), (fy - 0.5).max(0.0));
        let (ix, wx, iy, wy) = (fx.floor() as u32, fx.fract(), fy.floor() as u32, fy.fract());
        let mut any = false;
        let mut get = |x: u32, y: u32| {
            let (x, y) = (x.min(self.width.saturating_sub(1)), y.min(self.height.saturating_sub(1)));
            if self.is_allocated(x / TILE, y / TILE) {
                any = true;
                self.get(x, y)
            } else {
                let mut e = [0u16; 4];
                for (d, s) in e.iter_mut().zip(empty) {
                    *d = *s;
                }
                e
            }
        };
        let (a, b, c, d) = (get(ix, iy), get(ix + 1, iy), get(ix, iy + 1), get(ix + 1, iy + 1));
        for (k, slot) in out.iter_mut().enumerate().take(4) {
            let top = a[k] as f32 * (1.0 - wx) + b[k] as f32 * wx;
            let bot = c[k] as f32 * (1.0 - wx) + d[k] as f32 * wx;
            *slot = (top * (1.0 - wy) + bot * wy + 0.5) as u16;
        }
        any
    }

    /// Whether a bilinear read anywhere in the texel box `(x0, y0)..(x1, y1)` can touch an
    /// allocated tile (callers skip work for regions that would read only `empty`).
    pub fn any_allocated_in(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let span = |lo: f32, hi: f32, n: u32| ((lo - 1.0).max(0.0) as u32 / TILE, ((hi + 1.0).max(0.0) as u32 / TILE).min(n.saturating_sub(1)));
        let ((ax, bx), (ay, by)) = (span(x0.min(x1), x0.max(x1), self.tiles_x), span(y0.min(y1), y0.max(y1), self.tiles_y));
        (ay..=by).any(|sy| (ax..=bx).any(|sx| self.is_allocated(sx, sy)))
    }

    /// Tile (tx, ty) of a `width × height` image resampled through `map`: the center of
    /// destination texel (x, y) reads this image bilinearly at `map(x + 0.5, y + 0.5)` (texels;
    /// None leaves `empty`). None when no texel of the tile reads an allocated source texel.
    pub fn remapped_tile(&self, width: u32, height: u32, tx: u32, ty: u32, empty: &[u16], map: &dyn Fn(f32, f32) -> Option<(f32, f32)>) -> Option<Vec<u16>> {
        if self.allocated_count() == 0 || self.width == 0 || self.height == 0 {
            return None;
        }
        let comps = self.comps;
        let mut data = vec![0u16; TILE_AREA * comps];
        let mut any = false;
        for ly in 0..TILE.min(height.saturating_sub(ty * TILE)) {
            for lx in 0..TILE.min(width.saturating_sub(tx * TILE)) {
                let o = ((ly * TILE + lx) as usize) * comps;
                let Some(out) = data.get_mut(o..o + comps) else { continue };
                match map((tx * TILE + lx) as f32 + 0.5, (ty * TILE + ly) as f32 + 0.5) {
                    Some((sx, sy)) if sx.is_finite() && sy.is_finite() => any |= self.bilinear(sx, sy, empty, out),
                    _ => {
                        for (d, s) in out.iter_mut().zip(empty) {
                            *d = *s;
                        }
                    }
                }
            }
        }
        any.then_some(data)
    }
}

#[cfg(test)]
mod resample_tests {
    use super::*;

    #[test]
    fn resample_up_and_down_keeps_values_and_sparsity() {
        let mut img = TileImage::new(128, 128, 2);
        for y in 0..64 {
            for x in 0..64 {
                img.set(x, y, &[30000, 65535]);
            }
        }
        let up = img.resampled(256, 256, &[]);
        assert_eq!(up.get(40, 40)[..2], [30000, 65535]);
        assert!(!up.is_allocated(3, 3), "empty regions stay sparse");
        let down = img.resampled(64, 64, &[]);
        assert_eq!(down.get(10, 10)[..2], [30000, 65535]);
    }

    #[test]
    fn remap_moves_texels_and_keeps_sparsity() {
        let mut img = TileImage::new(128, 128, 2);
        img.set(10, 20, &[1000, 65535]);
        // Mirror horizontally into a 256-wide image (destination x reads source 127.x - x / 2).
        let map = |x: f32, y: f32| (x < 256.0).then(|| (128.0 - x / 2.0, y / 2.0));
        let tile = |tx, ty| img.remapped_tile(256, 256, tx, ty, &[0, 0], &map);
        assert!(tile(0, 0).is_none(), "nothing allocated is read there");
        let t = tile(3, 0).unwrap();
        // Destination (234, 41) center (234.5, 41.5) reads (10.75, 20.75): mostly texel (10, 20).
        let v = t[((41 * TILE + (234 - 192)) * 2) as usize];
        assert!(v > 500, "{v}");
    }
}
