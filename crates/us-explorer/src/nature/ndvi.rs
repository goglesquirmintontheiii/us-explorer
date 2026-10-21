//! Satellite vegetation greenness (NDVI) for the seasons: NASA GIBS's MODIS 8-day NDVI
//! composites around you, now and at the year's greenest and brownest (mid-July and
//! mid-January), give how far through green-up or leaf fall the area is. The phenology
//! model (species::season) fills in when offline or where the signal is weak.

use super::ndvi_palette::NDVI_RGB;
use std::collections::HashMap;
use std::sync::OnceLock;
use usx_core::tile::TileId;

/// Zoom of the NDVI tiles (about 600 km across; the composites are 1 km pixels).
const ZOOM: u8 = 6;

fn lookup() -> &'static HashMap<[u8; 3], f32> {
    static T: OnceLock<HashMap<[u8; 3], f32>> = OnceLock::new();
    T.get_or_init(|| NDVI_RGB.iter().map(|&(r, g, b, v)| ([r, g, b], v)).collect())
}

fn url(date: &str, t: TileId) -> String {
    format!(
        "https://gibs.earthdata.nasa.gov/wmts/epsg3857/best/MODIS_Terra_NDVI_8Day/default/{date}/GoogleMapsCompatible_Level9/{}/{}/{}.png",
        t.z, t.y, t.x
    )
}

/// Mean NDVI within ~40 km of a point on one date's composite.
fn sample(date: &str, lat: f64, lon: f64) -> Option<f32> {
    let (t, u, v) = TileId::containing(lat, lon, ZOOM);
    let key = format!("ndvi/{date}/{}.png", t.path());
    let bytes = crate::terrain::load::fetch(&key, &url(date, t)).ok()?;
    let img = usx_core::image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (cx, cy) = ((u * w as f64) as i64, (v * h as f64) as i64);
    let r = 6;
    let (mut sum, mut n) = (0.0f32, 0);
    for y in (cy - r).max(0)..(cy + r).min(h) {
        for x in (cx - r).max(0)..(cx + r).min(w) {
            let p = img.get_pixel(x as u32, y as u32);
            if p[3] == 0 {
                continue;
            }
            if let Some(&nd) = lookup().get(&[p[0], p[1], p[2]]) {
                sum += nd;
                n += 1;
            }
        }
    }
    (n > 8).then(|| sum / n as f32)
}

/// Satellite greenness where you are: (now, January, July) NDVI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Greenness {
    pub now: f32,
    pub winter: f32,
    pub summer: f32,
}

impl Greenness {
    /// How green relative to the year's range (0 = winter brown, 1 = summer peak);
    /// None where the year barely changes (evergreen or desert).
    pub fn leafiness(&self) -> Option<f32> {
        let span = self.summer - self.winter;
        (span > 0.12).then(|| ((self.now - self.winter) / span).clamp(0.0, 1.0))
    }
}

/// Fetch the three composites for a place and a unix time (blocking; call on a thread).
pub fn fetch(lat: f64, lon: f64, unix: f64) -> Option<Greenness> {
    // Composites are published about two weeks behind.
    let then = unix - 16.0 * 86_400.0;
    let date = crate::sun::format_utc(then);
    let ymd = &date[..10];
    let year: i32 = ymd[..4].parse().ok()?;
    let month: u32 = ymd[5..7].parse().ok()?;
    // This year's July once it's passed, else last year's.
    let jy = if month >= 8 { year } else { year - 1 };
    let now = sample(ymd, lat, lon)?;
    let summer = sample(&format!("{jy}-07-12"), lat, lon)?;
    let winter = sample(&format!("{}-01-17", jy.max(year - 1) + if month >= 8 { 0 } else { 1 }), lat, lon).or_else(|| sample(&format!("{jy}-01-17"), lat, lon))?;
    Some(Greenness { now, winter: winter.min(summer), summer: summer.max(winter) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leafiness() {
        let g = Greenness { now: 0.6, winter: 0.3, summer: 0.8 };
        assert!((g.leafiness().unwrap() - 0.6).abs() < 1e-5);
        let evergreen = Greenness { now: 0.8, winter: 0.75, summer: 0.82 };
        assert!(evergreen.leafiness().is_none());
        assert!(lookup().len() > 100);
    }
}
