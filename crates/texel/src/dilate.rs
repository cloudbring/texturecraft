//! UV padding: fill uncovered texels from their nearest covered texel.

/// No source texel.
pub const NONE: u32 = u32::MAX;

/// Distance used to select a covered texel and to apply the padding radius.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaddingMetric {
    /// Exact Euclidean distance (squared distances are compared without rounding).
    #[default]
    Euclidean,
    /// Eight-neighbour breadth-first expansion, preserving legacy source ties.
    Chebyshev,
}

/// For every texel, the index of the covered texel whose value it takes (itself when covered,
/// [`NONE`] when beyond the padding distance). Computed once per UV layout and reused for every
/// dilation of that texture set.
#[derive(Clone, Debug, PartialEq)]
pub struct DilationMap {
    pub width: u32,
    pub height: u32,
    pub src: Vec<u32>,
}

impl DilationMap {
    /// `covered.len()` must be `width × height` (shorter input is treated as uncovered).
    /// `max_dist` limits Euclidean padding in texels; `None` fills everything reachable.
    pub fn new(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>) -> DilationMap {
        DilationMap::in_blocks(covered, width, height, max_dist, 0)
    }

    /// [`DilationMap::new`] that never pads across the borders of `block × block` squares (the
    /// UV tiles of an atlas); 0 means one block.
    pub fn in_blocks(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, block: u32) -> DilationMap {
        Self::in_blocks_with_metric(covered, width, height, max_dist, block, PaddingMetric::Euclidean)
    }

    /// Select the distance metric for a layout without atlas blocks.
    pub fn with_metric(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, metric: PaddingMetric) -> DilationMap {
        Self::in_blocks_with_metric(covered, width, height, max_dist, 0, metric)
    }

    /// Select the distance metric, isolating each `block × block` atlas tile.
    pub fn in_blocks_with_metric(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, block: u32, metric: PaddingMetric) -> DilationMap {
        match metric {
            PaddingMetric::Euclidean => Self::euclidean(covered, width, height, max_dist, block),
            PaddingMetric::Chebyshev => Self::chebyshev(covered, width, height, max_dist, block),
        }
    }

    fn euclidean(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, block: u32) -> DilationMap {
        let n = (width as usize).saturating_mul(height as usize);
        let mut src = vec![NONE; n];
        if width == 0 || height == 0 {
            return DilationMap { width, height, src };
        }
        let stride = width as usize;
        let block = if block == 0 { width.max(height) } else { block } as usize;
        let len = stride.max(height as usize);
        let mut costs = vec![u64::MAX; len];
        let mut sources = vec![NONE; len];
        let mut candidates = Vec::with_capacity(len);
        let mut starts = Vec::with_capacity(len);
        let mut argmins = Vec::with_capacity(len);
        let limit_squared = max_dist.map(|radius| u64::from(radius) * u64::from(radius));

        for block_y in (0..height as usize).step_by(block) {
            let end_y = (block_y + block).min(height as usize);
            for block_x in (0..stride).step_by(block) {
                let end_x = (block_x + block).min(stride);
                let row_len = end_x - block_x;
                let column_len = end_y - block_y;
                for y in block_y..end_y {
                    for x in block_x..end_x {
                        let index = y * stride + x;
                        let offset = x - block_x;
                        costs[offset] = if covered.get(index).copied().unwrap_or(false) { 0 } else { u64::MAX };
                        sources[offset] = index as u32;
                    }
                    lower_envelope(&costs[..row_len], &mut candidates, &mut starts, &mut argmins);
                    for x in block_x..end_x {
                        let offset = x - block_x;
                        src[y * stride + x] = argmins.get(offset).map_or(NONE, |&seed| sources[seed]);
                    }
                }
                for x in block_x..end_x {
                    for y in block_y..end_y {
                        let offset = y - block_y;
                        let source = src[y * stride + x];
                        sources[offset] = source;
                        costs[offset] = if source == NONE {
                            u64::MAX
                        } else {
                            let dx = x.abs_diff(source as usize % stride) as u64;
                            dx * dx
                        };
                    }
                    lower_envelope(&costs[..column_len], &mut candidates, &mut starts, &mut argmins);
                    for y in block_y..end_y {
                        let offset = y - block_y;
                        src[y * stride + x] = match argmins.get(offset) {
                            Some(&seed) => {
                                let dy = offset.abs_diff(seed) as u64;
                                if limit_squared.is_some_and(|limit| costs[seed] + dy * dy > limit) { NONE } else { sources[seed] }
                            }
                            None => NONE,
                        };
                    }
                }
            }
        }
        DilationMap { width, height, src }
    }

    fn chebyshev(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, block: u32) -> DilationMap {
        let n = (width as usize).saturating_mul(height as usize);
        let mut src = vec![NONE; n];
        let bsz = if block == 0 { u32::MAX } else { block };
        let is_covered = |i: usize| covered.get(i).copied().unwrap_or(false);
        // Breadth first, one distance at a time; only island borders seed it (interior texels
        // have nothing to spread to, so the result is the same at a fraction of the work).
        let mut frontier = Vec::new();
        for i in 0..n {
            if is_covered(i) {
                if let Some(s) = src.get_mut(i) {
                    *s = i as u32;
                }
                if neighbours(i, width, height, bsz).any(|j| !is_covered(j)) {
                    frontier.push(i);
                }
            }
        }
        let limit = max_dist.unwrap_or(u32::MAX);
        let mut d = 0;
        while d < limit && !frontier.is_empty() {
            let mut next = Vec::new();
            for &i in &frontier {
                let from = src.get(i).copied().unwrap_or(NONE);
                for j in neighbours(i, width, height, bsz) {
                    if let Some(s) = src.get_mut(j).filter(|s| **s == NONE) {
                        *s = from;
                        next.push(j);
                    }
                }
            }
            frontier = next;
            d += 1;
        }
        DilationMap { width, height, src }
    }

    /// Copy padded values in place: `data` holds `comps` components per texel. Texels with no
    /// source keep their value.
    pub fn apply<T: Copy>(&self, data: &mut [T], comps: usize) {
        for (i, &s) in self.src.iter().enumerate() {
            let s = s as usize;
            if s == i || s == NONE as usize {
                continue;
            }
            for c in 0..comps {
                if let Some(v) = data.get(s * comps + c).copied()
                    && let Some(d) = data.get_mut(i * comps + c)
                {
                    *d = v;
                }
            }
        }
    }

    /// The source texel of (x, y), if any.
    pub fn source(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let s = *self.src.get((y * self.width + x) as usize)?;
        (s != NONE).then(|| (s % self.width, s / self.width))
    }
}

/// One-dimensional squared-distance lower envelope; returns the minimizing input slot for each
/// integer position. Equal distances retain the earlier parabola.
fn lower_envelope(costs: &[u64], candidates: &mut Vec<usize>, starts: &mut Vec<usize>, argmins: &mut Vec<usize>) {
    candidates.clear();
    starts.clear();
    argmins.clear();
    for (position, &cost) in costs.iter().enumerate() {
        if cost == u64::MAX {
            continue;
        }
        let start = loop {
            let Some(&previous) = candidates.last() else { break 0 };
            let numerator = (cost as i128 + (position as i128).pow(2)) - (costs[previous] as i128 + (previous as i128).pow(2));
            let denominator = 2 * (position - previous) as i128;
            let crossing = numerator.div_euclid(denominator) + 1;
            if starts.last().is_some_and(|&begin| crossing <= begin as i128) {
                candidates.pop();
                starts.pop();
            } else {
                break crossing.max(0) as usize;
            }
        };
        if start < costs.len() {
            candidates.push(position);
            starts.push(start);
        }
    }
    if candidates.is_empty() {
        return;
    }
    let mut active = 0;
    for position in 0..costs.len() {
        while active + 1 < candidates.len() && starts[active + 1] <= position {
            active += 1;
        }
        argmins.push(candidates[active]);
    }
}

/// The 8-neighbours of texel `i` of a `w × h` image inside the image and inside its `bsz`
/// square block.
fn neighbours(i: usize, w: u32, h: u32, bsz: u32) -> impl Iterator<Item = usize> {
    let (wu, x, y) = (w as usize, (i % w.max(1) as usize) as u32, (i / w.max(1) as usize) as u32);
    let left = x > 0 && x % bsz != 0;
    let right = x + 1 < w && (x + 1) % bsz != 0;
    let up = y > 0 && y % bsz != 0;
    let down = y + 1 < h && (y + 1) % bsz != 0;
    [
        (left, i.wrapping_sub(1)),
        (right, i + 1),
        (up, i.wrapping_sub(wu)),
        (down, i + wu),
        (left && up, i.wrapping_sub(wu + 1)),
        (right && up, i.wrapping_sub(wu).wrapping_add(1)),
        (left && down, (i + wu).wrapping_sub(1)),
        (right && down, i + wu + 1),
    ]
    .into_iter()
    .filter_map(|(ok, j)| ok.then_some(j))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncovered_texels_take_the_nearest_covered_value() {
        // 5×1 strip, only texel 1 covered.
        let covered = [false, true, false, false, false];
        let m = DilationMap::new(&covered, 5, 1, Some(2));
        assert_eq!(m.src, vec![1, 1, 1, 1, NONE]);
        let mut data = [0u8, 9, 0, 0, 0];
        m.apply(&mut data, 1);
        assert_eq!(data, [9, 9, 9, 9, 0]);
    }

    #[test]
    fn infinite_padding_fills_everything_and_keeps_covered_values() {
        let covered = [true, false, false, false, false, false, false, false, true];
        let m = DilationMap::new(&covered, 3, 3, None);
        assert!(m.src.iter().all(|s| *s != NONE));
        assert_eq!(m.src[0], 0);
        assert_eq!(m.src[8], 8);
        assert_eq!(m.source(2, 2), Some((2, 2)));
        assert_eq!(m.source(9, 9), None);
    }

    #[test]
    fn blocks_never_pad_across_their_borders() {
        // 4×1 strip of two 2-texel blocks, only texel 0 covered.
        let covered = [true, false, false, false];
        assert_eq!(DilationMap::in_blocks(&covered, 4, 1, None, 2).src, vec![0, 0, NONE, NONE]);
        assert_eq!(DilationMap::new(&covered, 4, 1, None).src, vec![0; 4]);
    }

    /// The plain multi-source BFS (every covered texel seeds, one queue).
    fn reference(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>, block: u32) -> Vec<u32> {
        let n = (width * height) as usize;
        let (mut src, mut dist) = (vec![NONE; n], vec![u32::MAX; n]);
        let mut queue = std::collections::VecDeque::new();
        for i in (0..n).filter(|i| covered[*i]) {
            src[i] = i as u32;
            dist[i] = 0;
            queue.push_back(i);
        }
        let (w, h, b) = (width as i64, height as i64, if block == 0 { i64::MAX } else { block as i64 });
        while let Some(i) = queue.pop_front() {
            if dist[i] >= max_dist.unwrap_or(u32::MAX) {
                continue;
            }
            let (x, y) = (i as i64 % w, i as i64 / w);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h || nx / b != x / b || ny / b != y / b {
                    continue;
                }
                let j = (ny * w + nx) as usize;
                if dist[j] == u32::MAX {
                    dist[j] = dist[i] + 1;
                    src[j] = src[i];
                    queue.push_back(j);
                }
            }
        }
        src
    }

    #[test]
    fn matches_the_plain_breadth_first_search() {
        let (w, h) = (40, 24);
        let mut seed = 7u32;
        let covered: Vec<bool> = (0..w * h)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                // Blobs: a coarse random field.
                let (x, y) = (i % w, i / w);
                (x / 5 + y / 3 + (seed >> 29)).is_multiple_of(4)
            })
            .collect();
        for (max_dist, block) in [(None, 0), (Some(3), 0), (Some(2), 8), (None, 16), (Some(1), 5)] {
            assert_eq!(
                DilationMap::in_blocks_with_metric(&covered, w, h, max_dist, block, PaddingMetric::Chebyshev).src,
                reference(&covered, w, h, max_dist, block),
                "{max_dist:?} {block}"
            );
        }
    }

    fn check_euclidean(covered: &[bool], width: u32, height: u32, radius: Option<u32>, block: u32) {
        let map = DilationMap::in_blocks_with_metric(covered, width, height, radius, block, PaddingMetric::Euclidean);
        let stride = width as usize;
        for (index, &source) in map.src.iter().enumerate() {
            let x = index % stride;
            let y = index / stride;
            let best = (0..map.src.len())
                .filter(|&seed| {
                    covered.get(seed).copied().unwrap_or(false)
                        && (block == 0 || (seed % stride / block as usize == x / block as usize && seed / stride / block as usize == y / block as usize))
                })
                .map(|seed| {
                    let dx = x.abs_diff(seed % stride) as u64;
                    let dy = y.abs_diff(seed / stride) as u64;
                    dx * dx + dy * dy
                })
                .min()
                .filter(|distance| radius.is_none_or(|r| *distance <= u64::from(r) * u64::from(r)));
            match best {
                None => assert_eq!(source, NONE, "at {x},{y}"),
                Some(distance) => {
                    let seed = source as usize;
                    assert!(covered.get(seed).copied().unwrap_or(false), "invalid source at {x},{y}");
                    let dx = x.abs_diff(seed % stride) as u64;
                    let dy = y.abs_diff(seed / stride) as u64;
                    assert_eq!(dx * dx + dy * dy, distance, "at {x},{y}");
                    if block > 0 {
                        assert_eq!((seed % stride / block as usize, seed / stride / block as usize), (x / block as usize, y / block as usize));
                    }
                }
            }
        }
    }

    #[test]
    fn euclidean_matches_brute_force_on_random_masks() {
        let mut state = 20u32;
        for (width, height) in [(2, 9), (9, 2), (7, 6), (11, 13)] {
            for _ in 0..40 {
                let mask: Vec<bool> = (0..width * height)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        state >> 28 < 3
                    })
                    .collect();
                for (radius, block) in [(None, 0), (Some(0), 0), (Some(2), 0), (Some(3), 4), (None, 3), (None, 1)] {
                    check_euclidean(&mask, width, height, radius, block);
                }
            }
        }
    }

    #[test]
    fn euclidean_handles_empty_full_single_and_thin_islands() {
        check_euclidean(&[], 7, 6, None, 0);
        check_euclidean(&[true; 42], 7, 6, Some(0), 0);
        let mut mask = [false; 42];
        mask[8] = true;
        check_euclidean(&mask, 7, 6, Some(3), 0);
        check_euclidean(&mask, 7, 6, None, 1);
        check_euclidean(&mask, 7, 6, None, 7);
        check_euclidean(&mask, 7, 6, None, 2);
        check_euclidean(&[], 0, 6, None, 0);
    }

    #[test]
    fn euclidean_differs_from_chebyshev_near_diagonal_seams() {
        let mut mask = [false; 25];
        mask[6] = true;
        assert_eq!(DilationMap::with_metric(&mask, 5, 5, Some(2), PaddingMetric::Euclidean).src[18], NONE);
        assert_eq!(DilationMap::with_metric(&mask, 5, 5, Some(2), PaddingMetric::Chebyshev).src[18], 6);
        mask[16] = true;
        let euclidean = DilationMap::new(&mask, 5, 5, None);
        let chebyshev = DilationMap::with_metric(&mask, 5, 5, None, PaddingMetric::Chebyshev);
        assert_eq!(euclidean.src[18], 16);
        assert_eq!(chebyshev.src[18], 6);
    }

    #[test]
    fn nothing_covered_is_a_no_op() {
        let m = DilationMap::new(&[], 4, 4, None);
        let mut data = [3u16; 16];
        m.apply(&mut data, 1);
        assert_eq!(data, [3u16; 16]);
    }

    #[test]
    #[ignore = "4096² timing benchmark; run with --ignored --nocapture"]
    fn bench_large_padding() {
        let size = 4096u32;
        let covered: Vec<bool> = (0..size * size).map(|i| (i % size / 64 + i / size / 64).is_multiple_of(3)).collect();
        for metric in [PaddingMetric::Chebyshev, PaddingMetric::Euclidean] {
            let start = std::time::Instant::now();
            let map = DilationMap::in_blocks_with_metric(&covered, size, size, None, size, metric);
            println!("4096² {metric:?}: {:.2} ms, {} sources", start.elapsed().as_secs_f64() * 1000.0, map.src.len());
        }
    }
}
