//! Surfaces you can stand on besides the terrain: bridge decks and overpasses for now,
//! later floors and roofs. Each is a straight strip between two points with a height at
//! each end, kept in a coarse grid so lookups stay cheap.

use bevy::prelude::*;
use std::collections::HashMap;

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

    /// Push a body of `radius` metres at (lat, lon, feet height h) out of any posts.
    /// Returns the corrected position.
    pub fn push_out(&self, lat: f64, lon: f64, h: f64, radius: f64) -> (f64, f64) {
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
