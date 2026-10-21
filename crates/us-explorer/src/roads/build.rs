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
    /// On the visible ground (levelled under roads), raised by `off` where the terrain
    /// hasn't been levelled (coarse tiles, or roads that arrived late). For objects.
    Ground { off: f32 },
    /// Road surfaces: the natural ground at `at` (the centre line, so a road is level
    /// across) plus `off`. Independent of how the terrain was levelled, so a road
    /// crossing over another isn't pulled down to the lower one's cutting.
    GroundAt { at: Vec2, off: f32 },
    /// Like `GroundAt`, raised by `off`, of which the terrain carries only `fill` (cities
    /// don't build big embankments: the rest is a structure on retaining walls).
    Raised { at: Vec2, off: f32, fill: f32 },
    /// The terrain itself, ignoring any road offsets (foot of walls and piers).
    Terrain,
    /// On a bridge or tunnel deck, between two deck nodes.
    Deck { a: u32, b: u32, t: f32 },
    /// A water surface: the lowest natural ground within a few metres (a river in a
    /// channel lies at the channel's bottom, not at the street level beside it).
    Water { at: Vec2 },
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
    pub const STEPS: u32 = 1 << 23;
    /// Parking lots: surface, paint and islands (the kind in `uv2[0]`).
    pub const LOT: u32 = 1 << 22;
    /// Inland water surface (lakes, rivers).
    pub const WATER: u32 = 1 << 24;
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
    pub(crate) fn push(&mut self, v: V) -> u32 {
        self.v.push(v);
        (self.v.len() - 1) as u32
    }
    pub(crate) fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.i.extend_from_slice(&[a, b, c]);
    }
    pub(crate) fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        // a-b across one row (left to right of travel), c-d the next row: counter-
        // clockwise seen from above (x east, z south, y up).
        self.i.extend_from_slice(&[a, b, c, b, d, c]);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropKind {
    StreetSign {
        names: [String; 2],
    },
    Stop {
        all_way: bool,
    },
    Yield,
    Signal {
        junction: i64,
        group: u8,
        mast: f32,
    },
    PedSignal {
        junction: i64,
        group: u8,
    },
    Lamp {
        arm: f32,
    },
    Guardrail {
        len: f32,
    },
    Cone,
    Barrel,
    Alpr,
    UtilityPole,
    Hydrant,
    Mailbox,
    BusStop {
        shelter: bool,
        bench: bool,
    },
    Bench,
    Shield {
        text: String,
    },
    ExitSign {
        exit: String,
        text: String,
    },
    Bollard,
    SpeedLimit {
        mph: u32,
    },
    MileMarker {
        text: String,
    },
    /// Overhead guide sign on a gantry spanning the road (`span` between posts).
    Gantry {
        span: f32,
        exit: String,
        text: String,
    },
    /// Guide sign fixed to the side of an overpass.
    OverpassSign {
        exit: String,
        text: String,
    },
    Crossbuck {
        lights: bool,
    },
    /// Reserved (accessible) parking sign at the head of a stall.
    ParkingSign,
    /// A parked car (see `cars`).
    Car {
        body: u8,
        paint: u8,
    },
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
    /// Extra height above the height rule (cones standing on the road surface).
    pub dy: f32,
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

/// A lake, reservoir or river area for the terrain to dig a bed under (elevation
/// models record the water surface, so without this the water has no depth).
#[derive(Clone, Debug)]
pub struct Lake {
    pub outer: Vec<Vec<(f64, f64)>>,
    pub inner: Vec<Vec<(f64, f64)>>,
    pub bbox: (f64, f64, f64, f64), // s, w, n, e
    /// Deepest point, metres below the surface; the bed slopes down from the shore.
    pub depth: f32,
    /// Not water: a paved area (a parking lot) whose ground is smoothed so the paving
    /// lies flat on it.
    pub level: bool,
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
    /// Water areas touching the tile (for swimming), as in the OSM data.
    pub water: Vec<usx_core::osm::Area>,
    /// The same areas as beds for the terrain.
    pub lakes: Vec<Lake>,
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
    pub parking_stalls: usize,
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
    /// A short link inside a merged intersection (divided roads): drawn as part of the
    /// intersection surface, without markings.
    internal: bool,
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
    /// Index of the first junction of the merged intersection this one belongs to.
    cluster: usize,
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

pub(crate) fn hash64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^ (x >> 33)
}

pub(crate) fn hash01(x: u64) -> f32 {
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
    pack_g(el, seed, 0, flags)
}

/// Like `pack`, with 4 bits of street-light layout (see `lamp_layout`).
pub fn pack_g(el: f32, seed: u32, glow: u32, flags: u32) -> (f32, f32) {
    let hi = (flags >> 20) & 31;
    (el + ((seed & 0xff) * 8) as f32 + ((glow & 15) << 11) as f32 + (hi << 15) as f32, (flags & 0xfffff) as f32)
}

/// Inferred street lights along a way: (spacing, distance of the first, staggered both
/// sides, glow code for the road shader). Mirrored in road.wgsl so the pools of light
/// on the road line up with the lamps.
fn lamp_layout(w: &RWay) -> (f32, f32, bool, u32) {
    let major = w.class.major();
    let spacing = if major { 36.0 } else { 45.0 };
    let q = (hash64(w.id as u64 ^ 0x1a3) & 3) as u32;
    (spacing, spacing * (0.5 + 0.125 * q as f32), major, 1 | (major as u32) << 1 | q << 2)
}

pub(crate) fn heading_of(d: Vec2) -> f32 {
    // x east, z south: heading clockwise from north.
    d.x.atan2(-d.y)
}

// ------------------------------------------------------------------ build

pub fn build(tile: TileId, osm: &OsmTile, dem: &dyn Fn(f64, f64) -> Option<f64>) -> Built {
    build_with(tile, osm, dem, &[])
}

/// Flowing water nearby: centrelines (lat, lon) in the direction of flow, and speed (m/s).
pub type Flows = [(Vec<(f64, f64)>, f32)];

/// Encode a flow (local x, z metres per second) for a water vertex's `uv2.x`.
pub fn encode_flow(v: Vec2) -> f32 {
    let speed = v.length();
    if speed < 0.02 {
        return 0.0;
    }
    let a = v.y.atan2(v.x).rem_euclid(std::f32::consts::TAU);
    let aq = ((a / std::f32::consts::TAU * 1024.0).round() as u32) % 1024;
    let sq = ((speed * 20.0).round() as u32).min(80);
    1.0 + (sq * 1024 + aq) as f32
}

pub fn build_with(tile: TileId, osm: &OsmTile, dem: &dyn Fn(f64, f64) -> Option<f64>, flows: &Flows) -> Built {
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
    let mut power_lines: Vec<(Vec<i64>, Vec<Vec2>)> = Vec::new();
    for w in &osm.ways {
        if w.tags.is("barrier", "guard_rail") && w.tags.get("highway").is_none() {
            guard_rails.push(w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect());
            continue;
        }
        if w.tags.is("power", "minor_line") || w.tags.is("power", "line") && w.tags.get("voltage").is_some_and(|v| v.len() <= 5) {
            power_lines.push((w.nodes.clone(), w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect()));
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
                internal: false,
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
        junctions.push(Junction { node: nid, p: n.p, arms, control, urban, poly: Vec::new(), corners: Vec::new(), cluster: 0 });
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

    // ---- merge junctions joined by short links: divided roads cross at 2-4 nodes
    // that are really one intersection.
    let mut jparent: Vec<usize> = (0..junctions.len()).collect();
    fn jfind(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        p[x] = r;
        r
    }
    for si in 0..segs.len() {
        let s = &segs[si];
        if s.end0 != EndKind::Junction || s.end1 != EndKind::Junction {
            continue;
        }
        let w = &ways[s.way as usize];
        if !w.class.vehicle() || w.elevated() || w.class <= Class::Trunk && w.link {
            continue;
        }
        let (Some(&ja), Some(&jb)) = (junction_at.get(&w.nodes[s.i0]), junction_at.get(&w.nodes[s.i1])) else { continue };
        let wide = |j: &Junction| j.arms.iter().map(|a| a.3).fold(0.0f32, f32::max);
        let limit = ((wide(&junctions[ja]) + wide(&junctions[jb])) * 1.3 + 8.0).min(42.0);
        if polyline_len(&w.pts[s.i0..=s.i1]) < limit {
            let (ra, rb) = (jfind(&mut jparent, ja), jfind(&mut jparent, jb));
            jparent[ra.max(rb)] = ra.min(rb);
            segs[si].internal = true;
        }
    }
    for k in 0..junctions.len() {
        junctions[k].cluster = jfind(&mut jparent, k);
    }
    let mut cluster_members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, j) in junctions.iter().enumerate() {
        cluster_members.entry(j.cluster).or_default().push(k);
    }
    // One control for the whole merged intersection: the strongest of its members.
    for members in cluster_members.values().filter(|m| m.len() > 1) {
        let best = members.iter().map(|&k| junctions[k].control).max_by_key(|c| match c {
            Control::Signals => 4,
            Control::AllStop => 3,
            Control::MinorStop => 2,
            Control::MinorYield => 1,
            Control::None => 0,
        });
        for &k in members {
            junctions[k].control = best.unwrap_or(Control::None);
        }
    }
    // Signal phases by direction, so every node of a merged intersection agrees: group
    // 0 runs along the main road, group 1 across it.
    let mut axis_of: HashMap<usize, Vec2> = HashMap::new();
    for (root, members) in &cluster_members {
        let mut best: Option<(Class, Vec2)> = None;
        for &k in members {
            for a in &junctions[k].arms {
                let c = ways[segs[a.0].way as usize].class;
                if !segs[a.0].internal && best.is_none_or(|b| c < b.0) {
                    best = Some((c, a.2));
                }
            }
        }
        axis_of.insert(*root, best.map(|b| b.1).unwrap_or(Vec2::X));
    }
    let group_of =
        |cluster: usize, d: Vec2| -> u8 { if axis_of.get(&cluster).is_some_and(|ax| ax.dot(d).abs() >= 0.7071) { 0 } else { 1 } };

    // ---- markings at segment ends
    for j in &junctions {
        for &(si, at_start, d, _) in &j.arms {
            if segs[si].internal {
                continue;
            }
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
        // Decks as they'd be with nothing to clear: straight between their ends. The
        // clearance each layer needs is kept aside for the crossings below.
        let layer_clear = std::mem::replace(&mut decks.clear, vec![0.0; decks.p.len()]);
        let deck_h = solve_decks(&decks, &|i| ground[i].unwrap_or(0.0), &|i| decks.off[i]);
        // Clearance is needed only where a deck crosses a road or railway on the ground.
        let mut grid: HashMap<(i32, i32), Vec<(Vec2, Vec2, usize)>> = HashMap::new();
        let cell = |p: Vec2| ((p.x / 50.0).floor() as i32, (p.y / 50.0).floor() as i32);
        // Footpaths don't need a bridge raised over them: where one passes under (a
        // river walk under street bridges) it's the path that's down low, and the
        // elevation data has the street level.
        for (wi, w) in ways.iter().enumerate().filter(|(_, w)| !w.elevated() && !w.class.path() && w.layer >= 0) {
            for q in w.pts.windows(2) {
                let (a, b) = (cell(q[0]), cell(q[1]));
                for x in a.0.min(b.0)..=a.0.max(b.0) {
                    for z in a.1.min(b.1)..=a.1.max(b.1) {
                        grid.entry((x, z)).or_default().push((q[0], q[1], wi));
                    }
                }
            }
        }
        let debug = std::env::var_os("USX_DEBUG_DECKS").is_some();
        let mut boundary_pts: Vec<Vec<Vec2>> = vec![Vec::new(); decks.nets as usize];
        for i in 0..decks.p.len() {
            if decks.boundary[i] {
                boundary_pts[decks.net[i] as usize].push(decks.p[i]);
            }
        }
        // Each crossing sets a height the deck must reach there: the nodes either side
        // get that as a clearance (the solver lifts the deck locally), and only the deck
        // ends too close to climb it in time (7% grade) are raised, with the roads on
        // the ground ramping up to them. (Raising a whole interchange by its worst
        // crossing put every ramp end metres in the air.)
        const DECK_GRADE: f32 = 0.07;
        let mut node_clear = vec![0.0f32; decks.p.len()];
        let mut raise = vec![0.0f32; decks.p.len()];
        let boundary_ids: Vec<Vec<usize>> = {
            let mut v = vec![Vec::new(); decks.nets as usize];
            for i in 0..decks.p.len() {
                if decks.boundary[i] {
                    v[decks.net[i] as usize].push(i);
                }
            }
            v
        };
        for i in 0..decks.p.len() {
            for &(j, _) in &decks.edges[i] {
                let j = j as usize;
                if j < i {
                    continue;
                }
                let (pa, pb) = (decks.p[i], decks.p[j]);
                let (ca, cb) = (cell(pa), cell(pb));
                let mut hit: Option<(f32, usize)> = None;
                for x in ca.0.min(cb.0)..=ca.0.max(cb.0) {
                    for z in ca.1.min(cb.1)..=ca.1.max(cb.1) {
                        for &(qa, qb, wi) in grid.get(&(x, z)).map(|v| v.as_slice()).unwrap_or(&[]) {
                            if let Some(t) = seg_cross(pa, pb, qa, qb) {
                                hit = Some(hit.map_or((t, wi), |h: (f32, usize)| if (h.0 - 0.5).abs() < (t - 0.5).abs() { h } else { (t, wi) }));
                            }
                        }
                    }
                }
                let Some((t, under)) = hit else { continue };
                // A crossing right at the end of a short deck is an at-grade junction
                // drawn across a bridge's end, not a road passing underneath.
                let cross_at = pa.lerp(pb, t);
                let net = decks.net[i] as usize;
                let near_end = boundary_ids[net].iter().any(|&q| decks.p[q].distance(cross_at) < 12.0);
                if near_end {
                    if debug {
                        let (la, lo) = local.ll(cross_at);
                        eprintln!("deck net {net} at {la:.6},{lo:.6}: crossing {:?} next to the deck's end ignored", ways[under].tags.get("name"));
                    }
                    continue;
                }
                let c = if layer_clear[i].abs() > layer_clear[j].abs() { layer_clear[i] } else { layer_clear[j] };
                let (la, lo) = local.ll(cross_at);
                let g = dem(la, lo).map(|h| h as f32).unwrap_or(ground[i].unwrap_or(0.0));
                let target = g + c;
                let line = deck_h[i] + (deck_h[j] - deck_h[i]) * t;
                if (c > 0.0 && line >= target) || (c < 0.0 && line <= target) {
                    continue;
                }
                if debug {
                    eprintln!(
                        "deck net {net} at {la:.6},{lo:.6} crosses {:?} {:?} ({}): ground {g:.1} line {line:.1} clear {c:.1} -> needs {:.1} more",
                        ways[under].class,
                        ways[under].tags.get("name"),
                        ways[under].id,
                        target - line
                    );
                }
                // The deck reaches the target here and slopes away from it at deck
                // grade (so a viaduct over several roads stays smooth between them).
                let len_ij = pa.distance(pb);
                let mut dist: HashMap<usize, f32> = HashMap::new();
                let mut heap: BinaryHeap<(OrdF, usize)> = BinaryHeap::new();
                for (k, d0) in [(i, t * len_ij), (j, (1.0 - t) * len_ij)] {
                    dist.insert(k, d0);
                    heap.push((OrdF(-d0), k));
                }
                let reach = (c.abs() + 2.0) / DECK_GRADE;
                while let Some((OrdF(nd), u)) = heap.pop() {
                    let du = -nd;
                    if dist.get(&u).is_some_and(|&x| x < du) || du > reach {
                        continue;
                    }
                    if !decks.boundary[u] {
                        let gk = ground[u].unwrap_or(g);
                        let want = if c > 0.0 { target - du * DECK_GRADE - gk } else { target + du * DECK_GRADE - gk };
                        let nc = &mut node_clear[u];
                        if c > 0.0 {
                            *nc = nc.max(want);
                        } else {
                            *nc = nc.min(want);
                        }
                    }
                    for &(v, w) in &decks.edges[u] {
                        let v = v as usize;
                        let dv = du + w;
                        if dist.get(&v).is_none_or(|&x| dv < x) {
                            dist.insert(v, dv);
                            heap.push((OrdF(-dv), v));
                        }
                    }
                }
                for &b in &boundary_ids[net] {
                    let gb = ground[b].unwrap_or(g);
                    let d = decks.p[b].distance(cross_at);
                    let r = &mut raise[b];
                    if c > 0.0 {
                        let short = target - (gb + d * DECK_GRADE);
                        if short > 0.4 {
                            *r = r.max(short);
                        }
                    } else {
                        let short = target - (gb - d * DECK_GRADE);
                        if short < -0.4 {
                            *r = r.min(short);
                        }
                    }
                }
            }
        }
        for i in 0..decks.p.len() {
            decks.clear[i] = node_clear[i];
            if decks.boundary[i] && raise[i] != 0.0 {
                decks.off[i] = raise[i];
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

    // Rails: nodes where a road crosses at grade, with that road's half width.
    let crossing_half = |nid: i64| -> Option<f32> {
        nodes.get(&nid).and_then(|n| {
            n.arms.iter().filter(|a| ways[a.way as usize].class.vehicle()).map(|a| ways[a.way as usize].prof.half()).reduce(f32::max)
        })
    };
    // Distance along each bridge network from its first end, for towers and cables.
    let deck_dist = chain_distances(&decks);
    let mut net_len = vec![0.0f32; decks.nets as usize];
    for (i, &d) in deck_dist.iter().enumerate() {
        let n = &mut net_len[decks.net[i] as usize];
        *n = n.max(d);
    }

    // Street lights: mapped ones where the area has them, otherwise inferred along
    // urban (or lit) streets.
    let lamps_mapped = osm.nodes.iter().filter(|n| n.tags.is("highway", "street_lamp")).count() >= 8;
    // Mapped lamps by 40 m cell, so streets lined with them get pools of light too.
    let mut lamp_cells: HashMap<(i32, i32), u32> = HashMap::new();
    for n in osm.nodes.iter().filter(|n| n.tags.is("highway", "street_lamp")) {
        let p = local.xz(n.lat, n.lon);
        *lamp_cells.entry(((p.x / 40.0).floor() as i32, (p.y / 40.0).floor() as i32)).or_default() += 1;
    }
    let lamps_along = |pts: &[Vec2]| -> u32 {
        let mut seen: HashSet<(i32, i32)> = HashSet::new();
        for q in pts.iter().step_by(4) {
            seen.insert(((q.x / 40.0).floor() as i32, (q.y / 40.0).floor() as i32));
        }
        seen.iter().map(|c| lamp_cells.get(c).copied().unwrap_or(0)).sum()
    };
    let infer_lamps = |w: &RWay, p0: Vec2| {
        !lamps_mapped && w.class.vehicle() && w.class <= Class::Residential && !w.tunnel && (is_urban(p0) || w.tags.is("lit", "yes"))
    };
    // One bridge, one look: a carriageway mapped as two ways (or split into pieces)
    // shares the network's type and paint, and each piece only builds the parts on
    // its outer side.
    let mut net_ways: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut net_style: HashMap<u32, (BridgeKind, Option<[f32; 3]>)> = HashMap::new();
    let rank = |k: BridgeKind| match k {
        BridgeKind::Beam => 0,
        BridgeKind::Truss => 1,
        BridgeKind::Arch => 2,
        BridgeKind::CableStayed => 3,
        BridgeKind::Suspension => 4,
    };
    for (wi, w) in ways.iter().enumerate().filter(|(_, w)| w.bridge) {
        let Some(net) = w.nodes.iter().find_map(|n| deck_node.get(n)).map(|&d| decks.net[d as usize]) else { continue };
        net_ways.entry(net).or_default().push(wi);
        let k = bridge_kind(w, net_len.get(net as usize).copied().unwrap_or(0.0));
        let e = net_style.entry(net).or_insert((k, None));
        if rank(k) > rank(e.0) {
            e.0 = k;
        }
        if e.1.is_none() {
            e.1 = bridge_paint(w);
        }
    }
    // Distance to another piece of the same bridge on one side of a way (if within 45 m).
    let inner_gap = |wi: usize, net: u32, m: Vec2, dir: Vec2, side: f32| -> Option<f32> {
        let mut best: Option<f32> = None;
        for &o in net_ways.get(&net).into_iter().flatten() {
            if o == wi {
                continue;
            }
            for q in ways[o].pts.windows(2) {
                let ab = q[1] - q[0];
                let t = ((m - q[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let c = q[0] + ab * t;
                let d = m.distance(c);
                // Running alongside (roughly parallel), on this side.
                if d < 45.0 && d > 1.0 && ab.normalize_or(Vec2::X).dot(dir).abs() > 0.8 && right(dir).dot(c - m) * side > 0.0 {
                    best = Some(best.map_or(d, |b| b.min(d)));
                }
            }
        }
        best
    };

    for s in &segs {
        let w = &ways[s.way as usize];
        let full = &w.pts[s.i0..=s.i1];
        let pts = cut(full, s.trim0, s.trim1);
        if pts.len() < 2 {
            continue;
        }
        // Motor roads get denser rows so they follow the ground closely.
        let pts = densify(
            &pts,
            if w.class.major() {
                3.0
            } else if w.class.vehicle() {
                4.0
            } else {
                STEP
            },
        );
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
        let along: Vec<f32> = {
            let mut acc = 0.0;
            let mut v = vec![0.0];
            for p in pts.windows(2) {
                acc += p[0].distance(p[1]);
                v.push(acc);
            }
            v
        };
        let rows = pts.len();
        let urban_here = is_urban(pts[rows / 2]);
        // Cities don't build big earth embankments: the terrain carries at most a metre
        // of fill there, and the rest of a raised road stands on retaining walls.
        let fill_cap = if urban_here { 1.0 } else { 8.0 };
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
                        _ => H::GroundAt { at: pts[(d / len.max(1e-3) * (pts.len() - 1) as f32) as usize], off: 0.0 },
                    }
                })
                .collect()
        } else {
            along
                .iter()
                .enumerate()
                .map(|(r, &d)| {
                    let o = off_at(d);
                    if o > fill_cap { H::Raised { at: pts[r], off: o, fill: fill_cap } } else { H::GroundAt { at: pts[r], off: o } }
                })
                .collect()
        };
        let raised = |r: usize| matches!(hrule[r], H::Raised { off, fill, .. } if off - fill > 0.3);
        let (swl, swr) = sidewalks(w, sidewalks_mapped, urban_here);
        let (curb_l, curb_r) = curbs(w, (swl, swr), sidewalks_mapped, urban_here);
        let sw_w = 1.8;
        let side_extra = |sw: bool, curb: bool| -> f32 {
            if sw {
                sw_w
            } else if curb {
                0.6
            } else {
                0.0
            }
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
                            ga(a) + (ga(b) - ga(a)) * t
                        }
                        _ => 0.0,
                    })
                    .collect()
            });
            corridors.push(Corridor {
                off: hrule
                    .iter()
                    .map(|h| match *h {
                        H::Ground { off } | H::GroundAt { off, .. } => off,
                        H::Raised { fill, .. } => fill,
                        _ => 0.0,
                    })
                    .collect(),
                pts: ll,
                half: w.prof.half() + side_extra(swl, curb_l).max(side_extra(swr, curb_r)) + if w.class.vehicle() { 0.5 } else { 0.2 },
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
        // Links inside merged intersections are part of the intersection surface.
        if s.internal {
            continue;
        }

        // Vertex attributes for the road shader.
        let hw0 = w.prof.half();
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
        let seed = (hash64(w.id as u64) & 0xff) as u32;
        let lanes_pk = w.prof.fwd.min(15) as u32
            + w.prof.bwd.min(15) as u32 * 16
            + ((w.prof.shoulder_l * 10.0).round() as u32).min(63) * 256
            + ((w.prof.shoulder_r * 10.0).round() as u32).min(63) * 16384;
        let turns = (w.turn_f & 0x1ff) | (w.turn_b & 0x1ff) << 9;
        let glow = if infer_lamps(w, pts[0]) || lamps_mapped && w.class.vehicle() && !w.tunnel && lamps_along(&pts) >= 2 {
            lamp_layout(w).3
        } else {
            0
        };

        // Width per row: where a road joins one of a different width (a lane added or
        // dropped), both meet at the average width and taper to their own.
        let other_half = |nid: i64| -> Option<f32> {
            nodes
                .get(&nid)
                .and_then(|n| n.arms.iter().find(|a| a.way != s.way && ways[a.way as usize].class.vehicle() == w.class.vehicle()))
                .map(|a| ways[a.way as usize].prof.half())
        };
        let h_start = (s.end0 == EndKind::Join).then(|| other_half(w.nodes[s.i0])).flatten();
        let h_end = (s.end1 == EndKind::Join).then(|| other_half(w.nodes[s.i1])).flatten();
        let smooth = |e0: f32, e1: f32, x: f32| {
            let t = ((x - e0) / (e1 - e0).max(1e-3)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let hwr: Vec<f32> = along
            .iter()
            .map(|&d| {
                let mut h = hw0;
                if let Some(o) = h_start {
                    let l = ((o - hw0).abs() * 25.0).clamp(12.0, 60.0).min(len * 0.5);
                    h += ((o + hw0) * 0.5 - hw0) * (1.0 - smooth(0.0, l, d));
                }
                if let Some(o) = h_end {
                    let l = ((o - hw0).abs() * 25.0).clamp(12.0, 60.0).min(len * 0.5);
                    h += ((o + hw0) * 0.5 - hw0) * (1.0 - smooth(0.0, l, len - d));
                }
                h
            })
            .collect();
        let hwr: &[f32] = &hwr;
        // Rails sink flush with the road where a road crosses them.
        let flush: Vec<bool> = if w.class.rail() {
            let xs: Vec<(f32, f32)> = node_ids
                .iter()
                .enumerate()
                .filter_map(|(i, &nid)| crossing_half(nid).map(|h| (along_nodes[i] - s.trim0, h * 1.6 + 1.8)))
                .collect();
            along.iter().map(|&d| xs.iter().any(|&(x, r)| (d - x).abs() < r)).collect()
        } else {
            vec![false; rows]
        };
        // Bridge parapets open where another deck merges in at a shallow angle (ramps).
        let mut gap = [vec![false; rows], vec![false; rows]];
        if w.bridge {
            for (end_node, at_start) in [(w.nodes[s.i0], true), (w.nodes[s.i1], false)] {
                let Some(n) = nodes.get(&end_node) else { continue };
                let own =
                    if at_start { (pts[1] - pts[0]).normalize_or(Vec2::X) } else { (pts[rows - 2] - pts[rows - 1]).normalize_or(Vec2::X) };
                for a in &n.arms {
                    if a.way == s.way || !ways[a.way as usize].class.vehicle() {
                        continue;
                    }
                    let o = &ways[a.way as usize];
                    let j = (a.idx as i64 + a.dir as i64) as usize;
                    let od = (o.pts[j] - o.pts[a.idx as usize]).normalize_or(Vec2::X);
                    if own.dot(od) < 0.8 {
                        continue;
                    }
                    // Which side of travel the other way lies on.
                    let travel = if at_start { own } else { -own };
                    let side = if right(travel).dot(od) > 0.0 { 1 } else { 0 };
                    for r in 0..rows {
                        let dist = if at_start { along[r] } else { len - along[r] };
                        if dist < 45.0 {
                            gap[side][r] = true;
                        }
                    }
                }
            }
        }

        let nrm = normals(&pts, if s.trim0 > 0.0 { None } else { s.n0 }, if s.trim1 > 0.0 { None } else { s.n1 });
        let lift = w.class.lift();
        let row_pt = |r: usize, so: f32| -> Vec2 {
            let (n, m) = nrm[r];
            pts[r] + n * so * m
        };
        // Height for something at the road edge (sidewalks, curbs): level with the road.
        let edge_h = |r: usize, side: f32| -> H {
            match hrule[r] {
                H::Ground { off } => H::GroundAt { at: pts[r], off },
                h => h,
            }
        };
        let my_net = hrule.iter().find_map(|h| if let H::Deck { a, .. } = h { Some(decks.net[*a as usize]) } else { None });
        let (kind, paint) = match my_net.and_then(|n| net_style.get(&n)) {
            Some(&(k, p)) => (k, p),
            None => (bridge_kind(w, len), bridge_paint(w)),
        };
        // Which sides face out (another piece of the same bridge isn't alongside).
        let gap_on = |side: f32| -> Option<f32> {
            let (m, d) = (pts[rows / 2], (pts[rows - 1] - pts[0]).normalize_or(Vec2::X));
            my_net.and_then(|n| inner_gap(s.way as usize, n, m, d, side))
        };
        let gaps = [gap_on(-1.0), gap_on(1.0)];
        let outer = |side: f32| gaps[(side > 0.0) as usize].is_none();
        let u_of = |r: usize| -> f32 {
            match hrule[r] {
                H::Deck { a, b, t } => deck_dist[a as usize] + (deck_dist[b as usize] - deck_dist[a as usize]) * t,
                _ => along[r],
            }
        };
        let net_l = match hrule.iter().find_map(|h| if let H::Deck { a, .. } = h { Some(*a) } else { None }) {
            Some(a) => net_len[decks.net[a as usize] as usize].max(1.0),
            None => len,
        };
        let edge_out = |r: usize, side: f32| -> f32 { hwr[r] + if side < 0.0 { side_extra(swl, curb_l) } else { side_extra(swr, curb_r) } };

        // Split the ribbon into chunks by segment midpoint so each chunk can drape alone.
        let mut ranges: Vec<(TileId, usize, usize)> = Vec::new();
        for k in 0..rows - 1 {
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
            // One strip: `cols` vertices across per row, rows k0..=k1.
            let emit = |g: &mut Geom,
                        cols: usize,
                        el: f32,
                        fl: u32,
                        color: Option<[f32; 3]>,
                        vert: &dyn Fn(usize, usize) -> (Vec2, H, f32, f32)| {
                let lit = el == elem::ROAD || el == elem::SIDEWALK || el == elem::CURB;
                let pk = pack_g(el, seed, if lit { glow } else { 0 }, fl);
                let tint = color.map(paint_code);
                let start = g.v.len() as u32;
                for r in k0..=k1 {
                    for c in 0..cols {
                        let (p, h, dy, su) = vert(r, c);
                        g.v.push(V {
                            p,
                            h,
                            dy,
                            uv: [su, along[r]],
                            uv2: [tint.unwrap_or(len), pk.0],
                            col: [hwr[r], lanes_pk as f32, pk.1, turns as f32],
                        });
                    }
                }
                let n = cols as u32;
                for r in 0..(k1 - k0) as u32 {
                    for c in 0..n - 1 {
                        let a = start + r * n + c;
                        g.quad(a, a + 1, a + n, a + n + 1);
                    }
                }
            };
            // A flat strip between two lateral offsets (functions of the row).
            let flat = |g: &mut Geom,
                        cols: usize,
                        el: f32,
                        fl: u32,
                        color: Option<[f32; 3]>,
                        s0: &dyn Fn(usize) -> f32,
                        s1: &dyn Fn(usize) -> f32,
                        dy: &dyn Fn(usize) -> f32,
                        h: &dyn Fn(usize, f32) -> H| {
                emit(g, cols, el, fl, color, &|r, c| {
                    let so = s0(r) + (s1(r) - s0(r)) * c as f32 / (cols - 1) as f32;
                    (row_pt(r, so), h(r, so), dy(r), so)
                })
            };
            // A vertical face at a lateral offset, from dy0 to dy1 (both per row).
            let wall = |g: &mut Geom,
                        el: f32,
                        fl: u32,
                        color: Option<[f32; 3]>,
                        so: &dyn Fn(usize) -> f32,
                        dy0: &dyn Fn(usize) -> f32,
                        dy1: &dyn Fn(usize) -> f32,
                        h0: &dyn Fn(usize) -> H,
                        h1: &dyn Fn(usize) -> H| {
                emit(g, 2, el, fl, color, &|r, c| {
                    let p = row_pt(r, so(r));
                    if c == 0 { (p, h0(r), dy0(r), 0.0) } else { (p, h1(r), dy1(r), 1.0) }
                })
            };
            let own = |r: usize, _so: f32| hrule[r];
            let road_cols = if w.class.vehicle() { ((2.0 * hw0 / 4.0).ceil() as usize + 1).clamp(2, 6) } else { 2 };
            if w.class.rail() {
                // Ballast bed, then two rails on top (flush with the road at crossings).
                flat(g, 2, elem::RAIL_BED, f, None, &|r| -hwr[r], &|r| hwr[r], &|r| if flush[r] { 0.05 } else { lift }, &own);
                // Level crossings: concrete panels flush with the road, rails set into them.
                if (k0..=k1).any(|r| flush[r]) {
                    let pad = flag::RAIL | (Surface::Concrete as u32) << flag::SURFACE_SHIFT;
                    let pd = |r: usize| if flush[r] { 0.145 } else { -0.3 };
                    flat(g, 2, elem::ROAD, pad, None, &|_| -1.6, &|_| 1.6, &pd, &own);
                }
                let gauge = 0.7175;
                for side in [-1.0f32, 1.0] {
                    let c = side * gauge;
                    let top = |r: usize| if flush[r] { 0.152 } else { lift + 0.32 };
                    let foot = |r: usize| if flush[r] { 0.14 } else { lift + 0.18 };
                    flat(g, 2, elem::RAIL, f, None, &|_| c - 0.035, &|_| c + 0.035, &top, &own);
                    wall(g, elem::RAIL, f, None, &|_| c - 0.035, &foot, &top, &|r| hrule[r], &|r| hrule[r]);
                    wall(g, elem::RAIL, f, None, &|_| c + 0.035, &top, &foot, &|r| hrule[r], &|r| hrule[r]);
                }
            } else {
                flat(g, road_cols, elem::ROAD, f, None, &|r| -hwr[r], &|r| hwr[r], &|_| lift, &own);
            }
            // Sidewalks (level with the road edge) and curbs.
            for (sw, curb, side) in [(swl, curb_l, -1.0f32), (swr, curb_r, 1.0)] {
                if w.elevated() && !w.bridge || !(sw || curb) {
                    continue;
                }
                let kerb = 0.15;
                let width = if sw { sw_w } else { 0.6 };
                let fl = flag::SIDEWALK | (f & flag::DECK);
                let inner = move |r: usize| side * hwr[r];
                let outer = move |r: usize| side * (hwr[r] + width);
                let hs = |r: usize, _so: f32| edge_h(r, side);
                if side > 0.0 {
                    flat(g, 2, elem::SIDEWALK, fl, None, &inner, &outer, &|_| lift + kerb, &hs);
                    wall(g, elem::CURB, flag::SIDEWALK, None, &inner, &|_| lift - 0.02, &|_| lift + kerb, &|r| edge_h(r, side), &|r| {
                        edge_h(r, side)
                    });
                } else {
                    flat(g, 2, elem::SIDEWALK, fl, None, &outer, &inner, &|_| lift + kerb, &hs);
                    wall(g, elem::CURB, flag::SIDEWALK, None, &inner, &|_| lift + kerb, &|_| lift - 0.02, &|r| edge_h(r, side), &|r| {
                        edge_h(r, side)
                    });
                }
            }
            // Retaining walls under raised city roads (the terrain only carries a metre of fill).
            if (k0..=k1).any(raised) {
                let st = flag::STRUCTURE;
                for side in [-1.0f32, 1.0] {
                    let so = move |r: usize| side * (edge_out(r, side) + 0.05);
                    let top = |r: usize| if raised(r) { lift + 0.6 } else { -0.4 };
                    if side > 0.0 {
                        wall(g, elem::STRUCTURE, st, None, &so, &|_| -0.5, &top, &|_| H::Terrain, &|r| hrule[r]);
                    } else {
                        wall(g, elem::STRUCTURE, st, None, &so, &top, &|_| -0.5, &|r| hrule[r], &|_| H::Terrain);
                    }
                }
            }
            // Bridge deck sides, underside, girders and parapets.
            if w.bridge {
                let e0 = |r: usize| -edge_out(r, -1.0);
                let e1 = |r: usize| edge_out(r, 1.0);
                let thick = if w.class.vehicle() { 1.4 } else { 0.5 };
                let st = flag::STRUCTURE;
                let deck_paint = if matches!(kind, BridgeKind::Truss) { paint } else { None };
                flat(g, 2, elem::STRUCTURE, st, deck_paint, &e1, &e0, &|_| lift - thick, &own);
                wall(g, elem::STRUCTURE, st, deck_paint, &e1, &|_| lift - thick, &|_| lift + 0.2, &|r| hrule[r], &|r| hrule[r]);
                wall(g, elem::STRUCTURE, st, deck_paint, &e0, &|_| lift + 0.2, &|_| lift - thick, &|r| hrule[r], &|r| hrule[r]);
                // Girders under the deck of beam bridges.
                if matches!(kind, BridgeKind::Beam) && w.class.vehicle() {
                    let n = ((2.0 * hw0) / 3.2).ceil().max(2.0) as usize;
                    for k in 0..n {
                        let x = move |r: usize| -hwr[r] * 0.85 + 2.0 * hwr[r] * 0.85 * k as f32 / (n - 1) as f32;
                        let bot = lift - thick - 1.3;
                        wall(g, elem::STRUCTURE, st, None, &move |r| x(r) - 0.3, &|_| lift - thick, &|_| bot, &|r| hrule[r], &|r| hrule[r]);
                        wall(g, elem::STRUCTURE, st, None, &move |r| x(r) + 0.3, &|_| bot, &|_| lift - thick, &|r| hrule[r], &|r| hrule[r]);
                        flat(g, 2, elem::STRUCTURE, st, None, &move |r| x(r) + 0.3, &move |r| x(r) - 0.3, &|_| bot, &own);
                    }
                }
                // Parapets: concrete barriers for roads, lower walls for paths; open where a ramp merges.
                let ph = if w.class.vehicle() { 0.95 } else { 1.1 };
                let pw = if w.class.vehicle() { 0.35 } else { 0.15 };
                for side in [0usize, 1] {
                    let base = move |r: usize| if side == 0 { -edge_out(r, -1.0) } else { edge_out(r, 1.0) };
                    let inner = move |r: usize| if side == 0 { base(r) + pw } else { base(r) - pw };
                    let top = |r: usize| if gap[side][r] { lift } else { lift + ph };
                    let (a, b) = if side == 0 {
                        (&base as &dyn Fn(usize) -> f32, &inner as &dyn Fn(usize) -> f32)
                    } else {
                        (&inner as &dyn Fn(usize) -> f32, &base as &dyn Fn(usize) -> f32)
                    };
                    flat(g, 2, elem::STRUCTURE, st, deck_paint, a, b, &top, &own);
                    wall(g, elem::STRUCTURE, st, deck_paint, b, &|_| lift, &top, &|r| hrule[r], &|r| hrule[r]);
                    wall(g, elem::STRUCTURE, st, deck_paint, a, &top, &|_| lift, &|r| hrule[r], &|r| hrule[r]);
                }
                // Superstructure that follows the deck: truss chords, arch ribs, main cables.
                let st_col = paint.or(Some(match kind {
                    BridgeKind::Truss => [0.30, 0.34, 0.32],
                    BridgeKind::Suspension => [0.52, 0.54, 0.55],
                    _ => [0.62, 0.62, 0.6],
                }));
                for side in [-1.0f32, 1.0] {
                    if !outer(side) {
                        continue;
                    }
                    let x = move |r: usize| side * (edge_out(r, side) + 0.1);
                    match kind {
                        BridgeKind::Truss => {
                            let hgt = truss_height(hw0);
                            wall(g, elem::STRUCTURE, st, st_col, &x, &|_| lift + hgt - 0.5, &|_| lift + hgt, &|r| hrule[r], &|r| hrule[r]);
                            flat(g, 2, elem::STRUCTURE, st, st_col, &move |r| x(r) - 0.25, &move |r| x(r) + 0.25, &|_| lift + hgt, &own);
                        }
                        BridgeKind::Arch => {
                            let rise = (net_l * 0.18).clamp(8.0, 45.0);
                            let arch = move |r: usize| {
                                let t = (u_of(r) / net_l).clamp(0.0, 1.0);
                                lift + rise * 4.0 * t * (1.0 - t)
                            };
                            wall(g, elem::STRUCTURE, st, st_col, &x, &move |r| arch(r) - 1.4, &arch, &|r| hrule[r], &|r| hrule[r]);
                            flat(g, 2, elem::STRUCTURE, st, st_col, &move |r| x(r) - 0.4, &move |r| x(r) + 0.4, &arch, &own);
                        }
                        BridgeKind::Suspension => {
                            let c = move |r: usize| cable_dy(u_of(r), net_l, hw0) + lift;
                            wall(
                                g,
                                elem::STRUCTURE,
                                st,
                                st_col,
                                &move |r| x(r) + side * 0.6,
                                &move |r| c(r) - 0.5,
                                &c,
                                &|r| hrule[r],
                                &|r| hrule[r],
                            );
                        }
                        _ => {}
                    }
                }
            }
            // Tunnel walls and ceiling.
            if w.tunnel && !w.class.path() {
                let wallx = move |r: usize| hwr[r] + 1.0;
                let ceil = if w.class.rail() { 6.5 } else { 5.6 };
                let fl2 = flag::STRUCTURE | flag::TUNNEL_LIGHT;
                let st = flag::STRUCTURE;
                wall(g, elem::TUNNEL, fl2, None, &wallx, &|_| lift + ceil, &|_| lift - 0.1, &|r| hrule[r], &|r| hrule[r]);
                wall(g, elem::TUNNEL, fl2, None, &move |r| -wallx(r), &|_| lift - 0.1, &|_| lift + ceil, &|r| hrule[r], &|r| hrule[r]);
                flat(g, 2, elem::TUNNEL, fl2, None, &wallx, &move |r| -wallx(r), &|_| lift + ceil, &own);
                // Outer shell (seen from above where the terrain is cut away near portals).
                flat(g, 2, elem::STRUCTURE, st, None, &move |r| -wallx(r) - 0.5, &move |r| wallx(r) + 0.5, &|_| lift + ceil + 0.6, &own);
                wall(
                    g,
                    elem::STRUCTURE,
                    st,
                    None,
                    &move |r| wallx(r) + 0.5,
                    &|_| lift - 0.3,
                    &|_| lift + ceil + 0.6,
                    &|r| hrule[r],
                    &|r| hrule[r],
                );
                wall(
                    g,
                    elem::STRUCTURE,
                    st,
                    None,
                    &move |r| -wallx(r) - 0.5,
                    &|_| lift + ceil + 0.6,
                    &|_| lift - 0.3,
                    &|r| hrule[r],
                    &|r| hrule[r],
                );
                // Portal headwalls where the tunnel meets open ground.
                for (end, row) in [(s.i0, 0usize), (s.i1, rows - 1)] {
                    if !(k0..=k1).contains(&row) {
                        continue;
                    }
                    let portal = nodes.get(&w.nodes[end]).is_some_and(|n| n.arms.iter().any(|a| !ways[a.way as usize].tunnel));
                    if !portal {
                        continue;
                    }
                    let p = pts[row];
                    let (nr, _) = nrm[row];
                    let dir =
                        if row == 0 { (pts[1] - pts[0]).normalize_or(Vec2::X) } else { (pts[row] - pts[row - 1]).normalize_or(Vec2::X) };
                    let out = if row == 0 { -dir } else { dir };
                    let h = hrule[row];
                    let pk = pack(elem::STRUCTURE, 0, flag::STRUCTURE);
                    let mk = |q: Vec2, dy: f32| V { p: q, h, dy, uv: [q.x, dy], uv2: [0.0, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
                    let wl = wallx(row);
                    let outer = wl + 5.0;
                    let top = lift + ceil + 5.0;
                    for (s0, s1, y0, y1) in [(-outer, -wl, lift - 0.5, top), (wl, outer, lift - 0.5, top), (-wl, wl, lift + ceil, top)] {
                        let q0 = p + nr * s0 + out * 0.3;
                        let q1 = p + nr * s1 + out * 0.3;
                        let a = g.push(mk(q0, y1));
                        let bb = g.push(mk(q1, y1));
                        let c0 = g.push(mk(q0, y0));
                        let d = g.push(mk(q1, y0));
                        g.quad(a, bb, c0, d);
                        g.quad(bb, a, d, c0);
                    }
                }
            }
        }

        // Bridge supports and members placed at distances along the deck.
        if w.bridge && w.class != Class::Footway {
            let at_d = |d: f32| -> (Vec2, Vec2, H, f32) {
                let k = along.partition_point(|&x| x <= d).clamp(1, rows - 1);
                let t = ((d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-3)).clamp(0.0, 1.0);
                let p = pts[k - 1].lerp(pts[k], t);
                let dir = (pts[k] - pts[k - 1]).normalize_or(Vec2::X);
                let h = match (hrule[k - 1], hrule[k]) {
                    (H::Deck { a, b, t: t0 }, H::Deck { t: t1, .. }) => H::Deck { a, b, t: t0 + (t1 - t0) * t },
                    (h, _) => h,
                };
                let hw = hwr[k - 1] + (hwr[k] - hwr[k - 1]) * t;
                (p, dir, h, hw)
            };
            let u_at = |d: f32| -> f32 {
                let k = along.partition_point(|&x| x <= d).clamp(1, rows - 1);
                let t = ((d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-3)).clamp(0.0, 1.0);
                u_of(k - 1) + (u_of(k) - u_of(k - 1)) * t
            };
            let side_w = |side: f32| if side < 0.0 { side_extra(swl, curb_l) } else { side_extra(swr, curb_r) };
            let st_col = paint.or(Some(match kind {
                BridgeKind::Truss => [0.30, 0.34, 0.32],
                BridgeKind::Suspension => [0.52, 0.54, 0.55],
                _ => [0.62, 0.62, 0.6],
            }));
            let mut put = |p: Vec2, f: &mut dyn FnMut(&mut Geom)| {
                if in_tile(p) {
                    let cid = chunk_of(p);
                    f(&mut chunks.entry(cid).or_insert_with(|| new_chunk(cid)).geom);
                }
            };
            let thick = if w.class.vehicle() { 1.4 } else { 0.5 };
            // Piers: bents (columns under a cap beam) for road bridges, single columns for paths.
            let tower_us: Vec<f32> = match kind {
                BridgeKind::Suspension if net_l > 300.0 => vec![net_l * 0.27, net_l * 0.73],
                BridgeKind::Suspension => vec![net_l * 0.5],
                BridgeKind::CableStayed if net_l > 700.0 => vec![net_l * 0.3, net_l * 0.7],
                BridgeKind::CableStayed => vec![net_l * 0.5],
                _ => Vec::new(),
            };
            let long_span = matches!(kind, BridgeKind::Suspension | BridgeKind::CableStayed | BridgeKind::Arch);
            let spacing = if w.class.vehicle() { 32.0 } else { 20.0 };
            let mut d = spacing * 0.5 + hash01(w.id as u64) * 6.0;
            while d < len - 8.0 {
                let u = u_at(d);
                // Main spans of long-span bridges have no piers.
                let in_main_span = long_span && (u > net_l * 0.12 && u < net_l * 0.88);
                if !in_main_span {
                    let (p, dir, h, hw) = at_d(d);
                    put(p, &mut |g| {
                        if w.class.vehicle() {
                            let half = hw + side_w(1.0).max(side_w(-1.0));
                            prism(g, p, right(dir), half, 0.75, (h, lift - thick - 1.3), (h, lift - thick), None);
                            let n = ((2.0 * half) / 9.0).floor().max(1.0) as usize + 1;
                            for k in 0..n {
                                let x = -half * 0.8 + 1.6 * half * k as f32 / (n - 1).max(1) as f32;
                                let x = if n == 1 { 0.0 } else { x };
                                prism(g, p + right(dir) * x, dir, 0.55, 0.55, (H::Terrain, -1.0), (h, lift - thick - 1.3), None);
                            }
                        } else {
                            prism(g, p, dir, 0.3, 0.3, (H::Terrain, -1.0), (h, lift - thick), None);
                        }
                    });
                }
                d += spacing;
            }
            // Towers (suspension and cable-stayed): two legs, cross beams.
            for &tu in &tower_us {
                // Find where along this segment the tower falls.
                let Some(dk) = (0..rows).find(|&r| u_of(r) >= tu).filter(|&r| r > 0) else { continue };
                let t = ((tu - u_of(dk - 1)) / (u_of(dk) - u_of(dk - 1)).max(1e-3)).clamp(0.0, 1.0);
                let d = along[dk - 1] + (along[dk] - along[dk - 1]) * t;
                let (p, dir, h, hw) = at_d(d);
                let span = if tower_us.len() > 1 { (tower_us[1] - tower_us[0]).abs() } else { net_l * 0.5 };
                let ht = match kind {
                    BridgeKind::Suspension => (span * 0.12).clamp(25.0, 160.0),
                    _ => (span * 0.22).clamp(30.0, 120.0),
                };
                let half = hw + side_w(1.0).max(side_w(-1.0)) + 1.6;
                let col = st_col;
                // Beams reach the outer legs, or halfway to the other carriageway.
                let reach = |side: f32| gaps[(side > 0.0) as usize].map(|g| g * 0.5).unwrap_or(half);
                let (a, bb) = (reach(-1.0), reach(1.0));
                put(p, &mut |g| {
                    for side in [-1.0f32, 1.0] {
                        if outer(side) {
                            prism(g, p + right(dir) * side * half, dir, 1.6, 1.6, (H::Terrain, -1.0), (h, lift + ht), col);
                        }
                    }
                    let mid = p + right(dir) * (bb - a) * 0.5;
                    for y in [lift - 3.0, lift + ht * 0.55, lift + ht - 3.0] {
                        prism(g, mid, right(dir), (a + bb) * 0.5, 1.2, (h, y - 1.5), (h, y), col);
                    }
                });
            }
            // Members at intervals: truss verticals and diagonals, arch hangers, suspenders, stays.
            let step = match kind {
                BridgeKind::Truss => 6.0,
                BridgeKind::Arch => 8.0,
                BridgeKind::Suspension => 15.0,
                BridgeKind::CableStayed => 10.0,
                BridgeKind::Beam => 0.0,
            };
            if step > 0.0 {
                let mut d = step * 0.5;
                let mut i = 0;
                while d < len - 0.5 {
                    let (p, dir, h, hw) = at_d(d);
                    let u = u_at(d);
                    for side in [-1.0f32, 1.0] {
                        if !outer(side) {
                            continue;
                        }
                        let x = side * (hw + side_w(side) + 0.1);
                        let q = p + right(dir) * x;
                        match kind {
                            BridgeKind::Truss => {
                                let hgt = truss_height(hw0);
                                put(q, &mut |g| prism(g, q, dir, 0.18, 0.18, (h, lift), (h, lift + hgt), st_col));
                                // Diagonal to the next panel point, alternating direction.
                                if d + step < len {
                                    let (p2, dir2, h2, hw2) = at_d(d + step);
                                    let q2 = p2 + right(dir2) * side * (hw2 + side_w(side) + 0.1);
                                    let (lo, hi) = if i % 2 == 0 {
                                        ((q, h, lift), (q2, h2, lift + hgt))
                                    } else {
                                        ((q, h, lift + hgt), (q2, h2, lift))
                                    };
                                    put(q, &mut |g| strut(g, lo, hi, 0.3, st_col));
                                }
                            }
                            BridgeKind::Arch => {
                                let rise = (net_l * 0.18).clamp(8.0, 45.0);
                                let t = (u / net_l).clamp(0.0, 1.0);
                                let top = lift + rise * 4.0 * t * (1.0 - t) - 1.4;
                                if top > lift + 1.0 {
                                    put(q, &mut |g| prism(g, q, dir, 0.12, 0.12, (h, lift), (h, top), st_col));
                                }
                            }
                            BridgeKind::Suspension => {
                                let top = cable_dy(u, net_l, hw0) + lift - 0.4;
                                if top > lift + 1.0 && !tower_us.iter().any(|&tu| (tu - u).abs() < 4.0) {
                                    let qs = q + right(dir) * side * 0.6;
                                    put(q, &mut |g| prism(g, qs, dir, 0.06, 0.06, (h, lift), (h, top), st_col));
                                }
                            }
                            BridgeKind::CableStayed => {
                                // Stay from the nearest pylon top down to this deck point.
                                if let Some(&tu) = tower_us.iter().min_by(|a, b| (*a - u).abs().total_cmp(&(*b - u).abs())) {
                                    let reach = if tower_us.len() > 1 { (tower_us[1] - tower_us[0]).abs() * 0.5 } else { net_l * 0.45 };
                                    if (tu - u).abs() > 6.0 && (tu - u).abs() < reach {
                                        let span = if tower_us.len() > 1 { (tower_us[1] - tower_us[0]).abs() } else { net_l * 0.5 };
                                        let ht = (span * 0.22).clamp(30.0, 120.0);
                                        // The pylon's position: walk to it along this segment if it's here.
                                        if let Some(dk) = (1..rows).find(|&r| u_of(r) >= tu) {
                                            let t = ((tu - u_of(dk - 1)) / (u_of(dk) - u_of(dk - 1)).max(1e-3)).clamp(0.0, 1.0);
                                            let (pp, pdir, ph, phw) = at_d(along[dk - 1] + (along[dk] - along[dk - 1]) * t);
                                            let top = pp + right(pdir) * side * (phw + side_w(side) + 1.6);
                                            let hgt = ht - 4.0 * ((tu - u).abs() / reach);
                                            put(q, &mut |g| {
                                                strut(g, (q, h, lift + 0.5), (top, ph, lift + hgt), 0.15, Some([0.85, 0.85, 0.84]))
                                            });
                                        }
                                    }
                                }
                            }
                            BridgeKind::Beam => {}
                        }
                    }
                    d += step;
                    i += 1;
                }
            }
        }
    }

    // Merged intersections: one paved surface over the member junctions and the short
    // links between them (the convex hull), a little under the member surfaces.
    for (&root, members) in cluster_members.iter().filter(|(_, m)| m.len() > 1) {
        let mut cloud: Vec<Vec2> = Vec::new();
        let mut lift = 0.0f32;
        let mut off = 0.0f32;
        for &k in members {
            let j = &junctions[k];
            cloud.push(j.p);
            cloud.extend(j.poly.iter().map(|&q| j.p + q));
            off += node_off.get(&j.node).copied().unwrap_or(0.0);
            for a in &j.arms {
                let w = &ways[segs[a.0].way as usize];
                lift = lift.max(w.class.lift());
                if segs[a.0].internal {
                    let sg = &segs[a.0];
                    for q in &w.pts[sg.i0..=sg.i1] {
                        cloud.push(*q + a.2.perp() * a.3);
                        cloud.push(*q - a.2.perp() * a.3);
                    }
                }
            }
        }
        off /= members.len() as f32;
        let hull = convex_hull(&cloud);
        if hull.len() < 3 {
            continue;
        }
        let c = hull.iter().copied().sum::<Vec2>() / hull.len() as f32;
        let radius = hull.iter().map(|q| q.distance(c)).fold(0.0f32, f32::max);
        if radius > 90.0 {
            continue;
        }
        // Level the ground under all of it.
        let ll = [local.ll(c - Vec2::X * 0.5), local.ll(c + Vec2::X * 0.5)];
        let pad = (radius + 40.0) as f64 / 111_000.0;
        let (la, lo) = local.ll(c);
        corridors.push(Corridor {
            pts: ll.to_vec(),
            off: vec![off, off],
            half: radius + 1.0,
            tunnel: None,
            bbox: (la - pad, lo - pad * 1.5, la + pad, lo + pad * 1.5),
        });
        if !in_tile(c) {
            continue;
        }
        let j0 = &junctions[root];
        let w0 = &ways[segs[j0.arms[0].0].way as usize];
        let fl = flag::JUNCTION | (w0.surface as u32) << flag::SURFACE_SHIFT | (w0.smooth.min(7) as u32) << flag::SMOOTH_SHIFT;
        let pk = pack(elem::JUNCTION, (hash64(j0.node as u64) & 0xff) as u32, fl);
        let cid = chunk_of(c);
        let g = &mut chunks.entry(cid).or_insert_with(|| new_chunk(cid)).geom;
        let mk =
            |p: Vec2| V { p, h: H::GroundAt { at: p, off }, dy: lift - 0.06, uv: [p.x, p.y], uv2: [0.0, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
        let ci = g.push(mk(c));
        let ring: Vec<u32> = hull.iter().map(|&q| g.push(mk(q))).collect();
        for k in 0..ring.len() {
            g.tri(ci, ring[(k + 1) % ring.len()], ring[k]);
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
                None => H::GroundAt { at: j.p, off: 0.0 },
            }
        } else {
            H::GroundAt { at: j.p, off: node_off.get(&j.node).copied().unwrap_or(0.0) }
        };
        // Ring points take the height of the arm they belong to, at that distance out,
        // so the surface meets each road's end exactly on side slopes.
        let h_for = |q: Vec2| -> H {
            match h {
                H::GroundAt { off, .. } => {
                    let r = q - j.p;
                    let best = j.arms.iter().max_by(|a, b| a.2.dot(r).total_cmp(&b.2.dot(r)));
                    let at = best.map(|a| j.p + a.2 * a.2.dot(r).max(0.0)).unwrap_or(j.p);
                    H::GroundAt { at, off }
                }
                other => other,
            }
        };
        let fl = (flag::JUNCTION | (surface as u32) << flag::SURFACE_SHIFT | (smooth as u32) << flag::SMOOTH_SHIFT) as f32;
        let seed = (hash64(j.node as u64) & 0xfff) as u32;
        let cid = chunk_of(j.p);
        let chunk = chunks.entry(cid).or_insert_with(|| new_chunk(cid));
        let g = &mut chunk.geom;
        let pk = pack(elem::JUNCTION, seed, fl as u32);
        let mk = |p: Vec2| V { p, h: h_for(p), dy: lift, uv: [p.x, p.y], uv2: [0.0, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
        let c = g.push(mk(j.p));
        let ring: Vec<u32> = j.poly.iter().map(|&q| g.push(mk(j.p + q))).collect();
        for k in 0..ring.len() {
            // Fan, ccw in x/z (z south) -> wind so the face points up.
            g.tri(c, ring[(k + 1) % ring.len()], ring[k]);
        }
        // Sidewalk corners: follow the curb return between neighbouring arms.
        let urban = is_urban(j.p);
        for (a, b, curve) in &j.corners {
            if segs[j.arms[*a].0].internal || segs[j.arms[*b].0].internal {
                continue;
            }
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
                let mk = |p: Vec2, dy: f32, s: f32, pk: (f32, f32)| V {
                    p,
                    h: h_for(q),
                    dy,
                    uv: [s, along],
                    uv2: [0.0, pk.0],
                    col: [0.0, 0.0, pk.1, 0.0],
                };
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
                    let ci = g.push(V {
                        p: corner,
                        h: h_for(corner),
                        dy: lift + curb,
                        uv: [width, 0.0],
                        uv2: [0.0, fl_sw.0],
                        col: [0.0, 0.0, fl_sw.1, 0.0],
                    });
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
        let off = node_off.get(&nd.id).copied().unwrap_or(0.0);
        let arm_dir = {
            let a = &n.arms[0];
            let j = (a.idx as i64 + a.dir as i64) as usize;
            (w.pts[j] - w.pts[a.idx as usize]).normalize_or(Vec2::X)
        };
        let mk = |p: Vec2| V {
            p,
            h: H::GroundAt { at: n.p + arm_dir * (p - n.p).dot(arm_dir).max(0.0), off },
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
    let next_dy = std::cell::Cell::new(0.0f32);
    let mut add = |kind: PropKind, p: Vec2, h: H, heading: f32, id: u64| {
        let dy = next_dy.replace(0.0);
        if in_tile(p) {
            props.push(Prop { kind, p, h, heading, id, dy });
        }
    };
    let ground = |n: i64| H::Ground { off: node_off.get(&n).copied().unwrap_or(0.0) };
    for j in &junctions {
        let arm_ways: Vec<&RWay> = j.arms.iter().map(|a| &ways[segs[a.0].way as usize]).collect();
        // Street name signs on one corner: the two most important distinct names.
        let mut named: Vec<(Class, &str, Vec2)> = Vec::new();
        for (a, w) in j.arms.iter().zip(&arm_ways) {
            if segs[a.0].internal {
                continue;
            }
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
        let merged = cluster_members.get(&j.cluster).is_some_and(|m| m.len() > 1);
        let ctl_id = junctions[j.cluster].node;
        let mut group_of_arm = vec![0u8; j.arms.len()];
        let groups = if j.control == Control::Signals && merged {
            group_of_arm = j.arms.iter().map(|a| group_of(j.cluster, a.2)).collect();
            2
        } else if j.control == Control::Signals {
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
        if j.control == Control::Signals && (!merged || j.cluster == junction_at[&j.node]) {
            controllers.push(Controller { id: ctl_id, p: j.p, groups, cycle_offset: hash01(ctl_id as u64) * 60.0 });
        }
        for (ai, (a, w)) in j.arms.iter().zip(&arm_ways).enumerate() {
            let arriving = if a.1 { !w.prof.oneway } else { true };
            if !arriving || w.class <= Class::Trunk && j.control != Control::Signals || segs[a.0].internal {
                continue;
            }
            let d = a.2;
            let trim = segs[a.0].trim(a.1);
            // Arriving traffic travels along -d; its right is right(-d) = -right(d).
            let rside = -right(d);
            let base = j.p + d * (trim + 1.5) + rside * (a.3 + 0.8);
            let face = heading_of(d); // faces traffic coming toward the junction
            let id = hash64(j.node as u64 ^ ((ai as u64 + 1) * 0x9e37));
            match j.control {
                Control::Signals => {
                    let mast = (a.3 * 1.2).clamp(3.0, 12.0);
                    add(PropKind::Signal { junction: ctl_id, group: group_of_arm[ai], mast }, base, ground(j.node), face, id);
                    if j.urban {
                        // On the far corner of this approach, facing people waiting to
                        // cross it from the near corner.
                        let pp = j.p + d * (trim + 0.5) - rside * (a.3 + 1.0);
                        add(
                            PropKind::PedSignal { junction: ctl_id, group: group_of_arm[ai] },
                            pp,
                            ground(j.node),
                            heading_of(rside),
                            id ^ 0x77,
                        );
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
    // Nearest road (vehicle ways), for facing mapped objects toward the street.
    let mut road_grid: HashMap<(i32, i32), Vec<(Vec2, Vec2, f32)>> = HashMap::new();
    const GRID: f32 = 30.0;
    for w in ways.iter().filter(|w| w.class.vehicle() && !w.elevated()) {
        for q in w.pts.windows(2) {
            let (lo, hi) = (q[0].min(q[1]), q[0].max(q[1]));
            for gx in (lo.x / GRID).floor() as i32..=(hi.x / GRID).floor() as i32 {
                for gz in (lo.y / GRID).floor() as i32..=(hi.y / GRID).floor() as i32 {
                    road_grid.entry((gx, gz)).or_default().push((q[0], q[1], w.prof.half()));
                }
            }
        }
    }
    // (closest point on a road, distance from its edge, road direction)
    let nearest_road = |p: Vec2| -> Option<(Vec2, f32, Vec2)> {
        let (cx, cz) = ((p.x / GRID).floor() as i32, (p.y / GRID).floor() as i32);
        let mut best: Option<(Vec2, f32, Vec2)> = None;
        for gx in cx - 1..=cx + 1 {
            for gz in cz - 1..=cz + 1 {
                for &(a, b, hw) in road_grid.get(&(gx, gz)).into_iter().flatten() {
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let c = a + ab * t;
                    let d = c.distance(p) - hw;
                    if best.is_none_or(|x| d < x.1) {
                        best = Some((c, d, ab.normalize_or(Vec2::X)));
                    }
                }
            }
        }
        best
    };
    let mut line_dir: HashMap<i64, Vec2> = HashMap::new();
    for (ids, pts) in &power_lines {
        for (k, &id) in ids.iter().enumerate() {
            let a = pts[k.saturating_sub(1)];
            let b = pts[(k + 1).min(pts.len() - 1)];
            line_dir.insert(id, (b - a).normalize_or(Vec2::X));
        }
    }
    let mut mapped_poles = 0usize;
    let mut mapped_hydrants = 0usize;
    // Mapped point features.
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
        // Face the street: toward the nearest road, or across it for things on a road node.
        let heading = match on_way {
            Some((d, _)) => heading_of(-right(d)),
            None => match nearest_road(p) {
                Some((c, dist, _)) if dist < 30.0 && c.distance(p) > 0.01 => heading_of((c - p).normalize()),
                _ => (hash01(id) - 0.5) * 6.28,
            },
        };
        let t = &nd.tags;
        mapped_poles += t.is("power", "pole") as usize;
        mapped_hydrants += t.is("emergency", "fire_hydrant") as usize;
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
            let text = t.get("exit_to").or(t.get("destination")).or(t.get("name")).unwrap_or("").to_string();
            (!exit.is_empty() || !text.is_empty()).then_some(PropKind::ExitSign { exit, text })
        } else if t.is("highway", "milestone") {
            t.get("distance").map(|d| PropKind::MileMarker { text: d.split(['.', ' ']).next().unwrap_or(d).to_string() })
        } else if t.is("railway", "level_crossing") || t.is("railway", "crossing") {
            // Crossbucks go on each road approach (below), not on the node itself.
            None
        } else {
            None
        };
        let Some(kind) = kind else { continue };
        // Things standing on a road node go beside the road instead.
        let p = match (&kind, on_way) {
            (PropKind::ExitSign { .. } | PropKind::BusStop { .. } | PropKind::Lamp { .. } | PropKind::MileMarker { .. }, Some((d, wi))) => {
                p + right(d) * (ways[wi as usize].prof.half() + 1.5)
            }
            _ => p,
        };
        // Mile markers face oncoming traffic, not the road; poles on a mapped line turn
        // their crossarms across it.
        let heading = match (&kind, on_way) {
            (PropKind::MileMarker { .. }, Some((d, _))) => heading_of(-d),
            (PropKind::UtilityPole, _) => line_dir.get(&nd.id).map(|&d| heading_of(d)).unwrap_or(heading),
            _ => heading,
        };
        add(kind, p, h, heading, id);
    }
    // Railroad crossings: a crossbuck on the right of each road approach.
    for (&nid, n) in &nodes {
        let tagged = n.tags.is_some_and(|ti| osm.nodes[ti].tags.is("railway", "level_crossing"));
        let is_xing = tagged
            && n.arms.iter().any(|a| ways[a.way as usize].class.rail())
            && n.arms.iter().any(|a| ways[a.way as usize].class.vehicle());
        if !is_xing || !in_tile(n.p) {
            continue;
        }
        let lights = n.tags.is_some_and(|ti| {
            let t = &osm.nodes[ti].tags;
            t.flag("crossing:light") || t.flag("crossing:barrier") || t.flag("crossing:bell")
        });
        let rail_half =
            n.arms.iter().filter(|a| ways[a.way as usize].class.rail()).map(|a| ways[a.way as usize].prof.half()).fold(1.5f32, f32::max);
        for (k, a) in n.arms.iter().enumerate() {
            let w = &ways[a.way as usize];
            if !w.class.vehicle() || w.elevated() {
                continue;
            }
            let j = (a.idx as i64 + a.dir as i64) as usize;
            let d = (w.pts[j] - w.pts[a.idx as usize]).normalize_or(Vec2::X);
            // Traffic arriving along -d; its right is -right(d).
            let arriving = a.dir < 0 || !w.prof.oneway;
            if !arriving && w.prof.oneway {
                continue;
            }
            let pos = n.p + d * (rail_half + 3.5) - right(d) * (w.prof.half() + 1.0);
            add(PropKind::Crossbuck { lights }, pos, ground(nid), heading_of(d), hash64(nid as u64 ^ 0xc805 ^ k as u64));
        }
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
        if infer_lamps(w, pts[0]) {
            let (spacing, first, major, _) = lamp_layout(w);
            let mut d = first;
            let mut k = 0;
            while d < len - 5.0 {
                let (p, dir) = at(d);
                // Major roads: staggered both sides; minor roads: one side.
                let side = if major && k % 2 == 1 { -1.0 } else { 1.0 };
                let pos = p + right(dir) * side * (hw + 0.9 + if major { 1.0 } else { 0.0 });
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
            // Leave gaps where other roads join (ramps, side roads).
            let joins: Vec<f32> = {
                let mut acc = -s.trim0;
                let mut v = Vec::new();
                for (k, &nid) in w.nodes[s.i0..=s.i1].iter().enumerate() {
                    if k > 0 {
                        acc += full[k].distance(full[k - 1]);
                    }
                    let other = nodes.get(&nid).is_some_and(|n| {
                        n.arms.iter().any(|a| {
                            let o = &ways[a.way as usize];
                            a.way != s.way
                                && o.class.vehicle()
                                && !o.bridge
                                && (o.link || o.tags.get("name") != w.tags.get("name") || o.class != w.class)
                        })
                    });
                    if other {
                        v.push(acc);
                    }
                }
                v
            };
            for (i, &f) in fills.iter().enumerate() {
                let d = i as f32 * 4.0;
                if joins.iter().any(|&x| (x - d - 2.0).abs() < 60.0) {
                    continue;
                }
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
                if !w.elevated() {
                    next_dy.set(w.class.lift());
                }
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

    // ---- freeways: chains of one-way motorway ways, for mile markers and guide signs
    let mut by_first: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut by_last: HashMap<i64, usize> = HashMap::new();
    let mway: Vec<usize> = (0..ways.len()).filter(|&i| ways[i].class == Class::Motorway && !ways[i].link && ways[i].prof.oneway).collect();
    for &i in &mway {
        by_first.entry(ways[i].nodes[0]).or_default().push(i);
        by_last.insert(*ways[i].nodes.last().unwrap(), i);
    }
    let mut used: HashSet<usize> = HashSet::new();
    let milestones_mapped = osm.nodes.iter().any(|n| n.tags.is("highway", "milestone"));
    let exit_num = |t: &Tags| -> Option<f32> {
        let r = t.get("ref")?;
        let digits: String = r.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<f32>().ok()
    };
    let junction_nodes: Vec<(Vec2, f32)> = osm
        .nodes
        .iter()
        .filter(|n| n.tags.is("highway", "motorway_junction"))
        .filter_map(|n| exit_num(&n.tags).map(|e| (local.xz(n.lat, n.lon), e)))
        .collect();
    for &start in &mway {
        if used.contains(&start) {
            continue;
        }
        // Walk back to the head of the chain, then forward collecting points.
        let mut head = start;
        let mut guard = 0;
        while let Some(&prev) = by_last.get(&ways[head].nodes[0]) {
            if prev == start || used.contains(&prev) || guard > 200 || ways[prev].tags.get("ref") != ways[start].tags.get("ref") {
                break;
            }
            head = prev;
            guard += 1;
        }
        let mut pts: Vec<Vec2> = Vec::new();
        let mut ids: Vec<i64> = Vec::new();
        let mut hw = 0.0f32;
        let mut cur = Some(head);
        while let Some(wi) = cur {
            if !used.insert(wi) {
                break;
            }
            let w = &ways[wi];
            hw = hw.max(w.prof.half());
            let skip = if pts.is_empty() { 0 } else { 1 };
            pts.extend_from_slice(&w.pts[skip..]);
            ids.extend_from_slice(&w.nodes[skip..]);
            cur = by_first
                .get(w.nodes.last().unwrap())
                .and_then(|v| v.iter().copied().find(|&n| !used.contains(&n) && ways[n].tags.get("ref") == w.tags.get("ref")));
        }
        if pts.len() < 2 {
            continue;
        }
        let mut along = vec![0.0f32];
        for q in pts.windows(2) {
            along.push(along.last().unwrap() + q[0].distance(q[1]));
        }
        let total = *along.last().unwrap();
        let at = |d: f32| -> (Vec2, Vec2) {
            let k = along.partition_point(|&x| x <= d).clamp(1, pts.len() - 1);
            let t = ((d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-3)).clamp(0.0, 1.0);
            (pts[k - 1].lerp(pts[k], t), (pts[k] - pts[k - 1]).normalize_or(Vec2::X))
        };
        let reference = &ways[head];
        let seed = hash64(reference.id as u64 ^ 0x3171);
        // Mile markers from exit numbers (most states number exits by milepost): fit
        // exit number against distance along the chain's direction; keep it only if
        // it runs at about one exit-mile per mile.
        if !milestones_mapped && reference.tags.get("ref").is_some() {
            let (p0, p1) = (pts[0], pts[pts.len() - 1]);
            let axis = (p1 - p0).normalize_or(Vec2::X);
            let samples: Vec<(f32, f32)> = junction_nodes
                .iter()
                .filter(|(q, _)| {
                    let (c, _) = {
                        // Closest chain point (coarse: by vertices) for the lateral check.
                        let mut best = (f32::MAX, 0usize);
                        for (k, &pp) in pts.iter().enumerate() {
                            let d = pp.distance(*q);
                            if d < best.0 {
                                best = (d, k);
                            }
                        }
                        (best.0, best.1)
                    };
                    let lateral = (*q - p0).perp_dot(axis).abs();
                    c < 400.0 || lateral < 350.0 && (*q - p0).dot(axis).abs() < 16_000.0
                })
                .map(|(q, e)| ((*q - p0).dot(axis), *e))
                .collect();
            if samples.len() >= 2 {
                let n = samples.len() as f32;
                let (sx, sy) = samples.iter().fold((0.0, 0.0), |a, s| (a.0 + s.0, a.1 + s.1));
                let (mx, my) = (sx / n, sy / n);
                let (sxx, sxy) = samples.iter().fold((0.0, 0.0), |a, s| (a.0 + (s.0 - mx).powi(2), a.1 + (s.0 - mx) * (s.1 - my)));
                let b = if sxx > 1.0 { sxy / sxx * 1609.34 } else { 0.0 };
                let resid = samples.iter().map(|s| (s.1 - (my + b * (s.0 - mx) / 1609.34)).abs()).fold(0.0f32, f32::max);
                if (0.8..1.25).contains(&b.abs()) && resid < 1.5 {
                    // Mile at chain distance d (via projection on the axis).
                    let mile_at = |d: f32| my + b * ((at(d).0 - p0).dot(axis) - mx) / 1609.34;
                    let mut d = 0.0;
                    let mut last = mile_at(0.0).floor();
                    while d < total {
                        let m = mile_at(d);
                        let fl = m.floor();
                        if fl != last && fl >= 0.0 {
                            let (p, dir) = at(d);
                            let mark = if b > 0.0 { fl } else { last };
                            add(
                                PropKind::MileMarker { text: format!("{}", mark as i32) },
                                p + right(dir) * (hw + 2.0),
                                H::Ground { off: 0.0 },
                                heading_of(-dir),
                                seed ^ mark as u64,
                            );
                        }
                        last = fl;
                        d += 10.0;
                    }
                }
            }
        }
        // Overhead guide signs about 250 m before each exit: on an overpass if one
        // crosses there, otherwise on a gantry.
        for (k, &nid) in ids.iter().enumerate() {
            let Some(ti) = node_tags.get(&nid) else { continue };
            let t = &osm.nodes[*ti].tags;
            if !t.is("highway", "motorway_junction") {
                continue;
            }
            let exit = t.get("ref").unwrap_or("").to_string();
            let link_dest = nodes.get(&nid).and_then(|n| {
                n.arms.iter().find_map(|a| {
                    let o = &ways[a.way as usize];
                    (o.link && a.dir > 0).then(|| o.tags.get("destination").or(o.tags.get("destination:ref"))).flatten()
                })
            });
            let text = t.get("exit_to").or(t.get("destination")).or(link_dest).or(t.get("name")).unwrap_or("").replace(';', " / ");
            if exit.is_empty() && text.is_empty() {
                continue;
            }
            let dj = along[k];
            if dj < 140.0 {
                continue;
            }
            // Bridges crossing the chain 120..450 m before the exit.
            let mut over: Option<(f32, Vec2, Vec2, H, f32)> = None;
            for (bi, bw) in ways.iter().enumerate() {
                if !bw.bridge || !bw.class.vehicle() || mway.contains(&bi) {
                    continue;
                }
                for (bk, bq) in bw.pts.windows(2).enumerate() {
                    for (ck, cq) in pts.windows(2).enumerate() {
                        let d0 = along[ck];
                        if d0 > dj - 100.0 || along[ck + 1] < dj - 450.0 {
                            continue;
                        }
                        if let Some(tc) = seg_cross(cq[0], cq[1], bq[0], bq[1]) {
                            let tb = seg_cross(bq[0], bq[1], cq[0], cq[1]).unwrap_or(0.5);
                            let d = d0 + (along[ck + 1] - d0) * tc;
                            if (120.0..450.0).contains(&(dj - d))
                                && over.is_none_or(|o| (o.0 - (dj - 250.0)).abs() > (d - (dj - 250.0)).abs())
                            {
                                let h = match (deck_node.get(&bw.nodes[bk]), deck_node.get(&bw.nodes[bk + 1])) {
                                    (Some(&a), Some(&b)) => H::Deck { a, b, t: tb },
                                    _ => continue,
                                };
                                over = Some((d, cq[0].lerp(cq[1], tc), (cq[1] - cq[0]).normalize_or(Vec2::X), h, bw.prof.half()));
                            }
                        }
                    }
                }
            }
            if let Some((_, p, dir, h, bhw)) = over {
                add(
                    PropKind::OverpassSign { exit, text },
                    p - dir * (bhw + 0.7) - right(dir) * (hw * 0.4),
                    h,
                    heading_of(-dir),
                    seed ^ nid as u64,
                );
            } else {
                let (p, dir) = at(dj - 250.0);
                add(
                    PropKind::Gantry { span: 2.0 * hw + 3.0, exit, text },
                    p,
                    H::Ground { off: 0.0 },
                    heading_of(-dir),
                    seed ^ nid as u64 ^ 0x9a,
                );
            }
        }
    }

    // ---- utility poles (with wires) and hydrants along streets where none are mapped
    let mut wires: Vec<(Vec2, Vec2, Vec2)> = Vec::new(); // (a, b, crossarm direction)
    for (ids, pts) in &power_lines {
        let _ = ids;
        for q in pts.windows(2) {
            if q[0].distance(q[1]) < 120.0 {
                wires.push((q[0], q[1], right((q[1] - q[0]).normalize_or(Vec2::X))));
            }
        }
    }
    if mapped_poles < 4 || mapped_hydrants < 3 {
        for s in &segs {
            let w = &ways[s.way as usize];
            if s.internal
                || w.elevated()
                || !matches!(w.class, Class::Secondary | Class::Tertiary | Class::Residential | Class::Unclassified)
                || w.link
            {
                continue;
            }
            let pts = cut(&w.pts[s.i0..=s.i1], s.trim0 + 6.0, s.trim1 + 6.0);
            let len = polyline_len(&pts);
            if len < 50.0 {
                continue;
            }
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
            let seed = hash64(w.id as u64 ^ 0x7013);
            let urban = is_urban(pts[0]);
            let side = if seed & 1 == 0 { 1.0 } else { -1.0 };
            let (swl, swr) = sidewalks(w, sidewalks_mapped, urban);
            let walk = if side > 0.0 { swr } else { swl };
            let hw = w.prof.half();
            // Lines run on one side of most streets that aren't downtown.
            if mapped_poles < 4 && power_lines.is_empty() && hash01(seed ^ 5) < if urban { 0.55 } else { 0.7 } {
                let spacing = 38.0 + hash01(seed ^ 6) * 8.0;
                let n = (len / spacing).floor().max(1.0) as usize;
                let step = len / n as f32;
                let mut prev: Option<(Vec2, Vec2)> = None;
                for k in 0..=n {
                    let (p, dir) = at(k as f32 * step);
                    let pos = p + right(dir) * side * (hw + if walk { 2.3 } else { 1.6 });
                    add(PropKind::UtilityPole, pos, H::Ground { off: 0.0 }, heading_of(dir), seed ^ k as u64);
                    if let Some((pp, pd)) = prev {
                        wires.push((pp, pos, right((pd + dir).normalize_or(dir))));
                    }
                    prev = Some((pos, dir));
                }
            }
            // Hydrants every block or so in town, at the curb.
            if mapped_hydrants < 3 && urban && w.class != Class::Secondary {
                let mut d = 20.0 + hash01(seed ^ 8) * 60.0;
                let mut k = 0u64;
                while d < len - 10.0 {
                    let (p, dir) = at(d);
                    let pos = p - right(dir) * side * (hw + 0.7);
                    add(PropKind::Hydrant, pos, H::Ground { off: 0.0 }, heading_of(right(dir) * side), seed ^ 0x40 ^ k);
                    d += 130.0;
                    k += 1;
                }
            }
        }
    }
    // Cars parked along the curb of residential streets (unless mapped as no parking).
    for s in &segs {
        let w = &ways[s.way as usize];
        if s.internal
            || w.elevated()
            || w.link
            || w.construction
            || !matches!(w.class, Class::Residential | Class::Unclassified | Class::Living | Class::Tertiary)
        {
            continue;
        }
        let hw = w.prof.half();
        if hw < 3.4 {
            continue;
        }
        let banned = |side: &str| {
            let t = &w.tags;
            let v = [
                format!("parking:{side}"),
                format!("parking:lane:{side}"),
                "parking:both".to_string(),
                "parking:lane:both".to_string(),
            ];
            v.iter().any(|k| matches!(t.get(k), Some("no" | "no_parking" | "no_stopping" | "separate" | "fire_lane")))
        };
        let mapped = |side: &str| {
            let t = &w.tags;
            [format!("parking:{side}"), format!("parking:lane:{side}"), "parking:both".into(), "parking:lane:both".into()]
                .iter()
                .any(|k| matches!(t.get(k), Some("lane" | "street_side" | "parallel" | "on_street" | "half_on_kerb" | "yes")))
        };
        // Main streets only where parking is mapped.
        if w.class == Class::Tertiary && !mapped("left") && !mapped("right") && !mapped("both") {
            continue;
        }
        let pts = cut(&w.pts[s.i0..=s.i1], s.trim0 + 9.0, s.trim1 + 9.0);
        let len = polyline_len(&pts);
        if len < 14.0 {
            continue;
        }
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
        let seed = hash64(w.id as u64 ^ 0xca75 ^ (s.i0 as u64) << 32);
        let urban = is_urban(pts[0]);
        let busy = if urban { 0.55 } else { 0.22 } * (0.5 + hash01(seed ^ 1));
        for (k, (side, name)) in [(1.0f32, "right"), (-1.0f32, "left")].into_iter().enumerate() {
            if banned(name) || (w.prof.oneway && side < 0.0 && !mapped(name)) {
                continue;
            }
            let mut d = hash01(seed ^ 7 ^ k as u64) * 6.0;
            let mut n = 0u64;
            while d < len {
                let r = hash64(seed ^ (k as u64) << 40 ^ n);
                let body = super::cars::body_for(hash01(r ^ 3));
                let l = super::cars::length(body);
                if d + l > len {
                    break;
                }
                if hash01(r) < busy {
                    let (p, dir) = at(d + l * 0.5);
                    let pos = p + right(dir) * side * (hw - 1.05);
                    // Parked with the traffic on that side.
                    let face = if side > 0.0 { dir } else { -dir };
                    let paint = super::cars::paint_for(hash01(r ^ 5));
                    add(PropKind::Car { body, paint }, pos, H::Ground { off: 0.0 }, heading_of(face) + (hash01(r ^ 9) - 0.5) * 0.04, r);
                }
                d += l + 0.8 + hash01(r ^ 11) * 2.5 + if hash01(r ^ 13) < 0.15 { 6.0 } else { 0.0 };
                n += 1;
            }
        }
    }
    // Three sagging conductors per span, from the crossarm ends.
    for &(a, b, arm) in &wires {
        let mid = (a + b) * 0.5;
        if !in_tile(mid) {
            continue;
        }
        let cid = chunk_of(mid);
        let g = &mut chunks.entry(cid).or_insert_with(|| new_chunk(cid)).geom;
        let span = a.distance(b);
        let sag = (span * 0.025).clamp(0.3, 1.6);
        for x in [-1.0f32, 0.0, 1.0] {
            let (pa, pb) = (a + arm * x, b + arm * x);
            let n = 6;
            for k in 0..n {
                let (t0, t1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
                let y = |t: f32| 10.6 - sag * 4.0 * t * (1.0 - t);
                strut(
                    g,
                    (pa.lerp(pb, t0), H::Ground { off: 0.0 }, y(t0)),
                    (pa.lerp(pb, t1), H::Ground { off: 0.0 }, y(t1)),
                    0.025,
                    Some([0.05, 0.05, 0.05]),
                );
            }
        }
    }
    // ---- surface parking lots
    {
        let (sw, ne) = (local.xz(ts, tw), local.xz(tn, te));
        let bounds = (sw.x.min(ne.x), sw.x.max(ne.x), sw.y.min(ne.y), sw.y.max(ne.y));
        let lots = super::parking::lots(osm, &local, bounds, &in_tile, &nearest_road);
        stats.parking_stalls = lots.stalls;
        for (c, pg) in lots.pieces {
            let cid = chunk_of(c);
            let g = &mut chunks.entry(cid).or_insert_with(|| new_chunk(cid)).geom;
            let base = g.v.len() as u32;
            g.v.extend(pg.v);
            g.i.extend(pg.i.into_iter().map(|i| i + base));
        }
        props.extend(lots.props);
    }
    // Nothing stands in a roadway: objects that ended up on a road (exit signs on a
    // junction node of another carriageway, lamps of crossing streets...) step out to
    // the nearest edge.
    for pr in props.iter_mut() {
        if matches!(
            pr.kind,
            PropKind::Gantry { .. }
                | PropKind::OverpassSign { .. }
                | PropKind::Car { .. }
                | PropKind::Cone
                | PropKind::Barrel
                | PropKind::Guardrail { .. }
                | PropKind::Bollard
        ) || matches!(pr.h, H::Deck { .. })
        {
            continue;
        }
        if let Some((c, d, dir)) = nearest_road(pr.p)
            && d < 0.4
        {
            let hw = c.distance(pr.p) - d;
            let mut n = (pr.p - c).normalize_or(Vec2::ZERO);
            if n == Vec2::ZERO {
                n = right(dir);
            }
            pr.p = c + n * (hw + 1.3);
        }
    }
    stats.props = props.len();
    for p in props {
        let cid = chunk_of(p.p);
        chunks.entry(cid).or_insert_with(|| new_chunk(cid)).props.push(p);
    }

    // ---- inland water: lakes, reservoirs and river areas, as a surface draped just
    // above the terrain (elevation models record the water surface). Cut into 24 m
    // cells so it follows the ground; cells straddling the shore are clipped.
    {
        const CELL: f32 = 24.0;
        let (sw, ne) = (local.xz(ts, tw), local.xz(tn, te));
        let (x0, x1, z0, z1) = (sw.x.min(ne.x), sw.x.max(ne.x), sw.y.min(ne.y), sw.y.max(ne.y));
        let pk = pack(elem::JUNCTION, 0, flag::WATER);
        let flow_lines: Vec<(Vec<Vec2>, f32)> = flows.iter().map(|(l, sp)| (l.iter().map(|&(a, b)| local.xz(a, b)).collect(), *sp)).collect();
        // The current at a point: along the nearest centreline within reach.
        let flow_at = |p: Vec2| -> Vec2 {
            let mut best = (f32::MAX, Vec2::ZERO);
            for (line, sp) in &flow_lines {
                for q in line.windows(2) {
                    let d = q[1] - q[0];
                    let l2 = d.length_squared().max(1e-4);
                    let t = ((p - q[0]).dot(d) / l2).clamp(0.0, 1.0);
                    let dist = p.distance(q[0] + d * t);
                    if dist < best.0 {
                        best = (dist, d.normalize_or(Vec2::ZERO) * *sp);
                    }
                }
            }
            if best.0 < 180.0 { best.1 } else { Vec2::ZERO }
        };
        for area in osm.areas.iter().filter(|a| usx_core::osm::is_water(&a.tags)) {
            let river = matches!(area.tags.get("water"), Some("river" | "canal" | "stream" | "rapids")) || area.tags.is("waterway", "riverbank");
            let tint = water_tint(&area.tags, cla, clo, area.id);
            let ring = |r: &Vec<(f64, f64)>| -> Vec<Vec2> {
                let mut v: Vec<Vec2> = r.iter().map(|&(a, b)| local.xz(a, b)).collect();
                if v.len() > 1 && v[0].distance(v[v.len() - 1]) < 0.01 {
                    v.pop();
                }
                v
            };
            let outers: Vec<Vec<Vec2>> = area.outer.iter().map(ring).filter(|r| r.len() >= 3).collect();
            let inners: Vec<Vec<Vec2>> = area.inner.iter().map(ring).filter(|r| r.len() >= 3).collect();
            if outers.is_empty() {
                continue;
            }
            // Bounding box, limited to the tile.
            let (mut bx0, mut bx1, mut bz0, mut bz1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for p in outers.iter().flatten() {
                bx0 = bx0.min(p.x);
                bx1 = bx1.max(p.x);
                bz0 = bz0.min(p.y);
                bz1 = bz1.max(p.y);
            }
            let (bx0, bx1, bz0, bz1) = (bx0.max(x0), bx1.min(x1), bz0.max(z0), bz1.min(z1));
            if bx0 >= bx1 || bz0 >= bz1 {
                continue;
            }
            // Which rings have an edge in each cell.
            let cell_of = |x: f32, z: f32| ((x / CELL).floor() as i32, (z / CELL).floor() as i32);
            let mut edges_in: HashMap<(i32, i32), Vec<(bool, usize)>> = HashMap::new();
            for (is_outer, rings) in [(true, &outers), (false, &inners)] {
                for (ri, r) in rings.iter().enumerate() {
                    for k in 0..r.len() {
                        let (a, b) = (r[k], r[(k + 1) % r.len()]);
                        let steps = (a.distance(b) / (CELL * 0.5)).ceil().max(1.0) as usize;
                        for t in 0..=steps {
                            let q = a.lerp(b, t as f32 / steps as f32);
                            let c = cell_of(q.x, q.y);
                            let e = edges_in.entry(c).or_default();
                            if !e.contains(&(is_outer, ri)) {
                                e.push((is_outer, ri));
                            }
                        }
                    }
                }
            }
            let inside = |rings: &Vec<Vec<Vec2>>, p: Vec2| rings.iter().any(|r| point_in_ring(r, p));
            let (c0, c1) = (cell_of(bx0, bz0), cell_of(bx1, bz1));
            for cz in c0.1..=c1.1 {
                for cx in c0.0..=c1.0 {
                    let (qx0, qz0) = ((cx as f32 * CELL).max(x0), (cz as f32 * CELL).max(z0));
                    let (qx1, qz1) = (((cx + 1) as f32 * CELL).min(x1), ((cz + 1) as f32 * CELL).min(z1));
                    if qx0 >= qx1 || qz0 >= qz1 {
                        continue;
                    }
                    let centre = Vec2::new((qx0 + qx1) * 0.5, (qz0 + qz1) * 0.5);
                    let square = vec![Vec2::new(qx0, qz0), Vec2::new(qx1, qz0), Vec2::new(qx1, qz1), Vec2::new(qx0, qz1)];
                    let mut polys: Vec<Vec<Vec2>> = Vec::new();
                    let mut holes: Vec<Vec<Vec2>> = Vec::new();
                    match edges_in.get(&(cx, cz)) {
                        None => {
                            if inside(&outers, centre) && !inside(&inners, centre) {
                                polys.push(square);
                            }
                        }
                        Some(list) => {
                            for (ri, r) in outers.iter().enumerate() {
                                if list.contains(&(true, ri)) {
                                    let c = clip_rect(r, qx0, qz0, qx1, qz1);
                                    if c.len() >= 3 {
                                        polys.push(c);
                                    }
                                } else if point_in_ring(r, centre) {
                                    polys.push(square.clone());
                                }
                            }
                            for (ri, r) in inners.iter().enumerate() {
                                if list.contains(&(false, ri)) {
                                    let c = clip_rect(r, qx0, qz0, qx1, qz1);
                                    if c.len() >= 3 {
                                        holes.push(c);
                                    }
                                } else if point_in_ring(r, centre) {
                                    polys.clear();
                                }
                            }
                        }
                    }
                    if polys.is_empty() {
                        continue;
                    }
                    let cid = chunk_of(centre);
                    let g = &mut chunks.entry(cid).or_insert_with(|| new_chunk(cid)).geom;
                    let one = polys.len() == 1;
                    for poly in polys {
                        let mut flat: Vec<f32> = poly.iter().flat_map(|p| [p.x, p.y]).collect();
                        let mut hole_idx = Vec::new();
                        if one {
                            for h in &holes {
                                hole_idx.push(flat.len() / 2);
                                flat.extend(h.iter().flat_map(|p| [p.x, p.y]));
                            }
                        }
                        let Ok(tris) = earcutr::earcut(&flat, &hole_idx, 2) else { continue };
                        let base = g.v.len() as u32;
                        let flow = if river { encode_flow(flow_at(centre)) } else { 0.0 };
                        for k in 0..flat.len() / 2 {
                            let p = Vec2::new(flat[2 * k], flat[2 * k + 1]);
                            g.v.push(V {
                                p,
                                h: H::Water { at: p },
                                dy: 0.05,
                                uv: [p.x, p.y],
                                uv2: [flow, pk.0],
                                col: [tint[0], tint[1], pk.1, tint[2]],
                            });
                        }
                        for t in tris.chunks_exact(3) {
                            g.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
                        }
                    }
                }
            }
        }
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
    let water: Vec<usx_core::osm::Area> = osm.areas.iter().filter(|a| usx_core::osm::is_water(&a.tags)).cloned().collect();
    let lakes: Vec<Lake> = water
        .iter()
        .map(|a| {
            let (mut s0, mut w0, mut n0, mut e0) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
            let mut area_m2 = 0.0f64;
            for r in &a.outer {
                for &(la, lo) in r {
                    s0 = s0.min(la);
                    n0 = n0.max(la);
                    w0 = w0.min(lo);
                    e0 = e0.max(lo);
                }
                let pts: Vec<Vec2> = r.iter().map(|&(la, lo)| local.xz(la, lo)).collect();
                area_m2 += pts.windows(2).map(|q| q[0].perp_dot(q[1]) as f64).sum::<f64>().abs() * 0.5;
            }
            // Rivers are shallower than lakes of the same size.
            let river = a.tags.is("water", "river") || a.tags.is("waterway", "riverbank") || a.tags.is("water", "canal");
            let k = if river { 0.012 } else { 0.02 };
            Lake {
                outer: a.outer.clone(),
                inner: a.inner.clone(),
                bbox: (s0, w0, n0, e0),
                depth: ((area_m2.sqrt() * k) as f32).clamp(1.5, if river { 6.0 } else { 18.0 }),
                level: false,
            }
        })
        .collect();
    let mut lakes = lakes;
    // Parking lots: the ground under them is smoothed (see terrain::conform).
    for a in osm.areas.iter().filter(|a| usx_core::osm::is_parking(&a.tags) && a.tags.get("building").is_none()) {
        let (mut s0, mut w0, mut n0, mut e0) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
        for &(la, lo) in a.outer.iter().flatten() {
            s0 = s0.min(la);
            n0 = n0.max(la);
            w0 = w0.min(lo);
            e0 = e0.max(lo);
        }
        if a.outer.is_empty() || n0 < ts || s0 > tn || e0 < tw || w0 > te {
            continue;
        }
        lakes.push(Lake { outer: a.outer.clone(), inner: a.inner.clone(), bbox: (s0, w0, n0, e0), depth: 0.0, level: true });
    }
    Built { tile, local, chunks: out_chunks, decks, corridors, controllers, lanes, water, lakes, stats }
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
    // Inferred only on town streets: not rural unclassified roads, not fast roads.
    let candidate = matches!(w.class, Class::Primary | Class::Secondary | Class::Tertiary | Class::Residential | Class::Living);
    let slow = w.tags.get("maxspeed").and_then(parse_mph).is_none_or(|m| m <= 40);
    let unpaved = matches!(w.surface, Surface::Gravel | Surface::Dirt);
    let on = candidate && urban && slow && !unpaved && !mapped && !w.link;
    (on, on)
}

/// Water colour (linear RGB of the water body seen from above) by region and type:
/// tea-brown blackwater in the Southeast, green lakes on the farm belt, clear blue in
/// the mountains and desert reservoirs, silty rivers.
pub fn water_tint(t: &Tags, lat: f64, lon: f64, id: i64) -> [f32; 3] {
    let mut c: [f32; 3] = if lon < -114.0 && lat > 42.0 {
        [0.018, 0.055, 0.065] // Pacific Northwest
    } else if lon < -103.0 && lat < 37.5 {
        [0.015, 0.05, 0.10] // desert Southwest reservoirs
    } else if lon < -104.0 {
        [0.02, 0.06, 0.09] // mountain West
    } else if lat < 35.5 && lon > -98.0 {
        [0.055, 0.06, 0.035] // Southeast blackwater and Gulf coast
    } else if lon > -80.0 && lat > 38.0 {
        [0.03, 0.055, 0.06] // Northeast
    } else {
        [0.04, 0.075, 0.05] // Plains and Midwest
    };
    let mix = |c: &mut [f32; 3], o: [f32; 3], k: f32| {
        for i in 0..3 {
            c[i] = c[i] * (1.0 - k) + o[i] * k;
        }
    };
    if t.is("water", "river") || t.is("waterway", "riverbank") || t.is("water", "canal") {
        mix(&mut c, [0.085, 0.075, 0.048], 0.45); // silt
    } else if t.is("water", "pond") || t.is("landuse", "basin") || t.is("water", "wastewater") || t.is("water", "basin") {
        mix(&mut c, [0.045, 0.08, 0.035], 0.5); // algae
    } else if t.is("water", "reservoir") || t.is("landuse", "reservoir") {
        mix(&mut c, [0.015, 0.05, 0.09], 0.3);
    } else if t.is("water", "lagoon") {
        mix(&mut c, [0.03, 0.09, 0.09], 0.5);
    }
    let v = 0.85 + 0.3 * hash01(id as u64 ^ 0x3a7e);
    c.map(|x| x * v)
}

pub(crate) fn point_in_ring(r: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = r.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
        j = i;
    }
    c
}

/// Sutherland-Hodgman: a ring clipped to an axis-aligned rectangle.
pub(crate) fn clip_rect(r: &[Vec2], x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = r.to_vec();
    let planes: [(Vec2, f32); 4] = [(Vec2::X, x0), (-Vec2::X, -x1), (Vec2::Y, z0), (-Vec2::Y, -z1)];
    for (n, d) in planes {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        let inside = |p: Vec2| p.dot(n) >= d;
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            let (ia, ib) = (inside(a), inside(b));
            if ia {
                out.push(a);
            }
            if ia != ib {
                let t = (d - a.dot(n)) / (b - a).dot(n);
                out.push(a + (b - a) * t);
            }
        }
    }
    out
}

/// Convex hull, counter-clockwise in x/z as the junction polygons are.
fn convex_hull(pts: &[Vec2]) -> Vec<Vec2> {
    let mut p: Vec<Vec2> = pts.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    p.dedup_by(|a, b| a.distance(*b) < 1e-3);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut lower: Vec<Vec2> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<Vec2> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Where there are curbs: with a sidewalk, or on city streets even when the sidewalk
/// is mapped as its own way (or tagged kerb=raised).
fn curbs(w: &RWay, sidewalk: (bool, bool), mapped: bool, urban: bool) -> (bool, bool) {
    if w.tags.is("kerb", "raised") || w.tags.is("kerb", "regular") {
        return (true, true);
    }
    let street =
        matches!(w.class, Class::Primary | Class::Secondary | Class::Tertiary | Class::Residential | Class::Living | Class::Unclassified);
    let explicit_no = |k: &str| w.tags.get(k).is_some_and(|v| v == "no" || v == "none");
    let city = street && urban && !w.link && !w.elevated();
    let separate = |k: &str| w.tags.get(k).is_some_and(|v| v == "separate");
    let l = sidewalk.0
        || city && (mapped || separate("sidewalk") || separate("sidewalk:left") || separate("sidewalk:both")) && !explicit_no("sidewalk");
    let r = sidewalk.1
        || city && (mapped || separate("sidewalk") || separate("sidewalk:right") || separate("sidewalk:both")) && !explicit_no("sidewalk");
    (l, r)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum BridgeKind {
    Beam,
    Truss,
    Arch,
    Suspension,
    CableStayed,
}

/// Bridge type from `bridge:structure` (and a little inference for unlabelled big ones).
fn bridge_kind(w: &RWay, length: f32) -> BridgeKind {
    match w.tags.get("bridge:structure") {
        Some("suspension" | "simple-suspension") => BridgeKind::Suspension,
        Some("cable-stayed") => BridgeKind::CableStayed,
        Some("arch" | "tied-arch") => BridgeKind::Arch,
        Some("truss") => BridgeKind::Truss,
        Some(_) => BridgeKind::Beam,
        None if w.tags.is("bridge", "suspension") => BridgeKind::Suspension,
        None if w.class.rail() && length > 60.0 && length < 400.0 && w.tags.is("bridge", "yes") => BridgeKind::Truss,
        None => BridgeKind::Beam,
    }
}

/// The `colour` tag as linear-ish RGB (hex, or a few common names).
fn bridge_paint(w: &RWay) -> Option<[f32; 3]> {
    let c = w.tags.get("colour")?.trim().to_ascii_lowercase();
    if let Some(h) = c.strip_prefix('#')
        && h.len() == 6
    {
        let v = u32::from_str_radix(h, 16).ok()?;
        return Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0]);
    }
    Some(match c.as_str() {
        "international orange" | "orange" => [0.75, 0.22, 0.10],
        "red" => [0.62, 0.10, 0.08],
        "green" => [0.18, 0.35, 0.22],
        "blue" => [0.15, 0.25, 0.5],
        "black" => [0.06, 0.06, 0.06],
        "white" => [0.85, 0.85, 0.83],
        "grey" | "gray" | "silver" => [0.55, 0.56, 0.57],
        "yellow" => [0.8, 0.65, 0.1],
        "brown" => [0.35, 0.22, 0.14],
        _ => return None,
    })
}

/// A colour for structure vertices, carried in uv2.x as -(RGB565 + 1).
fn paint_code(c: [f32; 3]) -> f32 {
    let q = |v: f32, bits: u32| (v.clamp(0.0, 1.0) * ((1 << bits) - 1) as f32).round() as u32;
    -((q(c[0], 5) << 11 | q(c[1], 6) << 5 | q(c[2], 5)) as f32 + 1.0)
}

fn truss_height(hw: f32) -> f32 {
    (hw * 1.1).clamp(5.5, 9.0)
}

/// Height of a suspension bridge's main cable above the deck at distance `u` along a
/// bridge of length `l`: high at the towers (27% and 73%), sagging between them.
fn cable_dy(u: f32, l: f32, hw: f32) -> f32 {
    let (a, b) = if l > 300.0 { (l * 0.27, l * 0.73) } else { (l * 0.5, l * 0.5) };
    let span = if b > a { b - a } else { l * 0.5 };
    let ht = (span * 0.12).clamp(25.0, 160.0) - 2.0;
    let low = 2.5 + hw * 0.0;
    if u <= a {
        low + (ht - low) * (u / a.max(1.0)).powf(1.6)
    } else if u >= b {
        low + (ht - low) * ((l - u) / (l - b).max(1.0)).powf(1.6)
    } else {
        let x = (u - a) / span.max(1.0) * 2.0 - 1.0;
        low + (ht - low) * x * x
    }
}

/// A box with its long axis along `dir`: half length `hl`, half width `hw`, from
/// `bottom` to `top` (height rules with an extra lift each).
pub(crate) fn prism(g: &mut Geom, c: Vec2, dir: Vec2, hl: f32, hw: f32, bottom: (H, f32), top: (H, f32), color: Option<[f32; 3]>) {
    let r = right(dir);
    let corners = [c + dir * hl + r * hw, c + dir * hl - r * hw, c - dir * hl - r * hw, c - dir * hl + r * hw];
    let pk = pack(elem::STRUCTURE, 0, flag::STRUCTURE);
    let tint = color.map(paint_code).unwrap_or(0.0);
    let mk = |p: Vec2, (h, dy): (H, f32), u: f32, v: f32| V { p, h, dy, uv: [u, v], uv2: [tint, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
    for k in 0..4 {
        let (p0, p1) = (corners[k], corners[(k + 1) % 4]);
        let a = g.push(mk(p0, top, k as f32, 1.0));
        let b = g.push(mk(p1, top, k as f32 + 1.0, 1.0));
        let c0 = g.push(mk(p0, bottom, k as f32, 0.0));
        let d = g.push(mk(p1, bottom, k as f32 + 1.0, 0.0));
        g.quad(a, b, c0, d);
    }
    // Top and bottom caps.
    for (lvl, flip) in [(top, false), (bottom, true)] {
        let ids: Vec<u32> = corners.iter().map(|&p| g.push(mk(p, lvl, p.x, p.y))).collect();
        if flip {
            g.tri(ids[0], ids[1], ids[2]);
            g.tri(ids[0], ids[2], ids[3]);
        } else {
            g.tri(ids[0], ids[2], ids[1]);
            g.tri(ids[0], ids[3], ids[2]);
        }
    }
}

/// A thin flat member between two points at given heights (truss diagonals, cable stays),
/// as a vertical ribbon `width` tall; the material is two-sided.
fn strut(g: &mut Geom, a: (Vec2, H, f32), b: (Vec2, H, f32), width: f32, color: Option<[f32; 3]>) {
    let pk = pack(elem::STRUCTURE, 0, flag::STRUCTURE);
    let tint = color.map(paint_code).unwrap_or(0.0);
    let mk = |p: Vec2, h: H, dy: f32| V { p, h, dy, uv: [p.x, dy], uv2: [tint, pk.0], col: [0.0, 0.0, pk.1, 0.0] };
    let i0 = g.push(mk(a.0, a.1, a.2));
    let i1 = g.push(mk(a.0, a.1, a.2 + width));
    let i2 = g.push(mk(b.0, b.1, b.2));
    let i3 = g.push(mk(b.0, b.1, b.2 + width));
    g.quad(i0, i1, i2, i3);
}

/// Distance of every deck node from the start of its network (Dijkstra from one end).
fn chain_distances(d: &Decks) -> Vec<f32> {
    let n = d.p.len();
    let mut dist = vec![f32::MAX; n];
    for net in 0..d.nets {
        // Start from a boundary node (the lowest-index one), or any node.
        let start = (0..n).filter(|&i| d.net[i] == net).min_by_key(|&i| (!d.boundary[i], i));
        let Some(start) = start else { continue };
        let mut heap = BinaryHeap::new();
        dist[start] = 0.0;
        heap.push((OrdF(-0.0), start));
        while let Some((OrdF(nd), u)) = heap.pop() {
            let du = -nd;
            if du > dist[u] {
                continue;
            }
            for &(v, w) in &d.edges[u] {
                let v = v as usize;
                if du + w < dist[v] {
                    dist[v] = du + w;
                    heap.push((OrdF(-(du + w)), v));
                }
            }
        }
    }
    dist.iter().map(|&x| if x == f32::MAX { 0.0 } else { x }).collect()
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
    /// Build tiles straight from the game's download cache:
    /// USX_TILES="15/7419/13579 ..." [USX_CACHE=dir] [USX_DEBUG_DECKS=1]
    /// cargo test --profile fast cached_tiles -- --ignored --nocapture
    #[test]
    #[ignore]
    fn cached_tiles() {
        let root = std::env::var("USX_CACHE").unwrap_or_else(|_| format!("{}/.cache/us-explorer", std::env::var("HOME").unwrap()));
        let _ = crate::terrain::load::CACHE.set(usx_core::cache::DiskCache::open(root, 50 << 30));
        for t in std::env::var("USX_TILES").expect("USX_TILES").split_whitespace() {
            let p: Vec<u32> = t.split('/').map(|x| x.parse().unwrap()).collect();
            let tile = TileId { z: p[0] as u8, x: p[1], y: p[2] };
            let Some(osm) = usx_core::osm::cached(tile, crate::terrain::load::cache()) else {
                println!("{t}: not cached");
                continue;
            };
            let dem = |la: f64, lo: f64| crate::terrain::load::dem_height(la, lo);
            let b = build(tile, &osm, &dem);
            println!(
                "{t}: {} ways, {} junctions, {} bridges, {} tunnels, {} lots, {} chunks",
                b.stats.ways, b.stats.junctions, b.stats.bridges, b.stats.tunnels, b.stats.parking_stalls, b.chunks.len()
            );
            // Ground roads raised or sunk a lot (ramps up to bridges, down to underpasses).
            let mut big: Vec<(f32, f64, f64)> = Vec::new();
            for c in &b.corridors {
                for (k, o) in c.off.iter().enumerate() {
                    if o.abs() > 2.5 {
                        big.push((*o, c.pts[k].0, c.pts[k].1));
                    }
                }
            }
            big.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
            for x in big.iter().take(15) {
                println!("  ground road offset {:.1} m at {:.6},{:.6}", x.0, x.1, x.2);
            }
        }
    }

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
            // (A z14 fixture covers its four z15 game tiles; extra data is clipped away.)
            if let Ok(seed) = std::env::var("USX_SEED_CACHE") {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let child = TileId { z: 15, x: tile.x * 2 + dx, y: tile.y * 2 + dy };
                    let path = std::path::Path::new(&seed).join(format!("osm/v3/{}.bin", child.path()));
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, &bin).unwrap();
                }
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
