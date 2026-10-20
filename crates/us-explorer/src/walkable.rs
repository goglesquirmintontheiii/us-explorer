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
}

/// Roughly 1 km cells.
const CELL_DEG: f64 = 0.01;

#[derive(Resource, Default)]
pub struct Walkables {
    strips: Vec<Option<Strip>>,
    grid: HashMap<(i32, i32), Vec<usize>>,
    by_owner: HashMap<u64, Vec<usize>>,
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

    /// The highest surface under a point that is at or below `max_h`.
    pub fn surface_below(&self, lat: f64, lon: f64, max_h: f64) -> Option<f64> {
        let ids = self.grid.get(&cell(lat, lon))?;
        let coslat = lat.to_radians().cos();
        let mut best: Option<f64> = None;
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
            if h <= max_h && best.is_none_or(|b| h > b) {
                best = Some(h);
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
        w.add(Strip { a: (40.0, -105.0), b: (40.0, -104.99), ha: 1610.0, hb: 1620.0, half_width_m: 6.0, owner: 1 });
        let mid = w.surface_below(40.0, -104.995, 2000.0).unwrap();
        assert!((mid - 1615.0).abs() < 0.2);
        assert!(w.surface_below(40.001, -104.995, 2000.0).is_none()); // 110 m off to the side
        assert!(w.surface_below(40.0, -104.995, 1600.0).is_none()); // you're below it
        w.remove_owner(1);
        assert!(w.surface_below(40.0, -104.995, 2000.0).is_none());
    }
}
