//! Embedded Natural Earth line work (public domain), decoded once on first use.
//!
//! Binary layout (see `data/README.md`): `u32` line count, then per line a
//! `u16` point count followed by `(i16 lon, i16 lat)` pairs in 0.01 degrees,
//! all little-endian.

use std::sync::OnceLock;

use crate::{
    continents,
    geo::{LatLon, V3},
};

/// A coastline or border, pre-converted to unit vectors so that drawing a
/// frame is pure rotation with no trigonometry.
pub type Polyline = Vec<V3>;

static COAST_110M: &[u8] = include_bytes!("../data/coast110.bin");
static COAST_50M: &[u8] = include_bytes!("../data/coast50.bin");
static BORDERS_110M: &[u8] = include_bytes!("../data/borders110.bin");

/// 1-bit equirectangular land/sea raster, 0.25 degrees per cell, row 0 at
/// the north pole, bits LSB-first.
static LAND_MASK: &[u8] = include_bytes!("../data/landmask.bin");
const MASK_W: usize = 1440;
const MASK_H: usize = 720;

/// Is this coordinate on land? Used to stipple the continents: instead of
/// rasterising land polygons forward onto the screen, the renderer asks
/// this question about each dot it wants to light.
pub fn is_land(p: LatLon) -> bool {
    let row = (((90.0 - p.lat) / 180.0 * MASK_H as f64) as isize).clamp(0, MASK_H as isize - 1);
    let col = ((p.lon + 180.0).rem_euclid(360.0) / 360.0 * MASK_W as f64) as usize % MASK_W;
    let i = row as usize * MASK_W + col;
    LAND_MASK
        .get(i >> 3)
        .is_some_and(|byte| byte >> (i & 7) & 1 == 1)
}

fn decode(bytes: &[u8]) -> Vec<Polyline> {
    let count = bytes
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .unwrap_or(0);
    let mut at = 4usize;
    let mut lines = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let Some(n) = bytes
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
        else {
            break;
        };
        at += 2;
        let Some(chunk) = bytes.get(at..at + n * 4) else {
            break;
        };
        at += n * 4;
        lines.push(
            chunk
                .chunks_exact(4)
                .map(|p| {
                    let lon = i16::from_le_bytes([p[0], p[1]]) as f64 / 100.0;
                    let lat = i16::from_le_bytes([p[2], p[3]]) as f64 / 100.0;
                    LatLon::new(lat, lon).to_vec()
                })
                .collect(),
        );
    }
    lines
}

/// Coastlines. `detailed` selects the 1:50m set, worth it once zoomed in.
pub fn coast(detailed: bool) -> &'static [Polyline] {
    static LOW: OnceLock<Vec<Polyline>> = OnceLock::new();
    static HIGH: OnceLock<Vec<Polyline>> = OnceLock::new();
    if detailed {
        HIGH.get_or_init(|| decode(COAST_50M))
    } else {
        LOW.get_or_init(|| decode(COAST_110M))
    }
}

/// The continent of every coastline point (as `Continent::index`), in the
/// same shape as [`coast`], so a shore can be drawn in its continent's
/// colour. Worked out once: classifying a point needs its latitude and
/// longitude back, which costs the trigonometry the unit vectors avoid.
pub fn coast_continents(detailed: bool) -> &'static [Vec<u8>] {
    static LOW: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    static HIGH: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let classify = || {
        coast(detailed)
            .iter()
            .map(|line| {
                line.iter()
                    .map(|v| continents::of(LatLon::from_vec(*v)).index() as u8)
                    .collect()
            })
            .collect()
    };
    if detailed {
        HIGH.get_or_init(classify)
    } else {
        LOW.get_or_init(classify)
    }
}

/// Land borders between countries (1:110m).
pub fn borders() -> &'static [Polyline] {
    static LINES: OnceLock<Vec<Polyline>> = OnceLock::new();
    LINES.get_or_init(|| decode(BORDERS_110M))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::dot;

    #[test]
    fn embedded_line_work_decodes_completely() {
        assert_eq!(coast(false).len(), 134);
        assert!(coast(true).len() > 1000);
        assert!(borders().len() > 100);
        let points: usize = coast(false).iter().map(Vec::len).sum();
        assert_eq!(points, 5128);
        assert!(
            coast(false)
                .iter()
                .flatten()
                .all(|v| (dot(*v, *v) - 1.0).abs() < 1e-9)
        );
    }

    #[test]
    fn land_mask_knows_continents_from_oceans() {
        assert_eq!(LAND_MASK.len(), MASK_W * MASK_H / 8);
        for (name, lat, lon) in [
            ("Paris", 48.85, 2.35),
            ("Denver", 39.74, -104.99),
            ("Alice Springs", -23.7, 133.88),
            ("South Pole", -89.0, 0.0),
        ] {
            assert!(is_land(LatLon::new(lat, lon)), "{name} should be land");
        }
        for (name, lat, lon) in [
            ("mid-Atlantic", 30.0, -40.0),
            ("mid-Pacific", 0.0, -140.0),
            ("date line", 20.0, 180.0),
            ("North Pole", 89.9, 10.0),
        ] {
            assert!(!is_land(LatLon::new(lat, lon)), "{name} should be sea");
        }
    }
}
