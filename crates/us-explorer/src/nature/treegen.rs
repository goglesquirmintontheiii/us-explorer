//! Procedural trees: a branching skeleton grown for each species (broadleaf crowns,
//! conifer whorls, palms, cacti, Joshua trees, shrubs), turned into bark-textured tubes
//! and leaf cards. Each species comes in a few shapes and four growth stages.
//!
//! Units are metres, the trunk base at the origin, y up. `uv_b` carries wind sway
//! (x: how far this vertex moves, y: its phase) for the vegetation shader.

use super::species::{Bark, Def, Form, LeafKind, Season, Sp, def, leaf_colour};
use bevy::math::{Quat, Vec2, Vec3};

/// Growth stage (sapling .. old giant).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Sapling,
    Young,
    Mature,
    Old,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Sapling, Stage::Young, Stage::Mature, Stage::Old];
    /// Height relative to the species' mature height.
    pub fn scale(self) -> f32 {
        match self {
            Stage::Sapling => 0.22,
            Stage::Young => 0.5,
            Stage::Mature => 1.0,
            Stage::Old => 1.25,
        }
    }
    /// Branching depth.
    fn depth(self) -> u32 {
        match self {
            Stage::Sapling => 2,
            Stage::Young => 3,
            Stage::Mature | Stage::Old => 3,
        }
    }
}

/// Detail level of a generated tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Detail {
    /// Every branch level and small twig cards (close up).
    Full,
    /// Main branches and big leaf clusters (middle distance).
    Low,
}

#[derive(Default, Clone)]
pub struct Buf {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub uv_b: Vec<[f32; 2]>,
    pub col: Vec<[f32; 4]>,
    pub idx: Vec<u32>,
}

impl Buf {
    fn vert(&mut self, p: Vec3, n: Vec3, uv: Vec2, sway: Vec2, c: [f32; 4]) -> u32 {
        self.pos.push(p.to_array());
        self.nrm.push(n.normalize_or(Vec3::Y).to_array());
        self.uv.push(uv.to_array());
        self.uv_b.push(sway.to_array());
        self.col.push(c);
        (self.pos.len() - 1) as u32
    }
    pub fn tris(&self) -> usize {
        self.idx.len() / 3
    }
}

/// One generated tree.
#[derive(Default, Clone)]
pub struct TreeMesh {
    pub wood: Buf,
    pub leaves: Buf,
    pub height: f32,
    /// Crown radius (for spacing and the impostor's size).
    pub radius: f32,
}

/// Leaf atlas: 8 x 4 cells of 256 px. Each leaf kind has a twig card (close) and a
/// cluster card (far); a few extra cells for fronds and spikes.
pub const ATLAS_COLS: u32 = 8;
pub const ATLAS_ROWS: u32 = 4;

pub fn atlas_cell(k: LeafKind, cluster: bool) -> u32 {
    let base = match k {
        LeafKind::Ovate => 0,
        LeafKind::Palmate => 1,
        LeafKind::Oak => 2,
        LeafKind::Small => 3,
        LeafKind::Needle => 4,
        LeafKind::ShortNeedle => 5,
        LeafKind::Scale => 6,
        LeafKind::Lance => 7,
        LeafKind::Pinnate => 16,
        LeafKind::Fan => 17,
        LeafKind::Spike => 18,
    };
    if base < 8 && cluster { base + 8 } else { base }
}

/// Dead palm fronds (Washingtonia skirts) use this cell.
pub const CELL_DEAD_FAN: u32 = 19;

pub fn cell_rect(cell: u32) -> (Vec2, Vec2) {
    let (cx, cy) = (cell % ATLAS_COLS, cell / ATLAS_COLS);
    let (w, h) = (1.0 / ATLAS_COLS as f32, 1.0 / ATLAS_ROWS as f32);
    let pad = 1.5 / 2048.0;
    (Vec2::new(cx as f32 * w + pad, cy as f32 * h + pad), Vec2::new((cx + 1) as f32 * w - pad, (cy + 1) as f32 * h - pad))
}

/// Small deterministic random numbers.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
    fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

fn any_perp(d: Vec3) -> Vec3 {
    if d.y.abs() < 0.95 { d.cross(Vec3::Y).normalize() } else { d.cross(Vec3::X).normalize() }
}

/// A branch to be built: a polyline with a radius at each point.
struct Limb {
    pts: Vec<Vec3>,
    radius: Vec<f32>,
    depth: u32,
    /// Sway at the start of the limb (grows towards the tips).
    sway0: f32,
    phase: f32,
}

struct Ctx<'a> {
    df: &'a Def,
    rng: Rng,
    wood: Buf,
    leaves: Buf,
    bark_col: [f32; 4],
    leaf_kind: LeafKind,
    detail: Detail,
    season: Season,
    /// Leaf colour and how much foliage there is.
    leaf_col: [f32; 3],
    leaf_amount: f32,
    fall_col: [f32; 3],
    height: f32,
    /// Crown centre and radii (for card normals).
    crown_c: Vec3,
    crown_r: Vec3,
}

impl Ctx<'_> {
    /// A tube along a limb. `u` wraps the circumference (bark texture 1 m repeat), `v`
    /// runs along it.
    fn tube(&mut self, l: &Limb, sides: u32, flare: bool) {
        let n = l.pts.len();
        if n < 2 {
            return;
        }
        let mut len = 0.0f32;
        let mut prev_side = any_perp(l.pts[1] - l.pts[0]);
        let mut rings: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let d = if i + 1 < n { l.pts[i + 1] - l.pts[i] } else { l.pts[i] - l.pts[i - 1] }.normalize_or(Vec3::Y);
            if i > 0 {
                len += l.pts[i].distance(l.pts[i - 1]);
            }
            // Parallel-transport the side vector so the tube doesn't twist.
            let side = (prev_side - d * prev_side.dot(d)).normalize_or(any_perp(d));
            prev_side = side;
            let other = d.cross(side);
            let r = l.radius[i];
            let circ = (r * std::f32::consts::TAU).max(0.02);
            // Bark repeats about every 0.6 m, a whole number of times round the limb.
            let tiles = (circ / 0.6).round().max(1.0);
            let sway = Vec2::new(l.sway0 + self.sway_at(l.pts[i]), l.phase);
            let base = self.wood.pos.len() as u32;
            for k in 0..=sides {
                let a = k as f32 / sides as f32 * std::f32::consts::TAU;
                let mut rr = r;
                // Root flare and buttresses at the foot of the trunk.
                if flare && l.pts[i].y < r * 4.0 {
                    let f = 1.0 - (l.pts[i].y / (r * 4.0)).clamp(0.0, 1.0);
                    rr *= 1.0 + f * f * (0.6 + 0.35 * (a * 5.0).sin().max(0.0));
                }
                let dir = side * a.cos() + other * a.sin();
                let p = l.pts[i] + dir * rr;
                let uv = Vec2::new(k as f32 / sides as f32 * tiles, len / 0.6);
                self.wood.vert(p, dir, uv, sway, self.bark_col);
            }
            rings.push(base);
        }
        for i in 0..n - 1 {
            let (a, b) = (rings[i], rings[i + 1]);
            for k in 0..sides {
                self.wood.idx.extend_from_slice(&[a + k, b + k, b + k + 1, a + k, b + k + 1, a + k + 1]);
            }
        }
        // Cap the tip.
        let last = rings[n - 1];
        let tip = l.pts[n - 1] + (l.pts[n - 1] - l.pts[n - 2]).normalize_or(Vec3::Y) * l.radius[n - 1];
        let sway = Vec2::new(self.sway_at(tip), l.phase);
        let c = self.wood.vert(tip, (l.pts[n - 1] - l.pts[n - 2]).normalize_or(Vec3::Y), Vec2::ZERO, sway, self.bark_col);
        for k in 0..sides {
            self.wood.idx.extend_from_slice(&[last + k, c, last + k + 1]);
        }
    }

    /// How far a point sways: nothing at the foot, more up in the crown and out at the
    /// branch tips.
    fn sway_at(&self, p: Vec3) -> f32 {
        let h = (p.y / self.height.max(1.0)).clamp(0.0, 1.2);
        let out = Vec2::new(p.x, p.z).length() / (self.crown_r.x.max(0.5));
        (h * h * 0.6 + out * 0.35).min(1.4)
    }

    /// A leaf card: a quad of `size` centred at `c`, facing roughly `facing`, folded a
    /// little along its middle so it has depth. Colour varies card by card (fall
    /// colour comes in patchily).
    fn card(&mut self, c: Vec3, facing: Vec3, up: Vec3, size: Vec2, cell: u32, phase: f32, fold: f32) {
        let (uv0, uv1) = cell_rect(cell);
        let n = facing.normalize_or(Vec3::Y);
        let right = up.cross(n).normalize_or(any_perp(n));
        let upv = n.cross(right).normalize();
        let hw = right * size.x * 0.5;
        let hh = upv * size.y * 0.5;
        // Normals bend out from the crown centre so the crown shades as one mass.
        let out = ((c - self.crown_c) / self.crown_r.max(Vec3::splat(0.3))).normalize_or(Vec3::Y);
        let nn = (n * 0.35 + out * 0.65 + Vec3::Y * 0.15).normalize();
        let r = self.rng.next();
        let fall_mix = (self.season.fall * 1.4 - r * 0.6).clamp(0.0, 1.0);
        let shade = 0.82 + 0.3 * self.rng.next();
        let mut col = [0.0; 3];
        for i in 0..3 {
            col[i] = (self.leaf_col[i] + (self.fall_col[i] - self.leaf_col[i]) * fall_mix) * shade;
        }
        let colour = [col[0], col[1], col[2], 1.0];
        let sw = self.sway_at(c) + 0.15;
        let sway = Vec2::new(sw, phase);
        // Midline points (folded towards the normal by `fold`).
        let mid_off = n * size.x * fold * 0.25;
        let b = self.leaves.pos.len() as u32;
        self.leaves.vert(c - hh - hw, nn, Vec2::new(uv0.x, uv1.y), sway, colour);
        self.leaves.vert(c - hh + mid_off, nn, Vec2::new((uv0.x + uv1.x) * 0.5, uv1.y), sway, colour);
        self.leaves.vert(c - hh + hw, nn, Vec2::new(uv1.x, uv1.y), sway, colour);
        self.leaves.vert(c + hh - hw, nn, Vec2::new(uv0.x, uv0.y), sway, colour);
        self.leaves.vert(c + hh + mid_off, nn, Vec2::new((uv0.x + uv1.x) * 0.5, uv0.y), sway, colour);
        self.leaves.vert(c + hh + hw, nn, Vec2::new(uv1.x, uv0.y), sway, colour);
        self.leaves.idx.extend_from_slice(&[b, b + 1, b + 4, b, b + 4, b + 3, b + 1, b + 2, b + 5, b + 1, b + 5, b + 4]);
    }

    /// A crossed pair of cards (looks full from every side).
    fn cross_card(&mut self, c: Vec3, dir: Vec3, size: Vec2, cell: u32, phase: f32) {
        let d = dir.normalize_or(Vec3::Y);
        let side = any_perp(d);
        let a = self.rng.range(0.0, std::f32::consts::PI);
        let n1 = Quat::from_axis_angle(d, a) * side;
        let n2 = Quat::from_axis_angle(d, a + std::f32::consts::FRAC_PI_2) * side;
        self.card(c, n1, d, size, cell, phase, 0.3);
        self.card(c, n2, d, size, cell, phase, 0.3);
    }

    /// Foliage along the outer part of a terminal branch.
    fn foliage(&mut self, l: &Limb, per_m: f32, size: f32) {
        if self.leaf_amount <= 0.01 {
            return;
        }
        let total: f32 = l.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let n = ((total * per_m * self.df.density * self.leaf_amount).round() as usize).max(1);
        let cell = atlas_cell(self.leaf_kind, self.detail == Detail::Low);
        for _ in 0..n {
            // Along the twig, thickest towards the tip.
            let t = (1.0 - self.rng.next() * self.rng.next()).min(1.0) * 0.9 + 0.1;
            let p = point_on(&l.pts, t);
            let d = dir_on(&l.pts, t);
            let jitter = Vec3::new(self.rng.signed(), self.rng.signed() * 0.7, self.rng.signed()) * size * 0.7;
            let c = p + jitter;
            let out = (c - self.crown_c).normalize_or(Vec3::Y);
            let facing = (out + Vec3::new(self.rng.signed(), self.rng.signed(), self.rng.signed()) * 0.7).normalize_or(Vec3::Y);
            let up = (d + Vec3::Y * 0.3).normalize();
            let s = size * self.rng.range(0.8, 1.25);
            let ph = l.phase + self.rng.next() * 0.5;
            if self.detail == Detail::Full {
                self.card(c, facing, up, Vec2::new(s, s), cell, ph, 0.35);
            } else {
                self.cross_card(c, up, Vec2::new(s, s), cell, ph);
            }
        }
    }
}

fn point_on(pts: &[Vec3], t: f32) -> Vec3 {
    let f = t.clamp(0.0, 1.0) * (pts.len() - 1) as f32;
    let i = (f as usize).min(pts.len() - 2);
    pts[i].lerp(pts[i + 1], f - i as f32)
}

fn dir_on(pts: &[Vec3], t: f32) -> Vec3 {
    let f = t.clamp(0.0, 1.0) * (pts.len() - 1) as f32;
    let i = (f as usize).min(pts.len() - 2);
    (pts[i + 1] - pts[i]).normalize_or(Vec3::Y)
}

/// Grow a curved limb from `start` along `dir` for `len` metres: it bends towards
/// `bend` (up for most trees, down for weeping ones) and wanders a little.
fn grow(rng: &mut Rng, start: Vec3, dir: Vec3, len: f32, r0: f32, r1: f32, segs: usize, bend: Vec3, wander: f32) -> (Vec<Vec3>, Vec<f32>) {
    let mut pts = vec![start];
    let mut radius = vec![r0];
    let mut d = dir.normalize_or(Vec3::Y);
    let step = len / segs as f32;
    let mut p = start;
    for i in 1..=segs {
        let t = i as f32 / segs as f32;
        d = (d + bend * (step / len.max(0.1)) * 1.6 + Vec3::new(rng.signed(), rng.signed() * 0.5, rng.signed()) * wander * 0.25).normalize_or(d);
        p += d * step;
        pts.push(p);
        radius.push(r0 + (r1 - r0) * t);
    }
    (pts, radius)
}

/// Where a ray from `p` along `d` leaves the crown ellipsoid (distance, at least 0.3).
fn to_crown_edge(p: Vec3, d: Vec3, c: Vec3, r: Vec3) -> f32 {
    let o = (p - c) / r;
    let dd = d / r;
    let a = dd.dot(dd);
    let b = 2.0 * o.dot(dd);
    let cc = o.dot(o) - 1.0;
    let disc = b * b - 4.0 * a * cc;
    if disc < 0.0 {
        return 0.3;
    }
    ((-b + disc.sqrt()) / (2.0 * a)).max(0.3)
}

/// Build one tree of a species: `variant` picks the shape, `stage` the size.
pub fn generate(sp: Sp, variant: u32, stage: Stage, detail: Detail, season: &Season) -> TreeMesh {
    let df = def(sp);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ ((sp as u64) << 32) ^ ((variant as u64) << 8) ^ stage as u64);
    for _ in 0..3 {
        rng.next();
    }
    let height = df.height * stage.scale() * rng.range(0.85, 1.12);
    let var = rng.next();
    let (leaf_col, amount) = leaf_colour(&df, season, var);
    let crown_h = height * (1.0 - df.clear);
    let crown_c = Vec3::new(0.0, height - crown_h * 0.5, 0.0);
    let crown_r = Vec3::new(df.width * height * 0.5, crown_h * 0.5, df.width * height * 0.5);
    let b = df.bark_col;
    let mut cx = Ctx {
        df: &df,
        rng,
        wood: Buf::default(),
        leaves: Buf::default(),
        bark_col: [b[0], b[1], b[2], 1.0],
        leaf_kind: df.leaf,
        detail,
        season: *season,
        leaf_col,
        leaf_amount: amount,
        fall_col: if df.evergreen { leaf_col } else { df.fall },
        height,
        crown_c,
        crown_r,
    };
    if df.evergreen {
        cx.season.fall = 0.0;
    }
    match df.form {
        Form::Broadleaf | Form::Weeping => broadleaf(&mut cx, stage),
        Form::Conifer => conifer(&mut cx, stage, sp),
        Form::Palm | Form::FanPalm => palm(&mut cx, stage, sp),
        Form::Cactus => cactus(&mut cx, stage),
        Form::Joshua => joshua(&mut cx, stage),
        Form::Shrub => shrub(&mut cx, stage, sp),
    }
    let radius = cx.crown_r.x.max(cx.crown_r.z).max(0.5);
    TreeMesh { wood: cx.wood, leaves: cx.leaves, height, radius }
}

fn broadleaf(cx: &mut Ctx, stage: Stage) {
    let df = cx.df;
    let h = cx.height;
    let weeping = df.form == Form::Weeping;
    let r0 = (h * 0.024 * if stage == Stage::Old { 1.35 } else { 1.0 }).max(0.03) * (0.9 + 0.2 * df.width.min(1.5) / 1.5);
    let clear = h * df.clear;
    // Trunk: up through the crown base, leaning a little; wide crowns fork lower.
    let lean = Vec3::new(cx.rng.signed(), 0.0, cx.rng.signed()) * 0.06;
    let trunk_len = clear + (h - clear) * if df.width > 1.1 { 0.25 } else { 0.55 };
    let (tp, tr) = grow(&mut cx.rng, Vec3::ZERO, (Vec3::Y + lean).normalize(), trunk_len, r0, r0 * 0.62, 8, Vec3::Y * 0.1, 0.25);
    let trunk = Limb { pts: tp.clone(), radius: tr.clone(), depth: 0, sway0: 0.0, phase: cx.rng.next() * 6.0 };
    cx.tube(&trunk, if cx.detail == Detail::Full { 10 } else { 7 }, true);
    // Leaders and main branches.
    let n_main = match stage {
        Stage::Sapling => 4,
        Stage::Young => 6,
        _ => 8 + (cx.rng.next() * 4.0) as usize,
    };
    let max_depth = if cx.detail == Detail::Full { stage.depth() } else { 1 };
    let golden = 2.399_963;
    let az0 = cx.rng.next() * 6.28;
    let mut terminals: Vec<Limb> = Vec::new();
    for i in 0..n_main {
        let t = 0.35 + 0.65 * (i as f32 + cx.rng.next() * 0.5) / n_main as f32;
        let at = point_on(&tp, t.min(1.0));
        let r_at = tr[0] + (tr[tr.len() - 1] - tr[0]) * t;
        let az = az0 + i as f32 * golden;
        // Angle from vertical: upright species climb steeply, spreading ones go out.
        let tilt = (1.25 - df.upright * 0.85 + cx.rng.signed() * 0.15).clamp(0.25, 1.45);
        let dir = Vec3::new(az.cos() * tilt.sin(), tilt.cos(), az.sin() * tilt.sin()).normalize();
        let len = to_crown_edge(at, dir, cx.crown_c, cx.crown_r) * cx.rng.range(0.82, 1.0);
        let bend = if weeping { Vec3::NEG_Y * 0.4 } else { Vec3::Y * (0.25 + df.upright * 0.2) };
        let (pts, rad) = grow(&mut cx.rng, at, dir, len, r_at * 0.7, r_at * 0.12, 6, bend, 0.35);
        let limb = Limb { pts, radius: rad, depth: 1, sway0: 0.0, phase: cx.rng.next() * 6.0 };
        branch(cx, limb, max_depth, &mut terminals);
    }
    // The leader continues to the top.
    let top_dir = (dir_on(&tp, 1.0) + Vec3::Y).normalize();
    let len = (h - tp[tp.len() - 1].y).max(0.5);
    let (pts, rad) = grow(&mut cx.rng, tp[tp.len() - 1], top_dir, len, tr[tr.len() - 1], 0.02, 5, Vec3::Y * 0.2, 0.3);
    let leader = Limb { pts, radius: rad, depth: 1, sway0: 0.0, phase: cx.rng.next() * 6.0 };
    branch(cx, leader, max_depth, &mut terminals);
    // Leaves.
    let (per_m, size) = match (cx.detail, cx.leaf_kind) {
        (Detail::Full, LeafKind::Small) => (14.0, 0.6),
        (Detail::Full, LeafKind::Lance) => (10.0, 0.8),
        (Detail::Full, _) => (11.0, 0.75),
        (Detail::Low, _) => (2.2, 1.9),
    };
    let scale = (h / 15.0).clamp(0.35, 1.4).sqrt();
    for t in &terminals {
        if weeping {
            // Long hanging strands from the branch.
            for k in 0..((4.0 * df.density) as usize).max(2) {
                let p = point_on(&t.pts, 0.4 + 0.6 * k as f32 / 4.0);
                let hang = (p.y * cx.rng.range(0.5, 0.85)).max(1.0);
                let c = p - Vec3::Y * hang * 0.5;
                let cell = atlas_cell(LeafKind::Lance, cx.detail == Detail::Low);
                let ph = t.phase + k as f32;
                let face = Vec3::new(cx.rng.signed(), 0.0, cx.rng.signed()).normalize_or(Vec3::X);
                cx.card(c, face, Vec3::Y, Vec2::new(0.6, hang), cell, ph, 0.2);
                cx.card(c, face.cross(Vec3::Y), Vec3::Y, Vec2::new(0.6, hang), cell, ph, 0.2);
            }
        } else {
            cx.foliage(t, per_m / scale, size * scale);
        }
    }
}

/// Grow side branches off a limb (recursively) and draw it.
fn branch(cx: &mut Ctx, l: Limb, max_depth: u32, terminals: &mut Vec<Limb>) {
    let sides = match (cx.detail, l.depth) {
        (Detail::Full, 1) => 7,
        (Detail::Full, _) => 5,
        (Detail::Low, _) => 5,
    };
    if l.radius[0] > 0.008 {
        cx.tube(&l, sides, false);
    }
    if l.depth >= max_depth {
        terminals.push(l);
        return;
    }
    let len: f32 = l.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    let n = match l.depth {
        1 => 4 + (cx.rng.next() * 3.0) as usize,
        _ => 3 + (cx.rng.next() * 2.0) as usize,
    };
    let weeping = cx.df.form == Form::Weeping;
    for k in 0..n {
        let t = 0.3 + 0.65 * (k as f32 + cx.rng.next() * 0.6) / n as f32;
        let at = point_on(&l.pts, t);
        let d = dir_on(&l.pts, t);
        let side = any_perp(d);
        let rot = Quat::from_axis_angle(d, cx.rng.next() * 6.28);
        let off = cx.rng.range(0.5, 0.95);
        let dir = (d * off.cos() + (rot * side) * off.sin() + Vec3::Y * 0.15 * cx.df.upright).normalize();
        let child_len = (len * (1.0 - t) * cx.rng.range(0.55, 0.85)).max(0.25);
        let r_at = l.radius[0] + (l.radius[l.radius.len() - 1] - l.radius[0]) * t;
        let bend = if weeping { Vec3::NEG_Y * 0.6 } else { Vec3::Y * 0.15 };
        let (pts, rad) = grow(&mut cx.rng, at, dir, child_len, (r_at * 0.65).max(0.006), 0.006, 4, bend, 0.4);
        let child = Limb { pts, radius: rad, depth: l.depth + 1, sway0: 0.0, phase: l.phase + cx.rng.next() };
        branch(cx, child, max_depth, terminals);
    }
    // The limb's own tip carries leaves too.
    terminals.push(Limb { pts: l.pts[l.pts.len() / 2..].to_vec(), radius: l.radius[l.radius.len() / 2..].to_vec(), depth: l.depth, sway0: 0.0, phase: l.phase });
}

fn conifer(cx: &mut Ctx, stage: Stage, sp: Sp) {
    let df = cx.df;
    let h = cx.height;
    let r0 = (h * 0.02).max(0.03) * if sp == Sp::Redwood { 1.6 } else { 1.0 };
    let pine = cx.leaf_kind == LeafKind::Needle;
    let deciduous_bare = !df.evergreen && cx.leaf_amount < 0.05;
    let (tp, tr) = grow(&mut cx.rng, Vec3::ZERO, Vec3::Y, h, r0, 0.015, 10, Vec3::ZERO, if pine { 0.15 } else { 0.05 });
    let trunk = Limb { pts: tp.clone(), radius: tr.clone(), depth: 0, sway0: 0.0, phase: cx.rng.next() * 6.0 };
    cx.tube(&trunk, if cx.detail == Detail::Full { 9 } else { 6 }, true);
    let clear = h * df.clear;
    // Whorls of branches up the trunk; length follows a cone (pines rounder).
    let spacing = (h / 22.0).clamp(0.25, 1.4) * if cx.detail == Detail::Low { 1.8 } else { 1.0 };
    let mut y = clear.max(0.3);
    let mut whorl = 0;
    let cell = atlas_cell(cx.leaf_kind, cx.detail == Detail::Low);
    let max_r = df.width * h * 0.5;
    while y < h - 0.3 {
        let rel = (y - clear) / (h - clear).max(0.1);
        let profile = if pine { (1.0 - rel).powf(0.6) * (0.4 + 0.6 * (rel * 3.0).min(1.0)) } else { 1.0 - rel };
        let blen = (max_r * profile * cx.rng.range(0.85, 1.1)).max(0.25);
        let count = if pine { 3 + (cx.rng.next() * 2.0) as usize } else { 4 + (cx.rng.next() * 3.0) as usize };
        let az0 = cx.rng.next() * 6.28;
        let r_at = r0 * (1.0 - y / h).max(0.1);
        for k in 0..count {
            let az = az0 + k as f32 / count as f32 * 6.28 + cx.rng.signed() * 0.3;
            // Spruce and hemlock branches droop; pines and firs angle up.
            let pitch = match sp {
                Sp::Hemlock | Sp::RedCedar | Sp::Spruce | Sp::DouglasFir => -0.08 + rel * 0.3,
                Sp::ItalianCypress => 1.2,
                _ => 0.12 + rel * 0.35,
            } + cx.rng.signed() * 0.1;
            let dir = Vec3::new(az.cos() * pitch.cos(), pitch.sin(), az.sin() * pitch.cos());
            let at = Vec3::new(tp[0].x, y, tp[0].z) + point_on(&tp, y / h) * Vec3::new(1.0, 0.0, 1.0);
            let bend = match sp {
                Sp::Hemlock | Sp::RedCedar | Sp::Spruce | Sp::DouglasFir => Vec3::NEG_Y * 0.12,
                _ => Vec3::Y * 0.12,
            };
            let (pts, rad) = grow(&mut cx.rng, at, dir, blen, (r_at * 0.45).max(0.01), 0.005, 4, bend, 0.2);
            let limb = Limb { pts, radius: rad, depth: 1, sway0: 0.0, phase: cx.rng.next() * 6.0 };
            if limb.radius[0] > 0.006 && (cx.detail == Detail::Full || whorl % 2 == 0 || blen > 1.5) {
                cx.tube(&limb, 4, false);
            }
            if deciduous_bare {
                continue;
            }
            // Foliage: sprays along the branch (pines: tufts towards the ends), each a
            // pair of cards (one lying along the branch, one standing) so the branch
            // reads as foliage from any side.
            let len = blen;
            let full = cx.detail == Detail::Full;
            let n = if full { ((len * 3.6 * df.density) as usize).max(2) } else { ((len * 1.2 * df.density) as usize).max(1) };
            let size = if full { (1.1 + len * 0.16).min(2.1) } else { (1.8 + len * 0.3).min(3.4) } * if pine { 1.15 } else { 1.0 };
            for j in 0..n {
                let t = if pine { cx.rng.range(0.4, 1.0) } else { cx.rng.range(0.08, 1.0) };
                let p = point_on(&limb.pts, t);
                let d = dir_on(&limb.pts, t);
                let s = size * cx.rng.range(0.8, 1.2) * if pine { 1.0 } else { 1.0 - t * 0.35 };
                let ph = limb.phase + j as f32 * 0.2;
                if pine {
                    cx.cross_card(p, (d + Vec3::Y * 0.4).normalize(), Vec2::splat(s), cell, ph);
                } else {
                    let side = d.cross(Vec3::Y).normalize_or(Vec3::X);
                    let tilt = cx.rng.signed() * 0.35;
                    let flat = (Vec3::Y + side * tilt).normalize();
                    // Lying along the branch...
                    cx.card(p + d * s * 0.3, flat, d, Vec2::new(s * 0.85, s), cell, ph, 0.3);
                    // ...and standing across it.
                    let stand = (side + Vec3::Y * 0.2).normalize();
                    cx.card(p + d * s * 0.3, stand, d, Vec2::new(s * 0.7, s), cell, ph, 0.2);
                }
            }
        }
        y += spacing * cx.rng.range(0.8, 1.2);
        whorl += 1;
    }
    // A tuft at the very top.
    if !deciduous_bare {
        let top = tp[tp.len() - 1];
        let s = if cx.detail == Detail::Full { 0.8 } else { 1.4 } * (h / 20.0).clamp(0.5, 1.5);
        cx.cross_card(top - Vec3::Y * s * 0.3, Vec3::Y, Vec2::splat(s), cell, 0.0);
    }
    let _ = stage;
}

fn palm(cx: &mut Ctx, stage: Stage, sp: Sp) {
    let h = cx.height;
    let fan = cx.df.form == Form::FanPalm;
    let r0 = match sp {
        Sp::WashingtoniaPalm => 0.22,
        Sp::RoyalPalm => 0.3,
        Sp::DatePalm => 0.33,
        Sp::SabalPalm => 0.2,
        _ => 0.17,
    } * stage.scale().sqrt();
    // Trunk: coconut palms lean and curve; others stand straight.
    let lean = if sp == Sp::CoconutPalm { cx.rng.range(0.15, 0.45) } else { cx.rng.range(0.0, 0.06) };
    let az = cx.rng.next() * 6.28;
    let lean_dir = Vec3::new(az.cos(), 0.0, az.sin());
    let mut pts = Vec::new();
    let mut rad = Vec::new();
    let segs = 12;
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        // Bend: leaning out low, curving back up near the top.
        let off = lean_dir * h * lean * (t * 1.4 - t * t * 0.6) * 0.5;
        pts.push(Vec3::new(0.0, t * h, 0.0) + off);
        let r = match sp {
            Sp::RoyalPalm => r0 * (1.0 + 0.25 * (t * 3.0).sin() * (1.0 - t)) * if t > 0.82 { 0.85 } else { 1.0 },
            Sp::WashingtoniaPalm => r0 * (1.25 - 0.45 * t),
            _ => r0 * (1.1 - 0.25 * t),
        };
        rad.push(r);
    }
    // Royal palms: a smooth green crownshaft at the top of the trunk.
    let shaft = if sp == Sp::RoyalPalm { 0.8 } else { 1.0 };
    let n_trunk = (segs as f32 * shaft) as usize;
    let trunk = Limb { pts: pts[..=n_trunk].to_vec(), radius: rad[..=n_trunk].to_vec(), depth: 0, sway0: 0.0, phase: cx.rng.next() * 6.0 };
    cx.tube(&trunk, if cx.detail == Detail::Full { 10 } else { 7 }, true);
    if sp == Sp::RoyalPalm {
        let save = cx.bark_col;
        cx.bark_col = [0.3, 0.42, 0.18, 1.0];
        let shaft_l = Limb { pts: pts[n_trunk..].to_vec(), radius: rad[n_trunk..].iter().map(|r| r * 1.05).collect(), depth: 0, sway0: 0.0, phase: trunk.phase };
        cx.tube(&shaft_l, 8, false);
        cx.bark_col = save;
    }
    let top = *pts.last().unwrap();
    let crown_up = (pts[segs] - pts[segs - 1]).normalize();
    // Washingtonia: a skirt of dead fronds hanging down the top of the trunk.
    if sp == Sp::WashingtoniaPalm && stage >= Stage::Young {
        let skirt = h * cx.rng.range(0.12, 0.3);
        let n = 10;
        for k in 0..n {
            let a = k as f32 / n as f32 * 6.28;
            let out = Vec3::new(a.cos(), 0.0, a.sin());
            let c = top - Vec3::Y * (skirt * 0.5 + 0.3) + out * (r0 + 0.15);
            let save = cx.leaf_col;
            cx.leaf_col = [0.42, 0.33, 0.22];
            let fall = cx.season.fall;
            cx.season.fall = 0.0;
            cx.card(c, out, Vec3::Y, Vec2::new(1.1, skirt), CELL_DEAD_FAN, k as f32, 0.15);
            cx.season.fall = fall;
            cx.leaf_col = save;
        }
    }
    // Fronds.
    let n = match (sp, stage) {
        (_, Stage::Sapling) => 8,
        (Sp::CoconutPalm, _) => 20,
        (Sp::WashingtoniaPalm, _) => 22,
        (Sp::SabalPalm, _) => 24,
        _ => 18,
    };
    let len = match sp {
        Sp::CoconutPalm => 4.5,
        Sp::RoyalPalm => 4.0,
        Sp::QueenPalm => 3.5,
        Sp::DatePalm => 4.0,
        Sp::WashingtoniaPalm => 1.4,
        Sp::SabalPalm => 1.9,
        _ => 3.0,
    } * stage.scale().clamp(0.35, 1.0).sqrt();
    let cell = atlas_cell(cx.leaf_kind, false);
    let golden = 2.399_963;
    let droop = match sp {
        Sp::CoconutPalm | Sp::QueenPalm => 1.4,
        Sp::RoyalPalm => 1.1,
        Sp::DatePalm => 0.6,
        _ => 0.5,
    };
    for k in 0..n {
        let az = k as f32 * golden + cx.rng.signed() * 0.2;
        // Young fronds stand up in the middle; old ones hang.
        let age = k as f32 / n as f32;
        let elev = (1.2 - age * 1.7 + cx.rng.signed() * 0.15).clamp(-0.9, 1.4);
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let d0 = (out * elev.cos() + Vec3::Y * elev.sin() + crown_up * 0.2).normalize();
        let ph = cx.rng.next() * 6.0;
        if fan {
            // Petiole then a pleated fan.
            let pet = len * 0.9;
            let (pp, pr) = grow(&mut cx.rng, top, d0, pet, 0.03, 0.015, 3, Vec3::NEG_Y * 0.2 * droop, 0.05);
            let petiole = Limb { pts: pp.clone(), radius: pr, depth: 2, sway0: 0.0, phase: ph };
            cx.tube(&petiole, 4, false);
            let tip = *pp.last().unwrap();
            let fd = dir_on(&pp, 1.0);
            fan_blade(cx, tip, fd, len * 0.85, cell, ph);
        } else {
            pinnate_frond(cx, top, d0, len, droop, cell, ph);
        }
    }
    // Coconuts.
    if sp == Sp::CoconutPalm && stage >= Stage::Mature && cx.detail == Detail::Full {
        for k in 0..6 {
            let a = k as f32 * 1.1;
            let c = top - Vec3::Y * 0.35 + Vec3::new(a.cos(), 0.0, a.sin()) * 0.25;
            ball(&mut cx.wood, c, 0.13, [0.35, 0.42, 0.12, 1.0]);
        }
    }
}

/// A feather palm frond: a drooping midrib with a V-shaped strip of leaflets.
fn pinnate_frond(cx: &mut Ctx, base: Vec3, d0: Vec3, len: f32, droop: f32, cell: u32, phase: f32) {
    let segs = 7;
    let (pts, _) = grow(&mut cx.rng, base, d0, len, 0.03, 0.01, segs, Vec3::NEG_Y * droop * 0.5, 0.05);
    let (uv0, uv1) = cell_rect(cell);
    let colour = {
        let s = 0.85 + 0.25 * cx.rng.next();
        [cx.leaf_col[0] * s, cx.leaf_col[1] * s, cx.leaf_col[2] * s, 1.0]
    };
    let mut prev: Option<[u32; 3]> = None;
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        let p = pts[i];
        let d = dir_on(&pts, t);
        let side = d.cross(Vec3::Y).normalize_or(Vec3::X);
        // Width: narrow at the base, widest a third along, tapering to the tip.
        let w = len * 0.22 * (t * 3.0).min(1.0) * (1.0 - t * 0.75);
        // Leaflets hang from the midrib in an inverted V.
        let keel = Vec3::NEG_Y * w * 0.55;
        let n = d.cross(side).normalize();
        let n = if n.y < 0.0 { -n } else { n };
        let v = uv0.y + (uv1.y - uv0.y) * t;
        let sway = Vec2::new(0.6 + t * 0.8, phase);
        let a = cx.leaves.vert(p + side * w + keel, n, Vec2::new(uv0.x, v), sway, colour);
        let m = cx.leaves.vert(p, n, Vec2::new((uv0.x + uv1.x) * 0.5, v), sway, colour);
        let b = cx.leaves.vert(p - side * w + keel, n, Vec2::new(uv1.x, v), sway, colour);
        if let Some([pa, pm, pb]) = prev {
            cx.leaves.idx.extend_from_slice(&[pa, a, m, pa, m, pm, pm, m, b, pm, b, pb]);
        }
        prev = Some([a, m, b]);
    }
}

/// A fan palm leaf: pleated segments spreading from the end of the stalk.
fn fan_blade(cx: &mut Ctx, at: Vec3, dir: Vec3, r: f32, cell: u32, phase: f32) {
    let (uv0, uv1) = cell_rect(cell);
    let side = dir.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = side.cross(dir).normalize();
    let colour = {
        let s = 0.85 + 0.25 * cx.rng.next();
        [cx.leaf_col[0] * s, cx.leaf_col[1] * s, cx.leaf_col[2] * s, 1.0]
    };
    let sway = Vec2::new(1.0, phase);
    let c = cx.leaves.vert(at, up, Vec2::new((uv0.x + uv1.x) * 0.5, uv1.y), sway, colour);
    let k = 10;
    let spread = 2.4;
    let mut prev = None;
    for i in 0..=k {
        let t = i as f32 / k as f32;
        let a = (t - 0.5) * spread;
        // Pleats: alternate up and down; the fan droops a little at its sides.
        let pleat = if i % 2 == 0 { 0.08 } else { -0.08 } * r;
        let dir2 = (dir * a.cos() + side * a.sin()).normalize();
        let p = at + dir2 * r + up * (pleat - a.abs() * 0.15 * r) - Vec3::Y * r * 0.15;
        let uv = Vec2::new(uv0.x + (uv1.x - uv0.x) * t, uv0.y);
        let v = cx.leaves.vert(p, up, uv, Vec2::new(1.3, phase), colour);
        if let Some(pv) = prev {
            cx.leaves.idx.extend_from_slice(&[c, pv, v]);
        }
        prev = Some(v);
    }
}

fn ball(b: &mut Buf, c: Vec3, r: f32, col: [f32; 4]) {
    let (rings, seg) = (4u32, 6u32);
    let base = b.pos.len() as u32;
    for i in 0..=rings {
        let t = i as f32 / rings as f32 * std::f32::consts::PI;
        for k in 0..=seg {
            let a = k as f32 / seg as f32 * std::f32::consts::TAU;
            let d = Vec3::new(t.sin() * a.cos(), t.cos(), t.sin() * a.sin());
            b.vert(c + d * r, d, Vec2::new(k as f32 / seg as f32, i as f32 / rings as f32), Vec2::new(0.9, 0.0), col);
        }
    }
    for i in 0..rings {
        for k in 0..seg {
            let a = base + i * (seg + 1) + k;
            let cc = a + seg + 1;
            b.idx.extend_from_slice(&[a, cc + 1, a + 1, a, cc, cc + 1]);
        }
    }
}

/// A ribbed column (saguaro trunk or arm).
fn ribbed(cx: &mut Ctx, pts: &[Vec3], r: f32) {
    let n = pts.len();
    let ribs = 12u32;
    let sides = 24u32;
    let mut rings = Vec::new();
    for i in 0..n {
        let d = if i + 1 < n { pts[i + 1] - pts[i] } else { pts[i] - pts[i - 1] }.normalize_or(Vec3::Y);
        let side = any_perp(d);
        let other = d.cross(side);
        // Rounded end.
        let t = i as f32 / (n - 1) as f32;
        let rr = r * if t > 0.85 { (1.0 - ((t - 0.85) / 0.15).powi(2)).max(0.0).sqrt().max(0.25) } else { 1.0 };
        let base = cx.wood.pos.len() as u32;
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let rib = 1.0 + 0.07 * (a * ribs as f32).cos();
            let dir = side * a.cos() + other * a.sin();
            let nrm = (dir - (side * -(a).sin() + other * a.cos()) * 0.3 * (a * ribs as f32).sin()).normalize();
            let sway = Vec2::new(0.0, 0.0);
            cx.wood.vert(pts[i] + dir * rr * rib, nrm, Vec2::new(k as f32 / sides as f32 * 2.0, pts[i].y), sway, cx.bark_col);
        }
        rings.push(base);
    }
    for i in 0..n - 1 {
        let (a, b) = (rings[i], rings[i + 1]);
        for k in 0..sides {
            cx.wood.idx.extend_from_slice(&[a + k, b + k, b + k + 1, a + k, b + k + 1, a + k + 1]);
        }
    }
}

fn cactus(cx: &mut Ctx, stage: Stage) {
    let h = cx.height;
    let r = 0.2 + 0.12 * stage.scale().min(1.0);
    let pts: Vec<Vec3> = (0..=10).map(|i| Vec3::new(0.0, h * i as f32 / 10.0, 0.0)).collect();
    ribbed(cx, &pts, r);
    // Arms on older plants.
    let arms = match stage {
        Stage::Sapling | Stage::Young => 0,
        Stage::Mature => 1 + (cx.rng.next() * 3.0) as usize,
        Stage::Old => 3 + (cx.rng.next() * 3.0) as usize,
    };
    for k in 0..arms {
        let y = h * cx.rng.range(0.35, 0.6);
        let az = k as f32 * 2.3 + cx.rng.next();
        let out = Vec3::new(az.cos(), 0.0, az.sin());
        let ar = r * 0.75;
        let up = h * cx.rng.range(0.2, 0.4);
        let mut p: Vec<Vec3> = Vec::new();
        for i in 0..=8 {
            let t = i as f32 / 8.0;
            // Out from the trunk, then curving up.
            let o = (t * 3.0).min(1.0) * (r + 0.45);
            let yy = y + (t - 0.25).max(0.0) / 0.75 * up + (1.0 - (t * 3.0).min(1.0)) * 0.0;
            p.push(out * o + Vec3::Y * yy);
        }
        ribbed(cx, &p, ar);
    }
}

fn joshua(cx: &mut Ctx, stage: Stage) {
    let h = cx.height;
    let r0 = 0.12 + 0.12 * stage.scale().min(1.2);
    let trunk_h = h * 0.4;
    let (tp, tr) = grow(&mut cx.rng, Vec3::ZERO, Vec3::Y, trunk_h, r0, r0 * 0.8, 4, Vec3::ZERO, 0.2);
    cx.tube(&Limb { pts: tp.clone(), radius: tr, depth: 0, sway0: 0.0, phase: 0.0 }, 7, true);
    let mut tips: Vec<(Vec3, Vec3, f32, u32)> = vec![(tp[tp.len() - 1], Vec3::Y, r0 * 0.8, 0)];
    let levels = match stage {
        Stage::Sapling => 0,
        Stage::Young => 1,
        _ => 2,
    };
    let cell = atlas_cell(LeafKind::Spike, false);
    let mut ends = Vec::new();
    while let Some((p, d, r, lvl)) = tips.pop() {
        if lvl >= levels {
            ends.push((p, d));
            continue;
        }
        for k in 0..2 {
            let az = cx.rng.next() * 6.28 + k as f32 * 3.14;
            let out = Vec3::new(az.cos(), 0.0, az.sin());
            let dir = (d * 0.6 + out * 0.8).normalize();
            let len = (h - trunk_h) / (levels as f32 + 0.5) * cx.rng.range(0.7, 1.1);
            let (pts, rad) = grow(&mut cx.rng, p, dir, len, r * 0.75, r * 0.6, 3, Vec3::Y * 0.5, 0.3);
            cx.tube(&Limb { pts: pts.clone(), radius: rad, depth: 1, sway0: 0.0, phase: 0.0 }, 6, false);
            tips.push((*pts.last().unwrap(), dir_on(&pts, 1.0), r * 0.75, lvl + 1));
        }
    }
    for (p, d) in ends {
        for k in 0..3 {
            let a = k as f32 * 1.05;
            let face = Quat::from_axis_angle(d, a) * any_perp(d);
            cx.card(p + d * 0.25, face, d, Vec2::new(0.9, 0.9), cell, k as f32, 0.1);
        }
    }
}

fn shrub(cx: &mut Ctx, stage: Stage, sp: Sp) {
    let h = cx.height.max(0.4);
    let w = h * cx.df.width;
    cx.crown_c = Vec3::new(0.0, h * 0.5, 0.0);
    cx.crown_r = Vec3::new(w * 0.5, h * 0.48, w * 0.5);
    if sp == Sp::SawPalmetto {
        let cell = atlas_cell(LeafKind::Fan, false);
        for k in 0..9 {
            let az = k as f32 * 2.4;
            let d = Vec3::new(az.cos() * 0.7, 0.7, az.sin() * 0.7).normalize();
            fan_blade(cx, d * h * 0.4, d, h * 0.55, cell, k as f32);
        }
        return;
    }
    // A few stems, leaves filling a squashed dome.
    for k in 0..4 {
        let az = k as f32 * 1.6 + cx.rng.next();
        let d = Vec3::new(az.cos() * 0.4, 1.0, az.sin() * 0.4).normalize();
        let (pts, rad) = grow(&mut cx.rng, Vec3::ZERO, d, h * 0.55, 0.03, 0.01, 3, Vec3::Y * 0.1, 0.3);
        cx.tube(&Limb { pts, radius: rad, depth: 1, sway0: 0.0, phase: 0.0 }, 4, false);
    }
    if cx.leaf_amount <= 0.01 {
        return;
    }
    // Densely leaved: many small clusters through the whole dome, thickest at its skin.
    let n = ((w * h * 36.0 * cx.df.density) as usize).clamp(24, 150) / if cx.detail == Detail::Low { 3 } else { 1 };
    let cell = atlas_cell(cx.leaf_kind, cx.detail == Detail::Low);
    let s = if cx.detail == Detail::Full { 0.36 } else { 0.7 } * (h / 2.0).clamp(0.5, 1.2);
    for _ in 0..n {
        let u = Vec3::new(cx.rng.signed(), cx.rng.range(-0.7, 1.0), cx.rng.signed()).normalize_or(Vec3::Y);
        let mut c = cx.crown_c + u * cx.crown_r * cx.rng.range(0.35, 1.0).sqrt();
        c.y = c.y.max(h * 0.12);
        let face = (u + Vec3::new(cx.rng.signed(), 0.0, cx.rng.signed()) * 0.5).normalize_or(Vec3::Y);
        let ph = cx.rng.next() * 6.0;
        cx.card(c, face, Vec3::Y, Vec2::splat(s), cell, ph, 0.3);
    }
    let _ = stage;
}

/// Bark texture cell for a bark kind (see sprites::bark_atlas).
pub fn bark_cell(b: Bark) -> u32 {
    match b {
        Bark::Furrowed => 0,
        Bark::Smooth => 1,
        Bark::White => 2,
        Bark::Mottled => 3,
        Bark::Plated => 4,
        Bark::Stringy => 5,
        Bark::PalmRings => 6,
        Bark::PalmBoots => 7,
        Bark::PalmSmooth => 1,
        Bark::Cactus => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::species::Season;

    #[test]
    fn every_species_builds() {
        let summer = Season { leaves: 1.0, fall: 0.0, spring: 0.0, dry: 0.0, dormant: 0.0 };
        let fall = Season { leaves: 0.8, fall: 0.9, spring: 0.0, dry: 0.0, dormant: 0.0 };
        let winter = Season { leaves: 0.0, fall: 0.0, spring: 0.0, dry: 0.0, dormant: 1.0 };
        let all = [
            Sp::RedMaple, Sp::SugarMaple, Sp::RedOak, Sp::WhiteOak, Sp::LiveOak, Sp::CoastLiveOak, Sp::Elm, Sp::Cottonwood, Sp::Sycamore,
            Sp::Birch, Sp::Aspen, Sp::Ash, Sp::Pecan, Sp::TulipPoplar, Sp::Sweetgum, Sp::Magnolia, Sp::CrapeMyrtle, Sp::Dogwood,
            Sp::Mesquite, Sp::PaloVerde, Sp::Eucalyptus, Sp::HoneyLocust, Sp::Willow, Sp::Mangrove, Sp::Olive, Sp::PonderosaPine,
            Sp::LodgepolePine, Sp::LoblollyPine, Sp::WhitePine, Sp::DouglasFir, Sp::RedCedar, Sp::Spruce, Sp::Fir, Sp::Hemlock,
            Sp::Juniper, Sp::Pinyon, Sp::Redwood, Sp::BaldCypress, Sp::ItalianCypress, Sp::SabalPalm, Sp::CoconutPalm, Sp::RoyalPalm,
            Sp::WashingtoniaPalm, Sp::QueenPalm, Sp::DatePalm, Sp::Saguaro, Sp::JoshuaTree, Sp::Shrub, Sp::Sagebrush, Sp::Creosote,
            Sp::SawPalmetto,
        ];
        let mut worst = (0usize, String::new());
        for sp in all {
            for stage in Stage::ALL {
                for (detail, s) in [(Detail::Full, &summer), (Detail::Low, &summer), (Detail::Full, &fall), (Detail::Full, &winter)] {
                    let t = generate(sp, 1, stage, detail, s);
                    for b in [&t.wood, &t.leaves] {
                        assert!(b.pos.iter().flatten().all(|x| x.is_finite()), "{sp:?} {stage:?}");
                        assert!(b.idx.iter().all(|&i| (i as usize) < b.pos.len()));
                    }
                    assert!(t.wood.tris() + t.leaves.tris() > 0, "{sp:?} is empty");
                    let tris = t.wood.tris() + t.leaves.tris();
                    if tris > worst.0 {
                        worst = (tris, format!("{sp:?} {stage:?} {detail:?}"));
                    }
                    if detail == Detail::Low {
                        assert!(tris < 9000, "{sp:?} {stage:?} low detail has {tris} triangles");
                    }
                }
            }
        }
        println!("heaviest tree: {} triangles ({})", worst.0, worst.1);
        assert!(worst.0 < 60000);
    }
}
