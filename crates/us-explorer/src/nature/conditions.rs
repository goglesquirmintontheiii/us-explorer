//! Live conditions where you are, refreshed every quarter hour in the background: wind
//! and cloud cover (Open-Meteo), sea state (Open-Meteo marine model, checked against
//! the nearest NOAA buoy when one reports nearby), and the aurora forecast (NOAA SWPC
//! OVATION model and Kp index). Used for swaying trees, wind turbines, waves, chop on
//! lakes and the northern lights. Offline, calm defaults are used.
//!
//! (The full live weather system arrives in Phase 7; this is just what Phase 5 needs.)

use crate::globe::Viewer;
use bevy::prelude::*;
use std::time::Duration;

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Conditions {
    /// Wind at 10 m, metres per second, and the direction it blows from (degrees).
    pub wind_speed: f32,
    pub wind_from: f32,
    /// 0..1.
    pub cloud_cover: f32,
    pub temperature_c: f32,
    /// Significant wave height (m), peak period (s), direction waves come from (deg).
    pub wave_height: f32,
    pub wave_period: f32,
    pub wave_from: f32,
    /// Swell (long waves from far away).
    pub swell_height: f32,
    pub swell_period: f32,
    pub swell_from: f32,
    /// Planetary K index (0..9).
    pub kp: f32,
    /// OVATION aurora intensity overhead and towards the pole (0..100).
    pub aurora_here: f32,
    pub aurora_north: f32,
    /// Where these numbers came from, for the debug overlay.
    pub source: String,
    /// When they were fetched (game seconds since start), None if never.
    pub fetched: Option<f64>,
}

impl Default for Conditions {
    fn default() -> Self {
        Self {
            wind_speed: 3.5,
            wind_from: 250.0,
            cloud_cover: 0.2,
            temperature_c: 18.0,
            wave_height: 0.6,
            wave_period: 6.0,
            wave_from: 90.0,
            swell_height: 0.5,
            swell_period: 9.0,
            swell_from: 100.0,
            kp: 2.0,
            aurora_here: 0.0,
            aurora_north: 0.0,
            source: "defaults (offline)".into(),
            fetched: None,
        }
    }
}

#[derive(Resource)]
struct Fetcher {
    rx: Option<crossbeam_channel::Receiver<Conditions>>,
    last_pos: Option<(f64, f64)>,
    next_at: f64,
}

pub struct ConditionsPlugin;

impl Plugin for ConditionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Conditions>().insert_resource(Fetcher { rx: None, last_pos: None, next_at: 2.0 }).add_systems(Update, refresh);
    }
}

fn refresh(mut f: ResMut<Fetcher>, mut c: ResMut<Conditions>, viewer: Res<Viewer>, time: Res<Time>, args: Res<crate::Args>) {
    let now = time.elapsed_secs_f64();
    if let Some(rx) = &f.rx
        && let Ok(mut got) = rx.try_recv()
    {
        got.fetched = Some(now);
        if *c != got {
            *c = got;
        }
        f.rx = None;
    }
    // Scripted screenshots keep the defaults (repeatable pictures).
    if f.rx.is_some() || !args.shots.is_empty() {
        return;
    }
    let g = viewer.geo();
    let moved = f.last_pos.is_none_or(|(la, lo)| (la - g.lat).abs() > 0.4 || (lo - g.lon).abs() > 0.4);
    if now < f.next_at && !moved {
        return;
    }
    if now < f.next_at - 840.0 {
        // Moved, but give it a minute between fetches.
        return;
    }
    f.next_at = now + 900.0;
    f.last_pos = Some((g.lat, g.lon));
    let (lat, lon) = (g.lat, g.lon);
    let base = c.clone();
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send(fetch(lat, lon, base));
    });
    f.rx = Some(rx);
}

#[derive(serde::Deserialize)]
struct OmCurrent<T> {
    current: T,
}

#[derive(serde::Deserialize)]
struct OmWeather {
    wind_speed_10m: Option<f32>,
    wind_direction_10m: Option<f32>,
    cloud_cover: Option<f32>,
    temperature_2m: Option<f32>,
}

#[derive(serde::Deserialize)]
struct OmMarine {
    wave_height: Option<f32>,
    wave_direction: Option<f32>,
    wave_period: Option<f32>,
    swell_wave_height: Option<f32>,
    swell_wave_direction: Option<f32>,
    swell_wave_period: Option<f32>,
}

#[derive(serde::Deserialize)]
struct Ovation {
    coordinates: Vec<[f32; 3]>,
}

fn fetch(lat: f64, lon: f64, mut c: Conditions) -> Conditions {
    let mut sources = Vec::new();
    let w = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.3}&longitude={lon:.3}&current=wind_speed_10m,wind_direction_10m,cloud_cover,temperature_2m&wind_speed_unit=ms"
    );
    if let Ok(r) = usx_core::net::get_json::<OmCurrent<OmWeather>>(&w) {
        let cur = r.current;
        c.wind_speed = cur.wind_speed_10m.unwrap_or(c.wind_speed);
        c.wind_from = cur.wind_direction_10m.unwrap_or(c.wind_from);
        c.cloud_cover = cur.cloud_cover.map(|x| x / 100.0).unwrap_or(c.cloud_cover);
        c.temperature_c = cur.temperature_2m.unwrap_or(c.temperature_c);
        sources.push("Open-Meteo");
    }
    let m = format!(
        "https://marine-api.open-meteo.com/v1/marine?latitude={lat:.3}&longitude={lon:.3}&current=wave_height,wave_direction,wave_period,swell_wave_height,swell_wave_direction,swell_wave_period"
    );
    if let Ok(r) = usx_core::net::get_json::<OmCurrent<OmMarine>>(&m) {
        let cur = r.current;
        if let Some(h) = cur.wave_height {
            c.wave_height = h;
            c.wave_period = cur.wave_period.unwrap_or(c.wave_period);
            c.wave_from = cur.wave_direction.unwrap_or(c.wave_from);
            c.swell_height = cur.swell_wave_height.unwrap_or(h * 0.8);
            c.swell_period = cur.swell_wave_period.unwrap_or(c.wave_period);
            c.swell_from = cur.swell_wave_direction.unwrap_or(c.wave_from);
            sources.push("Open-Meteo Marine");
        }
    }
    // A NOAA buoy close by trumps the model for wave height and period.
    if let Ok(b) = usx_core::net::get("https://www.ndbc.noaa.gov/data/latest_obs/latest_obs.txt")
        && let Some((h, p, dir)) = nearest_buoy(&String::from_utf8_lossy(&b), lat, lon, 60.0)
    {
        c.wave_height = h;
        if let Some(p) = p {
            c.wave_period = p;
        }
        if let Some(d) = dir {
            c.wave_from = d;
        }
        sources.push("NOAA NDBC buoy");
    }
    if let Ok(k) = usx_core::net::get_json::<Vec<serde_json::Value>>("https://services.swpc.noaa.gov/products/noaa-planetary-k-index.json")
        && let Some(kp) = k.iter().rev().find_map(|v| v.get("Kp").and_then(|x| x.as_f64()))
    {
        c.kp = kp as f32;
        sources.push("NOAA SWPC Kp");
    }
    if lat > 30.0
        && let Ok(o) = usx_core::net::get_json::<Ovation>("https://services.swpc.noaa.gov/json/ovation_aurora_latest.json")
    {
        let lon360 = lon.rem_euclid(360.0) as f32;
        let (mut here, mut north) = (0.0f32, 0.0f32);
        for p in &o.coordinates {
            let dlon = ((p[0] - lon360 + 540.0) % 360.0 - 180.0).abs();
            let dlat = p[1] - lat as f32;
            if dlon <= 2.0 && dlat.abs() <= 2.0 {
                here = here.max(p[2]);
            }
            // The aurora is 100+ km up: seen up to ~10 degrees towards the pole.
            if dlon <= 8.0 && dlat > 0.0 && dlat <= 12.0 {
                north = north.max(p[2] * (1.0 - dlat / 14.0));
            }
        }
        c.aurora_here = here;
        c.aurora_north = north;
        sources.push("NOAA SWPC OVATION");
    }
    c.source = if sources.is_empty() { "defaults (offline)".into() } else { sources.join(", ") };
    std::thread::sleep(Duration::from_millis(1));
    c
}

/// Nearest NDBC station reporting wave height within `max_km`: (height, period, from).
fn nearest_buoy(txt: &str, lat: f64, lon: f64, max_km: f64) -> Option<(f32, Option<f32>, Option<f32>)> {
    let mut best: Option<(f64, (f32, Option<f32>, Option<f32>))> = None;
    for line in txt.lines() {
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 15 {
            continue;
        }
        let (Ok(la), Ok(lo)) = (f[1].parse::<f64>(), f[2].parse::<f64>()) else { continue };
        let Ok(wvht) = f[11].parse::<f32>() else { continue };
        let dpd = f[12].parse::<f32>().ok();
        let mwd = f[14].parse::<f32>().ok();
        let dy = (la - lat) * 111.0;
        let dx = (lo - lon) * 111.0 * lat.to_radians().cos();
        let d = (dx * dx + dy * dy).sqrt();
        if d < max_km && best.as_ref().is_none_or(|b| d < b.0) {
            best = Some((d, (wvht, dpd, mwd)));
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buoy_table() {
        let t = "#STN       LAT      LON  YYYY MM DD hh mm WDIR WSPD   GST WVHT  DPD APD MWD   PRES  PTDY  ATMP  WTMP  DEWP  VIS   TIDE\n\
#text      deg      deg   yr mo day hr mn degT  m/s   m/s   m   sec sec degT   hPa   hPa  degC  degC  degC  nmi     ft\n\
41009    28.508  -80.185 2026 10 21 14 30  40   6.0   8.0   MM  MM   MM  MM 1017.2    MM  26.1    MM  18.2   MM     MM\n\
41114    27.551  -80.225 2026 10 21 14 30   MM    MM    MM  1.1  8.0  5.4  76     MM    MM    MM  27.6    MM   MM     MM\n";
        let b = nearest_buoy(t, 27.6, -80.3, 60.0).unwrap();
        assert_eq!(b.0, 1.1);
        assert_eq!(b.1, Some(8.0));
        assert_eq!(b.2, Some(76.0));
        assert!(nearest_buoy(t, 40.0, -74.0, 60.0).is_none());
    }
}
