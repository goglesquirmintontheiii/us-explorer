//! Make detailed terrain agree with the roads on it: level the ground across every
//! road (with fill under raised approaches and cuttings for dipped ones), blend back to
//! the natural slope beside it, and open the ground where a tunnel breaks the surface.
//!
//! Runs on the raw height lattice of each tile at zoom >= [`CONFORM_ZOOM`], on the
//! tile-loading worker threads.

use super::load::GRID;
use crate::roads::build::{Corridor, Local};
use bevy::math::Vec2;
use usx_core::tile::TileId;

/// Coarser tiles keep their natural shape; roads drawn on them add their offsets instead.
pub const CONFORM_ZOOM: u8 = 16;

/// What conforming did to a tile.
pub struct Conformed {
    /// Quads ((GRID-1)^2, row-major) to leave out of the mesh: tunnel openings.
    pub holes: Vec<bool>,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Height on the raw lattice at tile-local (u, v), on the mesh's own triangles.
fn sample(h: &[f32], u: f64, v: f64) -> f32 {
    let n = (GRID - 1) as f64;
    let (fx, fy) = ((u * n).clamp(0.0, n - 1e-9), (v * n).clamp(0.0, n - 1e-9));
    let (ix, iy) = (fx as usize, fy as usize);
    let (tx, ty) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
    let at = |x: usize, y: usize| h[y * GRID + x];
    let (a, b, c, d) = (at(ix, iy), at(ix + 1, iy), at(ix, iy + 1), at(ix + 1, iy + 1));
    if tx >= ty { a + (b - a) * tx + (d - b) * ty } else { a + (d - c) * tx + (c - a) * ty }
}

/// Tile-local (u, v) of a lat/lon (not clamped: may fall outside 0..1).
fn uv(id: TileId, lat: f64, lon: f64) -> (f64, f64) {
    let n = (1u64 << id.z) as f64;
    let latr = lat.to_radians();
    let fx = (lon + 180.0) / 360.0 * n;
    let fy = (1.0 - (latr.tan() + 1.0 / latr.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (fx - id.x as f64, fy - id.y as f64)
}

pub fn conform(id: TileId, h: &mut [f32], corridors: &[Corridor]) -> Conformed {
    let q = GRID - 1;
    let mut holes = vec![false; q * q];
    if id.z < CONFORM_ZOOM || corridors.is_empty() {
        return Conformed { holes };
    }
    let (w, s, e, nn) = id.bounds();
    let (cla, clo) = id.lat_lon_at(0.5, 0.5);
    let local = Local::new(cla, clo);
    let spacing = (id.width_m() / q as f64) as f32;
    // Vertex positions in local metres.
    let vpos: Vec<Vec2> = (0..GRID * GRID)
        .map(|k| {
            let (la, lo) = id.lat_lon_at((k % GRID) as f64 / q as f64, (k / GRID) as f64 / q as f64);
            local.xz(la, lo)
        })
        .collect();
    let raw = h.to_vec();
    // Best influence per vertex: (weight, target, distance).
    let mut best: Vec<(f32, f32, f32)> = vec![(0.0, 0.0, f32::MAX); GRID * GRID];
    let to_ij = |p: Vec2| {
        let (la, lo) = local.ll(p);
        let (u, v) = uv(id, la, lo);
        (u * q as f64, v * q as f64)
    };
    for c in corridors {
        if c.bbox.0 > nn || c.bbox.2 < s || c.bbox.1 > e || c.bbox.3 < w || c.pts.len() < 2 {
            continue;
        }
        let pts: Vec<Vec2> = c.pts.iter().map(|&(a, b)| local.xz(a, b)).collect();
        let max_off = c.off.iter().fold(0.0f32, |m, o| m.max(o.abs()));
        let blend = (max_off * 1.8 + 2.5).max(spacing * 1.3).max(3.0);
        let reach = c.half + blend;
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let ab = b - a;
            let l2 = ab.length_squared().max(1e-6);
            // Lattice index range covering the segment's reach.
            let (i0, j0) = to_ij(a.min(b) - Vec2::splat(reach));
            let (i1, j1) = to_ij(a.max(b) + Vec2::splat(reach));
            let (imin, imax) = (i0.min(i1).floor().max(0.0) as usize, (i0.max(i1).ceil() as i64).clamp(0, q as i64) as usize);
            let (jmin, jmax) = (j0.min(j1).floor().max(0.0) as usize, (j0.max(j1).ceil() as i64).clamp(0, q as i64) as usize);
            if imin > imax || jmin > jmax {
                continue;
            }
            for j in jmin..=jmax {
                for i in imin..=imax {
                    let vi = j * GRID + i;
                    let p = vpos[vi];
                    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                    let cp = a + ab * t;
                    let d = p.distance(cp);
                    if d > reach {
                        continue;
                    }
                    if c.tunnel.is_some() {
                        continue;
                    }
                    let wgt = if d <= c.half { 1.0 } else { 1.0 - smoothstep(c.half, reach, d) };
                    let (cu, cv) = {
                        let (la, lo) = local.ll(cp);
                        uv(id, la, lo)
                    };
                    let target = sample(&raw, cu, cv) + c.off[k] + (c.off[k + 1] - c.off[k]) * t;
                    let bv = &mut best[vi];
                    if wgt > bv.0 + 1e-4 || (wgt >= bv.0 - 1e-4 && d < bv.2) {
                        *bv = (wgt, target, d);
                    }
                }
            }
        }
        // Tunnel openings: quads whose centre is inside the tube, where the ground dips
        // to below the tunnel's roof.
        if let Some(deck) = &c.tunnel {
            let tube = c.half + 1.5;
            for k in 0..pts.len() - 1 {
                let (a, b) = (pts[k], pts[k + 1]);
                let ab = b - a;
                let l2 = ab.length_squared().max(1e-6);
                let (i0, j0) = to_ij(a.min(b) - Vec2::splat(tube + spacing));
                let (i1, j1) = to_ij(a.max(b) + Vec2::splat(tube + spacing));
                let (imin, imax) = (i0.min(i1).floor().max(0.0) as usize, (i0.max(i1).ceil() as i64).clamp(0, q as i64 - 1) as usize);
                let (jmin, jmax) = (j0.min(j1).floor().max(0.0) as usize, (j0.max(j1).ceil() as i64).clamp(0, q as i64 - 1) as usize);
                if imin > imax || jmin > jmax {
                    continue;
                }
                for j in jmin..=jmax {
                    for i in imin..=imax {
                        let centre =
                            (vpos[j * GRID + i] + vpos[j * GRID + i + 1] + vpos[(j + 1) * GRID + i] + vpos[(j + 1) * GRID + i + 1]) * 0.25;
                        let t = ((centre - a).dot(ab) / l2).clamp(0.0, 1.0);
                        if centre.distance(a + ab * t) > tube {
                            continue;
                        }
                        let dh = deck[k] + (deck[k + 1] - deck[k]) * t;
                        let g = (raw[j * GRID + i] + raw[j * GRID + i + 1] + raw[(j + 1) * GRID + i] + raw[(j + 1) * GRID + i + 1]) * 0.25;
                        if g < dh + 8.0 && g > dh - 3.0 {
                            holes[j * q + i] = true;
                        }
                    }
                }
            }
        }
    }
    for (vi, &(wgt, target, _)) in best.iter().enumerate() {
        if wgt > 0.0 {
            h[vi] = raw[vi] + (target - raw[vi]) * wgt;
        }
    }
    Conformed { holes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_a_road_across_a_slope() {
        let id = TileId::containing(39.75, -104.99, 18).0;
        // Ground sloping 20% from west to east.
        let w = id.width_m() as f32;
        let mut h: Vec<f32> = (0..GRID * GRID).map(|k| 1600.0 + (k % GRID) as f32 / (GRID - 1) as f32 * w * 0.2).collect();
        let before = h.clone();
        // A north-south road through the middle, raised 2 m.
        let (la0, lo) = id.lat_lon_at(0.5, 0.0);
        let (la1, _) = id.lat_lon_at(0.5, 1.0);
        let lo_mid = id.lat_lon_at(0.5, 0.5).1;
        let _ = lo;
        let c = Corridor {
            pts: vec![(la0, lo_mid), (la1, lo_mid)],
            off: vec![2.0, 2.0],
            half: 4.0,
            tunnel: None,
            bbox: (la1 - 0.01, lo_mid - 0.01, la0 + 0.01, lo_mid + 0.01),
        };
        conform(id, &mut h, &[c]);
        let mid = GRID / 2;
        let row = 10 * GRID;
        // Centre vertex raised by the fill; neighbours across the road level with it.
        assert!((h[row + mid] - (before[row + mid] + 2.0)).abs() < 0.05, "{} vs {}", h[row + mid], before[row + mid]);
        // Far from the road nothing changes.
        assert_eq!(h[row], before[row]);
        assert_eq!(h[row + GRID - 1], before[row + GRID - 1]);
    }
}
