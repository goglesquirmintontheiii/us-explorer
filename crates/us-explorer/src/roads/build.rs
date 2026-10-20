//! Turns one zoom-14 OSM tile into road geometry, worked out flat (x east, z south,
//! metres around the tile centre) with a height *rule* per vertex. Heights are filled
//! in later by `drape`, against whatever terrain is on screen at the time.
//!
//! - Ways are split at junctions and crossings. Junctions of motor roads become one
//!   merged surface with curb returns; the roads meeting there are trimmed back to it.
//! - Bridges and tunnels are grouped into networks (ramps on a viaduct, multi-part
//!   bridges). Their decks are interpolated between where they meet the ground, kept
//!   above (or below) whatever they cross by a clearance from the `layer` tag.
//! - Approaches to overpasses and underpasses climb or dip at a highway grade, and the
//!   terrain is told (through [`Corridor`]s) to fill or cut under every road.

use super::classify::{self, Class, Profile, Surface};
use bevy::math::Vec2;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use usx_core::osm::{OsmTile, Tags};
use usx_core::tile::TileId;

/// Longest flat step between vertices along a road, so draped roads follow the ground.
const STEP: f32 = 3.0;
/// Highway grade used for approach ramps (5%).
const GRADE: f32 = 0.05;
/// Zoom of the chunks geometry is split into (about 600 m).
pub const CHUNK_ZOOM: u8 = 16;

// ------------------------------------------------------------------ frame

/// Flat metres around a point; accurate to centimetres across a zoom-14 tile.
#[derive(Clone, Copy, Debug)]
pub struct Local {
    pub lat0: f64,
    pub lon0: f64,
    kx: f64,
    ky: f64,
}

impl Local {
    pub fn new(lat0: f64, lon0: f64) -> Self {
        let (a, e2) = (6_378_137.0f64, 6.694_379_990_14e-3f64);
        let s = lat0.to_radians().sin();
        let w = (1.0 - e2 * s * s).sqrt();
        let n = a / w;
        let m = a * (1.0 - e2) / (w * w * w);
        Self { lat0, lon0, kx: n * lat0.to_radians().cos() * std::f64::consts::PI / 180.0, ky: m * std::f64::consts::PI / 180.0 }
    }
    pub fn xz(&self, lat: f64, lon: f64) -> Vec2 {
        Vec2::new(((lon - self.lon0) * self.kx) as f32, (-(lat - self.lat0) * self.ky) as f32)
    }
    pub fn ll(&self, p: Vec2) -> (f64, f64) {
        (self.lat0 - p.y as f64 / self.ky, self.lon0 + p.x as f64 / self.kx)
    }
}

/// Right-hand side of travel direction `d` (x east, z south).
fn right(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x)
}

// ------------------------------------------------------------------ output

/// How a vertex gets its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum H {
    /// On the ground (terrain under roads is flattened to match), raised by `off`
    /// where the terrain hasn't been flattened (coarse tiles, or roads that arrived late).
    Ground { off: f32 },
    /// On a bridge or tunnel deck, between two deck nodes.
    Deck { a: u32, b: u32, t: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct V {
    pub p: Vec2,
    pub h: H,
    /// Added after the height rule: lift above the ground, curb height, wall height...
    pub dy: f32,
    /// Lateral and along-road metres (or tile metres on junctions).
    pub uv: [f32; 2],
    /// Segment length; packed element code, seed and high flags (see [`pack`]).
    pub uv2: [f32; 2],
    /// Half width; lanes (fwd + 16 bwd + 256 left shoulder dm + 16384 right shoulder dm);
    /// low 20 flag bits; turn arrows (9 bits forward, 9 backward).
    pub col: [f32; 4],
}

/// Flag bits in `col[2]`; mirrored in road.wgsl.
pub mod flag {
    pub const ONEWAY: u32 = 1 << 0;
    pub const XWALK_START: u32 = 1 << 1;
    pub const XWALK_END: u32 = 1 << 2;
    pub const STOP_START: u32 = 1 << 3;
    pub const STOP_END: u32 = 1 << 4;
    pub const JUNCTION: u32 = 1 << 5;
    pub const PATH: u32 = 1 << 6;
    pub const RAIL: u32 = 1 << 7;
    pub const CENTER_DOUBLE: u32 = 1 << 8;
    pub const CENTER_DASHED: u32 = 1 << 9;
    pub const EDGE_LINES: u32 = 1 << 10;
    pub const LANE_LINES: u32 = 1 << 11;
    pub const SURFACE_SHIFT: u32 = 12; // 3 bits
    pub const SMOOTH_SHIFT: u32 = 15; // 3 bits
    pub const SIDEWALK: u32 = 1 << 18;
    pub const STRUCTURE: u32 = 1 << 19; // concrete bridge sides, parapets, piers, tunnel walls
    pub const DECK: u32 = 1 << 20;
    pub const TUNNEL_LIGHT: u32 = 1 << 21;
    pub const CONSTRUCTION: u32 = 1 << 22;
    pub const STEPS: u32 = 1 << 23;
}

/// Element codes in `uv2[1]` (what the triangles are), mirrored in road.wgsl.
pub mod elem {
    pub const ROAD: f32 = 0.0;
    pub const JUNCTION: f32 = 1.0;
    pub const SIDEWALK: f32 = 2.0;
    pub const CURB: f32 = 3.0;
    pub const STRUCTURE: f32 = 4.0;
    pub const RAIL_BED: f32 = 5.0;
    pub const RAIL: f32 = 6.0;
    pub const TUNNEL: f32 = 7.0;
}

#[derive(Default)]
pub struct Geom {
    pub v: Vec<V>,
    pub i: Vec<u32>,
}

impl Geom {
    fn push(&mut self, v: V) -> u32 {
        self.v.push(v);
        (self.v.len() - 1) as u32
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.i.extend_from_slice(&[a, b, c]);
    }
    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        // a-b across one row (left to right of travel), c-d the next row: counter-
        // clockwise seen from above (x east, z south, y up).
        self.i.extend_from_slice(&[a, b, c, b, d, c]);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropKind {
    StreetSign { names: [String; 2] },
    Stop { all_way: bool },
    Yield,
    Signal { junction: i64, group: u8, mast: f32 },
    PedSignal { junction: i64, group: u8 },
    Lamp { arm: f32 },
    Guardrail { len: f32 },
    Cone,
    Barrel,
    Alpr,
    UtilityPole,
    Hydrant,
    Mailbox,
    BusStop { shelter: bool, bench: bool },
    Bench,
    Shield { text: String },
    ExitSign { exit: String, text: String },
    Bollard,
    SpeedLimit { mph: u32 },
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub kind: PropKind,
    pub p: Vec2,
    pub h: H,
    /// Direction the front faces, radians clockwise from north.
    pub heading: f32,
    /// Stable id for destruction state and signal lookups.
    pub id: u64,
}

/// One signal-controlled junction: approach groups that get green together.
#[derive(Clone, Debug)]
pub struct Controller {
    pub id: i64,
    pub p: Vec2,
    /// Number of phases (groups), each green in turn.
    pub groups: u8,
    pub cycle_offset: f32,
}

/// Where the terrain should be flattened (or cut away) under a road, in lat/lon.
#[derive(Clone, Debug)]
pub struct Corridor {
    pub pts: Vec<(f64, f64)>,
    /// Height offset from the natural ground at the centreline, per point (fill > 0, cut < 0).
    pub off: Vec<f32>,
    pub half: f32,
    /// Tunnel: no flattening; instead remove terrain that dips into the tube.
    /// Holds the deck height per point (absolute, from the coarse DEM).
    pub tunnel: Option<Vec<f32>>,
    pub bbox: (f64, f64, f64, f64), // s, w, n, e
}

/// Bridge and tunnel deck nodes, solved for height when draping.
#[derive(Clone, Debug, Default)]
pub struct Decks {
    pub p: Vec<Vec2>,
    pub off: Vec<f32>,
    /// Ground nodes the deck starts from.
    pub boundary: Vec<bool>,
    /// > 0: keep this far above the ground (bridges); < 0: this far below (tunnels).
    pub clear: Vec<f32>,
    pub edges: Vec<Vec<(u32, f32)>>,
    /// Which network each node is in.
    pub net: Vec<u32>,
    /// Node belongs to a tunnel (not a bridge).
    pub tunnel: Vec<bool>,
    pub nets: u32,
}

/// A road segment's place in the world, for later phases (vehicles, navigation).
#[derive(Clone, Debug)]
pub struct Lane {
    pub way: i64,
    pub class: Class,
    pub pts: Vec<Vec2>,
    pub h: Vec<H>,
    pub profile: Profile,
    pub name: Option<String>,
}

pub struct Chunk {
    pub id: TileId,
    pub geom: Geom,
    pub props: Vec<Prop>,
    /// Area whose terrain the chunk's heights depend on (s, w, n, e): the chunk plus
    /// the far ends of any bridge or tunnel touching it.
    pub influence: (f64, f64, f64, f64),
}

pub struct Built {
    pub tile: TileId,
    pub local: Local,
    pub chunks: Vec<Chunk>,
    pub decks: Decks,
    pub corridors: Vec<Corridor>,
    pub controllers: Vec<Controller>,
    pub lanes: Vec<Lane>,
    /// Sign text atlas requests: (text, style) per sign; resolved by the sign painter.
    pub stats: BuildStats,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct BuildStats {
    pub ways: usize,
    pub junctions: usize,
    pub bridges: usize,
    pub tunnels: usize,
    pub props: usize,
    pub vertices: usize,
}

// ------------------------------------------------------------------ input model

struct RWay {
    id: i64,
    class: Class,
    construction: bool,
    link: bool,
    prof: Profile,
    surface: Surface,
    smooth: u8,
    layer: i32,
    bridge: bool,
    tunnel: bool,
    pts: Vec<Vec2>,
    nodes: Vec<i64>,
    tags: Tags,
    turn_f: u32,
    turn_b: u32,
}

impl RWay {
    fn elevated(&self) -> bool {
        self.bridge || self.tunnel
    }
}

#[derive(Clone, Copy, Debug)]
struct Arm {
    way: u32,
    idx: u32,
    /// +1: leaves the node towards idx+1; -1: towards idx-1.
    dir: i8,
}

#[derive(Default)]
struct NodeInfo {
    p: Vec2,
    arms: Vec<Arm>,
    tags: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndKind {
    Junction,
    Join,
    Crossing,
    Free,
}

struct Seg {
    way: u32,
    i0: usize,
    i1: usize,
    trim0: f32,
    trim1: f32,
    end0: EndKind,
    end1: EndKind,
    /// Forced end normal (for joins between two ways), pointing right of travel.
    n0: Option<Vec2>,
    n1: Option<Vec2>,
    flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    None,
    Signals,
    AllStop,
    /// Stop signs on the minor approaches only.
    MinorStop,
    MinorYield,
}

struct Junction {
    node: i64,
    p: Vec2,
    /// (segment, at_start, direction away from the junction, half width)
    arms: Vec<(usize, bool, Vec2, f32)>,
    control: Control,
    urban: bool,
    /// Boundary of the merged surface (ccw), around `p`.
    poly: Vec<Vec2>,
    /// Curb line around each corner, from arm a's edge to arm b's (relative to `p`).
    corners: Vec<(usize, usize, Vec<Vec2>)>,
}

// ------------------------------------------------------------------ helpers

fn polyline_len(p: &[Vec2]) -> f32 {
    p.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// The part of a polyline between `a` metres from its start and `b` metres from its end.
fn cut(p: &[Vec2], a: f32, b: f32) -> Vec<Vec2> {
    let total = polyline_len(p);
    let (s0, s1) = (a.max(0.0), (total - b).min(total));
    if s1 - s0 < 0.05 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut acc = 0.0;
    for w in p.windows(2) {
        let l = w[0].distance(w[1]);
        let (t0, t1) = (acc, acc + l);
        if t1 >= s0 && t0 <= s1 && l > 1e-6 {
            if out.is_empty() {
                out.push(w[0].lerp(w[1], ((s0 - t0) / l).clamp(0.0, 1.0)));
            }
            if t1 <= s1 {
                out.push(w[1]);
            } else {
                out.push(w[0].lerp(w[1], ((s1 - t0) / l).clamp(0.0, 1.0)));
                break;
            }
        }
        acc = t1;
    }
    out.dedup_by(|a, b| a.distance(*b) < 0.01);
    out
}

/// Insert points so no step is longer than `step`.
fn densify(p: &[Vec2], step: f32) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(p.len() * 2);
    for w in p.windows(2) {
        let l = w[0].distance(w[1]);
        let n = (l / step).ceil().max(1.0) as usize;
        for k in 0..n {
            out.push(w[0].lerp(w[1], k as f32 / n as f32));
        }
    }
    if let Some(l) = p.last() {
        out.push(*l);
    }
    out
}

/// Per-point unit normals (right of travel) with miter scaling, clamped.
fn normals(p: &[Vec2], n0: Option<Vec2>, n1: Option<Vec2>) -> Vec<(Vec2, f32)> {
    let k = p.len();
    let dir = |i: usize| (p[(i + 1).min(k - 1)] - p[i.saturating_sub(1).min(k - 2)]).normalize_or(Vec2::X);
    (0..k)
        .map(|i| {
            if i == 0 {
                let d = (p[1] - p[0]).normalize_or(Vec2::X);
                return match n0 {
                    Some(n) => (n, (1.0 / n.dot(right(d)).abs().max(0.5)).min(2.0)),
                    None => (right(d), 1.0),
                };
            }
            if i == k - 1 {
                let d = (p[k - 1] - p[k - 2]).normalize_or(Vec2::X);
                return match n1 {
                    Some(n) => (n, (1.0 / n.dot(right(d)).abs().max(0.5)).min(2.0)),
                    None => (right(d), 1.0),
                };
            }
            let a = (p[i] - p[i - 1]).normalize_or(Vec2::X);
            let b = (p[i + 1] - p[i]).normalize_or(Vec2::X);
            let n = right((a + b).normalize_or(dir(i)));
            let c = n.dot(right(a)).max(0.5);
            (n, (1.0 / c).min(2.0))
        })
        .collect()
}

fn hash64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

fn hash01(x: u64) -> f32 {
    (hash64(x) & 0xffffff) as f32 / 16_777_216.0
}

/// Potholes on a road, as the shader draws them: one 6 m cell along the road at a time.
/// Returns (along, lateral fraction -1..1, radius) for a cell, or None.
pub fn pothole(seed: u32, cell: i64, smooth: u8) -> Option<(f32, f32, f32)> {
    let p = [0.0, 0.004, 0.012, 0.03, 0.07, 0.13, 0.2, 0.3][smooth.min(7) as usize];
    let h = |k: u64| hash01((seed as u64) << 32 ^ (cell as u64).wrapping_mul(0x9e37_79b9) ^ k);
    (h(1) < p).then(|| (cell as f32 * 6.0 + 1.0 + h(2) * 4.0, h(3) * 1.6 - 0.8, 0.25 + 0.45 * h(4)))
}

/// Where segment a-b crosses segment c-d, as a fraction along a-b (ends excluded).
fn seg_cross(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<f32> {
    let r = b - a;
    let s = d - c;
    let den = r.perp_dot(s);
    if den.abs() < 1e-6 {
        return None;
    }
    let t = (c - a).perp_dot(s) / den;
    let u = (c - a).perp_dot(r) / den;
    (t > 0.001 && t < 0.999 && u > 0.001 && u < 0.999).then_some(t)
}

/// Vertex attributes interpolate in floating point, so every packed integer stays
/// below 2^20 to survive rounding: (uv2.y, col.z) = (element + 8 seed + 32768 high
/// flags, low 20 flag bits).
pub fn pack(el: f32, seed: u32, flags: u32) -> (f32, f32) {
    let hi = (flags >> 20) & 31;
    (el + ((seed & 0xfff) * 8) as f32 + (hi << 15) as f32, (flags & 0xfffff) as f32)
}

fn heading_of(d: Vec2) -> f32 {
    // x east, z south: heading clockwise from north.
    d.x.atan2(-d.y)
}

// ------------------------------------------------------------------ build

pub fn build(tile: TileId, osm: &OsmTile, dem: &dyn Fn(f64, f64) -> Option<f64>) -> Built {
    let (cla, clo) = tile.lat_lon_at(0.5, 0.5);
    let local = Local::new(cla, clo);
    let (tw, ts, te, tn) = tile.bounds();
    let in_tile = |p: Vec2| {
        let (la, lo) = local.ll(p);
        la >= ts && la < tn && lo >= tw && lo < te
    };

    // Node tags by id.
    let node_tags: HashMap<i64, usize> = osm.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();

    // ---- ways
    let mut ways: Vec<RWay> = Vec::new();
    let mut guard_rails: Vec<Vec<Vec2>> = Vec::new();
    for w in &osm.ways {
        if w.tags.is("barrier", "guard_rail") && w.tags.get("highway").is_none() {
            guard_rails.push(w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect());
            continue;
        }
        let Some((class, construction)) = Class::from_tags(&w.tags) else { continue };
        if matches!(w.tags.get("access"), Some("no" | "private")) && class == Class::Service {
            // Still drawn: private driveways exist. Nothing special needed.
        }
        let link = w.tags.get("highway").is_some_and(|h| h.ends_with("_link"));
        let mut pts: Vec<Vec2> = w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect();
        let mut nodes = w.nodes.clone();
        let mut tags = w.tags.clone();
        if tags.is("oneway", "-1") {
            pts.reverse();
            nodes.reverse();
            tags.0.retain(|(k, _)| k != "oneway");
            tags.0.push(("oneway".into(), "yes".into()));
        }
        let prof = classify::profile(&tags, class, link);
        let (tf, tb) = if prof.oneway {
            (classify::turn_bits(tags.get("turn:lanes").or(tags.get("turn:lanes:forward"))), 0)
        } else {
            (classify::turn_bits(tags.get("turn:lanes:forward")), classify::turn_bits(tags.get("turn:lanes:backward")))
        };
        let layer = classify::layer(&tags);
        let bridge = tags.flag("bridge") && !tags.is("bridge", "no");
        let tunnel = tags.flag("tunnel") && !tags.is("tunnel", "building_passage") && !bridge;
        // Subways in tunnels are out of sight; skip them entirely.
        if class == Class::Subway && (tunnel || layer < 0) {
            continue;
        }
        ways.push(RWay {
            id: w.id,
            class,
            construction,
            link,
            prof,
            surface: Surface::from_tags(&tags, class),
            smooth: classify::smoothness(&tags, class),
            layer,
            bridge,
            tunnel,
            pts,
            nodes,
            tags,
            turn_f: tf,
            turn_b: tb,
        });
    }

    // ---- nodes and arms
    let mut nodes: HashMap<i64, NodeInfo> = HashMap::new();
    for (wi, w) in ways.iter().enumerate() {
        for (i, &nid) in w.nodes.iter().enumerate() {
            let n = nodes.entry(nid).or_default();
            n.p = w.pts[i];
            n.tags = node_tags.get(&nid).copied();
            if i > 0 {
                n.arms.push(Arm { way: wi as u32, idx: i as u32, dir: -1 });
            }
            if i + 1 < w.nodes.len() {
                n.arms.push(Arm { way: wi as u32, idx: i as u32, dir: 1 });
            }
        }
    }
    let ntag = |nid: i64| nodes.get(&nid).and_then(|n| n.tags).map(|i| &osm.nodes[i].tags);
    let vehicle_arms = |n: &NodeInfo| n.arms.iter().filter(|a| ways[a.way as usize].class.vehicle()).count();
    let distinct_ways = |n: &NodeInfo, veh: bool| {
        let mut s: Vec<u32> = n.arms.iter().filter(|a| !veh || ways[a.way as usize].class.vehicle()).map(|a| a.way).collect();
        s.sort();
        s.dedup();
        s.len()
    };

    // ---- split ways into segments
    let mut segs: Vec<Seg> = Vec::new();
    for (wi, w) in ways.iter().enumerate() {
        let k = w.nodes.len();
        let mut breaks = vec![0usize];
        for i in 1..k - 1 {
            let n = &nodes[&w.nodes[i]];
            let junction = if w.class.vehicle() { vehicle_arms(n) >= 3 } else { n.arms.len() >= 3 };
            let crossing = w.class.vehicle()
                && (ntag(w.nodes[i]).is_some_and(|t| t.is("highway", "crossing"))
                    || n.arms.iter().any(|a| {
                        let o = &ways[a.way as usize];
                        o.class.path() && o.tags.is("footway", "crossing")
                    }));
            if junction || crossing {
                breaks.push(i);
            }
        }
        breaks.push(k - 1);
        breaks.dedup();
        for b in breaks.windows(2) {
            let kind = |i: usize, end: bool| {
                let n = &nodes[&w.nodes[i]];
                let veh = w.class.vehicle();
                let arms = if veh { vehicle_arms(n) } else { n.arms.len() };
                if arms >= 3 && veh {
                    EndKind::Junction
                } else if arms >= 2 && distinct_ways(n, veh) >= 2 {
                    EndKind::Join
                } else if (i != 0 && i != k - 1) || (arms >= 2 && !end) {
                    EndKind::Crossing
                } else {
                    EndKind::Free
                }
            };
            segs.push(Seg {
                way: wi as u32,
                i0: b[0],
                i1: b[1],
                trim0: 0.0,
                trim1: 0.0,
                end0: kind(b[0], false),
                end1: kind(b[1], true),
                n0: None,
                n1: None,
                flags: 0,
            });
        }
    }
    // Segment lookup: (way, index) at an end -> segment id and whether it's the start.
    let mut seg_at: HashMap<(u32, u32, i8), usize> = HashMap::new();
    for (si, s) in segs.iter().enumerate() {
        seg_at.insert((s.way, s.i0 as u32, 1), si);
        seg_at.insert((s.way, s.i1 as u32, -1), si);
    }

    // ---- joins between two ways: share the end normal so the ribbons meet cleanly
    for (_, n) in nodes.iter() {
        if n.arms.len() != 2 || n.arms[0].way == n.arms[1].way {
            continue;
        }
        let (a, b) = (n.arms[0], n.arms[1]);
        let dir = |arm: Arm| {
            let w = &ways[arm.way as usize];
            let j = (arm.idx as i64 + arm.dir as i64) as usize;
            (w.pts[j] - w.pts[arm.idx as usize]).normalize_or(Vec2::X)
        };
        let (da, db) = (dir(a), dir(b));
        // Travel through the node from arm a to arm b: incoming = -da, outgoing = db.
        let through = (db - da).normalize_or(db);
        let nrm = right(through);
        for (arm, sign) in [(a, -1.0f32), (b, 1.0)] {
            if let Some(&si) = seg_at.get(&(arm.way, arm.idx, arm.dir)) {
                // A segment starting here travels along +dir; its right normal follows
                // `through` when it leaves (b) and is reversed when it arrives (a).
                let start = arm.dir == 1;
                let n_for_seg = if start == (sign > 0.0) { nrm } else { -nrm };
                if start {
                    segs[si].n0 = Some(n_for_seg);
                } else {
                    segs[si].n1 = Some(n_for_seg);
                }
            }
        }
    }

    // ---- junctions
    let urban_grid = {
        let mut g: HashMap<(i32, i32), u32> = HashMap::new();
        for (_, n) in nodes.iter() {
            if vehicle_arms(n) >= 3 {
                *g.entry(((n.p.x / 200.0).floor() as i32, (n.p.y / 200.0).floor() as i32)).or_default() += 1;
            }
        }
        g
    };
    let is_urban = |p: Vec2| {
        let (cx, cz) = ((p.x / 200.0).floor() as i32, (p.y / 200.0).floor() as i32);
        let mut c = 0;
        for dx in -1..=1 {
            for dz in -1..=1 {
                c += urban_grid.get(&(cx + dx, cz + dz)).copied().unwrap_or(0);
            }
        }
        c >= 8
    };
    let mut junctions: Vec<Junction> = Vec::new();
    let mut junction_at: HashMap<i64, usize> = HashMap::new();
    for (&nid, n) in nodes.iter() {
        if vehicle_arms(n) < 3 {
            continue;
        }
        let mut arms = Vec::new();
        for a in &n.arms {
            let w = &ways[a.way as usize];
            if !w.class.vehicle() {
                continue;
            }
            let Some(&si) = seg_at.get(&(a.way, a.idx, a.dir)) else { continue };
            let j = (a.idx as i64 + a.dir as i64) as usize;
            let d = (w.pts[j] - w.pts[a.idx as usize]).normalize_or(Vec2::X);
            arms.push((si, a.dir == 1, d, w.prof.half()));
        }
        if arms.len() < 3 {
            continue;
        }
        let tags = ntag(nid);
        let mut control = Control::None;
        if tags.is_some_and(|t| t.is("highway", "traffic_signals")) {
            control = Control::Signals;
        } else if tags.is_some_and(|t| t.is("highway", "stop")) {
            control = if tags.is_some_and(|t| t.is("stop", "all")) { Control::AllStop } else { Control::MinorStop };
        } else if tags.is_some_and(|t| t.is("highway", "give_way")) {
            control = Control::MinorYield;
        }
        let urban = is_urban(n.p);
        junction_at.insert(nid, junctions.len());
        junctions.push(Junction { node: nid, p: n.p, arms, control, urban, poly: Vec::new(), corners: Vec::new() });
    }
    // Signals and stop signs tagged on an approach node a little before the junction.
    for (ni, nd) in osm.nodes.iter().enumerate() {
        let kind = match nd.tags.get("highway") {
            Some("traffic_signals") => Control::Signals,
            Some("stop") => {
                if nd.tags.is("stop", "all") {
                    Control::AllStop
                } else {
                    Control::MinorStop
                }
            }
            Some("give_way") => Control::MinorYield,
            _ => continue,
        };
        let Some(n) = nodes.get(&nd.id) else { continue };
        if junction_at.contains_key(&nd.id) {
            continue;
        }
        // Walk along each arm up to 40 m looking for a junction.
        'arms: for a in &n.arms {
            let w = &ways[a.way as usize];
            if !w.class.vehicle() {
                continue;
            }
            let mut i = a.idx as i64;
            let mut dist = 0.0;
            loop {
                let j = i + a.dir as i64;
                if j < 0 || j >= w.nodes.len() as i64 {
                    break;
                }
                dist += w.pts[i as usize].distance(w.pts[j as usize]);
                if dist > 40.0 {
                    break;
                }
                if let Some(&ji) = junction_at.get(&w.nodes[j as usize]) {
                    let jn = &mut junctions[ji];
                    if kind == Control::Signals || jn.control == Control::None {
                        jn.control = kind;
                    }
                    let _ = ni;
                    break 'arms;
                }
                i = j;
            }
        }
    }
    // Untagged junctions: infer what's typical in the US.
    for j in junctions.iter_mut() {
        if j.control != Control::None {
            continue;
        }
        let classes: Vec<Class> = j.arms.iter().map(|a| ways[segs[a.0].way as usize].class).collect();
        let names: HashSet<&str> = j.arms.iter().filter_map(|a| ways[segs[a.0].way as usize].tags.get("name")).collect();
        let motorway = classes.iter().any(|c| matches!(c, Class::Motorway)) || classes.iter().all(|c| *c <= Class::Trunk);
        if motorway {
            continue;
        }
        let majors = classes.iter().filter(|c| c.major()).count();
        let big = classes.iter().filter(|c| **c <= Class::Secondary).count();
        if big >= 3 && names.len() >= 2 && j.urban {
            j.control = Control::Signals;
        } else if majors >= 1 && majors < classes.len() {
            j.control = Control::MinorStop;
        } else if majors == 0 && j.arms.len() >= 4 && j.urban && classes.iter().all(|c| *c == Class::Residential) {
            j.control = Control::AllStop;
        } else if majors >= 4 && j.arms.len() >= 4 && names.len() >= 2 {
            j.control = if j.urban { Control::Signals } else { Control::AllStop };
        }
    }

    // ---- junction polygons and trims
    for j in junctions.iter_mut() {
        j.arms.sort_by(|a, b| a.2.y.atan2(a.2.x).total_cmp(&b.2.y.atan2(b.2.x)));
        // Drop near-duplicate arms (two ways drawn on top of each other).
        j.arms.dedup_by(|a, b| a.2.dot(b.2) > 0.9995);
        let k = j.arms.len();
        if k < 2 {
            continue;
        }
        let radius = if j.urban { 4.5 } else { 7.0 };
        let mut trim = vec![0.0f32; k];
        let mut corners = Vec::with_capacity(k);
        for a in 0..k {
            let b = (a + 1) % k;
            let (da, wa) = (j.arms[a].2, j.arms[a].3);
            let (db, wb) = (j.arms[b].2, j.arms[b].3);
            // Arm a's edge facing b is at +right(da) (angles increase towards b).
            let pa = right(da) * wa;
            let pb = -right(db) * wb;
            let cross = da.perp_dot(db);
            let (s, t, phi) = if cross.abs() < 0.02 {
                (0.0, 0.0, std::f32::consts::PI)
            } else {
                // Solve pa + s*da = pb + t*db.
                let d = pb - pa;
                let s = d.perp_dot(db) / cross;
                let t = d.perp_dot(da) / cross;
                let ang = da.y.atan2(da.x);
                let mut phi = db.y.atan2(db.x) - ang;
                if phi <= 0.0 {
                    phi += std::f32::consts::TAU;
                }
                (s, t, phi)
            };
            // Reflex corner (> 180°): nothing to fillet, edges just meet at the centre side.
            let fillet = if phi < std::f32::consts::PI * 0.97 { (radius / (phi * 0.5).tan()).min(20.0) } else { 0.0 };
            let (s, t) = (s.clamp(-30.0, 30.0), t.clamp(-30.0, 30.0));
            trim[a] = trim[a].max(s + fillet);
            trim[b] = trim[b].max(t + fillet);
            corners.push((s, t, fillet, pa + da * s));
        }
        // Never eat more than 45% of a segment (short links between close junctions).
        for (a, arm) in j.arms.iter().enumerate() {
            let s = &segs[arm.0];
            let w = &ways[s.way as usize];
            let len = polyline_len(&w.pts[s.i0..=s.i1]);
            let cap = if s.end0 == EndKind::Junction && s.end1 == EndKind::Junction { len * 0.45 } else { len * 0.8 };
            trim[a] = trim[a].clamp(0.5, cap.max(0.5));
        }
        let mut poly = Vec::new();
        let mut corner_curves = Vec::new();
        for a in 0..k {
            let b = (a + 1) % k;
            let (da, wa) = (j.arms[a].2, j.arms[a].3);
            let (db, wb) = (j.arms[b].2, j.arms[b].3);
            let (s, t, fillet, c) = corners[a];
            let ea = da * trim[a] + right(da) * wa;
            let eb = db * trim[b] - right(db) * wb;
            poly.push(da * trim[a] - right(da) * wa);
            let mut curve = vec![ea];
            if fillet > 0.0 {
                let t1 = right(da) * wa + da * (s + fillet).min(trim[a]);
                let t2 = -right(db) * wb + db * (t + fillet).min(trim[b]);
                curve.push(t1);
                for q in 1..6 {
                    let u = q as f32 / 6.0;
                    let p = t1 * (1.0 - u) * (1.0 - u) + c * 2.0 * u * (1.0 - u) + t2 * u * u;
                    curve.push(p);
                }
                curve.push(t2);
            } else if da.perp_dot(db).abs() >= 0.02 {
                curve.push(c);
            }
            curve.push(eb);
            poly.extend_from_slice(&curve);
            curve.dedup_by(|a, b| a.distance(*b) < 0.05);
            corner_curves.push((a, b, curve));
        }
        poly.dedup_by(|a, b| a.distance(*b) < 0.05);
        j.poly = poly;
        j.corners = corner_curves;
        for (a, arm) in j.arms.iter().enumerate() {
            let s = &mut segs[arm.0];
            if arm.1 {
                s.trim0 = trim[a];
            } else {
                s.trim1 = trim[a];
            }
        }
    }

    // ---- markings at segment ends
    for j in &junctions {
        for &(si, at_start, d, _) in &j.arms {
            let w = &ways[segs[si].way as usize];
            let urbanish = j.urban && w.class <= Class::Residential;
            let xwalk = j.control == Control::Signals && urbanish;
            // Traffic arriving at this junction along the segment: at the start, that's
            // backward traffic; at the end, forward traffic.
            let arriving = if at_start { !w.prof.oneway } else { true };
            let stop = arriving
                && match j.control {
                    Control::Signals | Control::AllStop => true,
                    Control::MinorStop => !w.class.major() || j.arms.iter().all(|a| ways[segs[a.0].way as usize].class.major()),
                    _ => false,
                }
                && w.class > Class::Trunk;
            let s = &mut segs[si];
            let _ = d;
            if at_start {
                s.flags |= if xwalk { flag::XWALK_START } else { 0 } | if stop { flag::STOP_START } else { 0 };
            } else {
                s.flags |= if xwalk { flag::XWALK_END } else { 0 } | if stop { flag::STOP_END } else { 0 };
            }
        }
    }
    for s in segs.iter_mut() {
        let w = &ways[s.way as usize];
        if !w.class.vehicle() {
            continue;
        }
        if s.end0 == EndKind::Crossing && ntag(w.nodes[s.i0]).is_some_and(|t| t.is("highway", "crossing") && !t.is("crossing", "unmarked"))
        {
            s.flags |= flag::XWALK_START;
        }
        if s.end1 == EndKind::Crossing && ntag(w.nodes[s.i1]).is_some_and(|t| t.is("highway", "crossing") && !t.is("crossing", "unmarked"))
        {
            s.flags |= flag::XWALK_END;
        }
    }

    // ---- profiles: approach ramps from bridges (fill) and underpasses (cut)
    let mut decks = Decks::default();
    let mut deck_node: HashMap<i64, u32> = HashMap::new();
    let mut node_off: HashMap<i64, f32> = HashMap::new();
    let mut stats = BuildStats { ways: ways.len(), junctions: junctions.len(), ..default_stats() };
    {
        // Deck networks: union of elevated ways that share nodes, same kind.
        let elevated: Vec<usize> = (0..ways.len()).filter(|&i| ways[i].elevated() && !ways[i].class.path() || ways[i].bridge).collect();
        let mut parent: Vec<usize> = (0..ways.len()).collect();
        fn find(p: &mut [usize], x: usize) -> usize {
            let mut r = x;
            while p[r] != r {
                r = p[r];
            }
            let mut y = x;
            while p[y] != r {
                let n = p[y];
                p[y] = r;
                y = n;
            }
            r
        }
        let mut by_node: HashMap<i64, Vec<usize>> = HashMap::new();
        for &wi in &elevated {
            for &nid in &ways[wi].nodes {
                by_node.entry(nid).or_default().push(wi);
            }
        }
        for (_, list) in by_node.iter() {
            for w in list.windows(2) {
                if ways[w[0]].bridge == ways[w[1]].bridge {
                    let (a, b) = (find(&mut parent, w[0]), find(&mut parent, w[1]));
                    parent[a] = b;
                }
            }
        }
        let mut net_of: HashMap<usize, u32> = HashMap::new();
        for &wi in &elevated {
            let r = find(&mut parent, wi);
            let n = net_of.len() as u32;
            let id = *net_of.entry(r).or_insert(n);
            let w = &ways[wi];
            let mut prev: Option<u32> = None;
            for (i, &nid) in w.nodes.iter().enumerate() {
                let di = *deck_node.entry(nid).or_insert_with(|| {
                    decks.p.push(w.pts[i]);
                    decks.off.push(0.0);
                    decks.boundary.push(false);
                    decks.clear.push(0.0);
                    decks.edges.push(Vec::new());
                    decks.net.push(id);
                    decks.tunnel.push(w.tunnel);
                    (decks.p.len() - 1) as u32
                });
                let clear = if w.bridge { 6.2 + 6.5 * (w.layer.max(1) - 1) as f32 } else { -(7.0 * (-w.layer).max(1) as f32) };
                let c = &mut decks.clear[di as usize];
                *c = if w.bridge { c.max(clear) } else { c.min(clear) };
                // Boundary: shared with a road on the ground, or a dangling end.
                let n = &nodes[&nid];
                let touches_ground = n.arms.iter().any(|a| !ways[a.way as usize].elevated());
                let end = i == 0 || i + 1 == w.nodes.len();
                if touches_ground || (end && n.arms.len() == 1) {
                    decks.boundary[di as usize] = true;
                }
                if let Some(p) = prev {
                    let d = decks.p[p as usize].distance(decks.p[di as usize]);
                    decks.edges[p as usize].push((di, d));
                    decks.edges[di as usize].push((p, d));
                }
                prev = Some(di);
            }
        }
        decks.nets = net_of.len() as u32;
        stats.bridges = elevated.iter().filter(|&&w| ways[w].bridge).count();
        stats.tunnels = elevated.iter().filter(|&&w| ways[w].tunnel).count();

        // Seeds: how far each network's ground ends must rise (or sink) to clear what
        // they cross, judged on the coarse DEM.
        let ground: Vec<Option<f32>> = decks
            .p
            .iter()
            .map(|&p| {
                let (la, lo) = local.ll(p);
                dem(la, lo).map(|h| h as f32)
            })
            .collect();
        let deck_h = solve_decks(&decks, &|i| ground[i].unwrap_or(0.0), &|i| decks.off[i]);
        // Clearance is needed only where a deck crosses a road or railway on the ground.
        let mut grid: HashMap<(i32, i32), Vec<(Vec2, Vec2)>> = HashMap::new();
        let cell = |p: Vec2| ((p.x / 50.0).floor() as i32, (p.y / 50.0).floor() as i32);
        for w in ways.iter().filter(|w| !w.elevated()) {
            for q in w.pts.windows(2) {
                let (a, b) = (cell(q[0]), cell(q[1]));
                for x in a.0.min(b.0)..=a.0.max(b.0) {
                    for z in a.1.min(b.1)..=a.1.max(b.1) {
                        grid.entry((x, z)).or_default().push((q[0], q[1]));
                    }
                }
            }
        }
        let mut need = vec![0.0f32; decks.nets as usize];
        for i in 0..decks.p.len() {
            for &(j, _) in &decks.edges[i] {
                let j = j as usize;
                if j < i {
                    continue;
                }
                let (pa, pb) = (decks.p[i], decks.p[j]);
                let (ca, cb) = (cell(pa), cell(pb));
                let mut hit: Option<f32> = None;
                for x in ca.0.min(cb.0)..=ca.0.max(cb.0) {
                    for z in ca.1.min(cb.1)..=ca.1.max(cb.1) {
                        for &(qa, qb) in grid.get(&(x, z)).map(|v| v.as_slice()).unwrap_or(&[]) {
                            if let Some(t) = seg_cross(pa, pb, qa, qb) {
                                hit = Some(hit.map_or(t, |h: f32| if (h - 0.5).abs() < (t - 0.5).abs() { h } else { t }));
                            }
                        }
                    }
                }
                let Some(t) = hit else { continue };
                let c = if decks.clear[i].abs() > decks.clear[j].abs() { decks.clear[i] } else { decks.clear[j] };
                let p = pa.lerp(pb, t);
                let (la, lo) = local.ll(p);
                let g = dem(la, lo).map(|h| h as f32).unwrap_or(ground[i].unwrap_or(0.0));
                let line = deck_h[i] + (deck_h[j] - deck_h[i]) * t;
                let n = &mut need[decks.net[i] as usize];
                if c > 0.0 {
                    *n = n.max(g + c - line);
                } else {
                    *n = n.min(g + c - line);
                }
            }
        }
        // From here on, decks follow their ends; clearance was handled by raising them.
        decks.clear.iter_mut().for_each(|c| *c = 0.0);
        for i in 0..decks.p.len() {
            if decks.boundary[i] {
                let n = need[decks.net[i] as usize];
                if n.abs() > 0.4 {
                    decks.off[i] = n;
                }
            }
        }
        // Spread the seeds along roads on the ground at the ramp grade.
        let seeds: Vec<(i64, f32)> =
            deck_node.iter().filter(|(_, i)| decks.off[**i as usize] != 0.0).map(|(&n, &i)| (n, decks.off[i as usize])).collect();
        for sign in [1.0f32, -1.0] {
            let mut best: HashMap<i64, f32> = HashMap::new();
            let mut heap: BinaryHeap<(OrdF, i64)> = BinaryHeap::new();
            for &(n, o) in &seeds {
                if o * sign > 0.0 {
                    best.insert(n, o * sign);
                    heap.push((OrdF(o * sign), n));
                }
            }
            while let Some((OrdF(o), n)) = heap.pop() {
                if best.get(&n).is_some_and(|&b| b > o) {
                    continue;
                }
                let Some(info) = nodes.get(&n) else { continue };
                for a in &info.arms {
                    let w = &ways[a.way as usize];
                    if w.elevated() {
                        continue;
                    }
                    let j = (a.idx as i64 + a.dir as i64) as usize;
                    let m = w.nodes[j];
                    let no = o - GRADE * w.pts[a.idx as usize].distance(w.pts[j]);
                    if no > 0.05 && best.get(&m).is_none_or(|&b| no > b) {
                        best.insert(m, no);
                        heap.push((OrdF(no), m));
                    }
                }
            }
            for (n, o) in best {
                *node_off.entry(n).or_default() += o * sign;
            }
        }
        for (&n, &i) in deck_node.iter() {
            if decks.boundary[i as usize] {
                decks.off[i as usize] = node_off.get(&n).copied().unwrap_or(decks.off[i as usize]);
            }
        }
    }

    // ---- geometry
    let mut chunks: BTreeMap<TileId, Chunk> = BTreeMap::new();
    let chunk_of = |p: Vec2| {
        let (la, lo) = local.ll(p);
        TileId::containing(la, lo, CHUNK_ZOOM).0
    };
    let mut corridors = Vec::new();
    let mut lanes = Vec::new();
    let sidewalks_mapped = ways.iter().filter(|w| w.tags.is("footway", "sidewalk")).count() > 6;

    for s in &segs {
        let w = &ways[s.way as usize];
        let full = &w.pts[s.i0..=s.i1];
        let pts = cut(full, s.trim0, s.trim1);
        if pts.len() < 2 {
            continue;
        }
        let pts = densify(&pts, STEP);
        let len = polyline_len(&pts);
        // Distance along the way's own node list -> interpolated ramp offset.
        let node_ids = &w.nodes[s.i0..=s.i1];
        let along_nodes: Vec<f32> = {
            let mut acc = 0.0;
            let mut v = vec![0.0];
            for p in full.windows(2) {
                acc += p[0].distance(p[1]);
                v.push(acc);
            }
            v
        };
        let off_at = |d: f32| -> f32 {
            // d is measured from the trimmed start; shift by trim0 onto the full way.
            let d = d + s.trim0;
            let k = along_nodes.partition_point(|&x| x <= d).clamp(1, along_nodes.len() - 1);
            let (a, b) = (along_nodes[k - 1], along_nodes[k]);
            let t = if b > a { ((d - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
            let oa = node_off.get(&node_ids[k - 1]).copied().unwrap_or(0.0);
            let ob = node_off.get(&node_ids[k]).copied().unwrap_or(0.0);
            oa + (ob - oa) * t
        };
        // Height rule along the segment.
        let along: Vec<f32> = {
            let mut acc = 0.0;
            let mut v = vec![0.0];
            for p in pts.windows(2) {
                acc += p[0].distance(p[1]);
                v.push(acc);
            }
            v
        };
        let hrule: Vec<H> = if w.elevated() {
            along
                .iter()
                .map(|&d| {
                    let dd = d + s.trim0;
                    let k = along_nodes.partition_point(|&x| x <= dd).clamp(1, along_nodes.len() - 1);
                    let (a, b) = (along_nodes[k - 1], along_nodes[k]);
                    let t = if b > a { ((dd - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
                    match (deck_node.get(&node_ids[k - 1]), deck_node.get(&node_ids[k])) {
                        (Some(&na), Some(&nb)) => H::Deck { a: na, b: nb, t },
                        _ => H::Ground { off: 0.0 },
                    }
                })
                .collect()
        } else {
            along.iter().map(|&d| H::Ground { off: off_at(d) }).collect()
        };

        // Corridor for the terrain.
        if !w.bridge {
            let ll: Vec<(f64, f64)> = pts.iter().map(|&p| local.ll(p)).collect();
            let (mut s0, mut w0, mut n0, mut e0) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
            for &(la, lo) in &ll {
                s0 = s0.min(la);
                n0 = n0.max(la);
                w0 = w0.min(lo);
                e0 = e0.max(lo);
            }
            let pad = 60.0 / 111_000.0;
            let tunnel_h = w.tunnel.then(|| {
                hrule
                    .iter()
                    .map(|h| match *h {
                        H::Deck { a, b, t } => {
                            let ga = |i: u32| {
                                let (la, lo) = local.ll(decks.p[i as usize]);
                                dem(la, lo).unwrap_or(0.0) as f32 + decks.off[i as usize]
                            };
                            // Coarse: straight line between deck nodes' ground (good enough for cutting holes).
                            ga(a) + (ga(b) - ga(a)) * t + decks.clear[a as usize].min(0.0) * 0.0
                        }
                        H::Ground { .. } => 0.0,
                    })
                    .collect()
            });
            corridors.push(Corridor {
                off: hrule.iter().map(|h| if let H::Ground { off } = h { *off } else { 0.0 }).collect(),
                pts: ll,
                half: w.prof.half() + if w.class.vehicle() { 0.5 } else { 0.2 },
                tunnel: tunnel_h,
                bbox: (s0 - pad, w0 - pad, n0 + pad, e0 + pad),
            });
        }
        lanes.push(Lane {
            way: w.id,
            class: w.class,
            pts: pts.clone(),
            h: hrule.clone(),
            profile: w.prof,
            name: w.tags.get("name").map(str::to_string),
        });

        // Vertex attributes for the road shader.
        let hw = w.prof.half();
        let mut f = s.flags;
        if w.prof.oneway {
            f |= flag::ONEWAY;
        }
        if w.class.path() {
            f |= flag::PATH;
        }
        if w.class.rail() {
            f |= flag::RAIL;
        }
        if w.construction {
            f |= flag::CONSTRUCTION;
        }
        if w.class == Class::Steps {
            f |= flag::STEPS;
        }
        if w.bridge {
            f |= flag::DECK;
        }
        if w.class.vehicle() && w.class != Class::Track && w.surface == Surface::Asphalt
            || w.surface == Surface::Concrete && w.class.vehicle()
        {
            let lanes_total = w.prof.fwd + w.prof.bwd;
            if w.class.major() || w.class == Class::Unclassified && lanes_total >= 2 && w.tags.get("lanes").is_some() {
                f |= flag::EDGE_LINES;
                if !w.prof.oneway {
                    f |= if w.class <= Class::Secondary || lanes_total >= 4 { flag::CENTER_DOUBLE } else { flag::CENTER_DASHED };
                }
                if w.prof.fwd > 1 || w.prof.bwd > 1 {
                    f |= flag::LANE_LINES;
                }
            } else if w.tags.get("lanes").is_some() && !w.prof.oneway && w.class != Class::Service {
                f |= flag::CENTER_DASHED;
            } else if w.prof.oneway && w.prof.fwd > 1 {
                f |= flag::LANE_LINES;
            }
        }
        f |= (w.surface as u32) << flag::SURFACE_SHIFT;
        f |= (w.smooth.min(7) as u32) << flag::SMOOTH_SHIFT;
        let seed = (hash64(w.id as u64) & 0xfff) as u32;
        let lanes = w.prof.fwd.min(15) as u32
            + w.prof.bwd.min(15) as u32 * 16
            + ((w.prof.shoulder_l * 10.0).round() as u32).min(63) * 256
            + ((w.prof.shoulder_r * 10.0).round() as u32).min(63) * 16384;
        let turns = (w.turn_f & 0x1ff) | (w.turn_b & 0x1ff) << 9;
        let col = [hw, lanes as f32, 0.0, turns as f32];

        let nrm = normals(&pts, if s.trim0 > 0.0 { None } else { s.n0 }, if s.trim1 > 0.0 { None } else { s.n1 });
        let lift = w.class.lift();
        // Path ends overlap whatever they join; extend them a little.
        let ribbon = |g: &mut Geom, s0: f32, s1: f32, dy0: f32, dy1: f32, el: f32, fl: f32, uv_s: bool| {
            let mut prev: Option<(u32, u32)> = None;
            for (k, &p) in pts.iter().enumerate() {
                let (n, m) = nrm[k];
                let a = g.push(V {
                    p: p + n * s0 * m,
                    h: hrule[k],
                    dy: dy0,
                    uv: [if uv_s { s0 } else { 0.0 }, along[k]],
                    uv2: [len, pack(el, seed, fl as u32).0],
                    col: [col[0], col[1], pack(el, seed, fl as u32).1, col[3]],
                });
                let b = g.push(V {
                    p: p + n * s1 * m,
                    h: hrule[k],
                    dy: dy1,
                    uv: [if uv_s { s1 } else { 1.0 }, along[k]],
                    uv2: [len, pack(el, seed, fl as u32).0],
                    col: [col[0], col[1], pack(el, seed, fl as u32).1, col[3]],
                });
                if let Some((pa, pb)) = prev {
                    g.quad(pa, pb, a, b);
                }
                prev = Some((a, b));
            }
        };
        // Split the ribbon into chunks by segment midpoint so each chunk can drape alone.
        let mut ranges: Vec<(TileId, usize, usize)> = Vec::new();
        for k in 0..pts.len() - 1 {
            let mid = (pts[k] + pts[k + 1]) * 0.5;
            if !in_tile(mid) {
                continue;
            }
            let c = chunk_of(mid);
            match ranges.last_mut() {
                Some((cc, _, e)) if *cc == c && *e == k => *e = k + 1,
                _ => ranges.push((c, k, k + 1)),
            }
        }
        for (cid, k0, k1) in ranges {
            let chunk = chunks.entry(cid).or_insert_with(|| new_chunk(cid));
            let g = &mut chunk.geom;
            let base = g.v.len();
            // Build the full ribbon pieces then keep only rows k0..=k1. Simple and cheap.
            let mut tmp = Geom::default();
            let mk = |tmp: &mut Geom, s0: f32, s1: f32, dy0: f32, dy1: f32, el: f32, fl: f32| ribbon(tmp, s0, s1, dy0, dy1, el, fl, true);
            let fl = f as f32;
            if w.class.rail() {
                // Ballast bed, then two rails on top.
                mk(&mut tmp, -hw, hw, lift, lift, elem::RAIL_BED, fl);
                let gauge = 0.7175;
                for side in [-1.0f32, 1.0] {
                    let c = side * gauge;
                    mk(&mut tmp, c - 0.035, c + 0.035, lift + 0.32, lift + 0.32, elem::RAIL, fl);
                    mk(&mut tmp, c - 0.035, c - 0.035, lift + 0.18, lift + 0.32, elem::RAIL, fl);
                    mk(&mut tmp, c + 0.035, c + 0.035, lift + 0.32, lift + 0.18, elem::RAIL, fl);
                }
            } else {
                mk(&mut tmp, -hw, hw, lift, lift, elem::ROAD, fl);
            }
            // Sidewalks and curbs on urban motor roads.
            let (swl, swr) = sidewalks(w, sidewalks_mapped, is_urban(pts[k0]));
            for (on, side) in [(swl, -1.0f32), (swr, 1.0)] {
                if !on || w.elevated() && !w.bridge {
                    continue;
                }
                let curb = 0.15;
                let width = 1.8;
                let (a, b) = if side > 0.0 { (hw, hw + width) } else { (-hw - width, -hw) };
                mk(&mut tmp, a, b, lift + curb, lift + curb, elem::SIDEWALK, (flag::SIDEWALK | (f & flag::DECK)) as f32);
                // Curb face from the road surface up to the sidewalk.
                let e = side * hw;
                if side > 0.0 {
                    mk(&mut tmp, e, e, lift - 0.02, lift + curb, elem::CURB, (flag::SIDEWALK) as f32);
                } else {
                    mk(&mut tmp, e, e, lift + curb, lift - 0.02, elem::CURB, (flag::SIDEWALK) as f32);
                }
            }
            // Bridge deck sides, underside and parapets.
            if w.bridge {
                let (e0, e1) = if swl || swr { (-hw - if swl { 1.8 } else { 0.0 }, hw + if swr { 1.8 } else { 0.0 }) } else { (-hw, hw) };
                let thick = if w.class.vehicle() { 1.4 } else { 0.5 };
                let st = flag::STRUCTURE as f32;
                // Underside (facing down: reversed winding).
                mk(&mut tmp, e1, e0, lift - thick, lift - thick, elem::STRUCTURE, st);
                // Outer faces.
                mk(&mut tmp, e1, e1, lift - thick, lift + 0.2, elem::STRUCTURE, st);
                mk(&mut tmp, e0, e0, lift + 0.2, lift - thick, elem::STRUCTURE, st);
                // Parapets: concrete barriers for roads, lower walls with rails for paths.
                let ph = if w.class.vehicle() { 0.95 } else { 1.1 };
                let pw = if w.class.vehicle() { 0.35 } else { 0.15 };
                for (x0, x1) in [(e0, e0 + pw), (e1 - pw, e1)] {
                    mk(&mut tmp, x0, x1, lift + ph, lift + ph, elem::STRUCTURE, st);
                    mk(&mut tmp, x1, x1, lift, lift + ph, elem::STRUCTURE, st);
                    mk(&mut tmp, x0, x0, lift + ph, lift, elem::STRUCTURE, st);
                }
            }
            // Tunnel walls and ceiling.
            if w.tunnel && !w.class.path() {
                let wall = hw + 1.0;
                let ceil = if w.class.rail() { 6.5 } else { 5.6 };
                let t = elem::TUNNEL;
                let fl2 = (flag::STRUCTURE | flag::TUNNEL_LIGHT) as f32;
                mk(&mut tmp, wall, wall, lift + ceil, lift - 0.1, t, fl2);
                mk(&mut tmp, -wall, -wall, lift - 0.1, lift + ceil, t, fl2);
                mk(&mut tmp, wall, -wall, lift + ceil, lift + ceil, t, fl2);
                // Outer shell (seen from above where the terrain is cut away near portals).
                mk(&mut tmp, -wall - 0.5, wall + 0.5, lift + ceil + 0.6, lift + ceil + 0.6, elem::STRUCTURE, flag::STRUCTURE as f32);
                mk(&mut tmp, wall + 0.5, wall + 0.5, lift - 0.3, lift + ceil + 0.6, elem::STRUCTURE, flag::STRUCTURE as f32);
                mk(&mut tmp, -wall - 0.5, -wall - 0.5, lift + ceil + 0.6, lift - 0.3, elem::STRUCTURE, flag::STRUCTURE as f32);
            }
            // Portal headwalls where the tunnel meets open ground.
            if w.tunnel && !w.class.path() {
                let wall = hw + 1.0;
                let ceil = if w.class.rail() { 6.5 } else { 5.6 };
                for (end, row) in [(s.i0, 0usize), (s.i1, pts.len() - 1)] {
                    if !(k0..=k1).contains(&row) {
                        continue;
                    }
                    let portal = nodes.get(&w.nodes[end]).is_some_and(|n| n.arms.iter().any(|a| !ways[a.way as usize].tunnel));
                    if !portal {
                        continue;
                    }
                    let p = pts[row];
                    let (nr, _) = nrm[row];
                    let along =
                        if row == 0 { (pts[1] - pts[0]).normalize_or(Vec2::X) } else { (pts[row] - pts[row - 1]).normalize_or(Vec2::X) };
                    // Face outward (away from the tunnel).
                    let out = if row == 0 { -along } else { along };
                    let h = hrule[row];
                    let fl = flag::STRUCTURE as f32;
                    let mk = |q: Vec2, dy: f32| V {
                        p: q,
                        h,
                        dy,
                        uv: [q.x, dy],
                        uv2: [0.0, pack(elem::STRUCTURE, 0, fl as u32).0],
                        col: [0.0, 0.0, pack(elem::STRUCTURE, 0, fl as u32).1, 0.0],
                    };
                    let outer = wall + 5.0;
                    let top = lift + ceil + 5.0;
                    for (s0, s1, y0, y1) in
                        [(-outer, -wall, lift - 0.5, top), (wall, outer, lift - 0.5, top), (-wall, wall, lift + ceil, top)]
                    {
                        let q0 = p + nr * s0 + out * 0.3;
                        let q1 = p + nr * s1 + out * 0.3;
                        let a = g.push(mk(q0, y1));
                        let bb = g.push(mk(q1, y1));
                        let c0 = g.push(mk(q0, y0));
                        let d = g.push(mk(q1, y0));
                        // Two-sided: the wall is seen from outside and from within the tunnel.
                        g.quad(a, bb, c0, d);
                        g.quad(bb, a, d, c0);
                    }
                }
            }
            // Copy rows k0..=k1 of every strip in tmp into the chunk. Each strip has
            // pts.len() rows of 2 vertices.
            let rows = pts.len();
            let strips = tmp.v.len() / (rows * 2);
            for st in 0..strips {
                let o = st * rows * 2;
                let start = g.v.len() as u32;
                for r in k0..=k1 {
                    g.v.push(tmp.v[o + r * 2]);
                    g.v.push(tmp.v[o + r * 2 + 1]);
                }
                for r in 0..(k1 - k0) as u32 {
                    let a = start + r * 2;
                    g.quad(a, a + 1, a + 2, a + 3);
                }
            }
            let _ = base;
        }

        // Bridge piers every ~30 m where the deck is well above the ground.
        if w.bridge && w.class != Class::Footway {
            let spacing = if w.class.vehicle() { 32.0 } else { 20.0 };
            let mut d = spacing * 0.5 + (hash01(w.id as u64) * 6.0);
            while d < len - 8.0 {
                let k = along.partition_point(|&x| x <= d).clamp(1, pts.len() - 1);
                let t = (d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-3);
                let p = pts[k - 1].lerp(pts[k], t);
                let dir = (pts[k] - pts[k - 1]).normalize_or(Vec2::X);
                if in_tile(p) {
                    let h = match (hrule[k - 1], hrule[k]) {
                        (H::Deck { a, b, t: t0 }, H::Deck { t: t1, .. }) => H::Deck { a, b, t: t0 + (t1 - t0) * t },
                        (h, _) => h,
                    };
                    let chunk = chunks.entry(chunk_of(p)).or_insert_with(|| new_chunk(chunk_of(p)));
                    let cols = if hw > 6.0 { vec![-hw * 0.6, hw * 0.6] } else { vec![0.0] };
                    for c in cols {
                        pier(&mut chunk.geom, p + right(dir) * c, dir, h, lift - 1.4, if w.class.vehicle() { 0.8 } else { 0.35 });
                    }
                }
                d += spacing;
            }
        }
    }

    // Junction surfaces.
    for j in &junctions {
        if j.poly.len() < 3 || !in_tile(j.p) {
            continue;
        }
        let arm_ways: Vec<&RWay> = j.arms.iter().map(|a| &ways[segs[a.0].way as usize]).collect();
        let lift = arm_ways.iter().map(|w| w.class.lift()).fold(0.0f32, f32::max) + 0.002;
        let surface = arm_ways.iter().min_by_key(|w| w.class).map(|w| w.surface).unwrap_or(Surface::Asphalt);
        let smooth = arm_ways.iter().map(|w| w.smooth).min().unwrap_or(1);
        let elevated = arm_ways.iter().all(|w| w.elevated());
        let h = if elevated {
            match deck_node.get(&j.node) {
                Some(&n) => H::Deck { a: n, b: n, t: 0.0 },
                None => H::Ground { off: 0.0 },
            }
        } else {
            H::Ground { off: node_off.get(&j.node).copied().unwrap_or(0.0) }
        };
        let fl = (flag::JUNCTION | (surface as u32) << flag::SURFACE_SHIFT | (smooth as u32) << flag::SMOOTH_SHIFT) as f32;
        let seed = (hash64(j.node as u64) & 0xfff) as u32;
        let cid = chunk_of(j.p);
        let chunk = chunks.entry(cid).or_insert_with(|| new_chunk(cid));
        let g = &mut chunk.geom;
        let pk = pack(elem::JUNCTION, seed, fl as u32);
        let mk = |p: Vec2| V { p, h, dy: lift, uv: [p.x, p.y], uv2: [0.0, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
        let c = g.push(mk(j.p));
        let ring: Vec<u32> = j.poly.iter().map(|&q| g.push(mk(j.p + q))).collect();
        for k in 0..ring.len() {
            // Fan, ccw in x/z (z south) -> wind so the face points up.
            g.tri(c, ring[(k + 1) % ring.len()], ring[k]);
        }
        // Sidewalk corners: follow the curb return between neighbouring arms.
        let urban = is_urban(j.p);
        for (a, b, curve) in &j.corners {
            // Arm a's edge facing b is the segment's right side if it leaves the junction.
            let side_has = |arm: usize, facing_plus: bool| {
                let (si, at_start, _, _) = j.arms[arm];
                let w = &ways[segs[si].way as usize];
                let (l, r) = sidewalks(w, sidewalks_mapped, urban);
                // +right(d) is the segment's right side when it starts here, else its left.
                if facing_plus == at_start { r } else { l }
            };
            if (!side_has(*a, true) && !side_has(*b, false)) || curve.len() < 2 || elevated {
                continue;
            }
            let pts: Vec<Vec2> = curve.iter().map(|&q| j.p + q).collect();
            let width = 1.8;
            let curb = 0.15;
            let fl_sw = pack(elem::SIDEWALK, 0, flag::SIDEWALK);
            let fl_curb = pack(elem::CURB, 0, flag::SIDEWALK);
            let mut along = 0.0f32;
            let mut prev: Option<(u32, u32, u32, u32)> = None;
            let mut outer: Vec<u32> = Vec::with_capacity(pts.len());
            for (k, &q) in pts.iter().enumerate() {
                if k > 0 {
                    along += q.distance(pts[k - 1]);
                }
                let tan = if k + 1 < pts.len() { pts[k + 1] - q } else { q - pts[k - 1] }.normalize_or(Vec2::X);
                let mut n = right(tan);
                if n.dot(q - j.p) < 0.0 {
                    n = -n;
                }
                let mk =
                    |p: Vec2, dy: f32, s: f32, pk: (f32, f32)| V { p, h, dy, uv: [s, along], uv2: [0.0, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
                let i0 = g.push(mk(q, lift + curb, 0.0, fl_sw));
                let i1 = g.push(mk(q + n * width, lift + curb, width, fl_sw));
                let c0 = g.push(mk(q, lift - 0.02, 0.0, fl_curb));
                let c1 = g.push(mk(q, lift + curb, 0.15, fl_curb));
                if let Some((p0, p1, pc0, pc1)) = prev {
                    g.quad(p0, p1, i0, i1);
                    g.quad(pc0, pc1, c0, c1);
                }
                prev = Some((i0, i1, c0, c1));
                outer.push(i1);
            }
            // Fill out to where the two sidewalks' outer edges meet, so the corner is
            // paved like a real street corner instead of leaving a gap behind the curb.
            let (da, wa) = (j.arms[*a].2, j.arms[*a].3);
            let (db, wb) = (j.arms[*b].2, j.arms[*b].3);
            let pa = right(da) * (wa + width);
            let pb = -right(db) * (wb + width);
            let cross = da.perp_dot(db);
            if cross.abs() > 0.05 && outer.len() >= 2 {
                let d = pb - pa;
                let ta = d.perp_dot(db) / cross;
                let corner = j.p + pa + da * ta;
                if corner.distance(j.p) < 40.0 && ta > 0.0 {
                    let ci =
                        g.push(V { p: corner, h, dy: lift + curb, uv: [width, 0.0], uv2: [0.0, fl_sw.0], col: [0.0, 0.0, fl_sw.1, 0.0] });
                    for w in outer.windows(2) {
                        g.tri(ci, w[0], w[1]);
                    }
                }
            }
        }
    }

    // Turning circles at dead ends.
    for nd in &osm.nodes {
        if !nd.tags.is("highway", "turning_circle") {
            continue;
        }
        let Some(n) = nodes.get(&nd.id) else { continue };
        if !in_tile(n.p) || n.arms.is_empty() {
            continue;
        }
        let w = &ways[n.arms[0].way as usize];
        if !w.class.vehicle() || w.elevated() {
            continue;
        }
        let r = (w.prof.half() * 1.6).clamp(6.0, 13.0);
        let cid = chunk_of(n.p);
        let chunk = chunks.entry(cid).or_insert_with(|| new_chunk(cid));
        let g = &mut chunk.geom;
        let fl = (flag::JUNCTION | (w.surface as u32) << flag::SURFACE_SHIFT) as f32;
        let h = H::Ground { off: node_off.get(&nd.id).copied().unwrap_or(0.0) };
        let mk = |p: Vec2| V {
            p,
            h,
            dy: w.class.lift() - 0.003,
            uv: [p.x, p.y],
            uv2: [0.0, pack(elem::JUNCTION, 0, fl as u32).0],
            col: [0.0, 0.0, pack(elem::JUNCTION, 0, fl as u32).1, 0.0],
        };
        let c = g.push(mk(n.p));
        let ring: Vec<u32> = (0..16).map(|k| g.push(mk(n.p + Vec2::from_angle(k as f32 / 16.0 * std::f32::consts::TAU) * r))).collect();
        for k in 0..16 {
            g.tri(c, ring[(k + 1) % 16], ring[k]);
        }
    }

    // ---- props
    let mut props: Vec<Prop> = Vec::new();
    let mut controllers = Vec::new();
    let mut add = |kind: PropKind, p: Vec2, h: H, heading: f32, id: u64| {
        if in_tile(p) {
            props.push(Prop { kind, p, h, heading, id });
        }
    };
    let ground = |n: i64| H::Ground { off: node_off.get(&n).copied().unwrap_or(0.0) };
    for j in &junctions {
        let arm_ways: Vec<&RWay> = j.arms.iter().map(|a| &ways[segs[a.0].way as usize]).collect();
        // Street name signs on one corner: the two most important distinct names.
        let mut named: Vec<(Class, &str, Vec2)> = Vec::new();
        for (a, w) in j.arms.iter().zip(&arm_ways) {
            if let Some(n) = w.tags.get("name")
                && !named.iter().any(|x| x.1 == n)
                && w.class > Class::Trunk
            {
                named.push((w.class, n, a.2));
            }
        }
        named.sort_by_key(|x| x.0);
        if named.len() >= 2 {
            // On the corner to the right of the first named arm.
            let (a0, d) = (0usize, named[0].2);
            let arm = j.arms.iter().find(|a| a.2 == d).unwrap();
            let off = arm.3 + 1.2;
            let pos = j.p + d * (segs[arm.0].trim(arm.1) + 1.0) + right(d) * off;
            let _ = a0;
            add(
                PropKind::StreetSign { names: [named[0].1.to_string(), named[1].1.to_string()] },
                pos,
                ground(j.node),
                heading_of(d),
                hash64(j.node as u64 ^ 0x51),
            );
        }
        // Control devices on each approach, on the right side facing arriving traffic.
        let mut group_of_arm = vec![0u8; j.arms.len()];
        let groups = if j.control == Control::Signals {
            // Pair up opposite arms: arms whose directions are within 40° of opposite.
            let mut g: Vec<u8> = vec![u8::MAX; j.arms.len()];
            let mut next = 0u8;
            for a in 0..j.arms.len() {
                if g[a] != u8::MAX {
                    continue;
                }
                g[a] = next;
                for b in a + 1..j.arms.len() {
                    if g[b] == u8::MAX && j.arms[a].2.dot(j.arms[b].2) < -0.75 {
                        g[b] = next;
                    }
                }
                next += 1;
            }
            group_of_arm = g;
            next.max(2)
        } else {
            0
        };
        if j.control == Control::Signals {
            controllers.push(Controller { id: j.node, p: j.p, groups, cycle_offset: hash01(j.node as u64) * 60.0 });
        }
        for (ai, (a, w)) in j.arms.iter().zip(&arm_ways).enumerate() {
            let arriving = if a.1 { !w.prof.oneway } else { true };
            if !arriving || w.class <= Class::Trunk && j.control != Control::Signals {
                continue;
            }
            let d = a.2;
            let trim = segs[a.0].trim(a.1);
            // Arriving traffic travels along -d; its right is right(-d) = -right(d).
            let rside = -right(d);
            let base = j.p + d * (trim + 1.5) + rside * (a.3 + 0.8);
            let face = heading_of(d); // faces traffic coming toward the junction
            let id = hash64(j.node as u64 ^ (ai as u64 + 1) * 0x9e37);
            match j.control {
                Control::Signals => {
                    let mast = (a.3 * 1.2).clamp(3.0, 12.0);
                    add(PropKind::Signal { junction: j.node, group: group_of_arm[ai], mast }, base, ground(j.node), face, id);
                    if j.urban {
                        let pp = j.p + d * (trim + 0.5) - rside * (a.3 + 1.0);
                        add(PropKind::PedSignal { junction: j.node, group: group_of_arm[ai] }, pp, ground(j.node), face, id ^ 0x77);
                    }
                }
                Control::AllStop => add(PropKind::Stop { all_way: true }, base, ground(j.node), face, id),
                Control::MinorStop if !w.class.major() || arm_ways.iter().all(|w| w.class.major()) => {
                    add(PropKind::Stop { all_way: false }, base, ground(j.node), face, id)
                }
                Control::MinorYield if !w.class.major() => add(PropKind::Yield, base, ground(j.node), face, id),
                _ => {}
            }
        }
    }
    // Mapped point features.
    let lamps_mapped = osm.nodes.iter().filter(|n| n.tags.is("highway", "street_lamp")).count() >= 8;
    for nd in &osm.nodes {
        let (la, lo) = (nd.lat, nd.lon);
        let p = local.xz(la, lo);
        let id = hash64(nd.id as u64);
        let h = H::Ground { off: node_off.get(&nd.id).copied().unwrap_or(0.0) };
        // Facing: along the nearest road if the node is on one.
        let on_way = nodes.get(&nd.id).and_then(|n| n.arms.first()).map(|a| {
            let w = &ways[a.way as usize];
            let j = (a.idx as i64 + a.dir as i64) as usize;
            ((w.pts[j] - w.pts[a.idx as usize]).normalize_or(Vec2::X) * a.dir as f32, a.way)
        });
        let heading = on_way.map(|(d, _)| heading_of(right(d))).unwrap_or((hash01(id) - 0.5) * 6.28);
        let t = &nd.tags;
        let kind = if t.is("highway", "street_lamp") {
            Some(PropKind::Lamp { arm: 1.8 })
        } else if t.is("emergency", "fire_hydrant") {
            Some(PropKind::Hydrant)
        } else if t.is("amenity", "post_box") {
            Some(PropKind::Mailbox)
        } else if t.is("amenity", "bench") {
            Some(PropKind::Bench)
        } else if t.is("power", "pole") {
            Some(PropKind::UtilityPole)
        } else if t.is("man_made", "surveillance") && t.get("surveillance:type").is_some_and(|v| v.eq_ignore_ascii_case("alpr")) {
            Some(PropKind::Alpr)
        } else if t.is("highway", "bus_stop") {
            Some(PropKind::BusStop { shelter: t.flag("shelter"), bench: t.flag("bench") })
        } else if t.is("barrier", "bollard") {
            Some(PropKind::Bollard)
        } else if t.is("barrier", "traffic_cone") {
            Some(PropKind::Cone)
        } else if t.is("highway", "motorway_junction") {
            let exit = t.get("ref").unwrap_or("").to_string();
            let text = t.get("name").or(t.get("exit_to")).or(t.get("destination")).unwrap_or("").to_string();
            (!exit.is_empty() || !text.is_empty()).then_some(PropKind::ExitSign { exit, text })
        } else {
            None
        };
        let Some(kind) = kind else { continue };
        // Things standing on a road node go beside the road instead.
        let p = match (&kind, on_way) {
            (PropKind::ExitSign { .. } | PropKind::BusStop { .. } | PropKind::Lamp { .. }, Some((d, wi))) => {
                p + right(d) * (ways[wi as usize].prof.half() + 1.5)
            }
            _ => p,
        };
        add(kind, p, h, heading, id);
    }
    // Inferred streetlights along urban roads, mapped guardrails, inferred guardrails,
    // construction cones, route shields and speed limits.
    for s in &segs {
        let w = &ways[s.way as usize];
        let full = &w.pts[s.i0..=s.i1];
        let pts = cut(full, s.trim0, s.trim1);
        if pts.len() < 2 {
            continue;
        }
        let len = polyline_len(&pts);
        let at = |d: f32| -> (Vec2, Vec2) {
            let mut acc = 0.0;
            for q in pts.windows(2) {
                let l = q[0].distance(q[1]);
                if acc + l >= d {
                    let t = ((d - acc) / l.max(1e-4)).clamp(0.0, 1.0);
                    return (q[0].lerp(q[1], t), (q[1] - q[0]).normalize_or(Vec2::X));
                }
                acc += l;
            }
            let n = pts.len();
            (pts[n - 1], (pts[n - 1] - pts[n - 2]).normalize_or(Vec2::X))
        };
        let hw = w.prof.half();
        let seed = hash64(w.id as u64);
        let h_at = |p: Vec2| {
            if w.elevated() {
                // Props on decks use the nearest deck node pair (approximate).
                let mut best = (f32::MAX, 0u32);
                for &n in &w.nodes[s.i0..=s.i1] {
                    if let Some(&di) = deck_node.get(&n) {
                        let d = decks.p[di as usize].distance(p);
                        if d < best.0 {
                            best = (d, di);
                        }
                    }
                }
                H::Deck { a: best.1, b: best.1, t: 0.0 }
            } else {
                H::Ground { off: 0.0 }
            }
        };
        if !lamps_mapped && w.class.vehicle() && w.class <= Class::Residential && !w.tunnel && (is_urban(pts[0]) || w.tags.is("lit", "yes"))
        {
            let spacing = if w.class.major() { 36.0 } else { 45.0 };
            let mut d = spacing * 0.5 + hash01(seed) * spacing * 0.5;
            let mut k = 0;
            while d < len - 5.0 {
                let (p, dir) = at(d);
                // Major roads: staggered both sides; minor roads: one side.
                let side = if w.class.major() && k % 2 == 1 { -1.0 } else { 1.0 };
                let pos = p + right(dir) * side * (hw + 0.9 + if w.class.major() { 1.0 } else { 0.0 });
                add(PropKind::Lamp { arm: (hw * 0.5).clamp(1.2, 3.5) }, pos, h_at(pos), heading_of(-right(dir) * side), seed ^ k);
                d += spacing;
                k += 1;
            }
        }
        // Guardrails: highway bridge approaches and high embankments.
        if w.class <= Class::Primary && w.class.vehicle() && !w.elevated() {
            let fills: Vec<bool> = (0..=((len / 4.0) as usize))
                .map(|i| {
                    let (p, _) = at(i as f32 * 4.0);
                    let n = s.i0 + (0..=(s.i1 - s.i0)).min_by(|&a, &b| full[a].distance(p).total_cmp(&full[b].distance(p))).unwrap_or(0);
                    node_off.get(&w.nodes[n]).copied().unwrap_or(0.0) > 1.5
                })
                .collect();
            let near_bridge = [(s.i0, true), (s.i1, false)]
                .iter()
                .any(|&(i, _)| nodes.get(&w.nodes[i]).is_some_and(|n| n.arms.iter().any(|a| ways[a.way as usize].bridge)));
            for (i, &f) in fills.iter().enumerate() {
                let d = i as f32 * 4.0;
                let bridge_zone = near_bridge
                    && ((d < 40.0 && nodes[&w.nodes[s.i0]].arms.iter().any(|a| ways[a.way as usize].bridge))
                        || (d > len - 40.0 && nodes[&w.nodes[s.i1]].arms.iter().any(|a| ways[a.way as usize].bridge)));
                if (f || bridge_zone) && d + 4.0 <= len {
                    let (p, dir) = at(d + 2.0);
                    for side in [1.0f32, -1.0] {
                        if side < 0.0 && !w.prof.oneway {
                            // Two-way roads: both sides; divided carriageways: right side only.
                        } else if side < 0.0 {
                            continue;
                        }
                        let pos = p + right(dir) * side * (hw + 0.6);
                        add(
                            PropKind::Guardrail { len: 4.0 },
                            pos,
                            H::Ground { off: 0.0 },
                            heading_of(dir),
                            seed ^ (i as u64) << 1 ^ (side > 0.0) as u64,
                        );
                    }
                }
            }
        }
        if w.construction {
            let mut d = 2.0;
            let mut k = 0u64;
            while d < len {
                let (p, dir) = at(d);
                let pos = p + right(dir) * (hw - 0.4);
                add(if k % 5 == 4 { PropKind::Barrel } else { PropKind::Cone }, pos, h_at(pos), heading_of(dir), seed ^ k);
                d += 4.0;
                k += 1;
            }
        }
        // Route shields on numbered roads, roughly every 1.6 km, after leaving junctions.
        if let Some(r) = w.tags.get("ref")
            && w.class <= Class::Secondary
            && !w.link
        {
            let phase = (seed % 1600) as f32;
            let start_abs = phase % 1600.0;
            let mut d = start_abs;
            while d < len - 10.0 {
                let (p, dir) = at(d);
                let pos = p + right(dir) * (hw + 1.6);
                let text = r.split(';').next().unwrap_or(r).trim().to_string();
                add(PropKind::Shield { text }, pos, h_at(pos), heading_of(-dir), seed ^ 0x5a ^ d as u64);
                d += 1600.0;
            }
            // Short ways near junctions get one too, so junctions show what road you're on.
            if len > 60.0 && len < 1600.0 && hash01(seed ^ 3) < 0.35 {
                let (p, dir) = at(25.0);
                let pos = p + right(dir) * (hw + 1.6);
                let text = r.split(';').next().unwrap_or(r).trim().to_string();
                add(PropKind::Shield { text }, pos, h_at(pos), heading_of(-dir), seed ^ 0x5b);
            }
        }
        if let Some(mph) = w.tags.get("maxspeed").and_then(parse_mph)
            && w.class.vehicle()
            && len > 120.0
            && hash01(seed ^ 9) < 0.5
        {
            let (p, dir) = at(30.0);
            let pos = p + right(dir) * (hw + 1.4);
            add(PropKind::SpeedLimit { mph }, pos, h_at(pos), heading_of(-dir), seed ^ 0x99);
        }
    }
    for gr in &guard_rails {
        let pts = densify(gr, 4.0);
        for (i, q) in pts.windows(2).enumerate() {
            let mid = (q[0] + q[1]) * 0.5;
            let dir = (q[1] - q[0]).normalize_or(Vec2::X);
            add(
                PropKind::Guardrail { len: q[0].distance(q[1]) },
                mid,
                H::Ground { off: 0.0 },
                heading_of(dir),
                hash64(i as u64 ^ gr.len() as u64 ^ (mid.x.to_bits() as u64) << 20),
            );
        }
    }
    stats.props = props.len();
    for p in props {
        let cid = chunk_of(p.p);
        chunks.entry(cid).or_insert_with(|| new_chunk(cid)).props.push(p);
    }

    // Influence: chunk bounds plus the extent of any deck network used inside.
    let mut net_bbox: Vec<(f64, f64, f64, f64)> = vec![(90.0, 180.0, -90.0, -180.0); decks.nets as usize];
    for (i, &p) in decks.p.iter().enumerate() {
        let (la, lo) = local.ll(p);
        let b = &mut net_bbox[decks.net[i] as usize];
        *b = (b.0.min(la), b.1.min(lo), b.2.max(la), b.3.max(lo));
    }
    let mut out_chunks: Vec<Chunk> = chunks.into_values().collect();
    for c in out_chunks.iter_mut() {
        let (w, s, e, n) = c.id.bounds();
        let mut inf = (s, w, n, e);
        let mut seen = HashSet::new();
        for v in &c.geom.v {
            if let H::Deck { a, .. } = v.h
                && seen.insert(decks.net[a as usize])
            {
                let b = net_bbox[decks.net[a as usize] as usize];
                inf = (inf.0.min(b.0), inf.1.min(b.1), inf.2.max(b.2), inf.3.max(b.3));
            }
        }
        c.influence = inf;
        stats.vertices += c.geom.v.len();
    }
    Built { tile, local, chunks: out_chunks, decks, corridors, controllers, lanes, stats }
}

fn default_stats() -> BuildStats {
    BuildStats::default()
}

impl Seg {
    fn trim(&self, at_start: bool) -> f32 {
        if at_start { self.trim0 } else { self.trim1 }
    }
}

fn new_chunk(id: TileId) -> Chunk {
    let (w, s, e, n) = id.bounds();
    Chunk { id, geom: Geom::default(), props: Vec::new(), influence: (s, w, n, e) }
}

fn parse_mph(v: &str) -> Option<u32> {
    let n: u32 = v.split_whitespace().next()?.parse().ok()?;
    if v.contains("mph") { Some(n) } else { Some((n as f32 / 1.609).round() as u32) }.filter(|&m| (5..=85).contains(&m))
}

/// Which sides get a sidewalk: explicit tags win; otherwise urban streets get both,
/// unless sidewalks are mapped as their own ways in this area.
fn sidewalks(w: &RWay, mapped: bool, urban: bool) -> (bool, bool) {
    let t = &w.tags;
    let parse = |v: Option<&str>| v.map(|v| v != "no" && v != "none" && v != "separate");
    if let Some(v) = t.get("sidewalk") {
        return match v {
            "both" => (true, true),
            "left" => (true, false),
            "right" => (false, true),
            _ => (false, false),
        };
    }
    if let Some(b) = parse(t.get("sidewalk:both")) {
        return (b, b);
    }
    let l = parse(t.get("sidewalk:left"));
    let r = parse(t.get("sidewalk:right"));
    if l.is_some() || r.is_some() {
        return (l.unwrap_or(false), r.unwrap_or(false));
    }
    let candidate =
        matches!(w.class, Class::Primary | Class::Secondary | Class::Tertiary | Class::Residential | Class::Living | Class::Unclassified);
    let on = candidate && urban && !mapped && !w.link;
    (on, on)
}

/// A square column from the ground up to just under a deck.
fn pier(g: &mut Geom, c: Vec2, dir: Vec2, top: H, top_dy: f32, half: f32) {
    let r = right(dir);
    let corners = [c + (dir + r) * half, c + (dir - r) * half, c + (-dir - r) * half, c + (-dir + r) * half];
    let fl = flag::STRUCTURE as f32;
    for k in 0..4 {
        let (p0, p1) = (corners[k], corners[(k + 1) % 4]);
        let mk = |p: Vec2, h: H, dy: f32, v: f32| V {
            p,
            h,
            dy,
            uv: [k as f32, v],
            uv2: [0.0, pack(elem::STRUCTURE, 0, fl as u32).0],
            col: [0.0, 0.0, pack(elem::STRUCTURE, 0, fl as u32).1, 0.0],
        };
        let a = g.push(mk(p0, top, top_dy, 1.0));
        let b = g.push(mk(p1, top, top_dy, 1.0));
        let c0 = g.push(mk(p0, H::Ground { off: 0.0 }, -1.0, 0.0));
        let d = g.push(mk(p1, H::Ground { off: 0.0 }, -1.0, 0.0));
        g.quad(a, b, c0, d);
    }
}

#[derive(PartialEq, PartialOrd)]
struct OrdF(f32);
impl Eq for OrdF {}
impl Ord for OrdF {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&o.0)
    }
}

/// Deck heights for every node: boundary nodes sit on the ground (plus their ramp
/// offset); the rest are interpolated by inverse graph distance to the boundaries
/// (straight lines along simple bridges) and kept clear of what they cross.
pub fn solve_decks(d: &Decks, ground: &dyn Fn(usize) -> f32, off: &dyn Fn(usize) -> f32) -> Vec<f32> {
    let n = d.p.len();
    let mut num = vec![0.0f64; n];
    let mut den = vec![0.0f64; n];
    let mut h = vec![0.0f32; n];
    for b in 0..n {
        if !d.boundary[b] {
            continue;
        }
        let hb = ground(b) + off(b);
        h[b] = hb;
        // Dijkstra from this boundary over its network.
        let mut dist: HashMap<usize, f32> = HashMap::new();
        let mut heap = BinaryHeap::new();
        dist.insert(b, 0.0);
        heap.push((OrdF(-0.0), b));
        while let Some((OrdF(nd), u)) = heap.pop() {
            let du = -nd;
            if dist.get(&u).is_some_and(|&x| x < du) {
                continue;
            }
            for &(v, w) in &d.edges[u] {
                let v = v as usize;
                let dv = du + w;
                if d.boundary[v] {
                    continue;
                }
                if dist.get(&v).is_none_or(|&x| dv < x) {
                    dist.insert(v, dv);
                    heap.push((OrdF(-dv), v));
                }
            }
        }
        for (v, dv) in dist {
            if v != b {
                let wgt = 1.0 / dv.max(0.5) as f64;
                num[v] += wgt * hb as f64;
                den[v] += wgt;
            }
        }
    }
    // Distance to the nearest boundary, to fade clearances in near the ends.
    for i in 0..n {
        if !d.boundary[i] {
            h[i] = if den[i] > 0.0 { (num[i] / den[i]) as f32 } else { ground(i) + d.clear[i].max(0.0) };
        }
    }
    let mut out = h.clone();
    for i in 0..n {
        if d.boundary[i] {
            continue;
        }
        let c = d.clear[i];
        // How far from the nearest boundary (approx: weight sum is 1/d per boundary).
        let near = if den[i] > 0.0 { (1.0 / den[i]) as f32 } else { 100.0 };
        let fade = (near / 25.0).clamp(0.0, 1.0);
        let g = ground(i);
        if c > 0.0 {
            out[i] = out[i].max(g + c * fade);
        } else if c < 0.0 {
            out[i] = out[i].min(g + c * fade);
        }
    }
    // A few smoothing passes over interior nodes so clearance bumps blend in.
    for _ in 0..4 {
        let prev = out.clone();
        for i in 0..n {
            if d.boundary[i] || d.edges[i].is_empty() {
                continue;
            }
            let (mut s, mut ws) = (prev[i] as f64 * 2.0, 2.0f64);
            for &(v, w) in &d.edges[i] {
                let k = 1.0 / (w as f64).max(1.0);
                s += prev[v as usize] as f64 * k * 2.0;
                ws += k * 2.0;
            }
            let sm = (s / ws) as f32;
            let g = ground(i);
            let c = d.clear[i];
            out[i] = if c > 0.0 {
                sm.max(g + c * 0.9)
            } else if c < 0.0 {
                sm.min(g + c * 0.9)
            } else {
                sm
            };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use usx_core::osm::{Node, Way};

    fn tags(v: &[(&str, &str)]) -> Tags {
        Tags(v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect())
    }

    /// A + crossing of two streets with a bridge carrying a third road over one of them.
    fn sample(tile: TileId) -> OsmTile {
        let (la, lo) = tile.lat_lon_at(0.5, 0.5);
        let d = 0.001; // ~110 m
        let mut t = OsmTile::default();
        let n = |id: i64, a: f64, b: f64| (id, (la + a, lo + b));
        let pts = [n(1, 0.0, -2.0 * d), n(2, 0.0, 0.0), n(3, 0.0, 2.0 * d), n(4, -2.0 * d, 0.0), n(5, 2.0 * d, 0.0)];
        let p = |id: i64| pts.iter().find(|x| x.0 == id).unwrap().1;
        t.ways.push(Way {
            id: 10,
            nodes: vec![1, 2, 3],
            pts: vec![p(1), p(2), p(3)],
            tags: tags(&[("highway", "secondary"), ("name", "Main Street")]),
        });
        t.ways.push(Way {
            id: 11,
            nodes: vec![4, 2, 5],
            pts: vec![p(4), p(2), p(5)],
            tags: tags(&[("highway", "secondary"), ("name", "First Avenue")]),
        });
        // A road bridging over Main Street east of the junction.
        let b = |a: f64, c: f64| (la + a, lo + c);
        t.ways.push(Way {
            id: 20,
            nodes: vec![30, 31],
            pts: vec![b(-2.0 * d, 1.0 * d), b(-0.4 * d, 1.0 * d)],
            tags: tags(&[("highway", "tertiary")]),
        });
        t.ways.push(Way {
            id: 21,
            nodes: vec![31, 32],
            pts: vec![b(-0.4 * d, 1.0 * d), b(0.4 * d, 1.0 * d)],
            tags: tags(&[("highway", "tertiary"), ("bridge", "yes"), ("layer", "1")]),
        });
        t.ways.push(Way {
            id: 22,
            nodes: vec![32, 33],
            pts: vec![b(0.4 * d, 1.0 * d), b(2.0 * d, 1.0 * d)],
            tags: tags(&[("highway", "tertiary")]),
        });
        t.nodes.push(Node { id: 2, lat: la, lon: lo, tags: tags(&[("highway", "traffic_signals")]) });
        t
    }

    #[test]
    fn builds_junction_bridge_and_ramps() {
        let tile = TileId::containing(39.75, -104.99, 14).0;
        let osm = sample(tile);
        let b = build(tile, &osm, &|_, _| Some(1600.0));
        assert_eq!(b.stats.junctions, 1);
        assert_eq!(b.controllers.len(), 1);
        assert_eq!(b.controllers[0].groups, 2);
        assert!(b.stats.bridges == 1);
        // Bridge ends must be raised to clear the street, and the approaches ramp up.
        let solved = solve_decks(&b.decks, &|_| 1600.0, &|i| b.decks.off[i]);
        assert!(b.decks.boundary.iter().filter(|&&x| x).count() == 2);
        for (i, h) in solved.iter().enumerate() {
            assert!(*h > 1600.0 + 5.0, "deck node {i} at {h}");
        }
        let ramp = b.corridors.iter().flat_map(|c| c.off.iter()).copied().fold(0.0f32, f32::max);
        assert!(ramp > 5.0, "approach fill {ramp}");
        // Geometry: every index is valid; signals and street signs placed.
        let mut verts = 0;
        for c in &b.chunks {
            verts += c.geom.v.len();
            assert!(c.geom.i.iter().all(|&i| (i as usize) < c.geom.v.len()));
            assert_eq!(c.geom.i.len() % 3, 0);
        }
        assert!(verts > 100);
        let props: Vec<&PropKind> = b.chunks.iter().flat_map(|c| c.props.iter().map(|p| &p.kind)).collect();
        assert_eq!(props.iter().filter(|k| matches!(k, PropKind::Signal { .. })).count(), 4);
        assert!(props.iter().any(|k| matches!(k, PropKind::StreetSign { names } if names.contains(&"Main Street".to_string()))));
    }

    #[test]
    fn local_round_trip_and_cut() {
        let l = Local::new(39.75, -104.99);
        let p = l.xz(39.7512, -104.9873);
        let (a, b) = l.ll(p);
        assert!((a - 39.7512).abs() < 1e-9 && (b + 104.9873).abs() < 1e-9);
        let line = vec![Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let c = cut(&line, 2.0, 3.0);
        assert!((polyline_len(&c) - 15.0).abs() < 1e-4);
        assert_eq!(c[0], Vec2::new(2.0, 0.0));
        assert!(densify(&line, 3.0).windows(2).all(|w| w[0].distance(w[1]) <= 3.0 + 1e-4));
    }

    #[test]
    fn potholes_match_smoothness() {
        let n_good = (0..2000).filter(|&c| pothole(7, c, 1).is_some()).count();
        let n_bad = (0..2000).filter(|&c| pothole(7, c, 6).is_some()).count();
        assert!(n_bad > n_good * 10);
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;

    /// Real Overpass answers from the `fixtures` branch: USX_FIXTURES=dir cargo test fixtures -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fixtures() {
        let dir = std::env::var("USX_FIXTURES").expect("USX_FIXTURES");
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let parts: Vec<u32> = name.split('-').map(|x| x.parse().unwrap()).collect();
            let tile = TileId { z: parts[0] as u8, x: parts[1], y: parts[2] };
            let json = std::fs::read(&path).unwrap();
            let t0 = std::time::Instant::now();
            let osm = usx_core::osm::parse(&json).unwrap();
            let t1 = t0.elapsed();
            let bin = usx_core::osm::encode(&osm);
            assert_eq!(usx_core::osm::decode(&bin).unwrap(), osm);
            // Optionally seed a game cache so the game can run offline on this data.
            if let Ok(seed) = std::env::var("USX_SEED_CACHE") {
                let path = std::path::Path::new(&seed).join(format!("osm/v1/{}.bin", tile.path()));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, &bin).unwrap();
            }
            let t2 = std::time::Instant::now();
            let b = build(tile, &osm, &|_, _| Some(100.0));
            let t3 = t2.elapsed();
            let tris: usize = b.chunks.iter().map(|c| c.geom.i.len() / 3).sum();
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for c in &b.chunks {
                for p in &c.props {
                    let k = format!("{:?}", p.kind);
                    *kinds.entry(k.split([' ', '{', '(']).next().unwrap().to_string()).or_default() += 1;
                }
                for v in &c.geom.v {
                    assert!(v.p.is_finite(), "non-finite vertex");
                }
                assert!(c.geom.i.iter().all(|&i| (i as usize) < c.geom.v.len()));
            }
            println!(
                "{name}: json {} KB -> bin {} KB, parse {:?}, build {:?}, {} ways, {} junctions, {} bridges, {} tunnels, {} chunks, {} verts, {} tris, {} signals",
                json.len() / 1024,
                bin.len() / 1024,
                t1,
                t3,
                b.stats.ways,
                b.stats.junctions,
                b.stats.bridges,
                b.stats.tunnels,
                b.chunks.len(),
                b.stats.vertices,
                tris,
                b.controllers.len()
            );
            println!("   props: {kinds:?}");
        }
    }
}
