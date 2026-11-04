//! #188: OLD (`Vec<Option<TileData>>`) vs NEW (`craft-tiledir::DenseTileDir`) tile storage.
//!
//! OLD is `texturecraft-texel` at 4edb8b86bc67f58922d6f16a6489059a78b61a3e (merge-base of
//! `algo/35-persistent-tile-directory` with `origin/main`), linked as `texturecraft-texel-old`.
//! One op script is driven through both by a macro-generated driver; after every op the
//! observable state is compared: allocation order, tile bytes, sharing, texel reads (in and out
//! of range), resamples, remaps, and the undo/redo stacks.
//!
//! Undo is modelled both ways the app does it: a whole-image snapshot (`clone`, swapped back) and
//! a per-tile handle saved with `tile_handle` and put back with `set_tile`.

use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug)]
enum Op {
    Set {
        x: u32,
        y: u32,
        v: [u16; 4],
    },
    /// Allocate (filled) and edit one texel of a tile in place.
    TileMut {
        tx: u32,
        ty: u32,
        fill: [u16; 4],
        idx: usize,
        val: u16,
    },
    /// Replace a tile: `None` frees it, `Some(ok)` supplies full data (or wrong-length data).
    SetTile {
        tx: u32,
        ty: u32,
        kind: SetKind,
    },
    PruneEmpty,
    /// Flood: allocate every tile with `fill`.
    FloodAll {
        fill: [u16; 4],
    },
    Snapshot,
    Undo,
    Redo,
    /// Keep a clone aside; it must not change afterwards.
    KeepClone,
    SaveTile {
        tx: u32,
        ty: u32,
    },
    RestoreTile,
}

#[derive(Clone, Debug)]
enum SetKind {
    Free,
    Full(u16),
    ZeroFull,
    WrongLen,
}

fn op(max_tile: u32) -> impl Strategy<Value = Op> {
    let t = move || 0..max_tile + 2; // includes out-of-range tile coordinates
    let comp = || prop_oneof![Just(0u16), Just(1), Just(65535), any::<u16>()];
    let v4 = move || [comp(), comp(), comp(), comp()];
    prop_oneof![
        4 => (0u32..520, 0u32..420, v4()).prop_map(|(x, y, v)| Op::Set { x, y, v }),
        3 => (t(), t(), v4(), 0usize..16384, comp()).prop_map(|(tx, ty, fill, idx, val)| Op::TileMut { tx, ty, fill, idx, val }),
        2 => (t(), t(), prop_oneof![Just(SetKind::Free), (1u16..9).prop_map(SetKind::Full), Just(SetKind::ZeroFull), Just(SetKind::WrongLen)])
            .prop_map(|(tx, ty, kind)| Op::SetTile { tx, ty, kind }),
        1 => Just(Op::PruneEmpty),
        1 => v4().prop_map(|fill| Op::FloodAll { fill }),
        2 => Just(Op::Snapshot),
        2 => Just(Op::Undo),
        2 => Just(Op::Redo),
        1 => Just(Op::KeepClone),
        1 => (t(), t()).prop_map(|(tx, ty)| Op::SaveTile { tx, ty }),
        1 => Just(Op::RestoreTile),
    ]
}

#[derive(Clone, Debug, PartialEq)]
struct View {
    tiles_xy: (u32, u32),
    /// `allocated()` order.
    order: Vec<(u32, u32)>,
    count: usize,
    bytes: usize,
    /// (tx, ty, digest, sharing class), in `allocated()` order; plus `tile()` for every grid cell.
    tiles: Vec<(u32, u32, u64, usize)>,
    grid: Vec<Option<u64>>,
    gets: Vec<[u16; 4]>,
    resampled: Vec<(usize, u64)>,
    remapped: Vec<Option<u64>>,
    any_in: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq)]
struct Step {
    cur: View,
    undo: Vec<u64>,
    redo: Vec<u64>,
    kept: Vec<u64>,
    kept_equal: Vec<bool>,
    saved: Option<(u32, u32, bool)>,
}

fn fnv(data: impl IntoIterator<Item = u16>) -> u64 {
    data.into_iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ u64::from(v)).wrapping_mul(0x0100_0000_01b3))
}

macro_rules! side {
    ($m:ident, $texel:ident) => {
        mod $m {
            use super::*;
            use $texel::{TILE, TILE_AREA, TileData, TileImage};

            fn digest(img: &TileImage) -> u64 {
                fnv(img.allocated().flat_map(|(tx, ty)| {
                    let mut v = vec![tx as u16, ty as u16];
                    v.extend_from_slice(img.tile(tx, ty).unwrap());
                    v
                }))
            }

            fn view(img: &TileImage) -> View {
                let order: Vec<(u32, u32)> = img.allocated().collect();
                let mut classes: HashMap<usize, usize> = HashMap::new();
                let tiles = order
                    .iter()
                    .map(|&(tx, ty)| {
                        let h = img.tile_handle(tx, ty).unwrap();
                        let next = classes.len();
                        let class = *classes.entry(Arc::as_ptr(h) as usize).or_insert(next);
                        (tx, ty, fnv(h.iter().copied()), class)
                    })
                    .collect();
                let (w, h) = (img.width(), img.height());
                let grid = (0..img.tiles_y() + 1)
                    .flat_map(|ty| (0..img.tiles_x() + 1).map(move |tx| (tx, ty)))
                    .map(|(tx, ty)| img.tile(tx, ty).map(|t| fnv(t.iter().copied())))
                    .collect();
                let pts = [
                    (0, 0),
                    (1, 0),
                    (w.saturating_sub(1), h.saturating_sub(1)),
                    (w / 2, h / 2),
                    (TILE - 1, TILE),
                    (TILE, TILE - 1),
                    (w, h),
                    (w + 5, 0),
                    (0, h + 9),
                ];
                let gets = pts.iter().map(|&(x, y)| img.get(x, y)).collect();
                let comps = img.comps();
                let empty: Vec<u16> = (0..comps as u16).map(|c| 100 + c).collect();
                let mut resampled = vec![];
                for (dw, dh) in [(w / 2 + 1, h / 2 + 1), (w * 2 + 3, h + 7), (w, h)] {
                    let r = img.resampled(dw, dh, &empty);
                    resampled.push((r.allocated_count(), fnv(r.allocated().flat_map(|(tx, ty)| r.tile(tx, ty).unwrap().iter().copied()))));
                }
                let map = |x: f32, y: f32| if (x as u32 + y as u32) % 7 == 0 { None } else { Some((x * 0.5 + 3.0, y * 0.75 - 2.0)) };
                let mut remapped = vec![];
                for ty in 0..img.tiles_y().min(3) {
                    for tx in 0..img.tiles_x().min(3) {
                        remapped.push(img.remapped_tile(w, h, tx, ty, &empty, &map).map(|d| fnv(d.iter().copied())));
                    }
                }
                let any_in =
                    [(0.0, 0.0, 10.0, 10.0), (60.0, 60.0, 70.0, 70.0), (w as f32, h as f32, w as f32 + 100.0, h as f32 + 100.0), (-50.0, -50.0, 1.0, 1.0)]
                        .iter()
                        .map(|&(a, b, c, d)| img.any_allocated_in(a, b, c, d))
                        .collect();
                View {
                    tiles_xy: (img.tiles_x(), img.tiles_y()),
                    order,
                    count: img.allocated_count(),
                    bytes: img.bytes(),
                    tiles,
                    grid,
                    gets,
                    resampled,
                    remapped,
                    any_in,
                }
            }

            pub fn run(dims: (u32, u32, usize), ops: &[Op]) -> Vec<Step> {
                let (w, h, comps) = dims;
                let mut cur = TileImage::new(w, h, comps);
                let (mut undo, mut redo): (Vec<TileImage>, Vec<TileImage>) = (vec![], vec![]);
                let mut kept: Vec<(TileImage, u64)> = vec![];
                let mut saved: Option<(u32, u32, Option<TileData>)> = None;
                let mut out = vec![];
                for op in ops {
                    match op {
                        Op::Set { x, y, v } => cur.set(*x, *y, &v[..comps.clamp(1, 4)]),
                        Op::TileMut { tx, ty, fill, idx, val } => {
                            if let Some(t) = cur.tile_mut(*tx, *ty, &fill[..comps.clamp(1, 4)]) {
                                let len = t.len();
                                t[idx % len] = *val;
                            }
                        }
                        Op::SetTile { tx, ty, kind } => {
                            let n = TILE_AREA * cur.comps();
                            let data = match kind {
                                SetKind::Free => None,
                                SetKind::Full(v) => Some(Arc::new(vec![*v; n])),
                                SetKind::ZeroFull => Some(Arc::new(vec![0; n])),
                                SetKind::WrongLen => Some(Arc::new(vec![3; n - 1])),
                            };
                            cur.set_tile(*tx, *ty, data);
                        }
                        Op::PruneEmpty => cur.prune_empty(),
                        Op::FloodAll { fill } => {
                            for ty in 0..cur.tiles_y() {
                                for tx in 0..cur.tiles_x() {
                                    cur.tile_mut(tx, ty, &fill[..comps.clamp(1, 4)]);
                                }
                            }
                        }
                        Op::Snapshot => {
                            undo.push(cur.clone());
                            redo.clear();
                        }
                        Op::Undo => {
                            if let Some(prev) = undo.pop() {
                                redo.push(std::mem::replace(&mut cur, prev));
                            }
                        }
                        Op::Redo => {
                            if let Some(next) = redo.pop() {
                                undo.push(std::mem::replace(&mut cur, next));
                            }
                        }
                        Op::KeepClone => {
                            let c = cur.clone();
                            let d = digest(&c);
                            kept.push((c, d));
                        }
                        Op::SaveTile { tx, ty } => saved = Some((*tx, *ty, cur.tile_handle(*tx, *ty).cloned())),
                        Op::RestoreTile => {
                            if let Some((tx, ty, h)) = saved.take() {
                                cur.set_tile(tx, ty, h);
                            }
                        }
                    }
                    for (c, d) in &kept {
                        assert_eq!(digest(c), *d, "a kept clone changed after {op:?}");
                    }
                    out.push(Step {
                        cur: view(&cur),
                        undo: undo.iter().map(digest).collect(),
                        redo: redo.iter().map(digest).collect(),
                        kept: kept.iter().map(|(c, _)| digest(c)).collect(),
                        kept_equal: kept.iter().map(|(c, _)| *c == cur).collect(),
                        saved: saved.as_ref().map(|(x, y, h)| (*x, *y, h.is_some())),
                    });
                }
                out
            }
        }
    };
}

side!(old, texturecraft_texel_old);
side!(new, texturecraft_texel);

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Random edit histories leave identical tile storage, for any image size and component count.
    #[test]
    fn random_edit_histories_read_identically(
        w in 1u32..520, h in 1u32..420, comps in 0usize..6,
        ops in prop::collection::vec(op(9), 1..50),
    ) {
        let (a, b) = (old::run((w, h, comps), &ops), new::run((w, h, comps), &ops));
        prop_assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            prop_assert_eq!(x, y, "step {} after {:?}", i, ops[i]);
        }
    }
}

#[test]
fn row_major_iteration_and_wrong_length_replacement_match() {
    let ops = [
        Op::TileMut { tx: 2, ty: 0, fill: [10, 20, 0, 0], idx: 0, val: 11 },
        Op::TileMut { tx: 0, ty: 1, fill: [30, 40, 0, 0], idx: 0, val: 1 },
        Op::TileMut { tx: 1, ty: 0, fill: [50, 60, 0, 0], idx: 0, val: 1 },
        Op::Snapshot,
        Op::SetTile { tx: 1, ty: 0, kind: SetKind::WrongLen },
        Op::Undo,
    ];
    let (a, b) = (old::run((3 * 64 - 5, 2 * 64 - 3, 2), &ops), new::run((3 * 64 - 5, 2 * 64 - 3, 2), &ops));
    assert_eq!(a, b);
    assert_eq!(a[2].cur.order, vec![(1, 0), (2, 0), (0, 1)]);
    assert_eq!(a[4].cur.order, vec![(2, 0), (0, 1)]);
}

/// The comparison is sensitive: an extra edit on one side is caught.
#[test]
fn harness_detects_a_difference() {
    let base = [Op::Set { x: 3, y: 3, v: [9, 9, 9, 9] }];
    let more = [Op::Set { x: 3, y: 3, v: [9, 9, 9, 9] }, Op::Set { x: 70, y: 3, v: [1, 1, 1, 1] }];
    assert_eq!(old::run((100, 100, 4), &base), new::run((100, 100, 4), &base));
    assert_ne!(old::run((100, 100, 4), &more)[1], new::run((100, 100, 4), &base)[0]);
}
