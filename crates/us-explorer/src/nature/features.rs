//! Built details from the nature layer: fences, walls and hedges; power lines on their
//! towers; wind turbines that turn to face the real wind; buoys and beacons that ride
//! the waves (lit at night); piers on piles; lifeguard towers on beaches; and street
//! furniture (bins, bike racks, fountains, picnic tables, flagpoles, water towers...).
//!
//! Geometry is built per zoom-17 cell in local metres with a height rule per vertex, then
//! draped on the ground being drawn (like roads), one mesh per material per cell.

use super::nature_osm::{BarrierKind, FurnitureKind, NatureData, Seamark};
use crate::roads::build::Local;
use bevy::math::{Vec2, Vec3};
use usx_core::tile::TileId;

/// Cells for features are zoom-17 tiles, like vegetation.
pub const ZOOM: u8 = 17;

/// Materials.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FMat {
    /// Painted surfaces, plastic, stone and concrete (vertex colour).
    Paint,
    /// Steel and aluminium (vertex colour, metallic).
    Metal,
    /// Chain-link mesh (cut-out texture).
    ChainLink,
    /// Wooden boards.
    Wood,
    /// Hedges (leaf cards).
    Leaves,
    /// Glowing lamps (lighthouse lanterns, buoy lights).
    Lamp,
}

/// How a vertex's height is found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FH {
    /// The ground under it.
    Ground,
    /// The ground at another point (a level deck).
    GroundAt(Vec2),
    /// The water surface (sea level, or the lake's surface).
    Water,
    /// The higher of the ground under it and the water.
    Surface,
}

#[derive(Clone, Copy, Debug)]
pub struct FV {
    pub p: Vec2,
    pub h: FH,
    pub dy: f32,
    /// Local normal (x east, y up, z south).
    pub n: Vec3,
    pub uv: Vec2,
    pub col: [f32; 4],
}

#[derive(Default, Clone)]
pub struct FGeom {
    pub v: Vec<FV>,
    pub i: Vec<u32>,
}

/// Things that move or light up, placed by the drape.
#[derive(Clone, Debug)]
pub enum ObjKind {
    /// Rotor hub height, rotor diameter.
    Turbine { hub: f32, rotor: f32 },
    Buoy(Seamark),
}

#[derive(Clone, Debug)]
pub struct Obj {
    pub kind: ObjKind,
    pub p: Vec2,
    pub h: FH,
    pub yaw: f32,
    pub id: i64,
}

#[derive(Default)]
pub struct FChunk {
    pub id: Option<TileId>,
    pub geoms: std::collections::BTreeMap<FMat, FGeom>,
    pub objs: Vec<Obj>,
    /// (s, w, n, e) of everything in it.
    pub bbox: (f64, f64, f64, f64),
}

pub struct FBuilt {
    pub local: Local,
    pub chunks: Vec<FChunk>,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

fn rgb(r: f32, g: f32, b: f32) -> [f32; 4] {
    [r, g, b, 1.0]
}

fn hash01(x: u64) -> f32 {
    crate::buildings::model::hash01(x)
}

impl FGeom {
    fn push(&mut self, v: FV) -> u32 {
        self.v.push(v);
        (self.v.len() - 1) as u32
    }

    /// A quad (counter-clockwise seen from the front) with explicit corners.
    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, c: [(Vec2, FH, f32); 4], n: Vec3, uv: [Vec2; 4], col: [f32; 4]) {
        let b = self.v.len() as u32;
        for k in 0..4 {
            self.push(FV { p: c[k].0, h: c[k].1, dy: c[k].2, n, uv: uv[k], col });
        }
        self.i.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }

    /// A wall-like box along a ground segment a->b: `half` thick, from `y0` to `y1`
    /// above the ground (each end following its own ground).
    fn slab(&mut self, a: Vec2, b: Vec2, half: f32, y0: f32, y1: f32, h: (FH, FH), col: [f32; 4], uv_scale: f32) {
        let d = (b - a).normalize_or(Vec2::X);
        let n = Vec2::new(-d.y, d.x);
        let len = a.distance(b);
        let (ha, hb) = h;
        for side in [1.0f32, -1.0] {
            let o = n * half * side;
            let nn = Vec3::new(n.x * side, 0.0, n.y * side);
            let (p0, p1) = if side > 0.0 { (a + o, b + o) } else { (b + o, a + o) };
            let (h0, h1) = if side > 0.0 { (ha, hb) } else { (hb, ha) };
            self.quad(
                [(p0, h0, y0), (p1, h1, y0), (p1, h1, y1), (p0, h0, y1)],
                nn,
                [Vec2::new(0.0, y0), Vec2::new(len, y0), Vec2::new(len, y1), Vec2::new(0.0, y1)].map(|v| v / uv_scale),
                col,
            );
        }
        // Top.
        self.quad(
            [(a - n * half, ha, y1), (b - n * half, hb, y1), (b + n * half, hb, y1), (a + n * half, ha, y1)],
            Vec3::Y,
            [Vec2::ZERO, Vec2::new(len, 0.0), Vec2::new(len, half * 2.0), Vec2::new(0.0, half * 2.0)].map(|v| v / uv_scale),
            col,
        );
        // Ends.
        for (p, hh, s) in [(a, ha, -1.0f32), (b, hb, 1.0)] {
            let nn = Vec3::new(d.x * s, 0.0, d.y * s);
            let (l, r) = if s > 0.0 { (p - n * half, p + n * half) } else { (p + n * half, p - n * half) };
            self.quad([(l, hh, y0), (r, hh, y0), (r, hh, y1), (l, hh, y1)], nn, [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y], col);
        }
    }

    /// An upright box centred on `c`, `w` x `d` (rotated by `yaw`), from y0 to y1.
    #[allow(clippy::too_many_arguments)]
    fn post(&mut self, c: Vec2, w: f32, d: f32, yaw: f32, y0: f32, y1: f32, h: FH, col: [f32; 4]) {
        let ax = Vec2::new(yaw.cos(), yaw.sin());
        let a = c - ax * w * 0.5;
        let b = c + ax * w * 0.5;
        self.slab(a, b, d * 0.5, y0, y1, (h, h), col, 1.0);
    }

    /// A vertical cylinder.
    #[allow(clippy::too_many_arguments)]
    fn cyl(&mut self, c: Vec2, r0: f32, r1: f32, y0: f32, y1: f32, seg: u32, h: FH, col: [f32; 4]) {
        let b = self.v.len() as u32;
        for k in 0..=seg {
            let a = k as f32 / seg as f32 * std::f32::consts::TAU;
            let dir = Vec2::new(a.cos(), a.sin());
            let n = Vec3::new(dir.x, (r0 - r1) / (y1 - y0).max(0.01), dir.y).normalize();
            self.push(FV { p: c + dir * r0, h, dy: y0, n, uv: Vec2::new(k as f32 / seg as f32, 0.0), col });
            self.push(FV { p: c + dir * r1, h, dy: y1, n, uv: Vec2::new(k as f32 / seg as f32, 1.0), col });
        }
        for k in 0..seg {
            let i = b + k * 2;
            self.i.extend_from_slice(&[i, i + 3, i + 1, i, i + 2, i + 3]);
        }
        // Top cap.
        if r1 > 0.0 {
            let ct = self.push(FV { p: c, h, dy: y1, n: Vec3::Y, uv: Vec2::splat(0.5), col });
            for k in 0..seg {
                self.i.extend_from_slice(&[ct, b + k * 2 + 3, b + k * 2 + 1]);
            }
        }
    }

    /// A thin tube between two 3D points (wires, braces): `a`/`b` are (ground point,
    /// height rule, height above it).
    fn tube(&mut self, a: (Vec2, FH, f32), b: (Vec2, FH, f32), r: f32, col: [f32; 4]) {
        let d3 = Vec3::new(b.0.x - a.0.x, b.2 - a.2, b.0.y - a.0.y);
        let along = d3.normalize_or(Vec3::Y);
        let side = if along.y.abs() < 0.9 { along.cross(Vec3::Y).normalize() } else { along.cross(Vec3::X).normalize() };
        let up = side.cross(along).normalize();
        let base = self.v.len() as u32;
        let seg = 3;
        for k in 0..seg {
            let ang = k as f32 / seg as f32 * std::f32::consts::TAU;
            let o = side * ang.cos() * r + up * ang.sin() * r;
            let n = (side * ang.cos() + up * ang.sin()).normalize();
            for (p, h, y) in [a, b] {
                self.push(FV { p: p + Vec2::new(o.x, o.z), h, dy: y + o.y, n, uv: Vec2::ZERO, col });
            }
        }
        for k in 0..seg {
            let (i0, i1) = (base + k * 2, base + ((k + 1) % seg) * 2);
            self.i.extend_from_slice(&[i0, i1, i1 + 1, i0, i1 + 1, i0 + 1]);
        }
    }
}

/// Builder for one tile's features, sorted into zoom-17 chunks.
struct B {
    local: Local,
    chunks: std::collections::HashMap<TileId, FChunk>,
}

impl B {
    fn chunk(&mut self, p: Vec2) -> &mut FChunk {
        let (la, lo) = self.local.ll(p);
        let id = TileId::containing(la, lo, ZOOM).0;
        let c = self.chunks.entry(id).or_insert_with(|| {
            let (w, s, e, n) = id.bounds();
            FChunk { id: Some(id), bbox: (s, w, n, e), ..Default::default() }
        });
        let bb = &mut c.bbox;
        *bb = (bb.0.min(la), bb.1.min(lo), bb.2.max(la), bb.3.max(lo));
        c
    }
    fn geom(&mut self, p: Vec2, m: FMat) -> &mut FGeom {
        self.chunk(p).geoms.entry(m).or_default()
    }
    /// Make sure a chunk's box covers a point (for long spans).
    fn reach(&mut self, owner: Vec2, p: Vec2) {
        let (la, lo) = self.local.ll(p);
        let c = self.chunk(owner);
        c.bbox = (c.bbox.0.min(la), c.bbox.1.min(lo), c.bbox.2.max(la), c.bbox.3.max(lo));
    }
}

/// Build a nature tile's features (worker thread). `in_tile` keeps each feature in
/// exactly one tile (the one holding its first point).
pub fn build(tile: TileId, nat: &NatureData) -> FBuilt {
    let (cla, clo) = tile.lat_lon_at(0.5, 0.5);
    let local = Local::new(cla, clo);
    let (tw, ts, te, tn) = tile.bounds();
    let in_tile = |la: f64, lo: f64| la >= ts && la < tn && lo >= tw && lo < te;
    let mut b = B { local, chunks: Default::default() };
    for f in &nat.barriers {
        if f.pts.is_empty() || !in_tile(f.pts[0].0, f.pts[0].1) {
            continue;
        }
        let pts: Vec<Vec2> = f.pts.iter().map(|&(a, c)| local.xz(a, c)).collect();
        barrier(&mut b, &pts, f.kind, f.height, f.id as u64);
    }
    // Power lines: towers at the line's nodes, three conductors per circuit.
    for l in &nat.power {
        if l.pts.len() < 2 || !in_tile(l.pts[0].0, l.pts[0].1) {
            continue;
        }
        let pts: Vec<Vec2> = l.pts.iter().map(|&(a, c)| local.xz(a, c)).collect();
        power_line(&mut b, &pts, l.kv, l.cables, l.id as u64);
    }
    for t in &nat.turbines {
        if !in_tile(t.lat, t.lon) {
            continue;
        }
        let p = local.xz(t.lat, t.lon);
        turbine_tower(&mut b, p, t.hub);
        b.chunk(p).objs.push(Obj { kind: ObjKind::Turbine { hub: t.hub, rotor: t.rotor }, p, h: FH::Ground, yaw: 0.0, id: t.id });
    }
    for s in &nat.seamarks {
        if !in_tile(s.lat, s.lon) {
            continue;
        }
        let p = local.xz(s.lat, s.lon);
        let h = if s.beacon { FH::Surface } else { FH::Water };
        if s.beacon {
            beacon(&mut b, p, s);
        }
        b.chunk(p).objs.push(Obj { kind: ObjKind::Buoy(s.clone()), p, h, yaw: hash01(s.id as u64) * 6.28, id: s.id });
    }
    for p in &nat.piers {
        if p.pts.is_empty() || !in_tile(p.pts[0].0, p.pts[0].1) {
            continue;
        }
        let pts: Vec<Vec2> = p.pts.iter().map(|&(a, c)| local.xz(a, c)).collect();
        pier(&mut b, &pts, p.area, p.width, &p.kind, p.id as u64);
    }
    for f in &nat.furniture {
        if !in_tile(f.lat, f.lon) {
            continue;
        }
        let p = local.xz(f.lat, f.lon);
        furniture(&mut b, p, f.kind, f.height, f.id as u64);
    }
    // Lifeguard towers along beaches where none are mapped (Florida, California, Texas
    // and Hawaii beaches have them every few hundred metres).
    let mapped_guards = nat.furniture.iter().any(|f| f.kind == FurnitureKind::Lifeguard);
    if !mapped_guards {
        let warm = cla < 34.5 || cla < 38.5 && clo < -117.0;
        if warm {
            for a in nat.cover.iter().filter(|a| a.beach) {
                for p in beach_guard_spots(&local, a, nat) {
                    let (la, lo) = local.ll(p.0);
                    if in_tile(la, lo) {
                        lifeguard(&mut b, p.0, p.1, hash01(p.0.x.to_bits() as u64 ^ 0x6a));
                    }
                }
            }
        }
    }
    FBuilt { local, chunks: b.chunks.into_values().collect() }
}

/// Spots for lifeguard towers on a beach: every ~250 m along its seaward edge, set back
/// 30 m from the water, facing the sea.
fn beach_guard_spots(local: &Local, a: &super::nature_osm::CoverArea, nat: &NatureData) -> Vec<(Vec2, f32)> {
    let Some(ring) = a.outer.first() else { return Vec::new() };
    let pts: Vec<Vec2> = ring.iter().map(|&(la, lo)| local.xz(la, lo)).collect();
    let coast: Vec<Vec<Vec2>> = nat.coast.iter().map(|l| l.iter().map(|&(la, lo)| local.xz(la, lo)).collect()).collect();
    if coast.is_empty() {
        return Vec::new();
    }
    let nearest = |p: Vec2| -> Option<(f32, Vec2)> {
        let mut best: Option<(f32, Vec2)> = None;
        for l in &coast {
            for w in l.windows(2) {
                let ab = w[1] - w[0];
                let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                let q = w[0] + ab * t;
                let d = p.distance(q);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, q));
                }
            }
        }
        best
    };
    let mut out = Vec::new();
    let mut along = 125.0f32;
    for w in pts.windows(2) {
        let len = w[0].distance(w[1]);
        let mut s = 0.0;
        while s < len {
            let step = (along).min(len - s);
            s += step;
            along -= step;
            if along <= 0.0 {
                along = 250.0;
                let p = w[0].lerp(w[1], s / len.max(1e-3));
                if let Some((d, q)) = nearest(p)
                    && d < 60.0
                {
                    let to_sea = (q - p).normalize_or(Vec2::Y);
                    let spot = q - to_sea * 30.0;
                    out.push((spot, (to_sea.x).atan2(-to_sea.y)));
                }
            }
        }
    }
    out
}

fn barrier(b: &mut B, pts: &[Vec2], kind: BarrierKind, height: f32, seed: u64) {
    let h = height.clamp(0.4, 4.0);
    let (post_col, mat_post) = match kind {
        BarrierKind::ChainLink | BarrierKind::Metal => (rgb(0.55, 0.56, 0.57), FMat::Metal),
        _ => (rgb(0.5, 0.38, 0.26), FMat::Wood),
    };
    let iron = rgb(0.08, 0.08, 0.09);
    let wall_col = {
        let r = hash01(seed);
        if r < 0.35 {
            rgb(0.55, 0.32, 0.24)
        } else if r < 0.7 {
            rgb(0.78, 0.74, 0.66)
        } else {
            rgb(0.62, 0.62, 0.6)
        }
    };
    for w in pts.windows(2) {
        let (a, c) = (w[0], w[1]);
        let len = a.distance(c);
        if len < 0.05 {
            continue;
        }
        let d = (c - a) / len;
        let yaw = d.y.atan2(d.x);
        match kind {
            BarrierKind::Wall | BarrierKind::StoneWall | BarrierKind::Retaining | BarrierKind::Jersey => {
                let (half, col) = match kind {
                    BarrierKind::StoneWall => (0.3, rgb(0.52, 0.5, 0.46)),
                    BarrierKind::Retaining => (0.18, rgb(0.66, 0.65, 0.62)),
                    BarrierKind::Jersey => (0.3, rgb(0.7, 0.69, 0.66)),
                    _ => (0.12, wall_col),
                };
                // In pieces so it follows the ground.
                let n = (len / 4.0).ceil() as usize;
                for k in 0..n {
                    let p0 = a.lerp(c, k as f32 / n as f32);
                    let p1 = a.lerp(c, (k + 1) as f32 / n as f32);
                    let hh = if kind == BarrierKind::StoneWall { h * (0.9 + 0.2 * hash01(seed ^ k as u64)) } else { h };
                    let g = b.geom(p0, FMat::Paint);
                    g.slab(p0, p1, half, -0.3, hh, (FH::Ground, FH::Ground), col, 1.0);
                }
            }
            BarrierKind::Hedge => {
                let n = (len / 1.0).ceil().max(1.0) as usize;
                for k in 0..n {
                    let p0 = a.lerp(c, k as f32 / n as f32);
                    let p1 = a.lerp(c, (k + 1) as f32 / n as f32);
                    // A dark core and leaf cards over it.
                    b.geom(p0, FMat::Paint).slab(p0, p1, 0.35, -0.1, h * 0.92, (FH::Ground, FH::Ground), rgb(0.08, 0.14, 0.06), 1.0);
                    let g = b.geom(p0, FMat::Leaves);
                    let mid = (p0 + p1) * 0.5;
                    for q in 0..6 {
                        let r = |x: u64| hash01(seed ^ (k as u64 * 31 + q * 7 + x));
                        let side = Vec2::new(-d.y, d.x) * (r(1) - 0.5) * 0.9;
                        let at = mid + d * (r(2) - 0.5) * 1.0 + side;
                        let y = r(3) * h * 0.95;
                        let s = 0.7;
                        let face = Vec3::new(side.x + r(4) - 0.5, r(5) - 0.3, side.y + r(6) - 0.5).normalize_or(Vec3::Y);
                        let rt = face.cross(Vec3::Y).normalize_or(Vec3::X);
                        let up = rt.cross(face);
                        let corner = |cx: f32, cy: f32| {
                            let o = rt * cx * s + up * cy * s;
                            (at + Vec2::new(o.x, o.z), FH::Ground, y + o.y)
                        };
                        let (u0, u1) = super::treegen::cell_rect(super::treegen::atlas_cell(super::species::LeafKind::Small, true));
                        let col = {
                            let t = 0.8 + 0.3 * r(7);
                            rgb(0.2 * t, 0.33 * t, 0.12 * t)
                        };
                        let bse = g.v.len() as u32;
                        for (k2, (cx, cy, u, v)) in [(-0.5f32, -0.5f32, u0.x, u1.y), (0.5, -0.5, u1.x, u1.y), (0.5, 0.5, u1.x, u0.y), (-0.5, 0.5, u0.x, u0.y)].into_iter().enumerate() {
                            let (pp, hh, yy) = corner(cx, cy);
                            let _ = k2;
                            g.push(FV { p: pp, h: hh, dy: yy, n: (face + Vec3::Y * 0.4).normalize(), uv: Vec2::new(u, v), col });
                        }
                        g.i.extend_from_slice(&[bse, bse + 1, bse + 2, bse, bse + 2, bse + 3]);
                    }
                }
            }
            _ => {
                // Posts every ~2.5 m and the infill between them.
                let n = (len / 2.5).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let p = a.lerp(c, k as f32 / n as f32);
                    let g = b.geom(p, mat_post);
                    match kind {
                        BarrierKind::ChainLink => g.cyl(p, 0.03, 0.03, -0.3, h + 0.02, 6, FH::Ground, post_col),
                        BarrierKind::Metal => g.post(p, 0.07, 0.07, yaw, -0.2, h + 0.05, FH::Ground, iron),
                        _ => g.post(p, 0.1, 0.1, yaw, -0.3, h + 0.05, FH::Ground, post_col),
                    }
                }
                for k in 0..n {
                    let p0 = a.lerp(c, k as f32 / n as f32);
                    let p1 = a.lerp(c, (k + 1) as f32 / n as f32);
                    let seg = p0.distance(p1);
                    match kind {
                        BarrierKind::ChainLink => {
                            // The mesh, both faces (the material is double-sided), and the top rail.
                            let g = b.geom(p0, FMat::ChainLink);
                            let nn = Vec3::new(-d.y, 0.0, d.x);
                            g.quad(
                                [(p0, FH::Ground, 0.02), (p1, FH::Ground, 0.02), (p1, FH::Ground, h), (p0, FH::Ground, h)],
                                nn,
                                [Vec2::new(0.0, h), Vec2::new(seg, h), Vec2::new(seg, 0.0), Vec2::new(0.0, 0.0)].map(|v| v / 0.6),
                                WHITE,
                            );
                            b.geom(p0, FMat::Metal).tube((p0, FH::Ground, h), (p1, FH::Ground, h), 0.02, post_col);
                        }
                        BarrierKind::WoodPrivacy => {
                            let g = b.geom(p0, FMat::Wood);
                            g.slab(p0, p1, 0.012, 0.03, h, (FH::Ground, FH::Ground), rgb(0.82, 0.68, 0.5), 1.0);
                        }
                        BarrierKind::WoodPicket => {
                            let g = b.geom(p0, FMat::Wood);
                            let col = if hash01(seed) < 0.6 { rgb(0.95, 0.95, 0.93) } else { rgb(0.72, 0.58, 0.42) };
                            for y in [h * 0.3, h * 0.75] {
                                g.slab(p0, p1, 0.02, y, y + 0.08, (FH::Ground, FH::Ground), col, 1.0);
                            }
                            let m = (seg / 0.16).floor() as usize;
                            for q in 0..m {
                                let pp = p0.lerp(p1, (q as f32 + 0.5) / m as f32) + Vec2::new(-d.y, d.x) * 0.03;
                                g.post(pp, 0.08, 0.02, yaw, 0.05, h, FH::Ground, col);
                            }
                        }
                        BarrierKind::Metal => {
                            let g = b.geom(p0, FMat::Metal);
                            for y in [0.12, h - 0.06] {
                                g.slab(p0, p1, 0.015, y, y + 0.04, (FH::Ground, FH::Ground), iron, 1.0);
                            }
                            let m = (seg / 0.13).floor() as usize;
                            for q in 0..m {
                                let pp = p0.lerp(p1, (q as f32 + 0.5) / m as f32);
                                g.cyl(pp, 0.008, 0.008, 0.1, h, 4, FH::Ground, iron);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn power_line(b: &mut B, pts: &[Vec2], kv: f32, cables: u32, seed: u64) {
    // Tower kind by voltage: lattice steel above 200 kV, steel poles 69-200 kV, wood
    // H-frames below.
    let lattice = kv >= 200.0;
    let tall = if lattice { 38.0 } else if kv >= 69.0 { 26.0 } else { 16.0 };
    let arm = if lattice { 7.5 } else if kv >= 69.0 { 3.2 } else { 2.4 };
    let circuits = (cables / 3).clamp(1, 2);
    let steel = rgb(0.6, 0.61, 0.62);
    let wood = rgb(0.32, 0.25, 0.18);
    // Conductor attachment points per tower (relative: across, height).
    let mut attach: Vec<(f32, f32)> = Vec::new();
    for c in 0..circuits {
        let y = tall - 1.5 - c as f32 * 6.0;
        if lattice {
            attach.extend_from_slice(&[(-arm, y - 2.5), (0.0, y), (arm, y - 2.5)]);
        } else {
            let s = if circuits > 1 { if c == 0 { -1.0 } else { 1.0 } } else { 0.0 };
            if circuits > 1 {
                attach.extend_from_slice(&[(s * arm, y), (s * arm, y - 3.5), (s * arm, y - 7.0)]);
            } else {
                attach.extend_from_slice(&[(-arm, y), (arm, y), (-arm * 0.6, y - 3.0)]);
            }
        }
    }
    let n = pts.len();
    let dir_at = |i: usize| -> Vec2 {
        let a = if i > 0 { pts[i] - pts[i - 1] } else { pts[1] - pts[0] };
        let c = if i + 1 < n { pts[i + 1] - pts[i] } else { pts[i] - pts[i - 1] };
        (a.normalize_or(Vec2::X) + c.normalize_or(Vec2::X)).normalize_or(Vec2::X)
    };
    for i in 0..n {
        let p = pts[i];
        let d = dir_at(i);
        let across = Vec2::new(-d.y, d.x);
        let yaw = d.y.atan2(d.x);
        if lattice {
            // Four legs tapering to a waist, cross-bracing, and the cross arms.
            let g = b.geom(p, FMat::Metal);
            let base = 4.5;
            let top = 1.2;
            let legs = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            let at = |sx: f32, sz: f32, y: f32| {
                let w = base + (top - base) * (y / tall).min(1.0);
                (p + d * sx * w * 0.5 + across * sz * w * 0.5, FH::Ground, y)
            };
            for &(sx, sz) in &legs {
                g.tube(at(sx, sz, -0.3), at(sx, sz, tall), 0.12, steel);
            }
            let levels = 7;
            for k in 0..levels {
                let y0 = tall * k as f32 / levels as f32;
                let y1 = tall * (k + 1) as f32 / levels as f32;
                for q in 0..4 {
                    let (a0, a1) = (legs[q], legs[(q + 1) % 4]);
                    g.tube(at(a0.0, a0.1, y0), at(a1.0, a1.1, y1), 0.05, steel);
                    g.tube(at(a0.0, a0.1, y1), at(a1.0, a1.1, y1), 0.05, steel);
                }
            }
            for &(x, y) in &attach {
                g.tube((p, FH::Ground, y + 0.5), (p + across * x, FH::Ground, y + 0.5), 0.15, steel);
                // Insulator string.
                g.tube((p + across * x, FH::Ground, y + 0.5), (p + across * x, FH::Ground, y - 1.0), 0.12, rgb(0.75, 0.75, 0.72));
            }
            let _ = yaw;
        } else if kv >= 69.0 {
            let g = b.geom(p, FMat::Metal);
            g.cyl(p, 0.45, 0.25, -0.5, tall, 10, FH::Ground, rgb(0.5, 0.48, 0.45));
            for &(x, y) in &attach {
                g.tube((p, FH::Ground, y + 0.3), (p + across * x, FH::Ground, y + 0.3), 0.08, rgb(0.5, 0.48, 0.45));
                g.tube((p + across * x, FH::Ground, y + 0.3), (p + across * x, FH::Ground, y - 0.8), 0.1, rgb(0.75, 0.75, 0.72));
            }
        } else {
            let g = b.geom(p, FMat::Wood);
            for s in [-1.0f32, 1.0] {
                g.cyl(p + across * s * 1.8, 0.18, 0.14, -1.0, tall, 7, FH::Ground, wood);
            }
            g.tube((p - across * (arm + 0.3), FH::Ground, tall - 1.0), (p + across * (arm + 0.3), FH::Ground, tall - 1.0), 0.12, wood);
        }
        let _ = seed;
    }
    // Conductors: catenary sag between neighbouring towers.
    for i in 0..n - 1 {
        let (p0, p1) = (pts[i], pts[i + 1]);
        let (a0, a1) = (Vec2::new(-dir_at(i).y, dir_at(i).x), Vec2::new(-dir_at(i + 1).y, dir_at(i + 1).x));
        let span = p0.distance(p1);
        if span < 1.0 {
            continue;
        }
        let sag = (span * span / 8.0 / 900.0).clamp(0.4, 14.0);
        let mid = (p0 + p1) * 0.5;
        b.reach(mid, p0);
        b.reach(mid, p1);
        for &(x, y) in &attach {
            let insul = if lattice { 1.0 } else if kv >= 69.0 { 0.8 } else { 0.0 };
            let s = 10;
            for k in 0..s {
                let (t0, t1) = (k as f32 / s as f32, (k + 1) as f32 / s as f32);
                let q = |t: f32| {
                    let pa = p0 + a0 * x;
                    let pb = p1 + a1 * x;
                    let gp = pa.lerp(pb, t);
                    // Heights relative to each tower's own ground, blended.
                    (gp, FH::Ground, y - insul - sag * 4.0 * t * (1.0 - t))
                };
                let g = b.geom(mid, FMat::Metal);
                g.tube(q(t0), q(t1), if lattice { 0.035 } else { 0.022 }, rgb(0.2, 0.2, 0.21));
            }
        }
    }
}

fn turbine_tower(b: &mut B, p: Vec2, hub: f32) {
    let g = b.geom(p, FMat::Paint);
    let white = rgb(0.92, 0.92, 0.9);
    // Tapered tube on a concrete foundation.
    g.cyl(p, 2.2 * hub / 80.0, 1.3 * hub / 80.0, -0.5, hub - 2.0, 18, FH::Ground, white);
    g.cyl(p, 3.2 * hub / 80.0, 3.2 * hub / 80.0, -0.5, 0.4, 18, FH::Ground, rgb(0.62, 0.61, 0.58));
    // Door.
    let s = hub / 80.0;
    g.post(p + Vec2::new(2.2 * s, 0.0), 0.05, 0.9, 0.0, 0.4, 2.5, FH::Ground, rgb(0.6, 0.62, 0.65));
}

/// Fixed beacons (piles and stakes marking channels): a post in the water with a
/// coloured daymark board.
fn beacon(b: &mut B, p: Vec2, s: &Seamark) {
    let g = b.geom(p, FMat::Paint);
    let col = s.colours.first().map(|c| rgb(c[0], c[1], c[2])).unwrap_or(rgb(0.75, 0.08, 0.06));
    // Pile (from the bottom up past the water) and the daymark.
    g.cyl(p, 0.18, 0.15, -6.0, 5.0, 8, FH::Surface, rgb(0.4, 0.33, 0.25));
    let tri = s.shape.contains("conical") || s.kind.contains("lateral") && col[0] > 0.5;
    if tri {
        // Red triangle (IALA-B starboard).
        let n = Vec3::Z;
        g.quad(
            [(p + Vec2::new(-0.6, 0.0), FH::Surface, 3.8), (p + Vec2::new(0.6, 0.0), FH::Surface, 3.8), (p + Vec2::new(0.0, 0.0), FH::Surface, 4.9), (p + Vec2::new(-0.0, 0.0), FH::Surface, 4.9)],
            n,
            [Vec2::ZERO; 4],
            col,
        );
    } else {
        g.post(p + Vec2::new(0.0, 0.2), 1.2, 0.05, 0.0, 3.8, 4.9, FH::Surface, col);
    }
}

/// A pier: a deck on piles, level with its landward end, with railings.
fn pier(b: &mut B, pts: &[Vec2], area: bool, width: f32, kind: &str, seed: u64) {
    if area {
        // Mapped as an outline: a deck over the whole shape (triangulated as a fan).
        if pts.len() < 3 {
            return;
        }
        let c = pts.iter().copied().sum::<Vec2>() / pts.len() as f32;
        let level = FH::GroundAt(pts[0]);
        let g = b.geom(c, FMat::Wood);
        let flat: Vec<f32> = pts.iter().flat_map(|p| [p.x, p.y]).collect();
        let Ok(tris) = earcutr::earcut(&flat, &[], 2) else { return };
        let base = g.v.len() as u32;
        for p in pts {
            g.push(FV { p: *p, h: level, dy: 0.6, n: Vec3::Y, uv: *p / 2.0, col: rgb(0.62, 0.52, 0.4) });
        }
        for t in tris.chunks_exact(3) {
            g.i.extend_from_slice(&[base + t[0] as u32, base + t[2] as u32, base + t[1] as u32]);
        }
        return;
    }
    let breakwater = kind != "pier";
    let w = width.clamp(1.5, 30.0);
    let level = FH::GroundAt(pts[0]);
    for win in pts.windows(2) {
        let (a, c) = (win[0], win[1]);
        let len = a.distance(c);
        if len < 0.1 {
            continue;
        }
        let d = (c - a) / len;
        let n = Vec2::new(-d.y, d.x);
        if breakwater {
            // Rock rubble mound: a sloped grey bank (rocks as boxes).
            let g = b.geom(a, FMat::Paint);
            let k = (len / 2.0).ceil() as usize;
            for q in 0..k {
                let p = a.lerp(c, (q as f32 + 0.5) / k as f32);
                for s in [-0.3f32, 0.0, 0.3] {
                    let r = hash01(seed ^ (q * 7) as u64 ^ (s.to_bits() as u64));
                    g.post(p + n * s * w, 1.6 + r, 1.4, r * 3.0, -3.0, 1.0 + r * 0.8, FH::Surface, rgb(0.45 + r * 0.1, 0.44 + r * 0.1, 0.42 + r * 0.1));
                }
            }
            continue;
        }
        // Deck boards.
        let g = b.geom(a, FMat::Wood);
        g.quad(
            [(a - n * w * 0.5, level, 0.6), (c - n * w * 0.5, level, 0.6), (c + n * w * 0.5, level, 0.6), (a + n * w * 0.5, level, 0.6)],
            Vec3::Y,
            [Vec2::new(0.0, 0.0), Vec2::new(len, 0.0), Vec2::new(len, w), Vec2::new(0.0, w)].map(|v| v / 2.0),
            rgb(0.62, 0.52, 0.4),
        );
        // Deck edge beams.
        for s in [-0.5f32, 0.5] {
            g.slab(a + n * w * s, c + n * w * s, 0.08, 0.3, 0.6, (level, level), rgb(0.45, 0.36, 0.27), 1.0);
        }
        // Railings.
        for s in [-0.5f32, 0.5] {
            g.slab(a + n * (w * s - s.signum() * 0.05), c + n * (w * s - s.signum() * 0.05), 0.04, 1.6, 1.68, (level, level), rgb(0.55, 0.44, 0.32), 1.0);
        }
        // Piles every 4 m, down to the bottom.
        let k = (len / 4.0).ceil() as usize;
        for q in 0..=k {
            let p = a.lerp(c, q as f32 / k as f32);
            for s in [-0.45f32, 0.45] {
                let pp = p + n * w * s;
                let g = b.geom(pp, FMat::Wood);
                g.cyl(pp, 0.16, 0.16, -8.0, 0.35, 7, level, rgb(0.38, 0.3, 0.22));
                g.cyl(pp, 0.05, 0.05, 0.6, 1.66, 5, level, rgb(0.55, 0.44, 0.32));
            }
        }
    }
}

/// A beach lifeguard tower: a hut on legs with a ramp, painted bright.
fn lifeguard(b: &mut B, p: Vec2, yaw: f32, r: f32) {
    let col = [rgb(0.95, 0.75, 0.2), rgb(0.95, 0.45, 0.6), rgb(0.35, 0.75, 0.85), rgb(0.95, 0.95, 0.92), rgb(0.55, 0.85, 0.4)][(r * 5.0) as usize % 5];
    let ax = Vec2::new(yaw.cos(), yaw.sin());
    let fw = Vec2::new(-ax.y, ax.x);
    let g = b.geom(p, FMat::Paint);
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        g.post(p + ax * sx * 1.1 + fw * sz * 1.1, 0.15, 0.15, yaw, -0.5, 2.0, FH::Ground, rgb(0.85, 0.85, 0.82));
    }
    // Floor, cabin, roof.
    g.post(p, 3.2, 3.2, yaw, 2.0, 2.15, FH::Ground, rgb(0.8, 0.78, 0.74));
    g.post(p, 2.6, 2.4, yaw, 2.15, 4.2, FH::Ground, col);
    g.post(p, 3.4, 3.2, yaw, 4.2, 4.4, FH::Ground, rgb(0.9, 0.9, 0.88));
    // Windows (dark) on the sea side and the ramp at the back.
    g.post(p + fw * 1.21, 2.0, 0.02, yaw, 2.9, 3.8, FH::Ground, rgb(0.1, 0.12, 0.15));
    let back = p - fw * 1.6;
    let g = b.geom(back, FMat::Wood);
    let foot = p - fw * 5.0;
    g.slab(back, foot, 0.5, 0.0, 0.08, (FH::Ground, FH::Ground), rgb(0.7, 0.62, 0.5), 1.0);
    let _ = g;
    // Lift the ramp's top end to the floor: drawn as a sloped quad.
    let g = b.geom(back, FMat::Wood);
    let n = Vec3::new(fw.x, 0.6, fw.y).normalize();
    g.quad(
        [(foot - ax * 0.5, FH::Ground, 0.1), (foot + ax * 0.5, FH::Ground, 0.1), (back + ax * 0.5, FH::Ground, 2.1), (back - ax * 0.5, FH::Ground, 2.1)],
        n,
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        rgb(0.7, 0.62, 0.5),
    );
}

fn furniture(b: &mut B, p: Vec2, kind: FurnitureKind, height: Option<f32>, seed: u64) {
    let yaw = hash01(seed) * 6.28;
    use FurnitureKind as K;
    match kind {
        K::WasteBasket => {
            let g = b.geom(p, FMat::Metal);
            g.cyl(p, 0.28, 0.3, 0.0, 0.95, 10, FH::Ground, rgb(0.15, 0.25, 0.18));
        }
        K::Recycling => {
            let g = b.geom(p, FMat::Paint);
            for k in 0..3 {
                let c = [rgb(0.1, 0.35, 0.7), rgb(0.15, 0.5, 0.2), rgb(0.6, 0.6, 0.6)][k];
                g.post(p + Vec2::new(yaw.cos(), yaw.sin()) * (k as f32 - 1.0) * 1.3, 1.2, 1.0, yaw, 0.0, 1.4, FH::Ground, c);
            }
        }
        K::BikeRack => {
            let g = b.geom(p, FMat::Metal);
            let ax = Vec2::new(yaw.cos(), yaw.sin());
            for k in 0..4 {
                let c = p + ax * (k as f32 - 1.5) * 0.8;
                let n = Vec2::new(-ax.y, ax.x);
                // Inverted U.
                g.tube((c - n * 0.35, FH::Ground, 0.0), (c - n * 0.35, FH::Ground, 0.75), 0.025, rgb(0.6, 0.61, 0.62));
                g.tube((c + n * 0.35, FH::Ground, 0.0), (c + n * 0.35, FH::Ground, 0.75), 0.025, rgb(0.6, 0.61, 0.62));
                g.tube((c - n * 0.35, FH::Ground, 0.75), (c + n * 0.35, FH::Ground, 0.75), 0.025, rgb(0.6, 0.61, 0.62));
            }
        }
        K::DrinkingWater => {
            let g = b.geom(p, FMat::Metal);
            g.cyl(p, 0.18, 0.15, 0.0, 0.85, 10, FH::Ground, rgb(0.3, 0.35, 0.4));
            g.cyl(p, 0.25, 0.25, 0.85, 0.95, 10, FH::Ground, rgb(0.7, 0.72, 0.74));
        }
        K::Phone => {
            let g = b.geom(p, FMat::Metal);
            g.post(p, 0.08, 0.08, yaw, 0.0, 1.3, FH::Ground, rgb(0.5, 0.5, 0.52));
            g.post(p, 0.7, 0.5, yaw, 1.3, 2.1, FH::Ground, rgb(0.62, 0.64, 0.66));
        }
        K::Vending => {
            let g = b.geom(p, FMat::Paint);
            g.post(p, 1.0, 0.8, yaw, 0.0, 1.85, FH::Ground, [rgb(0.75, 0.1, 0.1), rgb(0.1, 0.3, 0.7), rgb(0.2, 0.2, 0.22)][(hash01(seed ^ 3) * 3.0) as usize % 3]);
        }
        K::Shelter => {
            let g = b.geom(p, FMat::Wood);
            let ax = Vec2::new(yaw.cos(), yaw.sin());
            let n = Vec2::new(-ax.y, ax.x);
            for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                g.post(p + ax * sx * 2.5 + n * sz * 2.0, 0.2, 0.2, yaw, 0.0, 2.6, FH::Ground, rgb(0.5, 0.38, 0.26));
            }
            g.post(p, 6.0, 5.0, yaw, 2.6, 2.8, FH::Ground, rgb(0.42, 0.3, 0.2));
            g.post(p, 2.0, 0.9, yaw, 0.7, 0.78, FH::Ground, rgb(0.55, 0.42, 0.3));
        }
        K::PicnicTable => {
            let g = b.geom(p, FMat::Wood);
            let col = rgb(0.6, 0.47, 0.33);
            g.post(p, 1.8, 0.75, yaw, 0.72, 0.78, FH::Ground, col);
            let n = Vec2::new(-yaw.sin(), yaw.cos());
            for s in [-1.0f32, 1.0] {
                g.post(p + n * s * 0.6, 1.8, 0.28, yaw, 0.42, 0.47, FH::Ground, col);
            }
            for s in [-0.7f32, 0.7] {
                g.post(p + Vec2::new(yaw.cos(), yaw.sin()) * s, 0.08, 1.6, yaw, 0.0, 0.72, FH::Ground, col);
            }
        }
        K::Grill | K::Firepit => {
            let g = b.geom(p, FMat::Metal);
            if kind == K::Grill {
                g.cyl(p, 0.05, 0.05, 0.0, 0.8, 6, FH::Ground, rgb(0.2, 0.2, 0.2));
                g.post(p, 0.6, 0.4, yaw, 0.8, 1.0, FH::Ground, rgb(0.15, 0.15, 0.15));
            } else {
                g.cyl(p, 0.6, 0.6, 0.0, 0.3, 12, FH::Ground, rgb(0.25, 0.24, 0.23));
            }
        }
        K::Fountain => {
            let g = b.geom(p, FMat::Paint);
            g.cyl(p, 2.5, 2.5, 0.0, 0.5, 20, FH::Ground, rgb(0.7, 0.69, 0.66));
            g.cyl(p, 0.3, 0.2, 0.5, 1.6, 10, FH::Ground, rgb(0.7, 0.69, 0.66));
            g.cyl(p, 1.0, 1.0, 1.4, 1.55, 14, FH::Ground, rgb(0.7, 0.69, 0.66));
        }
        K::Flagpole => {
            let h = height.unwrap_or(9.0).clamp(4.0, 40.0);
            let g = b.geom(p, FMat::Metal);
            g.cyl(p, 0.08, 0.04, 0.0, h, 8, FH::Ground, rgb(0.85, 0.86, 0.88));
            g.cyl(p, 0.09, 0.09, h, h + 0.15, 8, FH::Ground, rgb(0.85, 0.75, 0.3));
        }
        K::WaterTower => {
            let h = height.unwrap_or(40.0).clamp(15.0, 70.0);
            let g = b.geom(p, FMat::Paint);
            let col = rgb(0.82, 0.84, 0.86);
            g.cyl(p, 1.6, 1.4, 0.0, h * 0.72, 14, FH::Ground, col);
            // The tank: a squashed ball approximated by two cones.
            g.cyl(p, 1.4, 8.0, h * 0.72, h * 0.84, 20, FH::Ground, col);
            g.cyl(p, 8.0, 0.5, h * 0.84, h, 20, FH::Ground, col);
        }
        K::Mast => {
            let h = height.unwrap_or(50.0).clamp(10.0, 300.0);
            let g = b.geom(p, FMat::Metal);
            for k in 0..3 {
                let a = k as f32 * 2.094 + yaw;
                let o = Vec2::new(a.cos(), a.sin()) * 0.6;
                g.tube((p + o, FH::Ground, 0.0), (p + o * 0.3, FH::Ground, h), 0.06, rgb(0.85, 0.3, 0.2));
            }
        }
        K::Lighthouse => {
            let h = height.unwrap_or(25.0).clamp(8.0, 60.0);
            let g = b.geom(p, FMat::Paint);
            let bands = 4;
            for k in 0..bands {
                let y0 = h * 0.85 * k as f32 / bands as f32;
                let y1 = h * 0.85 * (k + 1) as f32 / bands as f32;
                let r0 = 3.0 - 1.2 * (y0 / h);
                let r1 = 3.0 - 1.2 * (y1 / h);
                let c = if k % 2 == 0 { rgb(0.92, 0.92, 0.9) } else { rgb(0.75, 0.1, 0.08) };
                g.cyl(p, r0, r1, y0, y1, 16, FH::Ground, c);
            }
            g.cyl(p, 2.2, 2.2, h * 0.85, h * 0.87, 16, FH::Ground, rgb(0.15, 0.15, 0.15));
            b.geom(p, FMat::Lamp).cyl(p, 1.3, 1.3, h * 0.87, h * 0.97, 12, FH::Ground, rgb(1.0, 0.95, 0.8));
            b.geom(p, FMat::Paint).cyl(p, 1.6, 0.2, h * 0.97, h * 1.05, 12, FH::Ground, rgb(0.15, 0.15, 0.15));
        }
        K::Windmill => {
            let g = b.geom(p, FMat::Metal);
            // An American farm windpump: lattice tower and a fan wheel.
            for k in 0..4 {
                let a = k as f32 * 1.571 + yaw;
                let o = Vec2::new(a.cos(), a.sin()) * 1.6;
                g.tube((p + o, FH::Ground, 0.0), (p + o * 0.15, FH::Ground, 10.0), 0.05, rgb(0.6, 0.6, 0.6));
            }
            g.cyl(p, 1.4, 1.4, 9.6, 9.75, 14, FH::Ground, rgb(0.55, 0.55, 0.55));
        }
        K::Lifeguard => lifeguard(b, p, yaw, hash01(seed ^ 9)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::nature_osm::*;

    #[test]
    fn builds_features() {
        let tile = TileId::containing(25.78, -80.13, 15).0;
        let (la, lo) = tile.lat_lon_at(0.5, 0.5);
        let mut nat = NatureData::default();
        let line = vec![(la, lo), (la + 0.001, lo), (la + 0.001, lo + 0.001)];
        for k in [BarrierKind::ChainLink, BarrierKind::WoodPicket, BarrierKind::WoodPrivacy, BarrierKind::Metal, BarrierKind::Wall, BarrierKind::Hedge] {
            nat.barriers.push(Barrier { kind: k, pts: line.clone(), height: 1.8, id: 1 });
        }
        nat.power.push(PowerLine { pts: line.clone(), nodes: vec![1, 2, 3], kv: 230.0, cables: 6, id: 2 });
        nat.power.push(PowerLine { pts: line.clone(), nodes: vec![1, 2, 3], kv: 115.0, cables: 3, id: 3 });
        nat.turbines.push(Turbine { lat: la, lon: lo, hub: 80.0, rotor: 100.0, id: 4 });
        nat.piers.push(Pier { pts: line.clone(), area: false, width: 5.0, kind: "pier".into(), id: 5 });
        for k in [FurnitureKind::WasteBasket, FurnitureKind::Lighthouse, FurnitureKind::WaterTower, FurnitureKind::Lifeguard] {
            nat.furniture.push(Furniture { kind: k, lat: la, lon: lo, id: 6, height: None });
        }
        let b = build(tile, &nat);
        let total: usize = b.chunks.iter().flat_map(|c| c.geoms.values()).map(|g| g.i.len() / 3).sum();
        assert!(total > 1000, "{total}");
        for c in &b.chunks {
            for g in c.geoms.values() {
                assert!(g.i.iter().all(|&i| (i as usize) < g.v.len()));
                assert!(g.v.iter().all(|v| v.p.is_finite() && v.dy.is_finite()));
            }
        }
        assert_eq!(b.chunks.iter().map(|c| c.objs.len()).sum::<usize>(), 1);
    }
}
