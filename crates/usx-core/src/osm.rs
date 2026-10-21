//! OpenStreetMap features, one zoom-15 tile (about 1.2 km) at a time.
//!
//! Sources, in order: the public Overpass servers, raced against each other (a second
//! one is asked if the first is slow), then the main OpenStreetMap API as a last resort.
//! Only the tags the game uses are kept, and tiles are cached in a compact binary form
//! (delta-coded coordinates, interned strings), typically 10x smaller than the JSON.

use crate::cache::{DiskCache, MISSING};
use crate::net::{self, NetError};
use crate::tile::TileId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const ZOOM: u8 = 15;

/// Public Overpass instances.
const ENDPOINTS: [&str; 3] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];

/// If a server hasn't answered after this long, ask the next one as well.
const HEDGE_AFTER: Duration = Duration::from_secs(8);

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
    "bridge:structure",
    "bridge:support",
    "bridge:name",
    "colour",
    "tunnel",
    "layer",
    "level",
    "surface",
    "smoothness",
    "sidewalk",
    "sidewalk:both",
    "sidewalk:left",
    "sidewalk:right",
    "kerb",
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
    "crossing:barrier",
    "crossing:light",
    "crossing:bell",
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
    "destination",
    "destination:ref",
    "exit_to",
    "junction:ref",
    "distance",
    "natural",
    "water",
    "waterway",
    "landuse",
    "intermittent",
    "type",
    "parking",
    "capacity",
    "capacity:disabled",
    "building",
    "building:part",
    "building:levels",
    "building:levels:underground",
    "building:min_level",
    "building:colour",
    "building:material",
    "building:use",
    "levels",
    "min_level",
    "max_level",
    "repeat_on",
    "height",
    "min_height",
    "roof:shape",
    "roof:height",
    "roof:levels",
    "roof:colour",
    "roof:material",
    "roof:orientation",
    "roof:direction",
    "roof:angle",
    "shop",
    "cuisine",
    "office",
    "craft",
    "tourism",
    "leisure",
    "healthcare",
    "religion",
    "entrance",
    "door",
    "indoor",
    "wikidata",
    "start_date",
    "historic",
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

/// A polygon (lake, reservoir, river area): outer rings and holes, closed (last = first).
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub id: i64,
    pub outer: Vec<Vec<(f64, f64)>>,
    pub inner: Vec<Vec<(f64, f64)>>,
    pub tags: Tags,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OsmTile {
    pub nodes: Vec<Node>,
    pub ways: Vec<Way>,
    pub areas: Vec<Area>,
}

/// Which kind of data a tile holds (they're fetched and cached separately).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Roads, railways, roadside objects, water, parking lots.
    Roads,
    /// Buildings and building parts, entrances, indoor mapping, and the shops and
    /// amenities that say what a building is used for.
    Buildings,
}

/// A surface parking lot (or any parking area) drawn as an area.
pub fn is_parking(t: &Tags) -> bool {
    t.is("amenity", "parking") && t.get("highway").is_none()
}

pub fn is_building(t: &Tags) -> bool {
    t.get("building").is_some_and(|v| v != "no") || t.get("building:part").is_some_and(|v| v != "no")
}

pub fn is_indoor(t: &Tags) -> bool {
    t.get("indoor").is_some_and(|v| matches!(v, "room" | "corridor" | "area" | "level"))
}

/// Closed ways and multipolygons kept as areas for a layer.
pub fn is_area(t: &Tags, layer: Layer) -> bool {
    match layer {
        Layer::Roads => is_water(t) || is_parking(t),
        Layer::Buildings => is_building(t) || is_indoor(t),
    }
}

/// Whether tags describe standing or flowing water drawn as a surface.
pub fn is_water(t: &Tags) -> bool {
    t.is("natural", "water")
        || t.is("waterway", "riverbank")
        || t.is("landuse", "reservoir")
        || t.is("landuse", "basin") && t.get("water").is_some()
}

/// The Overpass QL for one tile: roads, railways, guardrails, roadside objects, water,
/// and freeway exits within ~13 km (for working out mile markers).
pub fn query(id: TileId) -> String {
    let (w, s, e, n) = id.bounds();
    let b = format!("{s:.6},{w:.6},{n:.6},{e:.6}");
    let pad = 0.12;
    let wide = format!("{:.6},{:.6},{:.6},{:.6}", s - pad, w - pad, n + pad, e + pad);
    format!(
        "[out:json][timeout:60];(\
way[highway]({b});\
way[railway~\"^(rail|light_rail|subway|tram|narrow_gauge|monorail)$\"]({b});\
way[barrier=guard_rail]({b});\
way[power=minor_line]({b});\
way[natural=water]({b});way[waterway=riverbank]({b});way[landuse=reservoir]({b});\
way[amenity=parking]({b});relation[amenity=parking](if:count_members()<100)({b});\
relation[natural=water](if:count_members()<250)({b});\
relation[waterway=riverbank](if:count_members()<250)({b});\
relation[landuse=reservoir](if:count_members()<250)({b});\
node[highway~\"^(traffic_signals|stop|give_way|street_lamp|crossing|bus_stop|construction|turning_circle|milestone)$\"]({b});\
node[railway~\"^(level_crossing|crossing)$\"]({b});\
node[emergency=fire_hydrant]({b});\
node[amenity~\"^(bench|post_box)$\"]({b});\
node[power=pole]({b});\
node[man_made=surveillance]({b});\
node[barrier~\"^(bollard|traffic_cone)$\"]({b});\
node[highway=motorway_junction]({wide});\
);out body geom qt;"
    )
}

/// The Overpass QL for one tile's buildings: footprints and parts (whole, even where
/// they cross the tile edge), entrances, indoor mapping, and the points of interest
/// that say what each building is used for.
pub fn query_buildings(id: TileId) -> String {
    let (w, s, e, n) = id.bounds();
    let b = format!("{s:.6},{w:.6},{n:.6},{e:.6}");
    format!(
        "[out:json][timeout:90];(\
way[building]({b});\
relation[building](if:count_members()<400)({b});\
way[\"building:part\"]({b});\
relation[\"building:part\"](if:count_members()<200)({b});\
way[indoor~\"^(room|corridor|area|level)$\"]({b});\
node[entrance]({b});node[door]({b});\
node[shop]({b});node[amenity]({b});node[office]({b});node[craft]({b});node[tourism]({b});node[healthcare]({b});\
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
    members: Vec<OpMember>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

#[derive(serde::Deserialize)]
struct OpMember {
    r#type: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    geometry: Vec<Option<OpPoint>>,
}

#[derive(serde::Deserialize)]
struct OpPoint {
    lat: f64,
    lon: f64,
}

fn keep_tags(t: impl IntoIterator<Item = (String, String)>) -> Tags {
    let mut v: Vec<(String, String)> = t.into_iter().filter(|(k, _)| KEEP.contains(&k.as_str())).collect();
    v.sort();
    Tags(v)
}

/// Join way pieces into closed rings (multipolygon members come in any order/direction).
fn assemble(mut pieces: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let same = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-7 && (a.1 - b.1).abs() < 1e-7;
    let mut rings = Vec::new();
    pieces.retain(|p| p.len() >= 2);
    while let Some(mut ring) = pieces.pop() {
        loop {
            if ring.len() >= 4 && same(ring[0], *ring.last().unwrap()) {
                rings.push(ring);
                break;
            }
            let end = *ring.last().unwrap();
            let Some(k) = pieces.iter().position(|p| same(p[0], end) || same(*p.last().unwrap(), end)) else {
                break; // open ring (member outside the data): drop it
            };
            let mut p = pieces.swap_remove(k);
            if !same(p[0], end) {
                p.reverse();
            }
            ring.extend(p.into_iter().skip(1));
        }
    }
    rings
}

/// Parse an Overpass JSON answer (road layer).
pub fn parse(json: &[u8]) -> Result<OsmTile, String> {
    parse_layer(json, Layer::Roads)
}

/// Parse an Overpass JSON answer for a layer.
pub fn parse_layer(json: &[u8], layer: Layer) -> Result<OsmTile, String> {
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
                if el.nodes.len() != el.geometry.len() {
                    continue;
                }
                let tags = keep_tags(el.tags);
                let (mut nodes, mut pts) = (Vec::new(), Vec::new());
                for (nid, p) in el.nodes.iter().zip(&el.geometry) {
                    if let Some(p) = p {
                        nodes.push(*nid);
                        pts.push((p.lat, p.lon));
                    }
                }
                if is_area(&tags, layer) && tags.get("highway").is_none() {
                    if pts.len() >= 4 && el.nodes.first() == el.nodes.last() && pts.len() == el.nodes.len() {
                        out.areas.push(Area { id: el.id, outer: vec![pts], inner: Vec::new(), tags });
                    }
                    continue;
                }
                if layer == Layer::Buildings {
                    continue;
                }
                if pts.len() >= 2 {
                    out.ways.push(Way { id: el.id, nodes, pts, tags });
                }
            }
            "relation" => {
                let tags = keep_tags(el.tags);
                if !is_area(&tags, layer) {
                    continue;
                }
                let (mut outer, mut inner) = (Vec::new(), Vec::new());
                for m in el.members {
                    if m.r#type != "way" || m.geometry.iter().any(|p| p.is_none()) {
                        continue;
                    }
                    let pts: Vec<(f64, f64)> = m.geometry.into_iter().flatten().map(|p| (p.lat, p.lon)).collect();
                    if m.role == "inner" { inner.push(pts) } else { outer.push(pts) }
                }
                let outer = assemble(outer);
                if !outer.is_empty() {
                    out.areas.push(Area { id: -el.id, outer, inner: assemble(inner), tags });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Parse an OpenStreetMap API `map` answer (XML), keeping what the Overpass query would.
pub fn parse_osm_xml(xml: &[u8]) -> Result<OsmTile, String> {
    parse_osm_xml_layer(xml, Layer::Roads)
}

pub fn parse_osm_xml_layer(xml: &[u8], layer: Layer) -> Result<OsmTile, String> {
    use quick_xml::events::Event;
    let mut rd = quick_xml::Reader::from_reader(xml);
    let mut buf = Vec::new();
    #[derive(Default)]
    struct Cur {
        kind: u8, // 1 node, 2 way, 3 relation
        id: i64,
        lat: f64,
        lon: f64,
        nds: Vec<i64>,
        members: Vec<(i64, String)>,
        tags: Vec<(String, String)>,
    }
    let mut cur = Cur::default();
    let mut coords: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut nodes = Vec::new();
    let mut ways: Vec<(i64, Vec<i64>, Tags)> = Vec::new();
    let mut rels: Vec<(i64, Vec<(i64, String)>, Tags)> = Vec::new();
    let attr = |e: &quick_xml::events::BytesStart, k: &[u8]| -> Option<String> {
        e.attributes().flatten().find(|a| a.key.as_ref() == k).and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
    };
    let finish =
        |cur: &mut Cur, nodes: &mut Vec<Node>, ways: &mut Vec<(i64, Vec<i64>, Tags)>, rels: &mut Vec<(i64, Vec<(i64, String)>, Tags)>| {
            let tags = keep_tags(std::mem::take(&mut cur.tags));
            match cur.kind {
                1 if !tags.0.is_empty() => nodes.push(Node { id: cur.id, lat: cur.lat, lon: cur.lon, tags }),
                2 => ways.push((cur.id, std::mem::take(&mut cur.nds), tags)),
                3 => rels.push((cur.id, std::mem::take(&mut cur.members), tags)),
                _ => {}
            }
            cur.kind = 0;
        };
    loop {
        let ev = rd.read_event_into(&mut buf).map_err(|e| format!("bad OSM XML: {e}"))?;
        match ev {
            // Tag-less nodes are self-closing (<node .../>): only their coordinates matter.
            Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                b"node" => {
                    let id: i64 = attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let lat: f64 = attr(&e, b"lat").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let lon: f64 = attr(&e, b"lon").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    coords.insert(id, (lat, lon));
                    cur = Cur { kind: 1, id, lat, lon, ..Default::default() };
                }
                b"way" => cur = Cur { kind: 2, id: attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0), ..Default::default() },
                b"relation" => cur = Cur { kind: 3, id: attr(&e, b"id").and_then(|v| v.parse().ok()).unwrap_or(0), ..Default::default() },
                b"nd" => {
                    if let Some(r) = attr(&e, b"ref").and_then(|v| v.parse().ok()) {
                        cur.nds.push(r);
                    }
                }
                b"member" => {
                    if attr(&e, b"type").as_deref() == Some("way")
                        && let Some(r) = attr(&e, b"ref").and_then(|v| v.parse().ok())
                    {
                        cur.members.push((r, attr(&e, b"role").unwrap_or_default()));
                    }
                }
                b"tag" => {
                    if let (Some(k), Some(v)) = (attr(&e, b"k"), attr(&e, b"v")) {
                        cur.tags.push((k, v));
                    }
                }
                _ => {}
            },
            Event::End(e) => {
                if matches!(e.name().as_ref(), b"node" | b"way" | b"relation") {
                    finish(&mut cur, &mut nodes, &mut ways, &mut rels);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    let wanted_way = |t: &Tags| {
        layer == Layer::Roads && t.get("highway").is_some()
            || t.get("railway").is_some_and(|r| matches!(r, "rail" | "light_rail" | "subway" | "tram" | "narrow_gauge" | "monorail"))
            || layer == Layer::Roads && t.is("barrier", "guard_rail")
            || layer == Layer::Roads && t.is("power", "minor_line")
    };
    let mut out = OsmTile { nodes, ..Default::default() };
    let mut way_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    for (id, nds, tags) in ways {
        let pts: Vec<(f64, f64)> = nds.iter().filter_map(|n| coords.get(n).copied()).collect();
        if pts.len() != nds.len() || pts.len() < 2 {
            continue;
        }
        if is_area(&tags, layer) {
            if nds.first() == nds.last() && pts.len() >= 4 {
                out.areas.push(Area { id, outer: vec![pts.clone()], inner: Vec::new(), tags: tags.clone() });
            }
        } else if wanted_way(&tags) {
            out.ways.push(Way { id, nodes: nds, pts: pts.clone(), tags: tags.clone() });
        }
        way_geom.insert(id, pts);
    }
    for (id, members, tags) in rels {
        if !is_area(&tags, layer) {
            continue;
        }
        let (mut outer, mut inner) = (Vec::new(), Vec::new());
        for (r, role) in members {
            if let Some(g) = way_geom.get(&r) {
                if role == "inner" { inner.push(g.clone()) } else { outer.push(g.clone()) }
            }
        }
        let outer = assemble(outer);
        if !outer.is_empty() {
            out.areas.push(Area { id: -id, outer, inner: assemble(inner), tags });
        }
    }
    Ok(out)
}

static NEXT_ENDPOINT: AtomicUsize = AtomicUsize::new(0);

fn encode_query(q: &str) -> String {
    q.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn overpass_once(ep: &str, enc: &str, layer: Layer) -> Result<OsmTile, NetError> {
    let b = net::get_timeout(&format!("{ep}?data={enc}"), "application/json", 150, |_, _| {})?;
    // Overpass reports timeouts and overload inside a 200 answer as a "remark".
    if b.len() < 4096 && String::from_utf8_lossy(&b).contains("\"remark\"") {
        return Err(NetError::Io("Overpass query timed out".into()));
    }
    parse_layer(&b, layer).map_err(NetError::Io)
}

/// Download one tile: Overpass servers raced (a second is asked if the first is slow,
/// a third if both are), then the OpenStreetMap API.
pub fn download(id: TileId) -> Result<OsmTile, NetError> {
    download_layer(id, Layer::Roads)
}

pub fn download_layer(id: TileId, layer: Layer) -> Result<OsmTile, NetError> {
    let enc = encode_query(&match layer {
        Layer::Roads => query(id),
        Layer::Buildings => query_buildings(id),
    });
    let start = NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel::<Result<OsmTile, NetError>>();
    let mut launched = 0;
    let mut failed = 0;
    let mut last = NetError::Io("no Overpass server answered".into());
    let launch = |k: usize| {
        let tx = tx.clone();
        let ep = ENDPOINTS[(start + k) % ENDPOINTS.len()];
        let enc = enc.clone();
        std::thread::spawn(move || {
            let _ = tx.send(overpass_once(ep, &enc, layer));
        });
    };
    launch(0);
    launched += 1;
    while failed < ENDPOINTS.len() {
        match rx.recv_timeout(HEDGE_AFTER) {
            Ok(Ok(t)) => return Ok(t),
            Ok(Err(e)) => {
                failed += 1;
                last = e;
                if launched < ENDPOINTS.len() {
                    launch(launched);
                    launched += 1;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if launched < ENDPOINTS.len() {
                    launch(launched);
                    launched += 1;
                }
            }
            Err(_) => break,
        }
    }
    // Last resort: the main OpenStreetMap API (everything in the box, filtered here).
    let (w, s, e, n) = id.bounds();
    let url = format!("https://api.openstreetmap.org/api/0.6/map?bbox={w:.6},{s:.6},{e:.6},{n:.6}");
    match net::get_timeout(&url, "application/xml", 90, |_, _| {}) {
        Ok(b) => parse_osm_xml_layer(&b, layer).map_err(NetError::Io),
        Err(e2) => {
            log::warn!("OSM fallback failed for {}: {e2}", id.path());
            Err(last)
        }
    }
}

fn cache_key(id: TileId, layer: Layer) -> String {
    match layer {
        Layer::Roads => format!("osm/v3/{}.bin", id.path()),
        Layer::Buildings => format!("bld/v1/{}.bin", id.path()),
    }
}

/// A tile from the disk cache, or downloaded and cached.
pub fn load(id: TileId, cache: &DiskCache) -> Result<OsmTile, NetError> {
    load_layer(id, Layer::Roads, cache)
}

pub fn load_layer(id: TileId, layer: Layer, cache: &DiskCache) -> Result<OsmTile, NetError> {
    if let Some(t) = cached_layer(id, layer, cache) {
        return Ok(t);
    }
    let t = download_layer(id, layer)?;
    cache.put(&cache_key(id, layer), &encode(&t));
    Ok(t)
}

/// Cached copy only (no network).
pub fn cached(id: TileId, cache: &DiskCache) -> Option<OsmTile> {
    cached_layer(id, Layer::Roads, cache)
}

pub fn cached_layer(id: TileId, layer: Layer, cache: &DiskCache) -> Option<OsmTile> {
    cache.get(&cache_key(id, layer)).filter(|b| b != MISSING).and_then(|b| decode(&b))
}

// ---------------------------------------------------------------- binary codec

const MAGIC: &[u8; 4] = b"OSM2";

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
    let all_tags = t.nodes.iter().map(|n| &n.tags).chain(t.ways.iter().map(|w| &w.tags)).chain(t.areas.iter().map(|a| &a.tags));
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
    let pts_out = |body: &mut Vec<u8>, pts: &[(f64, f64)], plat: &mut i64, plon: &mut i64| {
        for &(la, lo) in pts {
            put_zig(body, fix(la) - *plat);
            put_zig(body, fix(lo) - *plon);
            (*plat, *plon) = (fix(la), fix(lo));
        }
    };
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
        pts_out(&mut body, &w.pts, &mut plat, &mut plon);
        tags_out(&mut body, &w.tags);
    }
    put_var(&mut body, t.areas.len() as u64);
    for a in &t.areas {
        put_zig(&mut body, a.id);
        for rings in [&a.outer, &a.inner] {
            put_var(&mut body, rings.len() as u64);
            for r in rings {
                put_var(&mut body, r.len() as u64);
                pts_out(&mut body, r, &mut plat, &mut plon);
            }
        }
        tags_out(&mut body, &a.tags);
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
    let pts_in = |r: &mut Reader, k: usize, plat: &mut i64, plon: &mut i64| -> Option<Vec<(f64, f64)>> {
        let mut pts = Vec::with_capacity(k.min(1 << 16));
        for _ in 0..k {
            *plat += r.zig()?;
            *plon += r.zig()?;
            pts.push((*plat as f64 / 1e7, *plon as f64 / 1e7));
        }
        Some(pts)
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
        let pts = pts_in(&mut r, k, &mut plat, &mut plon)?;
        let tg = tags(&mut r)?;
        t.ways.push(Way { id: pwid, nodes, pts, tags: tg });
    }
    let na = r.var()? as usize;
    for _ in 0..na {
        let id = r.zig()?;
        let mut sets = [Vec::new(), Vec::new()];
        for set in sets.iter_mut() {
            let nr = r.var()? as usize;
            for _ in 0..nr {
                let k = r.var()? as usize;
                set.push(pts_in(&mut r, k, &mut plat, &mut plon)?);
            }
        }
        let [outer, inner] = sets;
        let tg = tags(&mut r)?;
        t.areas.push(Area { id, outer, inner, tags: tg });
    }
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":0.6,"elements":[
      {"type":"node","id":42,"lat":39.7471,"lon":-104.9932,"tags":{"highway":"traffic_signals","note":"x"}},
      {"type":"way","id":7,"nodes":[1,2,3],"geometry":[{"lat":39.7,"lon":-105.0},{"lat":39.71,"lon":-105.01},null],
       "tags":{"highway":"residential","name":"Larimer Street","fixme":"drop me"}},
      {"type":"way","id":8,"nodes":[5,6,7,5],"geometry":[{"lat":39.0,"lon":-105.0},{"lat":39.01,"lon":-105.0},{"lat":39.01,"lon":-105.01},{"lat":39.0,"lon":-105.0}],
       "tags":{"natural":"water","name":"Pond"}},
      {"type":"relation","id":9,"members":[
         {"type":"way","ref":1,"role":"outer","geometry":[{"lat":40.0,"lon":-105.0},{"lat":40.01,"lon":-105.0}]},
         {"type":"way","ref":2,"role":"outer","geometry":[{"lat":40.0,"lon":-105.0},{"lat":40.0,"lon":-105.01},{"lat":40.01,"lon":-105.0}]}],
       "tags":{"type":"multipolygon","natural":"water"}}
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
        // A closed water way and a two-piece multipolygon become areas.
        assert_eq!(t.areas.len(), 2);
        assert_eq!(t.areas[1].outer[0].len(), 4);
        let b = encode(&t);
        let back = decode(&b).unwrap();
        assert_eq!(back.ways[0].nodes, t.ways[0].nodes);
        assert_eq!(back.ways[0].tags, t.ways[0].tags);
        assert_eq!(back.areas.len(), 2);
        assert!((back.nodes[0].lat - 39.7471).abs() < 1e-7);
        assert!((back.ways[0].pts[1].1 + 105.01).abs() < 1e-7);
    }

    #[test]
    fn parses_osm_api_xml() {
        let xml = r#"<?xml version="1.0"?><osm version="0.6">
<node id="1" lat="40.0" lon="-105.0"/><node id="2" lat="40.001" lon="-105.0"/>
<node id="3" lat="40.001" lon="-105.001"><tag k="highway" v="traffic_signals"/></node>
<way id="10"><nd ref="1"/><nd ref="2"/><nd ref="3"/><tag k="highway" v="primary"/><tag k="name" v="Colfax"/></way>
<way id="11"><nd ref="1"/><nd ref="2"/><tag k="building" v="yes"/></way>
</osm>"#;
        let t = parse_osm_xml(xml.as_bytes()).unwrap();
        assert_eq!(t.ways.len(), 1);
        assert_eq!(t.ways[0].pts.len(), 3);
        assert_eq!(t.nodes.len(), 1);
        assert!(t.nodes[0].tags.is("highway", "traffic_signals"));
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
