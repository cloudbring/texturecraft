//! Run alone in release mode with `--ignored --nocapture --test-threads=1`.
//! Compares a flat `Vec<Option<TileData>>` directory against `TileImage` (`craft_tiledir::DenseTileDir`).

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};
use std::alloc::System;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};
use texturecraft_texel::{TILE, TILE_AREA, TileData, TileImage};

#[global_allocator]
static ALLOCATOR: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

const SIDE: u32 = 4096;
const COLUMNS: u32 = SIDE / TILE;
const TILE_COUNT: usize = (COLUMNS * COLUMNS) as usize;
const COMPONENTS: usize = 4;
const HISTORY: usize = 200;
const REPEATS: usize = 9;

trait Image: Clone {
    fn allocated() -> Self;
    fn rewrite(&mut self, index: usize, value: u16);
    fn value(&self, index: usize) -> u16;
}

#[derive(Clone)]
struct DenseImage(Vec<Option<TileData>>);

impl Image for DenseImage {
    fn allocated() -> Self {
        Self((0..TILE_COUNT).map(|_| Some(Arc::new(vec![1; TILE_AREA * COMPONENTS]))).collect())
    }
    fn rewrite(&mut self, index: usize, value: u16) {
        Arc::make_mut(self.0[index].as_mut().unwrap()).fill(value);
    }
    fn value(&self, index: usize) -> u16 {
        self.0[index].as_ref().unwrap()[0]
    }
}

impl Image for TileImage {
    fn allocated() -> Self {
        let mut image = Self::new(SIDE, SIDE, COMPONENTS);
        (0..TILE_COUNT).for_each(|i| _ = image.tile_mut(i as u32 % COLUMNS, i as u32 / COLUMNS, &[1; COMPONENTS]).unwrap());
        assert_eq!(image.allocated_count(), TILE_COUNT);
        image
    }
    fn rewrite(&mut self, index: usize, value: u16) {
        self.tile_mut(index as u32 % COLUMNS, index as u32 / COLUMNS, &[]).unwrap().fill(value);
    }
    fn value(&self, index: usize) -> u16 {
        self.tile(index as u32 % COLUMNS, index as u32 / COLUMNS).unwrap()[0]
    }
}

fn measure<I: Image>(mode: u8) -> (Duration, Stats) {
    let (mut image, mut history) = (I::allocated(), Vec::with_capacity(HISTORY));
    let (region, start) = (Region::new(ALLOCATOR), Instant::now());
    match mode {
        0 => (0..HISTORY).for_each(|_| history.push(black_box(image.clone()))),
        1 => (0..HISTORY).for_each(|s| {
            history.push(black_box(image.clone()));
            image.rewrite(s * 67 % TILE_COUNT, s as u16 + 2);
        }),
        _ => {
            history.push(black_box(image.clone()));
            (0..TILE_COUNT).for_each(|i| image.rewrite(i, 2));
        }
    }
    let (elapsed, stats) = (start.elapsed(), region.change());
    black_box((&image, &history));
    assert_eq!(history[0].value(0), 1);
    match mode {
        0 => assert_eq!(image.value(0), 1),
        1 => (0..HISTORY).for_each(|s| assert_eq!((history[s].value(s * 67 % TILE_COUNT), image.value(s * 67 % TILE_COUNT)), (1, s as u16 + 2))),
        _ => (0..TILE_COUNT).for_each(|i| assert_eq!((image.value(i), history[0].value(i)), (2, 1))),
    }
    (elapsed, stats)
}

#[test]
#[ignore = "allocation-counted 4K RGBA snapshot benchmark; run alone in release mode"]
fn snapshots_and_edits_before_after() {
    println!("4096x4096 RGBA u16, 4096 separately allocated 64x64 tiles; median of {REPEATS}, excluding setup/drop; retained history");
    println!("workload | Vec ms | chunks ms | Vec allocations | chunks allocations | Vec bytes | chunks bytes");
    for (label, mode) in [("200 snapshots", 0), ("200 one-tile history edits", 1), ("one full-image history edit", 2)] {
        black_box((measure::<DenseImage>(mode), measure::<TileImage>(mode)));
        let (mut before, mut after) = (Vec::with_capacity(REPEATS), Vec::with_capacity(REPEATS));
        for r in 0..REPEATS {
            let (b, a) = if r % 2 == 0 {
                (measure::<DenseImage>(mode), measure::<TileImage>(mode))
            } else {
                let (a, b) = (measure::<TileImage>(mode), measure::<DenseImage>(mode));
                (b, a)
            };
            before.push(b);
            after.push(a);
        }
        for samples in [&before, &after] {
            for (_, s) in samples {
                assert_eq!((s.allocations, s.bytes_allocated, s.reallocations), (samples[0].1.allocations, samples[0].1.bytes_allocated, 0));
            }
        }
        before.sort_by_key(|s| s.0);
        after.sort_by_key(|s| s.0);
        let ((ot, os), (nt, ns)) = (before[REPEATS / 2], after[REPEATS / 2]);
        assert!(mode != 0 || (os.allocations, ns.allocations) == (HISTORY, 0));
        let (om, nm, oa, na, ob, nb) = (ot.as_secs_f64() * 1e3, nt.as_secs_f64() * 1e3, os.allocations, ns.allocations, os.bytes_allocated, ns.bytes_allocated);
        println!("{label} | {om:.6} | {nm:.6} | {oa} | {na} | {ob} | {nb}");
    }
}
