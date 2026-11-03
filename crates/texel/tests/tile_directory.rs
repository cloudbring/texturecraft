use std::sync::Arc;
use texturecraft_texel::{TILE, TILE_AREA, TileImage};

#[test]
fn tile_image_row_major_iteration_and_length_checked_tile_replacement() {
    let mut img = TileImage::new(3 * TILE - 5, 2 * TILE - 3, 2);
    img.tile_mut(2, 0, &[10, 20]).unwrap()[0] = 11;
    img.tile_mut(0, 1, &[30, 40]).unwrap();
    img.tile_mut(1, 0, &[50, 60]).unwrap();
    assert_eq!(img.allocated().collect::<Vec<_>>(), vec![(1, 0), (2, 0), (0, 1)]);
    assert_eq!(img.bytes(), 3 * TILE_AREA * 4);
    let snap = img.clone();
    img.set_tile(1, 0, Some(Arc::new(vec![1; TILE_AREA * 2 - 1])));
    assert!(!img.is_allocated(1, 0) && snap.is_allocated(1, 0));
    assert_eq!(img.allocated().collect::<Vec<_>>(), vec![(2, 0), (0, 1)]);
}
