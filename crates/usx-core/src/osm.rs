//! OpenStreetMap features from the Overpass API, one zoom-14 tile (about 2 km) at a time.
//!
//! Only the tags the game uses are kept, and tiles are cached in a compact binary form
//! (delta-coded coordinates, interned strings), typically 5-10x smaller than the JSON.

use crate::cache::{DiskCache, MISSING};
use crate::net::{self, NetError};
use crate::tile::TileId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const ZOOM: u8 = 14;

/// Public Overpass instances, tried in turn when one is busy or down.
const ENDPOINTS: [&str; 3] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];

/// Tags worth keeping; everything else is dropped on arrival.
const KEEP: &[&str] = &[
    "highway",
    "name",
    "ref",
    "lanes",
    "lanes:forward",
    "lanes:backward",
    "oneway",
    "bridge",
    "tunnel",
    "layer",
    "level",
    "surface",
    "smoothness",
    "sidewalk",
    "sidewalk:both",
    "sidewalk:left",
    "sidewalk:right",
    "width",
    "turn:lanes",
    "turn:lanes:forward",
    "turn:lanes:backward",
    "railway",
    "barrier",
    "junction",
    "construction",
    "service",
    "crossing",
    "crossing:markings",
    "footway",
    "man_made",
    "surveillance",
    "surveillance:type",
    "emergency",
    "amenity",
    "power",
    "shelter",
    "bench",
    "direction",
    "traffic_signals:direction",
    "stop",
    "maxspeed",
    "lit",
    "covered",
    "public_transport",
    "location",
    "gauge",
    "electrified",
    "usage",
    "embankment",
    "cutting",
    "area",
    "access",
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tags(pub Vec<(String, String)>);

impl Tags {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }
    pub fn is(&self, k: &str, v: &str) -> bool {
        self.get(k) == Some(v)
    }
    /// `yes`, `true`, `1` (and for bridges/tunnels any non-`no` value) count as set.
    pub fn flag(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| v != "no" && v != "false" && v != "0")
    }
    pub fn num(&self, k: &str) -> Option<f64> {
        let v = self.get(k)?;
        let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(v.len());
        v[..end].parse().ok()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
    pub tags: Tags,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    pub id: i64,
    pub nodes: Vec<i64>,
    /// (lat, lon) per node, same order as `nodes`.
    pub pts: Vec<(f64, f64)>,
    pub tags: Tags,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsmTile {
    pub nodes: Vec<Node>,
    pub ways: Vec<Way>,
}

/// The Overpass QL for one tile: roads, railways, guardrails and roadside objects.
pub fn query(id: TileId) -> String {
    let (w, s, e, n) = id.bounds();
    let b = format!("{s:.6},{w:.6},{n:.6},{e:.6}");
    format!(
        "[out:json][timeout:50];(\
way[highway]({b});\
way[railway~\"^(rail|light_rail|subway|tram|narrow_gauge|monorail)$\"]({b});\
way[barrier=guard_rail]({b});\
node[highway~\"^(traffic_signals|stop|give_way|street_lamp|crossing|bus_stop|construction|motorway_junction)$\"]({b});\
node[emergency=fire_hydrant]({b});\
node[amenity~\"^(bench|post_box)$\"]({b});\
node[power=pole]({b});\
node[man_made=surveillance]({b});\
node[barrier~\"^(bollard|traffic_cone)$\"]({b});\
);out body geom qt;"
    )
}

#[derive(serde::Deserialize)]
struct OpResponse {
    elements: Vec<OpElement>,
}

#[derive(serde::Deserialize)]
struct OpElement {
    r#type: String,
    id: i64,
    lat: Option<f64>,
    lon: Option<f64>,
    #[serde(default)]
    nodes: Vec<i64>,
    #[serde(default)]
    geometry: Vec<Option<OpPoint>>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct OpPoint {
    lat: f64,
    lon: f64,
}

fn keep_tags(t: HashMap<String, String>) -> Tags {
    let mut v: Vec<(String, String)> = t.into_iter().filter(|(k, _)| KEEP.contains(&k.as_str())).collect();
    v.sort();
    Tags(v)
}

/// Parse an Overpass JSON answer.
pub fn parse(json: &[u8]) -> Result<OsmTile, String> {
    let r: OpResponse = serde_json::from_slice(json).map_err(|e| format!("bad Overpass answer: {e}"))?;
    let mut out = OsmTile::default();
    for el in r.elements {
        match el.r#type.as_str() {
            "node" => {
                if let (Some(lat), Some(lon)) = (el.lat, el.lon) {
                    out.nodes.push(Node { id: el.id, lat, lon, tags: keep_tags(el.tags) });
                }
            }
            "way" => {
                // Ways leaving the query area can have null geometry entries; keep the
                // longest run with coordinates so ids and points stay paired.
                if el.nodes.len() != el.geometry.len() {
                    continue;
                }
                let (mut nodes, mut pts) = (Vec::new(), Vec::new());
                for (nid, p) in el.nodes.iter().zip(&el.geometry) {
                    if let Some(p) = p {
                        nodes.push(*nid);
                        pts.push((p.lat, p.lon));
                    }
                }
                if pts.len() >= 2 {
                    out.ways.push(Way { id: el.id, nodes, pts, tags: keep_tags(el.tags) });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

static NEXT_ENDPOINT: AtomicUsize = AtomicUsize::new(0);

/// Download one tile from Overpass, trying each public instance once.
pub fn download(id: TileId) -> Result<OsmTile, NetError> {
    let q = query(id);
    let enc: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let start = NEXT_ENDPOINT.load(Ordering::Relaxed);
    let mut last = NetError::Io("no Overpass server answered".into());
    for i in 0..ENDPOINTS.len() {
        let ep = ENDPOINTS[(start + i) % ENDPOINTS.len()];
        match net::get_with(&format!("{ep}?data={enc}"), "application/json", |_, _| {}) {
            Ok(b) => {
                // Overpass reports timeouts inside a 200 answer as a "remark".
                if b.len() < 4096 && String::from_utf8_lossy(&b).contains("\"remark\"") {
                    last = NetError::Io("Overpass query timed out".into());
                    continue;
                }
                return parse(&b).map_err(NetError::Io);
            }
            Err(e) => {
                // Busy (429/504) or down: move the next request to another instance too.
                NEXT_ENDPOINT.store((start + i + 1) % ENDPOINTS.len(), Ordering::Relaxed);
                last = e;
            }
        }
    }
    Err(last)
}

fn cache_key(id: TileId) -> String {
    format!("osm/v1/{}.bin", id.path())
}

/// A tile from the disk cache, or downloaded and cached.
pub fn load(id: TileId, cache: &DiskCache) -> Result<OsmTile, NetError> {
    let key = cache_key(id);
    if let Some(b) = cache.get(&key)
        && b != MISSING
        && let Some(t) = decode(&b)
    {
        return Ok(t);
    }
    let t = download(id)?;
    cache.put(&key, &encode(&t));
    Ok(t)
}

/// Cached copy only (no network).
pub fn cached(id: TileId, cache: &DiskCache) -> Option<OsmTile> {
    cache.get(&cache_key(id)).filter(|b| b != MISSING).and_then(|b| decode(&b))
}

// ---------------------------------------------------------------- binary codec

const MAGIC: &[u8; 4] = b"OSM1";

fn put_var(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_zig(out: &mut Vec<u8>, v: i64) {
    put_var(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn fix(d: f64) -> i64 {
    (d * 1e7).round() as i64
}

pub fn encode(t: &OsmTile) -> Vec<u8> {
    // String table first: every tag key and value, interned.
    let mut strings: Vec<&str> = Vec::new();
    let mut index: HashMap<&str, u64> = HashMap::new();
    let all_tags = t.nodes.iter().map(|n| &n.tags).chain(t.ways.iter().map(|w| &w.tags));
    for tags in all_tags {
        for (k, v) in &tags.0 {
            for s in [k.as_str(), v.as_str()] {
                index.entry(s).or_insert_with(|| {
                    strings.push(s);
                    (strings.len() - 1) as u64
                });
            }
        }
    }
    let tags_out = |body: &mut Vec<u8>, tags: &Tags| {
        put_var(body, tags.0.len() as u64);
        for (k, v) in &tags.0 {
            put_var(body, index[k.as_str()]);
            put_var(body, index[v.as_str()]);
        }
    };
    let mut body = Vec::new();
    put_var(&mut body, t.nodes.len() as u64);
    let (mut pid, mut plat, mut plon) = (0i64, 0i64, 0i64);
    for n in &t.nodes {
        put_zig(&mut body, n.id - pid);
        put_zig(&mut body, fix(n.lat) - plat);
        put_zig(&mut body, fix(n.lon) - plon);
        (pid, plat, plon) = (n.id, fix(n.lat), fix(n.lon));
        tags_out(&mut body, &n.tags);
    }
    put_var(&mut body, t.ways.len() as u64);
    let mut pwid = 0i64;
    for w in &t.ways {
        put_zig(&mut body, w.id - pwid);
        pwid = w.id;
        put_var(&mut body, w.nodes.len() as u64);
        let mut pn = 0i64;
        for &n in &w.nodes {
            put_zig(&mut body, n - pn);
            pn = n;
        }
        for &(la, lo) in &w.pts {
            put_zig(&mut body, fix(la) - plat);
            put_zig(&mut body, fix(lo) - plon);
            (plat, plon) = (fix(la), fix(lo));
        }
        tags_out(&mut body, &w.tags);
    }
    let mut out = MAGIC.to_vec();
    put_var(&mut out, strings.len() as u64);
    for s in &strings {
        put_var(&mut out, s.len() as u64);
        out.extend_from_slice(s.as_bytes());
    }
    out.extend_from_slice(&body);
    out
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn var(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let byte = *self.b.get(self.i)?;
            self.i += 1;
            v |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }
    fn zig(&mut self) -> Option<i64> {
        let v = self.var()?;
        Some(((v >> 1) as i64) ^ -((v & 1) as i64))
    }
}

pub fn decode(b: &[u8]) -> Option<OsmTile> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let mut r = Reader { b, i: 4 };
    let ns = r.var()? as usize;
    let mut strings = Vec::with_capacity(ns.min(1 << 20));
    for _ in 0..ns {
        let len = r.var()? as usize;
        let s = std::str::from_utf8(r.b.get(r.i..r.i + len)?).ok()?.to_string();
        r.i += len;
        strings.push(s);
    }
    let tags = |r: &mut Reader| -> Option<Tags> {
        let n = r.var()? as usize;
        let mut v = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            v.push((strings.get(r.var()? as usize)?.clone(), strings.get(r.var()? as usize)?.clone()));
        }
        Some(Tags(v))
    };
    let mut t = OsmTile::default();
    let (mut pid, mut plat, mut plon) = (0i64, 0i64, 0i64);
    let nn = r.var()? as usize;
    for _ in 0..nn {
        pid += r.zig()?;
        plat += r.zig()?;
        plon += r.zig()?;
        let tg = tags(&mut r)?;
        t.nodes.push(Node { id: pid, lat: plat as f64 / 1e7, lon: plon as f64 / 1e7, tags: tg });
    }
    let nw = r.var()? as usize;
    let mut pwid = 0i64;
    for _ in 0..nw {
        pwid += r.zig()?;
        let k = r.var()? as usize;
        let mut nodes = Vec::with_capacity(k.min(1 << 16));
        let mut pn = 0i64;
        for _ in 0..k {
            pn += r.zig()?;
            nodes.push(pn);
        }
        let mut pts = Vec::with_capacity(k.min(1 << 16));
        for _ in 0..k {
            plat += r.zig()?;
            plon += r.zig()?;
            pts.push((plat as f64 / 1e7, plon as f64 / 1e7));
        }
        let tg = tags(&mut r)?;
        t.ways.push(Way { id: pwid, nodes, pts, tags: tg });
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":0.6,"elements":[
      {"type":"node","id":42,"lat":39.7471,"lon":-104.9932,"tags":{"highway":"traffic_signals","note":"x"}},
      {"type":"way","id":7,"nodes":[1,2,3],"geometry":[{"lat":39.7,"lon":-105.0},{"lat":39.71,"lon":-105.01},null],
       "tags":{"highway":"residential","name":"Larimer Street","fixme":"drop me"}}
    ]}"#;

    #[test]
    fn parses_and_round_trips() {
        let t = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(t.nodes[0].tags.get("note"), None);
        assert_eq!(t.ways.len(), 1);
        assert_eq!(t.ways[0].nodes, vec![1, 2]);
        assert_eq!(t.ways[0].tags.get("name"), Some("Larimer Street"));
        assert!(t.ways[0].tags.get("fixme").is_none());
        let b = encode(&t);
        let back = decode(&b).unwrap();
        assert_eq!(back.ways[0].nodes, t.ways[0].nodes);
        assert_eq!(back.ways[0].tags, t.ways[0].tags);
        assert!((back.nodes[0].lat - 39.7471).abs() < 1e-7);
        assert!((back.ways[0].pts[1].1 + 105.01).abs() < 1e-7);
    }

    #[test]
    fn tag_helpers() {
        let t = Tags(vec![("lanes".into(), "3".into()), ("width".into(), "7.5 m".into()), ("bridge".into(), "viaduct".into())]);
        assert_eq!(t.num("lanes"), Some(3.0));
        assert_eq!(t.num("width"), Some(7.5));
        assert!(t.flag("bridge"));
        assert!(!t.flag("tunnel"));
    }
}
