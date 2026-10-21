//! Set a chunk's buildings on the ground that's on screen: each building's floor goes
//! level at the highest ground under its footprint (so the terrain never comes up
//! through a floor), and its foundation walls reach down to the ground all round.

use super::build::{BChunk, Built};
use crate::terrain::HeightSnap;
use bevy::math::{DMat3, DVec3, Vec2, Vec3};
use usx_core::geo::{EnuFrame, Geodetic};

/// Where one building ended up.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    /// Index into the tile's building list.
    pub b: u32,
    /// Ground floor level (m above the ellipsoid) and the lowest ground under the walls.
    pub base: f64,
    pub low: f64,
    pub origin: DVec3,
}

pub struct Draped {
    pub center: DVec3,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub color: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub placed: Vec<Placed>,
}

/// Floor level of one building from the ground under it (natural ground: roads beside
/// a house don't lift it), and the lowest point under its walls.
pub fn base_of(built: &Built, b: usize, snap: &HeightSnap) -> Option<(f64, f64)> {
    let bl = &built.buildings[b];
    // Natural ground decides the floor (stable however the terrain is levelled under
    // roads or dug out under buildings); the visible ground the lowest wall foot.
    let ground = |p: Vec2| {
        let (la, lo) = built.local.ll(p);
        snap.natural(la, lo).or_else(|| snap.ground(la, lo).map(|x| x.0))
    };
    let visible = |p: Vec2| {
        let (la, lo) = built.local.ll(p);
        snap.ground(la, lo).map(|x| x.0)
    };
    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    let mut any = false;
    let ring = &bl.outline;
    let n = ring.len();
    for i in 0..n {
        // Corners and edge midpoints.
        for p in [ring[i], (ring[i] + ring[(i + 1) % n]) * 0.5] {
            if let Some(h) = ground(p) {
                hi = hi.max(h);
                lo = lo.min(visible(p).unwrap_or(h).min(h));
                any = true;
            }
        }
    }
    // A few points inside (a hump in the middle of a big footprint).
    let c = super::geom::centroid(ring);
    for k in 0..6 {
        let a = k as f32 * std::f32::consts::TAU / 6.0;
        let p = c + Vec2::from_angle(a) * (bl.area.sqrt() * 0.25);
        if super::geom::contains(ring, p)
            && let Some(h) = ground(p)
        {
            hi = hi.max(h);
        }
    }
    if let Some(h) = ground(c) {
        hi = hi.max(h);
    }
    any.then(|| (hi + built.floor_lift[b] as f64, lo))
}

pub fn drape(built: &Built, chunk: usize, snap: &HeightSnap, hidden: &[u32]) -> Option<Draped> {
    let c: &BChunk = &built.chunks[chunk];
    if snap.is_empty() {
        return None;
    }
    let (cla, clo) = c.id.lat_lon_at(0.5, 0.5);
    let ch = snap.ground(cla, clo).map(|x| x.0).unwrap_or(0.0);
    let center = Geodetic::new(cla, clo, ch).to_ecef();
    let n = c.geom.v.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut uv0 = Vec::with_capacity(n);
    let mut uv1 = Vec::with_capacity(n);
    let mut color = Vec::with_capacity(n);
    let mut indices = Vec::with_capacity(c.geom.i.len());
    let mut placed = Vec::new();
    for r in &c.ranges {
        if hidden.contains(&r.b) {
            continue;
        }
        let Some((base, low)) = base_of(built, r.b as usize, snap) else { continue };
        let bl = &built.buildings[r.b as usize];
        let cen = super::geom::centroid(&bl.outline);
        let (la, lo) = built.local.ll(cen);
        let g = Geodetic::new(la, lo, base);
        let origin = g.to_ecef();
        // Local (x east, y up, z south) to ECEF.
        let to_ecef: DMat3 = EnuFrame::at(g).ecef_to_local.transpose();
        placed.push(Placed { b: r.b, base, low, origin });
        let first = positions.len() as u32;
        for v in &c.geom.v[r.v0 as usize..r.v1 as usize] {
            let mut y = v.p.y as f64;
            if v.ground {
                let (vla, vlo) = built.local.ll(Vec2::new(v.p.x, v.p.z));
                let gh = snap.ground(vla, vlo).map(|x| x.0).unwrap_or(low);
                y = (gh - 0.3 - base).min(y);
            }
            let local = DVec3::new((v.p.x - cen.x) as f64, y, (v.p.z - cen.y) as f64);
            let e = origin + to_ecef * local;
            positions.push((e - center).as_vec3().to_array());
            let nn = (to_ecef * v.n.as_dvec3()).as_vec3();
            normals.push(nn.normalize_or(Vec3::Y).to_array());
            uv0.push(v.uv);
            uv1.push(v.uv2);
            color.push(v.col);
        }
        for &i in &c.geom.i[r.i0 as usize..r.i1 as usize] {
            indices.push(i - r.v0 + first);
        }
    }
    Some(Draped { center, positions, normals, uv0, uv1, color, indices, placed })
}
