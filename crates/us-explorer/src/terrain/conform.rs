//! Make detailed terrain agree with the roads on it: level the ground across every
//! road (with fill under raised approaches and cuttings for dipped ones), blend back to
//! the natural slope beside it, and open the ground where a tunnel breaks the surface.
//!
//! Runs on the raw height lattice of each tile at zoom >= [`CONFORM_ZOOM`], on the
//! tile-loading worker threads.

use super::load::GRID;
use crate::roads::build::{Corridor, Lake, Local};
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

/// Ground under a building to keep below its floor (or basement), so the terrain never
/// shows inside: a footprint (lat, lon, a little inside the walls) and the highest the
/// ground may be there.
#[derive(Clone, Debug, PartialEq)]
pub struct Pad {
    pub ring: Vec<(f64, f64)>,
    /// (s, w, n, e)
    pub bbox: (f64, f64, f64, f64),
    pub top: f32,
}

type PadMap = std::collections::HashMap<TileId, (std::sync::Arc<Vec<Pad>>, u64)>;

fn pads() -> &'static std::sync::RwLock<PadMap> {
    static P: std::sync::OnceLock<std::sync::RwLock<PadMap>> = std::sync::OnceLock::new();
    P.get_or_init(Default::default)
}

/// Buildings of one building tile (zoom 15) ask for these pads (`sig` sums them up).
pub fn set_pads(tile: TileId, list: Vec<Pad>, sig: u64) {
    let Ok(mut m) = pads().write() else { return };
    if m.get(&tile).is_some_and(|x| x.1 == sig) {
        return;
    }
    m.insert(tile, (std::sync::Arc::new(list), sig));
    if m.len() > 600 {
        let far: Vec<TileId> = m.keys().filter(|k| k.x.abs_diff(tile.x) > 12 || k.y.abs_diff(tile.y) > 12).copied().collect();
        for k in far {
            m.remove(&k);
        }
    }
}

/// The building tiles whose pads can reach a terrain tile.
fn pad_tiles(id: TileId) -> Vec<TileId> {
    let z = usx_core::osm::ZOOM;
    if id.z < z {
        return Vec::new();
    }
    let own = id.ancestor(z);
    let mut out = Vec::with_capacity(9);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (x, y) = (own.x as i64 + dx, own.y as i64 + dy);
            if x >= 0 && y >= 0 {
                out.push(TileId::new(z, x as u32, y as u32));
            }
        }
    }
    out
}

/// What the pads around a terrain tile add up to (changes when any of them do).
pub fn pads_sig(id: TileId) -> u64 {
    let Ok(m) = pads().read() else { return 0 };
    pad_tiles(id).iter().filter_map(|t| m.get(t)).fold(0u64, |a, x| a.rotate_left(7) ^ x.1)
}

/// Pads touching a terrain tile, and their signature.
pub fn pads_for(id: TileId) -> (Vec<Pad>, u64) {
    let Ok(m) = pads().read() else { return (Vec::new(), 0) };
    let (w, s, e, n) = id.bounds();
    let mut out = Vec::new();
    let mut sig = 0u64;
    for t in pad_tiles(id) {
        if let Some((list, ts)) = m.get(&t) {
            sig = sig.rotate_left(7) ^ ts;
            out.extend(list.iter().filter(|p| !(p.bbox.0 > n || p.bbox.2 < s || p.bbox.1 > e || p.bbox.3 < w)).cloned());
        }
    }
    (out, sig)
}

pub fn conform(id: TileId, h: &mut [f32], corridors: &[Corridor], lakes: &[Lake], pads: &[Pad]) -> Conformed {
    let q = GRID - 1;
    let mut holes = vec![false; q * q];
    if id.z < CONFORM_ZOOM || corridors.is_empty() && lakes.is_empty() && pads.is_empty() {
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
        // Fully level one lattice step past the road edge: a triangle reaching across
        // the edge then has no corner left at the natural (higher) ground to poke up
        // through the road.
        let full = c.half + spacing * 0.9;
        let reach = full + blend;
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
                    let wgt = if d <= full { 1.0 } else { 1.0 - smoothstep(full, reach, d) };
                    let (cu, cv) = {
                        let (la, lo) = local.ll(cp);
                        uv(id, la, lo)
                    };
                    let target = sample(&raw, cu, cv) + c.off[k] + (c.off[k + 1] - c.off[k]) * t;
                    let bv = &mut best[vi];
                    // Where two roads both claim the ground fully (a road crossing over
                    // another), the lower one shapes it; the upper one is drawn above.
                    if wgt > bv.0 + 1e-4
                        || (wgt >= 0.999 && bv.0 >= 0.999 && target < bv.1)
                        || (wgt < 0.999 && wgt >= bv.0 - 1e-4 && d < bv.2)
                    {
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
    let ring_inside = |r: &[Vec2], p: Vec2| {
        let mut c = false;
        let n = r.len();
        if n < 3 {
            return false;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (r[i], r[j]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                c = !c;
            }
            j = i;
        }
        c
    };
    // Parking lots: the ground under them (and just past their edge) smoothed over
    // about 8 m, so the paving lies on it without bumps poking through. Roads keep
    // their levelling.
    let steps = ((8.0 / spacing).round() as i64).max(1);
    for l in lakes.iter().filter(|l| l.level) {
        if l.bbox.0 > nn || l.bbox.2 < s || l.bbox.1 > e || l.bbox.3 < w {
            continue;
        }
        let outer: Vec<Vec<Vec2>> = l.outer.iter().map(|r| r.iter().map(|&(a, b)| local.xz(a, b)).collect()).collect();
        let inner: Vec<Vec<Vec2>> = l.inner.iter().map(|r| r.iter().map(|&(a, b)| local.xz(a, b)).collect()).collect();
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in outer.iter().flatten() {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let m = spacing * 1.01;
        for (vi, &p) in vpos.iter().enumerate() {
            if best[vi].0 > 0.0 || p.x < lo.x - m || p.y < lo.y - m || p.x > hi.x + m || p.y > hi.y + m {
                continue;
            }
            // Inside, or within a lattice step of an edge (so triangles crossing the
            // edge are smoothed too).
            let near = |q: Vec2| outer.iter().any(|r| ring_inside(r, q)) && !inner.iter().any(|r| ring_inside(r, q));
            if !(near(p) || [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y].iter().any(|d| near(p + *d * m))) {
                continue;
            }
            let (i, j) = ((vi % GRID) as i64, (vi / GRID) as i64);
            let (mut sum, mut cnt) = (0.0f32, 0.0f32);
            for dj in -steps..=steps {
                for di in -steps..=steps {
                    let (x, y) = (i + di, j + dj);
                    if x >= 0 && y >= 0 && x < GRID as i64 && y < GRID as i64 {
                        sum += raw[y as usize * GRID + x as usize];
                        cnt += 1.0;
                    }
                }
            }
            h[vi] = sum / cnt;
        }
    }
    // Lake and river beds: down from the shore at about 1 in 6 to the lake's depth.
    for l in lakes.iter().filter(|l| !l.level) {
        if l.bbox.0 > nn || l.bbox.2 < s || l.bbox.1 > e || l.bbox.3 < w {
            continue;
        }
        let outer: Vec<Vec<Vec2>> = l.outer.iter().map(|r| r.iter().map(|&(a, b)| local.xz(a, b)).collect()).collect();
        let inner: Vec<Vec<Vec2>> = l.inner.iter().map(|r| r.iter().map(|&(a, b)| local.xz(a, b)).collect()).collect();
        let inside = |r: &[Vec2], p: Vec2| {
            let mut c = false;
            let n = r.len();
            if n < 3 {
                return false;
            }
            let mut j = n - 1;
            for i in 0..n {
                let (a, b) = (r[i], r[j]);
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                    c = !c;
                }
                j = i;
            }
            c
        };
        let shore_cap = l.depth * 6.0;
        for (vi, &p) in vpos.iter().enumerate() {
            if !outer.iter().any(|r| inside(r, p)) || inner.iter().any(|r| inside(r, p)) {
                continue;
            }
            // Distance to the nearest shore, capped (only needed up to full depth).
            let mut d = shore_cap;
            for r in outer.iter().chain(inner.iter()) {
                for k in 0..r.len().saturating_sub(1) {
                    let (a, b) = (r[k], r[k + 1]);
                    if (p.x - a.x.min(b.x)) > d && (a.x.max(b.x) - p.x) < -d {
                        continue;
                    }
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    d = d.min(p.distance(a + ab * t));
                }
            }
            let dig = (d / 6.0).min(l.depth) + 0.3;
            h[vi] = h[vi].min(raw[vi] - dig);
        }
    }
    // Under buildings: no higher than the floor.
    for pd in pads {
        if pd.bbox.0 > nn || pd.bbox.2 < s || pd.bbox.1 > e || pd.bbox.3 < w {
            continue;
        }
        let r: Vec<Vec2> = pd.ring.iter().map(|&(a, b)| local.xz(a, b)).collect();
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in &r {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        for (vi, &p) in vpos.iter().enumerate() {
            if p.x < lo.x || p.y < lo.y || p.x > hi.x || p.y > hi.y || h[vi] <= pd.top {
                continue;
            }
            let mut c = false;
            let n = r.len();
            let mut j = n - 1;
            for i in 0..n {
                let (a, b) = (r[i], r[j]);
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                    c = !c;
                }
                j = i;
            }
            if c {
                h[vi] = pd.top;
            }
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
        conform(id, &mut h, &[c], &[], &[]);
        let mid = GRID / 2;
        let row = 10 * GRID;
        // Centre vertex raised by the fill; neighbours across the road level with it.
        assert!((h[row + mid] - (before[row + mid] + 2.0)).abs() < 0.05, "{} vs {}", h[row + mid], before[row + mid]);
        // Far from the road nothing changes.
        assert_eq!(h[row], before[row]);
        assert_eq!(h[row + GRID - 1], before[row + GRID - 1]);
    }
}
