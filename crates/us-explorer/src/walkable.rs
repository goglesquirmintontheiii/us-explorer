//! Surfaces you can stand on besides the terrain: bridge decks and overpasses for now,
//! later floors and roofs. Each is a straight strip between two points with a height at
//! each end, kept in a coarse grid so lookups stay cheap.

use crate::roads::build::Local;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

/// A wall you can't walk through (building walls), in a solid's local metres.
#[derive(Clone, Debug)]
pub struct SWall {
    pub a: Vec2,
    pub b: Vec2,
    pub h0: f64,
    pub h1: f64,
    /// A doorway: passable while that door is open.
    pub door: Option<u32>,
}

/// A floor to stand on: a polygon at a height, with holes (stairwells).
#[derive(Clone, Debug)]
pub struct SFloor {
    pub ring: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
    pub h: f64,
    /// An elevator car: the height comes from `Walkables::lifts`.
    pub lift: Option<u32>,
}

/// A stair flight: rises from h0 at -hl along `dir` to h1 at +hl.
#[derive(Clone, Debug)]
pub struct SRamp {
    pub c: Vec2,
    pub dir: Vec2,
    pub hl: f32,
    pub hw: f32,
    pub h0: f64,
    pub h1: f64,
}

/// One building's collision.
#[derive(Clone, Debug)]
pub struct Solid {
    pub local: Local,
    /// (s, w, n, e)
    pub bbox: (f64, f64, f64, f64),
    pub walls: Vec<SWall>,
    pub floors: Vec<SFloor>,
    pub ramps: Vec<SRamp>,
}

fn ring_contains(r: &[Vec2], p: Vec2) -> bool {
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
}

const SOLID_CELL: f64 = 0.002;

fn solid_cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / SOLID_CELL).floor() as i32, (lon / SOLID_CELL).floor() as i32)
}

#[derive(Clone, Debug)]
pub struct Strip {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub ha: f64,
    pub hb: f64,
    pub half_width_m: f64,
    pub owner: u64,
    /// A tunnel floor: preferred over the terrain above it while you're down in it.
    pub tunnel: bool,
}

/// Roughly 1 km cells.
const CELL_DEG: f64 = 0.01;

/// A vertical post you can't walk through (sign poles, hydrants, guardrail posts).
#[derive(Clone, Copy, Debug)]
pub struct Post {
    pub lat: f64,
    pub lon: f64,
    pub radius: f64,
    pub h0: f64,
    pub h1: f64,
}

#[derive(Resource, Default)]
pub struct Walkables {
    strips: Vec<Option<Strip>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    by_owner: HashMap<u64, Vec<usize>>,
    /// Posts by ~100 m cell, with their owner.
    posts: HashMap<(i32, i32), Vec<(Post, u64)>>,
    post_cells: HashMap<u64, Vec<(i32, i32)>>,
    solids: HashMap<u64, Solid>,
    solid_grid: HashMap<(i32, i32), Vec<u64>>,
    /// Doors standing open (their doorways are passable).
    pub open_doors: HashSet<u32>,
    /// Elevator car floor heights.
    pub lifts: HashMap<u32, f64>,
}

const POST_CELL: f64 = 0.001;

fn post_cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / POST_CELL).floor() as i32, (lon / POST_CELL).floor() as i32)
}

fn cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / CELL_DEG).floor() as i32, (lon / CELL_DEG).floor() as i32)
}

impl Walkables {
    pub fn add(&mut self, s: Strip) {
        let i = self.strips.len();
        let (la0, la1) = (s.a.0.min(s.b.0), s.a.0.max(s.b.0));
        let (lo0, lo1) = (s.a.1.min(s.b.1), s.a.1.max(s.b.1));
        let pad = s.half_width_m / 111_000.0 * 2.0;
        let (c0, c1) = (cell(la0 - pad, lo0 - pad), cell(la1 + pad, lo1 + pad));
        for y in c0.0..=c1.0 {
            for x in c0.1..=c1.1 {
                self.grid.entry((y, x)).or_default().push(i);
            }
        }
        self.by_owner.entry(s.owner).or_default().push(i);
        self.strips.push(Some(s));
    }

    pub fn add_post(&mut self, p: Post, owner: u64) {
        let c = post_cell(p.lat, p.lon);
        self.posts.entry(c).or_default().push((p, owner));
        let cells = self.post_cells.entry(owner).or_default();
        if !cells.contains(&c) {
            cells.push(c);
        }
    }

    pub fn remove_posts(&mut self, owner: u64) {
        if let Some(cells) = self.post_cells.remove(&owner) {
            for c in cells {
                if let Some(v) = self.posts.get_mut(&c) {
                    v.retain(|(_, o)| *o != owner);
                    if v.is_empty() {
                        self.posts.remove(&c);
                    }
                }
            }
        }
    }

    pub fn add_solid(&mut self, key: u64, s: Solid) {
        self.remove_solid(key);
        let (c0, c1) = (solid_cell(s.bbox.0, s.bbox.1), solid_cell(s.bbox.2, s.bbox.3));
        for y in c0.0..=c1.0 {
            for x in c0.1..=c1.1 {
                self.solid_grid.entry((y, x)).or_default().push(key);
            }
        }
        self.solids.insert(key, s);
    }

    pub fn remove_solid(&mut self, key: u64) {
        if let Some(s) = self.solids.remove(&key) {
            let (c0, c1) = (solid_cell(s.bbox.0, s.bbox.1), solid_cell(s.bbox.2, s.bbox.3));
            for y in c0.0..=c1.0 {
                for x in c0.1..=c1.1 {
                    if let Some(v) = self.solid_grid.get_mut(&(y, x)) {
                        v.retain(|k| *k != key);
                        if v.is_empty() {
                            self.solid_grid.remove(&(y, x));
                        }
                    }
                }
            }
        }
    }

    pub fn solid_mut(&mut self, key: u64) -> Option<&mut Solid> {
        self.solids.get_mut(&key)
    }

    /// Highest building surface (floor, stair, roof, lift) at or below max_h.
    pub fn solid_surface(&self, lat: f64, lon: f64, max_h: f64) -> Option<f64> {
        let ids = self.solid_grid.get(&solid_cell(lat, lon))?;
        let mut best: Option<f64> = None;
        for k in ids {
            let Some(s) = self.solids.get(k) else { continue };
            if lat < s.bbox.0 || lat > s.bbox.2 || lon < s.bbox.1 || lon > s.bbox.3 {
                continue;
            }
            let p = s.local.xz(lat, lon);
            for f in &s.floors {
                let h = f.lift.and_then(|l| self.lifts.get(&l).copied()).unwrap_or(f.h);
                if h > max_h || best.is_some_and(|b| b >= h) {
                    continue;
                }
                if ring_contains(&f.ring, p) && !f.holes.iter().any(|r| ring_contains(r, p)) {
                    best = Some(h);
                }
            }
            for r in &s.ramps {
                let d = p - r.c;
                let (t, a) = (d.dot(r.dir), d.dot(r.dir.perp()));
                if t.abs() <= r.hl && a.abs() <= r.hw {
                    let h = r.h0 + (r.h1 - r.h0) * ((t + r.hl) / (2.0 * r.hl)) as f64;
                    if h <= max_h && best.is_none_or(|b| h > b) {
                        best = Some(h);
                    }
                }
            }
        }
        best
    }

    /// Where a straight line between two points first meets a building wall, floor or
    /// ceiling (closed doors included), as a fraction 0..1 of the way. For keeping
    /// cameras and aim rays inside rooms.
    pub fn solid_hit(&self, a: usx_core::geo::Geodetic, b: usx_core::geo::Geodetic) -> Option<f64> {
        let mut keys: Vec<u64> = Vec::new();
        for g in [a, b] {
            if let Some(ids) = self.solid_grid.get(&solid_cell(g.lat, g.lon)) {
                for k in ids {
                    if !keys.contains(k) {
                        keys.push(*k);
                    }
                }
            }
        }
        let mut best: Option<f64> = None;
        for k in keys {
            let Some(s) = self.solids.get(&k) else { continue };
            let (pa, pb) = (s.local.xz(a.lat, a.lon), s.local.xz(b.lat, b.lon));
            let r = pb - pa;
            for w in &s.walls {
                if w.door.is_some_and(|d| self.open_doors.contains(&d)) {
                    continue;
                }
                let sd = w.b - w.a;
                let den = r.perp_dot(sd);
                if den.abs() < 1e-9 {
                    continue;
                }
                let t = ((w.a - pa).perp_dot(sd) / den) as f64;
                let u = (w.a - pa).perp_dot(r) / den;
                if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let h = a.h + (b.h - a.h) * t;
                if h >= w.h0 && h <= w.h1 && best.is_none_or(|x| t < x) {
                    best = Some(t);
                }
            }
            for f in &s.floors {
                let fh = f.lift.and_then(|l| self.lifts.get(&l).copied()).unwrap_or(f.h);
                // Floors are slabs: the top at fh, a ceiling 0.3 m below.
                for y in [fh, fh - 0.3] {
                    if (a.h - y) * (b.h - y) >= 0.0 {
                        continue;
                    }
                    let t = (y - a.h) / (b.h - a.h);
                    let p = pa + r * t as f32;
                    if ring_contains(&f.ring, p) && !f.holes.iter().any(|h| ring_contains(h, p)) && best.is_none_or(|x| t < x) {
                        best = Some(t);
                    }
                }
            }
        }
        best
    }

    /// Push a body of `radius` metres at (lat, lon, feet height h) out of any posts.
    /// Returns the corrected position.
    pub fn push_out(&self, lat: f64, lon: f64, h: f64, radius: f64) -> (f64, f64) {
        let (lat, lon) = self.push_out_walls(lat, lon, h, radius);
        let (cy, cx) = post_cell(lat, lon);
        let (mut la, mut lo) = (lat, lon);
        let kx = 111_320.0 * lat.to_radians().cos();
        let ky = 110_540.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let Some(v) = self.posts.get(&(cy + dy, cx + dx)) else { continue };
                for (p, _) in v {
                    if h + 1.6 < p.h0 || h > p.h1 - 0.05 {
                        continue;
                    }
                    let (ex, ey) = ((lo - p.lon) * kx, (la - p.lat) * ky);
                    let d = (ex * ex + ey * ey).sqrt();
                    let min = p.radius + radius;
                    if d < min && d > 1e-6 {
                        let k = (min - d) / d;
                        lo += ex * k / kx;
                        la += ey * k / ky;
                    }
                }
            }
        }
        (la, lo)
    }

    /// Keep a body out of building walls (closed doors included).
    fn push_out_walls(&self, lat: f64, lon: f64, h: f64, radius: f64) -> (f64, f64) {
        let Some(ids) = self.solid_grid.get(&solid_cell(lat, lon)) else { return (lat, lon) };
        let (mut la, mut lo) = (lat, lon);
        for k in ids {
            let Some(s) = self.solids.get(k) else { continue };
            let pad = 0.0002;
            if la < s.bbox.0 - pad || la > s.bbox.2 + pad || lo < s.bbox.1 - pad || lo > s.bbox.3 + pad {
                continue;
            }
            let mut p = s.local.xz(la, lo);
            let mut moved = false;
            for w in &s.walls {
                if h + 1.7 < w.h0 || h + 0.3 > w.h1 || w.door.is_some_and(|d| self.open_doors.contains(&d)) {
                    continue;
                }
                let ab = w.b - w.a;
                let t = ((p - w.a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let c = w.a + ab * t;
                let d = p.distance(c);
                let r = radius as f32;
                if d < r {
                    let n = if d > 1e-4 { (p - c) / d } else { ab.perp().normalize_or(Vec2::X) };
                    p = c + n * r;
                    moved = true;
                }
            }
            if moved {
                (la, lo) = s.local.ll(p);
            }
        }
        (la, lo)
    }

    /// Forget every strip added by one owner (e.g. an unloaded road tile).
    pub fn remove_owner(&mut self, owner: u64) {
        if let Some(ids) = self.by_owner.remove(&owner) {
            for i in ids {
                self.strips[i] = None;
            }
        }
        // Compact occasionally so dead entries don't pile up.
        if self.strips.len() > 4096 && self.strips.iter().filter(|s| s.is_none()).count() > self.strips.len() / 2 {
            let live: Vec<Strip> = self.strips.drain(..).flatten().collect();
            self.grid.clear();
            self.by_owner.clear();
            for s in live {
                self.add(s);
            }
        }
    }

    /// The highest surface under a point that is at or below `max_h`, and whether it's a tunnel floor.
    pub fn surface_below(&self, lat: f64, lon: f64, max_h: f64) -> Option<(f64, bool)> {
        let strips = self.strip_surface(lat, lon, max_h);
        match (strips, self.solid_surface(lat, lon, max_h)) {
            (Some(a), Some(b)) => Some(if b > a.0 { (b, false) } else { a }),
            (a, None) => a,
            (None, Some(b)) => Some((b, false)),
        }
    }

    pub fn strip_surface(&self, lat: f64, lon: f64, max_h: f64) -> Option<(f64, bool)> {
        let ids = self.grid.get(&cell(lat, lon))?;
        let coslat = lat.to_radians().cos();
        let mut best: Option<(f64, bool)> = None;
        for &i in ids {
            let Some(s) = &self.strips[i] else { continue };
            // Project onto the strip in local metres.
            let to_m = |p: (f64, f64)| ((p.1 - s.a.1) * 111_320.0 * coslat, (p.0 - s.a.0) * 110_540.0);
            let (bx, by) = to_m(s.b);
            let (px, py) = to_m((lat, lon));
            let len2 = bx * bx + by * by;
            if len2 < 1e-6 {
                continue;
            }
            let t = ((px * bx + py * by) / len2).clamp(0.0, 1.0);
            let (dx, dy) = (px - bx * t, py - by * t);
            if dx * dx + dy * dy > s.half_width_m * s.half_width_m {
                continue;
            }
            let h = s.ha + (s.hb - s.ha) * t;
            if h <= max_h && best.is_none_or(|b| h > b.0) {
                best = Some((h, s.tunnel));
            }
        }
        best
    }
}

pub struct WalkablePlugin;

impl Plugin for WalkablePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Walkables>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_lookup() {
        let mut w = Walkables::default();
        w.add(Strip { a: (40.0, -105.0), b: (40.0, -104.99), ha: 1610.0, hb: 1620.0, half_width_m: 6.0, owner: 1, tunnel: false });
        let mid = w.surface_below(40.0, -104.995, 2000.0).unwrap().0;
        assert!((mid - 1615.0).abs() < 0.2);
        assert!(w.surface_below(40.001, -104.995, 2000.0).is_none()); // 110 m off to the side
        assert!(w.surface_below(40.0, -104.995, 1600.0).is_none()); // you're below it
        w.remove_owner(1);
        assert!(w.surface_below(40.0, -104.995, 2000.0).is_none());
    }
}
