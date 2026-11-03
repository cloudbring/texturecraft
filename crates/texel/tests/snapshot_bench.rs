//! Run alone in release mode with `--ignored --nocapture --test-threads=1`.
//! The dense model preserves the old Vec directory, Arc payloads and tile mutation path.

use std::{
    alloc::System,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

use stats_alloc::{INSTRUMENTED_SYSTEM, Region, Stats, StatsAlloc};
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
struct DenseImage {
    tiles: Vec<Option<TileData>>,
}

impl Image for DenseImage {
    fn allocated() -> Self {
        Self { tiles: (0..TILE_COUNT).map(|_| Some(Arc::new(vec![1; TILE_AREA * COMPONENTS]))).collect() }
    }

    fn rewrite(&mut self, index: usize, value: u16) {
        let (tx, ty) = (index as u32 % COLUMNS, index as u32 / COLUMNS);
        let index = (tx < COLUMNS && ty < COLUMNS).then(|| (ty * COLUMNS + tx) as usize).unwrap();
        let tile = self.tiles.get_mut(index).unwrap().as_mut().unwrap();
        Arc::make_mut(tile).fill(value);
    }

    fn value(&self, index: usize) -> u16 {
        self.tiles[index].as_ref().unwrap()[0]
    }
}

impl Image for TileImage {
    fn allocated() -> Self {
        let mut image = Self::new(SIDE, SIDE, COMPONENTS);
        for ty in 0..COLUMNS {
            for tx in 0..COLUMNS {
                image.tile_mut(tx, ty, &[1; COMPONENTS]).unwrap();
            }
        }
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

#[derive(Clone, Copy)]
enum Workload {
    Snapshots,
    History,
    Dense,
}

fn measure<ImageType: Image>(workload: Workload) -> (Duration, Stats) {
    let mut image = ImageType::allocated();
    let mut history = Vec::with_capacity(HISTORY);
    let region = Region::new(ALLOCATOR);
    let start = Instant::now();
    match workload {
        Workload::Snapshots => {
            for _ in 0..HISTORY {
                history.push(black_box(image.clone()));
            }
        }
        Workload::History => {
            for step in 0..HISTORY {
                history.push(black_box(image.clone()));
                image.rewrite(step * 67 % TILE_COUNT, step as u16 + 2);
            }
        }
        Workload::Dense => {
            history.push(black_box(image.clone()));
            for index in 0..TILE_COUNT {
                image.rewrite(index, 2);
            }
        }
    }
    let elapsed = start.elapsed();
    let stats = region.change();
    black_box((&image, &history));
    assert_eq!(history[0].value(0), 1);
    match workload {
        Workload::Snapshots => assert_eq!(image.value(0), 1),
        Workload::History => {
            for step in 0..HISTORY {
                let index = step * 67 % TILE_COUNT;
                assert_eq!(history[step].value(index), 1);
                assert_eq!(image.value(index), step as u16 + 2);
            }
        }
        Workload::Dense => {
            for index in 0..TILE_COUNT {
                assert_eq!(image.value(index), 2);
                assert_eq!(history[0].value(index), 1);
            }
        }
    }
    (elapsed, stats)
}

#[test]
#[ignore = "allocation-counted 4K RGBA snapshot benchmark; run alone in release mode"]
fn snapshots_and_edits_before_after() {
    println!("4096x4096 RGBA u16, 4096 separately allocated 64x64 tiles; median of {REPEATS}, excluding setup/drop; retained history");
    println!("workload | Vec ms | chunks ms | Vec allocations | chunks allocations | Vec bytes | chunks bytes");
    for (label, workload) in
        [("200 snapshots", Workload::Snapshots), ("200 one-tile history edits", Workload::History), ("one full-image history edit", Workload::Dense)]
    {
        black_box(measure::<DenseImage>(workload));
        black_box(measure::<TileImage>(workload));
        let mut before = Vec::with_capacity(REPEATS);
        let mut after = Vec::with_capacity(REPEATS);
        for repeat in 0..REPEATS {
            if repeat % 2 == 0 {
                before.push(measure::<DenseImage>(workload));
                after.push(measure::<TileImage>(workload));
            } else {
                after.push(measure::<TileImage>(workload));
                before.push(measure::<DenseImage>(workload));
            }
        }
        for samples in [&before, &after] {
            for (_, stats) in samples {
                assert_eq!(stats.allocations, samples[0].1.allocations);
                assert_eq!(stats.bytes_allocated, samples[0].1.bytes_allocated);
                assert_eq!(stats.reallocations, 0);
            }
        }
        before.sort_by_key(|sample| sample.0);
        after.sort_by_key(|sample| sample.0);
        let (old_time, old_stats) = before[REPEATS / 2];
        let (new_time, new_stats) = after[REPEATS / 2];
        if matches!(workload, Workload::Snapshots) {
            assert_eq!(old_stats.allocations, HISTORY);
            assert_eq!(new_stats.allocations, 0);
        }
        println!(
            "{label} | {:.6} | {:.6} | {} | {} | {} | {}",
            old_time.as_secs_f64() * 1000.0,
            new_time.as_secs_f64() * 1000.0,
            old_stats.allocations,
            new_stats.allocations,
            old_stats.bytes_allocated,
            new_stats.bytes_allocated
        );
    }
}
