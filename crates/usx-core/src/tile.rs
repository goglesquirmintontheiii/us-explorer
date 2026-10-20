//! Web Mercator (XYZ / "slippy map") tile addressing, shared by every tiled source.

use std::f64::consts::PI;

pub const MAX_MERCATOR_LAT: f64 = 85.051_128_779_806_59;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub const ROOT: TileId = TileId { z: 0, x: 0, y: 0 };

    pub const fn new(z: u8, x: u32, y: u32) -> Self {
        Self { z, x, y }
    }

    pub fn children(self) -> [TileId; 4] {
        let (z, x, y) = (self.z + 1, self.x * 2, self.y * 2);
        [TileId::new(z, x, y), TileId::new(z, x + 1, y), TileId::new(z, x, y + 1), TileId::new(z, x + 1, y + 1)]
    }

    pub fn parent(self) -> Option<TileId> {
        (self.z > 0).then(|| TileId::new(self.z - 1, self.x / 2, self.y / 2))
    }

    /// The ancestor at zoom `z` (or self when `z >= self.z`).
    pub fn ancestor(self, z: u8) -> TileId {
        if z >= self.z {
            return self;
        }
        let d = self.z - z;
        TileId::new(z, self.x >> d, self.y >> d)
    }

    /// Where this tile sits inside its ancestor at zoom `z`, as (u0, v0, size) in 0..1.
    pub fn rect_in_ancestor(self, z: u8) -> (f64, f64, f64) {
        if z >= self.z {
            return (0.0, 0.0, 1.0);
        }
        let d = self.z - z;
        let n = (1u32 << d) as f64;
        let ox = (self.x & ((1 << d) - 1)) as f64;
        let oy = (self.y & ((1 << d) - 1)) as f64;
        (ox / n, oy / n, 1.0 / n)
    }

    /// (west, south, east, north) in degrees.
    pub fn bounds(self) -> (f64, f64, f64, f64) {
        let n = (1u64 << self.z) as f64;
        let w = self.x as f64 / n * 360.0 - 180.0;
        let e = (self.x + 1) as f64 / n * 360.0 - 180.0;
        (w, y_to_lat(self.y as f64 + 1.0, n), e, y_to_lat(self.y as f64, n))
    }

    /// Lat/lon of a point given in tile-local (u, v), with v = 0 at the north edge.
    pub fn lat_lon_at(self, u: f64, v: f64) -> (f64, f64) {
        let n = (1u64 << self.z) as f64;
        let lon = (self.x as f64 + u) / n * 360.0 - 180.0;
        (y_to_lat(self.y as f64 + v, n), lon)
    }

    /// Ground width of the tile at its centre latitude, in metres.
    pub fn width_m(self) -> f64 {
        let (_, s, _, nn) = self.bounds();
        let lat = ((s + nn) * 0.5).to_radians();
        2.0 * PI * crate::geo::WGS84_A * lat.cos() / (1u64 << self.z) as f64
    }

    /// The tile at zoom `z` containing a point, plus the point's (u, v) inside it.
    pub fn containing(lat: f64, lon: f64, z: u8) -> (TileId, f64, f64) {
        let n = (1u64 << z) as f64;
        let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
        let fx = ((lon + 180.0) / 360.0 * n).clamp(0.0, n - 1e-9);
        let fy = ((1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0 * n).clamp(0.0, n - 1e-9);
        let (x, y) = (fx.floor(), fy.floor());
        (TileId::new(z, x as u32, y as u32), fx - x, fy - y)
    }

    pub fn path(self) -> String {
        format!("{}/{}/{}", self.z, self.x, self.y)
    }
}

fn y_to_lat(y: f64, n: f64) -> f64 {
    (PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees()
}

/// All tiles at zoom `z` intersecting a lat/lon box.
pub fn tiles_in_box(south: f64, west: f64, north: f64, east: f64, z: u8) -> impl Iterator<Item = TileId> {
    let (a, _, _) = TileId::containing(north, west, z);
    let (b, _, _) = TileId::containing(south, east, z);
    (a.y..=b.y).flat_map(move |y| (a.x..=b.x).map(move |x| TileId::new(z, x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containing_and_bounds_agree() {
        let (t, u, v) = TileId::containing(39.7392, -104.9903, 12);
        let (w, s, e, n) = t.bounds();
        assert!(w <= -104.9903 && -104.9903 <= e && s <= 39.7392 && 39.7392 <= n);
        let (lat, lon) = t.lat_lon_at(u, v);
        assert!((lat - 39.7392).abs() < 1e-9 && (lon + 104.9903).abs() < 1e-9);
    }

    #[test]
    fn ancestry() {
        let t = TileId::new(10, 300, 400);
        assert_eq!(t.ancestor(8), TileId::new(8, 75, 100));
        assert_eq!(t.parent().unwrap().children().iter().filter(|c| **c == t).count(), 1);
        let (u, v, s) = TileId::new(2, 3, 1).rect_in_ancestor(1);
        assert_eq!((u, v, s), (0.5, 0.5, 0.5));
    }
}
