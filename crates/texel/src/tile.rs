//! Sparse, copy-on-write tiled images of `u16` components.

use craft_tiledir::DenseTileDir;
use std::sync::Arc;

/// Tile edge in texels.
pub const TILE: u32 = 64;
/// Texels per tile.
pub const TILE_AREA: usize = (TILE * TILE) as usize;
/// A tile's texel data: `TILE_AREA × comps` interleaved components.
pub type TileData = Arc<Vec<u16>>;

/// An image split into 64×64 tiles; unallocated tiles read as zero. Cloning shares the directory
/// with one `Arc` bump, independent of tile count; pixel data remains separately copy-on-write.
#[derive(Clone, Debug, PartialEq)]
pub struct TileImage {
    width: u32,
    height: u32,
    comps: usize,
    tiles_x: u32,
    tiles_y: u32,
    tiles: DenseTileDir<TileData>,
}

impl TileImage {
    /// `comps` is clamped to 1..=4; sizes to at least 1.
    pub fn new(width: u32, height: u32, comps: usize) -> TileImage {
        let (width, height) = (width.max(1), height.max(1));
        let (tiles_x, tiles_y) = (width.div_ceil(TILE), height.div_ceil(TILE));
        TileImage { width, height, comps: comps.clamp(1, 4), tiles_x, tiles_y, tiles: DenseTileDir::new(tiles_x, tiles_y) }
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

    pub fn tile(&self, tx: u32, ty: u32) -> Option<&[u16]> {
        self.tiles.get(tx as i32, ty as i32).map(|t| t.as_slice())
    }

    /// The shared tile handle (for undo snapshots and change fingerprints).
    pub fn tile_handle(&self, tx: u32, ty: u32) -> Option<&TileData> {
        self.tiles.get(tx as i32, ty as i32)
    }

    /// Replace a tile wholesale (None frees it).
    pub fn set_tile(&mut self, tx: u32, ty: u32, data: Option<TileData>) {
        match data.filter(|d| d.len() == TILE_AREA * self.comps) {
            Some(data) => self.tiles.insert(tx as i32, ty as i32, data),
            None => self.tiles.remove(tx as i32, ty as i32),
        };
    }

    /// Mutable texels of a tile, allocating it filled with `fill` when absent and un-sharing it when shared.
    pub fn tile_mut(&mut self, tx: u32, ty: u32, fill: &[u16]) -> Option<&mut [u16]> {
        let comps = self.comps;
        let tile = self.tiles.get_or_insert_with(tx as i32, ty as i32, || {
            let mut v = vec![0u16; TILE_AREA * comps];
            if fill.iter().any(|f| *f != 0) {
                v.chunks_exact_mut(comps).for_each(|px| px.iter_mut().zip(fill).for_each(|(d, s)| *d = *s));
            }
            Arc::new(v)
        })?;
        Some(Arc::make_mut(tile).as_mut_slice())
    }

    pub fn is_allocated(&self, tx: u32, ty: u32) -> bool {
        self.tiles.contains_key(tx as i32, ty as i32)
    }

    /// Allocated tile coordinates in row-major `(ty, tx)` order.
    pub fn allocated(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.tiles.iter_row_major().map(|((tx, ty), _)| (tx as u32, ty as u32))
    }

    pub fn allocated_count(&self) -> usize {
        self.tiles.len()
    }

    /// Components of the texel at (x, y); zeros when out of range or unallocated.
    pub fn get(&self, x: u32, y: u32) -> [u16; 4] {
        let mut out = [0u16; 4];
        if let Some(t) = (x < self.width && y < self.height).then(|| self.tile(x / TILE, y / TILE)).flatten() {
            let o = (((y % TILE) * TILE + x % TILE) as usize) * self.comps;
            out.iter_mut().take(self.comps).enumerate().for_each(|(c, v)| *v = t.get(o + c).copied().unwrap_or(0));
        }
        out
    }

    /// Set one texel (allocating its tile with zeros).
    pub fn set(&mut self, x: u32, y: u32, v: &[u16]) {
        let comps = self.comps;
        if let Some(t) = (x < self.width && y < self.height).then(|| self.tile_mut(x / TILE, y / TILE, &[])).flatten() {
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
        self.tiles.retain(|_, data| data.iter().any(|value| *value != 0));
    }

    /// A bilinear resample to `width × height` (same component count).
    pub fn resampled(&self, width: u32, height: u32, empty: &[u16]) -> TileImage {
        let mut out = TileImage::new(width, height, self.comps);
        if self.allocated_count() > 0 {
            for ty in 0..out.tiles_y {
                for tx in 0..out.tiles_x {
                    if let Some(data) = self.resampled_tile(width, height, tx, ty, empty) {
                        out.set_tile(tx, ty, Some(Arc::new(data)));
                    }
                }
            }
        }
        out
    }

    /// Tile (tx, ty) of [`TileImage::resampled`] alone (None when its source footprint is entirely unallocated).
    pub fn resampled_tile(&self, width: u32, height: u32, tx: u32, ty: u32, empty: &[u16]) -> Option<Vec<u16>> {
        let (sw, sh, dw, dh, comps) = (self.width as f32, self.height as f32, width.max(1), height.max(1), self.comps);
        let (x0, y0) = ((tx * TILE) as f32 * sw / dw as f32, (ty * TILE) as f32 * sh / dh as f32);
        let (x1, y1) = (((tx + 1) * TILE) as f32 * sw / dw as f32, ((ty + 1) * TILE) as f32 * sh / dh as f32);
        let any = ((y0 as u32).saturating_sub(1) / TILE..=((y1 as u32 + 1) / TILE).min(self.tiles_y.saturating_sub(1))).any(|sy| {
            ((x0 as u32).saturating_sub(1) / TILE..=((x1 as u32 + 1) / TILE).min(self.tiles_x.saturating_sub(1))).any(|sx| self.is_allocated(sx, sy))
        });
        if !any || self.width == 0 || self.height == 0 {
            return None;
        }
        let mut data = vec![0u16; TILE_AREA * comps];
        for ly in 0..TILE.min(dh.saturating_sub(ty * TILE)) {
            let fy = ((ty * TILE + ly) as f32 + 0.5) * sh / dh as f32;
            for lx in 0..TILE.min(dw.saturating_sub(tx * TILE)) {
                let (fx, o) = (((tx * TILE + lx) as f32 + 0.5) * sw / dw as f32, ((ly * TILE + lx) as usize) * comps);
                if let Some(out) = data.get_mut(o..o + comps) {
                    self.bilinear(fx, fy, empty, out);
                }
            }
        }
        Some(data)
    }

    fn bilinear(&self, fx: f32, fy: f32, empty: &[u16], out: &mut [u16]) -> bool {
        let (fx, fy) = ((fx - 0.5).max(0.0), (fy - 0.5).max(0.0));
        let (ix, wx, iy, wy, mut any) = (fx.floor() as u32, fx.fract(), fy.floor() as u32, fy.fract(), false);
        let mut get = |x: u32, y: u32| {
            let (x, y) = (x.min(self.width.saturating_sub(1)), y.min(self.height.saturating_sub(1)));
            if self.is_allocated(x / TILE, y / TILE) {
                any = true;
                self.get(x, y)
            } else {
                let mut e = [0u16; 4];
                e.iter_mut().zip(empty).for_each(|(d, s)| *d = *s);
                e
            }
        };
        let (a, b, c, d) = (get(ix, iy), get(ix + 1, iy), get(ix, iy + 1), get(ix + 1, iy + 1));
        for (k, slot) in out.iter_mut().enumerate().take(4) {
            let (top, bot) = (a[k] as f32 * (1.0 - wx) + b[k] as f32 * wx, c[k] as f32 * (1.0 - wx) + d[k] as f32 * wx);
            *slot = (top * (1.0 - wy) + bot * wy + 0.5) as u16;
        }
        any
    }

    /// Whether a bilinear read anywhere in the texel box `(x0, y0)..(x1, y1)` can touch an allocated tile.
    pub fn any_allocated_in(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let span = |lo: f32, hi: f32, n: u32| ((lo - 1.0).max(0.0) as u32 / TILE, ((hi + 1.0).max(0.0) as u32 / TILE).min(n.saturating_sub(1)));
        let ((ax, bx), (ay, by)) = (span(x0.min(x1), x0.max(x1), self.tiles_x), span(y0.min(y1), y0.max(y1), self.tiles_y));
        (ay..=by).any(|sy| (ax..=bx).any(|sx| self.is_allocated(sx, sy)))
    }

    /// Tile (tx, ty) of a `width × height` image resampled through `map`.
    pub fn remapped_tile(&self, width: u32, height: u32, tx: u32, ty: u32, empty: &[u16], map: &dyn Fn(f32, f32) -> Option<(f32, f32)>) -> Option<Vec<u16>> {
        if self.allocated_count() == 0 || self.width == 0 || self.height == 0 {
            return None;
        }
        let (comps, mut data, mut any) = (self.comps, vec![0u16; TILE_AREA * self.comps], false);
        for ly in 0..TILE.min(height.saturating_sub(ty * TILE)) {
            for lx in 0..TILE.min(width.saturating_sub(tx * TILE)) {
                let o = ((ly * TILE + lx) as usize) * comps;
                let Some(out) = data.get_mut(o..o + comps) else { continue };
                match map((tx * TILE + lx) as f32 + 0.5, (ty * TILE + ly) as f32 + 0.5) {
                    Some((sx, sy)) if sx.is_finite() && sy.is_finite() => any |= self.bilinear(sx, sy, empty, out),
                    _ => out.iter_mut().zip(empty).for_each(|(d, s)| *d = *s),
                }
            }
        }
        any.then_some(data)
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
    fn directory_snapshots_only_copy_the_written_chunk() {
        let mut image = TileImage::new(65 * TILE, TILE, 1);
        for tx in 0..65 {
            image.tile_mut(tx, 0, &[7]).unwrap();
        }
        let snapshot = image.clone();
        assert!(image.tiles.ptr_eq(&snapshot.tiles));
        assert_eq!((image.tiles.chunk_len(0), image.tiles.chunk_len(1)), (Some(64), Some(1)));
        image.tile_mut(63, 0, &[]).unwrap()[0] = 9;
        assert!(!image.tiles.chunk_ptr_eq(&snapshot.tiles, 0) && image.tiles.chunk_ptr_eq(&snapshot.tiles, 1));
        assert!(Arc::ptr_eq(image.tile_handle(62, 0).unwrap(), snapshot.tile_handle(62, 0).unwrap()));
        image.set_tile(64, 0, None);
        assert_eq!((snapshot.tile(63, 0).unwrap()[0], snapshot.tile(64, 0).unwrap()[0], image.allocated().last()), (7, 7, Some((63, 0))));
    }

    #[test]
    fn no_op_pruning_and_invalid_writes_keep_directory_shared() {
        let mut image = TileImage::new(65 * TILE, TILE, 1);
        image.tile_mut(64, 0, &[7]).unwrap();
        let snapshot = image.clone();
        image.prune_empty();
        image.set_tile(65, 0, None);
        assert!(image.tile_mut(0, 1, &[]).is_none() && image.tiles.ptr_eq(&snapshot.tiles));
        image.tile_mut(64, 0, &[]).unwrap().fill(0);
        image.prune_empty();
        assert_eq!((image.allocated_count(), snapshot.allocated_count()), (0, 1));
    }

    #[test]
    fn directory_debug_and_equality_keep_flat_value_semantics() {
        let (mut directory, mut dense) = (DenseTileDir::new(65, 1), vec![None; 65]);
        dense[64] = Some(Arc::new(vec![7u16]));
        directory.insert(64, 0, dense[64].clone().unwrap());
        assert_eq!(format!("{directory:?}"), format!("{dense:?}"));
        let mut independent = DenseTileDir::new(65, 1);
        independent.insert(64, 0, Arc::new(vec![7u16]));
        assert_eq!(directory, independent);
        independent.remove(64, 0);
        assert_ne!(directory, independent);
    }

    #[test]
    fn unallocated_tiles_read_zero_and_writes_allocate() {
        let mut img = TileImage::new(200, 130, 2);
        assert_eq!((img.tiles_x(), img.tiles_y(), img.get(10, 10)), (4, 3, [0; 4]));
        img.set(199, 129, &[7, 9]);
        assert_eq!((img.get(199, 129), img.allocated_count(), img.allocated().collect::<Vec<_>>()), ([7, 9, 0, 0], 1, vec![(3, 2)]));
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
        assert_eq!((b.get(1, 0)[0], a.get(1, 0)[0]), (0, 6));
    }

    #[test]
    fn tile_mut_fills_new_tiles() {
        let mut img = TileImage::new(64, 64, 2);
        assert_eq!(&img.tile_mut(0, 0, &[100, 200]).unwrap()[..4], &[100, 200, 100, 200]);
        img.prune_empty();
        assert_eq!(img.allocated_count(), 1);
        img.set_tile(0, 0, Some(Arc::new(vec![0; TILE_AREA * 2])));
        img.prune_empty();
        assert_eq!(img.allocated_count(), 0);
    }

    #[test]
    fn unorm_round_trips() {
        for v in [0.0, 0.25, 0.5, 1.0] {
            assert!((from_u16(to_u16(v)) - v).abs() < 1e-4 && (from_u16_signed(to_u16_signed(v * 2.0 - 1.0)) - (v * 2.0 - 1.0)).abs() < 1e-4);
        }
        assert_eq!(to_u16(f32::NAN), 0);
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
        assert_eq!((up.get(40, 40)[..2].try_into().unwrap(), up.is_allocated(3, 3)), ([30000, 65535], false));
        assert_eq!(img.resampled(64, 64, &[]).get(10, 10)[..2], [30000, 65535]);
    }

    #[test]
    fn remap_moves_texels_and_keeps_sparsity() {
        let mut img = TileImage::new(128, 128, 2);
        img.set(10, 20, &[1000, 65535]);
        let map = |x: f32, y: f32| (x < 256.0).then(|| (128.0 - x / 2.0, y / 2.0));
        assert!(img.remapped_tile(256, 256, 0, 0, &[0, 0], &map).is_none());
        assert!(img.remapped_tile(256, 256, 3, 0, &[0, 0], &map).unwrap()[((41 * TILE + (234 - 192)) * 2) as usize] > 500);
    }
}
