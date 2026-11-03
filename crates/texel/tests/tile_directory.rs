use std::sync::Arc;

use proptest::prelude::*;
use texturecraft_texel::{TILE, TILE_AREA, TileImage};

fn check_model(image: &TileImage, model: &[Option<Vec<u16>>]) {
    let columns = image.tiles_x() as usize;
    let mut allocated = Vec::new();
    for (index, expected) in model.iter().enumerate() {
        let (tx, ty) = ((index % columns) as u32, (index / columns) as u32);
        assert_eq!(image.tile(tx, ty), expected.as_deref());
        assert_eq!(image.tile_handle(tx, ty).map(|data| data.as_slice()), expected.as_deref());
        assert_eq!(image.is_allocated(tx, ty), expected.is_some());
        if expected.is_some() {
            allocated.push((tx, ty));
        }
    }
    assert_eq!(image.allocated_count(), allocated.len());
    assert_eq!(image.bytes(), allocated.len() * TILE_AREA * image.comps() * 2);
    assert_eq!(image.allocated().collect::<Vec<_>>(), allocated);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn writes_match_dense_model_and_preserve_snapshots(
        columns in 1u32..20,
        rows in 1u32..10,
        comps in 1usize..=4,
        script in prop::collection::vec((0u8..6, any::<usize>(), 0usize..TILE_AREA, prop::collection::vec(any::<u16>(), 0..6)), 1..40),
    ) {
        let mut image = TileImage::new(columns * TILE - 7, rows * TILE - 3, comps);
        let mut model: Vec<Option<Vec<u16>>> = vec![None; (columns * rows) as usize];
        let mut snapshots = Vec::new();
        for (operation, address, pixel, values) in script {
            snapshots.push((image.clone(), model.clone()));
            let index = address % (model.len() + 2);
            let (tx, ty) = ((index % columns as usize) as u32, (index / columns as usize) as u32);
            match operation {
                0 => {
                    let data = vec![values.first().copied().unwrap_or(0); TILE_AREA * comps];
                    image.set_tile(tx, ty, Some(Arc::new(data.clone())));
                    if let Some(slot) = model.get_mut(index) {
                        *slot = Some(data);
                    }
                }
                1 | 2 => {
                    image.set_tile(tx, ty, (operation == 2).then(|| Arc::new(vec![1; TILE_AREA * comps - 1])));
                    if let Some(slot) = model.get_mut(index) {
                        *slot = None;
                    }
                }
                3 => {
                    let actual = image.tile_mut(tx, ty, &values);
                    if let Some(slot) = model.get_mut(index) {
                        let data = slot.get_or_insert_with(|| {
                            let mut data = vec![0; TILE_AREA * comps];
                            for texel in data.chunks_exact_mut(comps) {
                                for (dest, value) in texel.iter_mut().zip(&values) {
                                    *dest = *value;
                                }
                            }
                            data
                        });
                        data[pixel * comps] = 42;
                        actual.unwrap()[pixel * comps] = 42;
                    } else {
                        prop_assert!(actual.is_none());
                    }
                }
                4 => {
                    let (x, y) = (tx * TILE + pixel as u32 % TILE, ty * TILE + pixel as u32 / TILE);
                    image.set(x, y, &values);
                    let mut expected = [0; 4];
                    if x < image.width() && y < image.height() {
                        let data = model[index].get_or_insert_with(|| vec![0; TILE_AREA * comps]);
                        for (dest, value) in data[pixel * comps..][..comps].iter_mut().zip(&values) {
                            *dest = *value;
                        }
                        expected[..comps].copy_from_slice(&data[pixel * comps..][..comps]);
                    }
                    prop_assert_eq!(image.get(x, y), expected);
                }
                _ => {
                    image.prune_empty();
                    for slot in &mut model {
                        if slot.as_ref().is_some_and(|data| data.iter().all(|value| *value == 0)) {
                            *slot = None;
                        }
                    }
                }
            }
            check_model(&image, &model);
        }
        for (snapshot, expected) in snapshots {
            check_model(&snapshot, &expected);
        }
        let mut independent = TileImage::new(image.width(), image.height(), comps);
        for (index, tile) in model.into_iter().enumerate() {
            independent.set_tile(index as u32 % columns, index as u32 / columns, tile.map(Arc::new));
        }
        prop_assert_eq!(image, independent);
    }
}
