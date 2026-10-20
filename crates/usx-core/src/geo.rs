//! WGS84 geodesy: geodetic <-> ECEF conversion and local East-North-Up frames.
//!
//! All world positions are kept as f64 ECEF. The renderer works in f32 relative to a
//! floating origin, using [`EnuFrame`] to turn ECEF offsets into local metres.

use glam::{DMat3, DQuat, DVec3};

pub const WGS84_A: f64 = 6_378_137.0;
pub const WGS84_F: f64 = 1.0 / 298.257_223_563;
pub const WGS84_B: f64 = WGS84_A * (1.0 - WGS84_F);
pub const WGS84_E2: f64 = WGS84_F * (2.0 - WGS84_F);
/// Mean radius used where a sphere is good enough (horizon tests, atmosphere).
pub const EARTH_MEAN_RADIUS: f64 = 6_371_008.8;

/// Latitude and longitude in degrees, height above the ellipsoid in metres.
#[derive(Clone, Copy, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Geodetic {
    pub lat: f64,
    pub lon: f64,
    pub h: f64,
}

impl Geodetic {
    pub const fn new(lat: f64, lon: f64, h: f64) -> Self {
        Self { lat, lon, h }
    }

    pub fn to_ecef(self) -> DVec3 {
        let (sl, cl) = self.lat.to_radians().sin_cos();
        let (so, co) = self.lon.to_radians().sin_cos();
        let n = WGS84_A / (1.0 - WGS84_E2 * sl * sl).sqrt();
        DVec3::new((n + self.h) * cl * co, (n + self.h) * cl * so, (n * (1.0 - WGS84_E2) + self.h) * sl)
    }

    /// Bowring's method, accurate to well under a millimetre near the surface.
    pub fn from_ecef(p: DVec3) -> Self {
        let lon = p.y.atan2(p.x);
        let r = (p.x * p.x + p.y * p.y).sqrt();
        let ep2 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B);
        let theta = (p.z * WGS84_A).atan2(r * WGS84_B);
        let (st, ct) = theta.sin_cos();
        let lat = (p.z + ep2 * WGS84_B * st * st * st).atan2(r - WGS84_E2 * WGS84_A * ct * ct * ct);
        let sl = lat.sin();
        let n = WGS84_A / (1.0 - WGS84_E2 * sl * sl).sqrt();
        let h = if lat.cos().abs() > 1e-9 { r / lat.cos() - n } else { p.z.abs() - WGS84_B };
        Self { lat: lat.to_degrees(), lon: lon.to_degrees(), h }
    }

    /// Unit normal to the ellipsoid ("up") at this point.
    pub fn up(self) -> DVec3 {
        let (sl, cl) = self.lat.to_radians().sin_cos();
        let (so, co) = self.lon.to_radians().sin_cos();
        DVec3::new(cl * co, cl * so, sl)
    }

    /// Great-circle distance in metres (haversine on the mean sphere).
    pub fn distance(self, o: Geodetic) -> f64 {
        let (p1, p2) = (self.lat.to_radians(), o.lat.to_radians());
        let dp = p2 - p1;
        let dl = (o.lon - self.lon).to_radians();
        let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
        2.0 * EARTH_MEAN_RADIUS * a.sqrt().asin()
    }
}

/// A local tangent frame. Engine axes are Y-up: +X east, +Y up, -Z north
/// (so a camera looking down -Z faces north).
#[derive(Clone, Copy, Debug)]
pub struct EnuFrame {
    pub origin: DVec3,
    /// Rotation taking ECEF vectors into local engine axes.
    pub ecef_to_local: DMat3,
}

impl EnuFrame {
    pub fn at(g: Geodetic) -> Self {
        let origin = g.to_ecef();
        let (sl, cl) = g.lat.to_radians().sin_cos();
        let (so, co) = g.lon.to_radians().sin_cos();
        let east = DVec3::new(-so, co, 0.0);
        let north = DVec3::new(-sl * co, -sl * so, cl);
        let up = DVec3::new(cl * co, cl * so, sl);
        // Rows are the local axes expressed in ECEF: x = east, y = up, z = -north.
        let ecef_to_local = DMat3::from_cols(east, up, -north).transpose();
        Self { origin, ecef_to_local }
    }

    pub fn to_local(&self, ecef: DVec3) -> DVec3 {
        self.ecef_to_local * (ecef - self.origin)
    }

    pub fn dir_to_local(&self, v: DVec3) -> DVec3 {
        self.ecef_to_local * v
    }

    pub fn to_ecef(&self, local: DVec3) -> DVec3 {
        self.ecef_to_local.transpose() * local + self.origin
    }

    /// The rotation as a quaternion (ECEF -> local), for placing ECEF-axis meshes.
    pub fn rotation(&self) -> DQuat {
        DQuat::from_mat3(&self.ecef_to_local)
    }
}

/// Parse "lat, lon" (decimal degrees, optional N/S/E/W) into a position.
pub fn parse_lat_lon(s: &str) -> Option<(f64, f64)> {
    let cleaned = s.replace('°', " ");
    let parts: Vec<&str> = cleaned.split([',', ' ', ';']).filter(|p| !p.is_empty()).collect();
    let mut nums = Vec::new();
    for p in parts {
        let up = p.to_ascii_uppercase();
        let (num, sign) = match up.chars().last() {
            Some('N') | Some('E') => (&up[..up.len() - 1], 1.0),
            Some('S') | Some('W') => (&up[..up.len() - 1], -1.0),
            _ => (up.as_str(), 1.0),
        };
        if num.is_empty() {
            if let Some(last) = nums.last_mut() {
                *last *= sign;
            }
            continue;
        }
        nums.push(num.parse::<f64>().ok()? * sign);
    }
    match nums.as_slice() {
        [lat, lon] if lat.abs() <= 90.0 && lon.abs() <= 180.0 => Some((*lat, *lon)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecef_round_trip() {
        for g in [Geodetic::new(39.7392, -104.9903, 1609.0), Geodetic::new(-33.9, 151.2, 5.0), Geodetic::new(64.8, -147.7, 130.0)] {
            let b = Geodetic::from_ecef(g.to_ecef());
            assert!((b.lat - g.lat).abs() < 1e-9 && (b.lon - g.lon).abs() < 1e-9 && (b.h - g.h).abs() < 1e-4, "{b:?}");
        }
    }

    #[test]
    fn enu_axes() {
        let g = Geodetic::new(40.0, -100.0, 0.0);
        let f = EnuFrame::at(g);
        let up = f.dir_to_local(g.up());
        assert!((up - DVec3::Y).length() < 1e-12);
        let north_pt = Geodetic::new(40.001, -100.0, 0.0).to_ecef();
        let l = f.to_local(north_pt);
        assert!(l.z < -100.0 && l.x.abs() < 1e-3);
    }

    #[test]
    fn parse_coords() {
        assert_eq!(parse_lat_lon("39.5, -98.35"), Some((39.5, -98.35)));
        assert_eq!(parse_lat_lon("39.5N 98.35W"), Some((39.5, -98.35)));
        assert_eq!(parse_lat_lon("Denver"), None);
    }
}

/// Dry land below sea level in the US: Death Valley, the Salton trough and Imperial
/// Valley, and New Orleans behind its levees. Elsewhere, ground below 0 m is sea.
/// Takes a box (south, west, north, east) and says whether it touches one of them.
pub fn dry_below_sea_level(s: f64, w: f64, n: f64, e: f64) -> bool {
    const BASINS: [(f64, f64, f64, f64); 3] = [(35.6, -117.6, 37.0, -116.3), (32.4, -116.4, 33.8, -115.0), (29.85, -90.3, 30.1, -89.85)];
    BASINS.iter().any(|&(bs, bw, bn, be)| n > bs && s < bn && e > bw && w < be)
}
