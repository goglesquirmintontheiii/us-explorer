//! Place search through OpenStreetMap Nominatim. The public server allows at most one
//! request per second and no search-as-you-type, so callers search on Enter only.

use crate::net::{self, NetError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub kind: String,
}

#[derive(serde::Deserialize)]
struct NomHit {
    lat: String,
    lon: String,
    display_name: String,
    #[serde(default)]
    r#type: String,
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn search(query: &str) -> Result<Vec<Place>, NetError> {
    // Space requests at least 1.1 s apart, as Nominatim's usage policy requires.
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut l = LAST.lock().unwrap();
        if let Some(t) = *l {
            let since = t.elapsed();
            if since < Duration::from_millis(1100) {
                std::thread::sleep(Duration::from_millis(1100) - since);
            }
        }
        *l = Some(Instant::now());
    }
    let url = format!("{}?q={}&format=jsonv2&limit=8&countrycodes=us", crate::sources::NOMINATIM, encode(query.trim()));
    let hits: Vec<NomHit> = net::get_json(&url)?;
    Ok(hits
        .into_iter()
        .filter_map(|h| Some(Place { lat: h.lat.parse().ok()?, lon: h.lon.parse().ok()?, name: h.display_name, kind: h.r#type }))
        .collect())
}

#[derive(serde::Deserialize)]
struct NomReverse {
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
}

/// A short name for a point ("Coors Field, Denver"), or None when nothing is known.
pub fn reverse(lat: f64, lon: f64) -> Result<Option<String>, NetError> {
    let url = format!("{}?lat={lat:.6}&lon={lon:.6}&format=jsonv2&zoom=16", crate::sources::NOMINATIM.replace("/search", "/reverse"));
    let r: NomReverse = net::get_json(&url)?;
    let parts: Vec<&str> = r.display_name.split(", ").collect();
    let short = if !r.name.is_empty() {
        let place = parts.iter().skip(1).find(|p| !p.chars().next().is_some_and(|c| c.is_ascii_digit())).copied().unwrap_or("");
        if place.is_empty() { r.name.clone() } else { format!("{}, {}", r.name, place) }
    } else {
        parts.iter().take(2).copied().collect::<Vec<_>>().join(", ")
    };
    Ok((!short.is_empty()).then_some(short))
}
