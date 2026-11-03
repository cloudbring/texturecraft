//! # texturecraft-texel
//!
//! Texel storage and texel-space helpers: sparse copy-on-write [`TileImage`]s (64×64 tiles of
//! `u16` components), UV padding ([`DilationMap`]), deterministic [`noise`], and sRGB transfer
//! functions. No workspace dependencies.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod colorspace;
pub mod dilate;
pub mod noise;
pub mod tile;

pub use colorspace::ColorSpace;
pub use dilate::DilationMap;
pub use tile::{ResamplePolicy, TILE, TILE_AREA, TileData, TileImage, from_u16, from_u16_signed, to_u16, to_u16_signed};

/// Linear → sRGB transfer (0..1).
#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// sRGB → linear transfer (0..1).
#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Lookup table from the top 12 bits of a linear u16 to an sRGB u8 (fast display upload).
pub fn srgb_lut() -> &'static [u8; 4096] {
    static LUT: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0u8; 4096];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (linear_to_srgb((i as f32 + 0.5) / 4096.0) * 255.0 + 0.5) as u8;
        }
        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trips_and_lut_matches() {
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            assert!((srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-5);
        }
        let lut = srgb_lut();
        assert_eq!(lut[0], 0);
        assert_eq!(lut[4095], 255);
        assert!((lut[2048] as i32 - (linear_to_srgb(0.5) * 255.0).round() as i32).abs() <= 1);
    }
}
