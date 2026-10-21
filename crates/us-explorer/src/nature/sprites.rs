//! Textures painted in code: the leaf atlas (twig cards, leaf clusters, palm fronds),
//! bark, grass blades, and tree impostors (a tree's own mesh rasterised side-on on the
//! CPU, for drawing it as a flat card far away or on low graphics settings).

use super::treegen::{ATLAS_COLS, ATLAS_ROWS, CELL_DEAD_FAN, TreeMesh, atlas_cell};
use super::species::{Bark, LeafKind};
use bevy::math::{Vec2, Vec3};

/// A float RGBA image.
#[derive(Clone)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 4]>,
}

fn hash(x: u32, y: u32, s: u32) -> f32 {
    let mut h = x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x85eb_ca6b) ^ s.wrapping_mul(0x9e37_79b9);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

fn vnoise(x: f32, y: f32, s: u32, period: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let p = period.max(1) as i64;
    let w = |a: f32| (a as i64).rem_euclid(p) as u32;
    let (x0, x1, y0, y1) = (w(xi), w(xi + 1.0), w(yi), w(yi + 1.0));
    let a = hash(x0, y0, s);
    let b = hash(x1, y0, s);
    let c = hash(x0, y1, s);
    let d = hash(x1, y1, s);
    (a + (b - a) * ux) + ((c + (d - c) * ux) - (a + (b - a) * ux)) * uy
}

/// Tileable fractal noise over a unit square (`cells` across).
fn fbm(u: f32, v: f32, cells: u32, s: u32) -> f32 {
    let mut a = 0.0;
    let mut amp = 0.5;
    let mut c = cells;
    for o in 0..4 {
        a += vnoise(u * c as f32, v * c as f32, s + o, c) * amp;
        amp *= 0.5;
        c *= 2;
    }
    a / 0.9375
}

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn r(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![[0.0; 4]; w * h] }
    }

    /// Paint over (straight alpha).
    fn blend(&mut self, x: usize, y: usize, c: [f32; 4]) {
        let d = &mut self.px[y * self.w + x];
        let a = c[3];
        let out_a = a + d[3] * (1.0 - a);
        if out_a > 1e-5 {
            for i in 0..3 {
                d[i] = (c[i] * a + d[i] * d[3] * (1.0 - a)) / out_a;
            }
        }
        d[3] = out_a;
    }

    /// Fill a polygon (pixel coordinates) with 4x4 supersampled coverage.
    fn poly(&mut self, pts: &[Vec2], col: &dyn Fn(Vec2) -> [f32; 3], clip: (usize, usize, usize, usize)) {
        if pts.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let xa = (x0.floor().max(clip.0 as f32)) as usize;
        let ya = (y0.floor().max(clip.1 as f32)) as usize;
        let xb = (x1.ceil().min(clip.2 as f32 - 1.0)).max(0.0) as usize;
        let yb = (y1.ceil().min(clip.3 as f32 - 1.0)).max(0.0) as usize;
        if xa > xb || ya > yb {
            return;
        }
        let inside = |p: Vec2| {
            let mut c = false;
            let n = pts.len();
            let mut j = n - 1;
            for i in 0..n {
                let (a, b) = (pts[i], pts[j]);
                if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                    c = !c;
                }
                j = i;
            }
            c
        };
        for y in ya..=yb {
            for x in xa..=xb {
                let mut cov = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(Vec2::new(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0)) {
                            cov += 1;
                        }
                    }
                }
                if cov > 0 {
                    let c = col(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
                    self.blend(x, y, [c[0], c[1], c[2], cov as f32 / 16.0]);
                }
            }
        }
    }

    /// A thick line.
    fn line(&mut self, a: Vec2, b: Vec2, w: f32, c: [f32; 3], clip: (usize, usize, usize, usize)) {
        let d = (b - a).normalize_or(Vec2::X);
        let n = Vec2::new(-d.y, d.x) * w * 0.5;
        self.poly(&[a - n, b - n, b + n, a + n], &|_| c, clip);
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8)).collect()
    }
}

/// One leaf outline along +y from `base` (pixels), `len` long.
fn leaf_shape(kind: LeafKind, base: Vec2, dir: Vec2, len: f32, rng: &mut Rng) -> Vec<Vec2> {
    let side = Vec2::new(-dir.y, dir.x);
    let mut pts = Vec::new();
    let n = 14;
    let width = |t: f32| -> f32 {
        match kind {
            LeafKind::Ovate => (std::f32::consts::PI * t).sin().powf(0.8) * 0.32 * (1.0 - t * 0.2),
            LeafKind::Small => (std::f32::consts::PI * t).sin().powf(0.7) * 0.36,
            LeafKind::Lance => (std::f32::consts::PI * t).sin().powf(0.9) * 0.12,
            LeafKind::Oak => (std::f32::consts::PI * t).sin() * 0.3 * (0.65 + 0.35 * (t * 9.0).sin().abs()),
            _ => (std::f32::consts::PI * t).sin() * 0.3,
        }
    };
    if kind == LeafKind::Palmate {
        // Five pointed lobes around the stalk end.
        let c = base + dir * len * 0.45;
        let m = 40;
        let rot = rng.r(-0.2, 0.2);
        for k in 0..m {
            let a = k as f32 / m as f32 * std::f32::consts::TAU;
            let lobe = (a * 2.5 + rot).cos().abs().powf(3.0);
            let r = len * (0.22 + 0.33 * lobe);
            // Flatten the bottom (stalk side).
            let d2 = dir * a.cos() + side * a.sin();
            let r = if d2.dot(dir) < -0.6 { r * 0.55 } else { r };
            pts.push(c + d2 * r);
        }
        return pts;
    }
    for i in 0..=n {
        let t = i as f32 / n as f32;
        pts.push(base + dir * len * t + side * width(t) * len);
    }
    for i in (0..=n).rev() {
        let t = i as f32 / n as f32;
        pts.push(base + dir * len * t - side * width(t) * len);
    }
    pts
}

fn leaf_tone(rng: &mut Rng) -> [f32; 3] {
    let l = rng.r(0.72, 1.0);
    let warm = rng.r(-0.06, 0.08);
    [l * (1.0 + warm), l, l * (1.0 - warm * 1.5)]
}

/// The leaf atlas (ATLAS_COLS x ATLAS_ROWS cells of `cell` pixels). Colours are near
/// grey: each card's vertex colour gives the species and season colour.
pub fn leaf_atlas(cell: usize) -> Canvas {
    let mut c = Canvas::new(cell * ATLAS_COLS as usize, cell * ATLAS_ROWS as usize);
    let kinds = [
        LeafKind::Ovate,
        LeafKind::Palmate,
        LeafKind::Oak,
        LeafKind::Small,
        LeafKind::Needle,
        LeafKind::ShortNeedle,
        LeafKind::Scale,
        LeafKind::Lance,
    ];
    for (ki, &k) in kinds.iter().enumerate() {
        for cluster in [false, true] {
            let idx = atlas_cell(k, cluster) as usize;
            let (ox, oy) = ((idx % ATLAS_COLS as usize) * cell, (idx / ATLAS_COLS as usize) * cell);
            let clip = (ox + 1, oy + 1, ox + cell - 1, oy + cell - 1);
            let mut rng = Rng(0x51ea_f000 + ki as u64 * 31 + cluster as u64);
            let s = cell as f32;
            let o = Vec2::new(ox as f32, oy as f32);
            let twig = [0.42, 0.34, 0.24];
            match k {
                LeafKind::ShortNeedle => {
                    // Flat sprays: a twig with side twigs, short needles packed along
                    // every one (spruce, fir, hemlock, Douglas fir).
                    let sprays = if cluster { 7 } else { 1 };
                    for sp in 0..sprays {
                        let (root, tip) = if cluster {
                            let c = o + Vec2::new(rng.r(0.25, 0.75), rng.r(0.25, 0.75)) * s;
                            let a = rng.r(0.0, std::f32::consts::TAU);
                            let d = Vec2::new(a.cos(), a.sin()) * s * 0.3;
                            (c - d, c + d)
                        } else {
                            (o + Vec2::new(0.5, 0.98) * s, o + Vec2::new(rng.r(0.42, 0.58), 0.04) * s)
                        };
                        let along = (tip - root).normalize();
                        let side = Vec2::new(-along.y, along.x);
                        let len = root.distance(tip);
                        let mut twigs: Vec<(Vec2, Vec2)> = vec![(root, tip)];
                        let n_side = 9;
                        for k in 0..n_side {
                            let t = 0.08 + 0.85 * k as f32 / n_side as f32;
                            let p0 = root.lerp(tip, t);
                            let sgn = if k % 2 == 0 { 1.0 } else { -1.0 };
                            let l = len * 0.42 * (1.0 - t * 0.7);
                            let d = (side * sgn * 0.8 + along * 0.6).normalize();
                            twigs.push((p0, p0 + d * l));
                        }
                        for (a, b) in twigs {
                            c.line(a, b, (s * 0.007).max(1.0), twig, clip);
                            let tl = a.distance(b);
                            let d = (b - a).normalize_or(Vec2::Y);
                            let nrm = Vec2::new(-d.y, d.x);
                            let steps = (tl / (s * 0.012)).max(2.0) as usize;
                            for q in 0..steps {
                                let p = a.lerp(b, q as f32 / steps as f32);
                                for sgn in [-1.0f32, 1.0] {
                                    let nl = s * rng.r(0.03, 0.045) * (1.0 - 0.4 * q as f32 / steps as f32);
                                    let nd = (nrm * sgn + d * 0.45).normalize();
                                    let tone = leaf_tone(&mut rng);
                                    c.line(p, p + nd * nl, (s * 0.008).max(1.2), tone, clip);
                                }
                            }
                        }
                        let _ = sp;
                    }
                }
                LeafKind::Needle => {
                    // Pine: long needles in bundles towards the twig ends.
                    let tufts = if cluster { 12 } else { 7 };
                    for t in 0..tufts {
                        let base = if cluster {
                            o + Vec2::new(rng.r(0.25, 0.75), rng.r(0.25, 0.75)) * s
                        } else {
                            o + Vec2::new(0.5 + rng.r(-0.15, 0.15), 0.85 - t as f32 * 0.1) * s
                        };
                        let n = 55;
                        let len = s * rng.r(0.18, 0.26) * if cluster { 0.85 } else { 1.0 };
                        for _ in 0..n {
                            let a = rng.r(-2.6, -0.5);
                            let d = Vec2::new(a.cos(), a.sin());
                            let tone = leaf_tone(&mut rng);
                            c.line(base, base + d * len * rng.r(0.65, 1.0), (s * 0.009).max(1.3), tone, clip);
                        }
                    }
                    if !cluster {
                        c.line(o + Vec2::new(0.5, 0.99) * s, o + Vec2::new(0.5, 0.2) * s, s * 0.014, twig, clip);
                    }
                }
                LeafKind::Scale => {
                    // Flat branching sprays of scale leaves.
                    let sprays = if cluster { 9 } else { 3 };
                    for _ in 0..sprays {
                        let root = o + Vec2::new(rng.r(0.3, 0.7), rng.r(0.65, 0.92)) * s;
                        let tipd = Vec2::new(rng.r(-0.35, 0.35), -1.0).normalize();
                        let len = s * rng.r(0.4, 0.7) * if cluster { 0.6 } else { 1.0 };
                        let segs = 10;
                        for i in 0..segs {
                            let t = i as f32 / segs as f32;
                            let p = root + tipd * len * t;
                            let side = Vec2::new(-tipd.y, tipd.x);
                            for sgn in [-1.0f32, 1.0] {
                                let bl = len * 0.28 * (1.0 - t);
                                let q = p + (side * sgn + tipd * 0.6).normalize() * bl;
                                let tone = leaf_tone(&mut rng);
                                c.line(p, q, s * 0.03 * (1.0 - t * 0.5), tone, clip);
                            }
                            let tone = leaf_tone(&mut rng);
                            c.line(p, p + tipd * len / segs as f32, s * 0.035, tone, clip);
                        }
                    }
                }
                _ => {
                    if cluster {
                        // A blob of overlapping leaves, denser in the middle.
                        let n = match k {
                            LeafKind::Small => 150,
                            LeafKind::Lance => 90,
                            _ => 95,
                        };
                        for _ in 0..n {
                            let r = rng.f().sqrt() * 0.42;
                            let a = rng.r(0.0, std::f32::consts::TAU);
                            let base = o + (Vec2::splat(0.5) + Vec2::new(a.cos(), a.sin()) * r) * s;
                            let dir = Vec2::new(rng.r(-1.0, 1.0), rng.r(-1.0, 1.0)).normalize_or(Vec2::Y);
                            let len = s * match k {
                                LeafKind::Small => rng.r(0.08, 0.12),
                                LeafKind::Lance => rng.r(0.16, 0.24),
                                _ => rng.r(0.12, 0.18),
                            };
                            let pts = leaf_shape(k, base, dir, len, &mut rng);
                            let tone = leaf_tone(&mut rng);
                            let shade = 0.85 + 0.15 * (1.0 - r / 0.42);
                            c.poly(&pts, &|_| [tone[0] * shade, tone[1] * shade, tone[2] * shade], clip);
                        }
                    } else {
                        // A twig with leaves on both sides.
                        let root = o + Vec2::new(0.5, 0.97) * s;
                        let tip = o + Vec2::new(rng.r(0.4, 0.6), 0.08) * s;
                        c.line(root, tip, s * 0.018, twig, clip);
                        let n = match k {
                            LeafKind::Small => 22,
                            LeafKind::Lance => 16,
                            LeafKind::Palmate => 7,
                            _ => 12,
                        };
                        for i in 0..n {
                            let t = 0.12 + 0.85 * i as f32 / n as f32;
                            let p = root.lerp(tip, t);
                            let along = (tip - root).normalize();
                            let side = Vec2::new(-along.y, along.x) * if i % 2 == 0 { 1.0 } else { -1.0 };
                            let dir = (side * rng.r(0.6, 1.0) + along * rng.r(0.4, 0.9)).normalize();
                            let len = s * match k {
                                LeafKind::Small => 0.2,
                                LeafKind::Lance => 0.4,
                                LeafKind::Palmate => 0.42,
                                LeafKind::Oak => 0.36,
                                _ => 0.36,
                            } * rng.r(0.85, 1.15) * (1.0 - t * 0.25);
                            let pts = leaf_shape(k, p, dir, len, &mut rng);
                            let tone = leaf_tone(&mut rng);
                            // Lighter midrib, darker edges.
                            let mid_a = p;
                            let mid_d = dir;
                            c.poly(
                                &pts,
                                &|q| {
                                    let off = (q - mid_a).perp_dot(mid_d).abs() / (len * 0.3);
                                    let f = 1.05 - 0.2 * off.min(1.0);
                                    let vein = if off < 0.06 { 1.12 } else { 1.0 };
                                    [tone[0] * f * vein, tone[1] * f * vein, tone[2] * f * vein]
                                },
                                clip,
                            );
                        }
                    }
                }
            }
        }
    }
    // Feather palm frond strip: leaflets from the midrib (u = 0.5) out to the edges,
    // angled towards the tip (v grows towards the tip).
    {
        let idx = atlas_cell(LeafKind::Pinnate, false) as usize;
        let (ox, oy) = ((idx % ATLAS_COLS as usize) * cell, (idx / ATLAS_COLS as usize) * cell);
        let clip = (ox, oy, ox + cell, oy + cell);
        let s = cell as f32;
        let o = Vec2::new(ox as f32, oy as f32);
        let mut rng = Rng(0xf20d);
        let n = 24;
        for i in 0..n {
            let v = (i as f32 + 0.5) / n as f32;
            for sgn in [-1.0f32, 1.0] {
                // Separate leaflets with sky between them, drooping a little.
                let a = o + Vec2::new(0.5, v) * s;
                let b = o + Vec2::new(0.5 + sgn * rng.r(0.4, 0.5), (v + 0.16 + rng.r(-0.02, 0.02)).min(1.0)) * s;
                let tone = leaf_tone(&mut rng);
                let w = s * 0.011;
                // Leaflet: tapered.
                let d = (b - a).normalize();
                let nrm = Vec2::new(-d.y, d.x);
                c.poly(&[a - nrm * w, a + nrm * w, b], &|_| tone, clip);
            }
        }
        c.line(o + Vec2::new(0.5, 0.0) * s, o + Vec2::new(0.5, 1.0) * s, s * 0.02, [0.75, 0.72, 0.5], clip);
    }
    // Fan palm leaf: pleated segments from the stalk (bottom centre) to a split rim.
    {
        let idx = atlas_cell(LeafKind::Fan, false) as usize;
        let (ox, oy) = ((idx % ATLAS_COLS as usize) * cell, (idx / ATLAS_COLS as usize) * cell);
        let clip = (ox, oy, ox + cell, oy + cell);
        let s = cell as f32;
        let o = Vec2::new(ox as f32, oy as f32);
        let mut rng = Rng(0xfa17);
        let segs = 26;
        let centre = o + Vec2::new(0.5, 1.0) * s;
        for i in 0..segs {
            let (t0, t1) = (i as f32 / segs as f32, (i + 1) as f32 / segs as f32);
            let tone = leaf_tone(&mut rng);
            let shade = if i % 2 == 0 { 1.0 } else { 0.86 };
            // Each segment splits into a free tip in the outer third.
            let rim0 = o + Vec2::new(t0, 0.0) * s;
            let rim1 = o + Vec2::new(t1, 0.0) * s;
            let mid = o + Vec2::new((t0 + t1) * 0.5, 0.0) * s;
            let split = 0.35;
            let a = centre.lerp(rim0, 1.0 - split);
            let b = centre.lerp(rim1, 1.0 - split);
            c.poly(&[centre, a, b], &|_| [tone[0] * shade, tone[1] * shade, tone[2] * shade], clip);
            let tip = mid + Vec2::new(0.0, rng.r(0.0, 0.06) * s);
            c.poly(&[a, tip, b], &|_| [tone[0] * shade, tone[1] * shade, tone[2] * shade], clip);
        }
    }
    // Spikes (Joshua tree, yucca): a ball of stiff narrow leaves.
    {
        let idx = atlas_cell(LeafKind::Spike, false) as usize;
        let (ox, oy) = ((idx % ATLAS_COLS as usize) * cell, (idx / ATLAS_COLS as usize) * cell);
        let clip = (ox, oy, ox + cell, oy + cell);
        let s = cell as f32;
        let o = Vec2::new(ox as f32, oy as f32);
        let mut rng = Rng(0x5b1e);
        let c0 = o + Vec2::splat(0.5) * s;
        for _ in 0..70 {
            let a = rng.r(0.0, std::f32::consts::TAU);
            let d = Vec2::new(a.cos(), a.sin() * 0.9 - 0.15).normalize();
            let len = s * rng.r(0.3, 0.46);
            let tone = leaf_tone(&mut rng);
            let nrm = Vec2::new(-d.y, d.x) * s * 0.012;
            c.poly(&[c0 - nrm, c0 + nrm, c0 + d * len], &|_| tone, clip);
        }
    }
    // Dead hanging fan fronds (palm skirts): streaky brown, ragged bottom edge.
    {
        let idx = CELL_DEAD_FAN as usize;
        let (ox, oy) = ((idx % ATLAS_COLS as usize) * cell, (idx / ATLAS_COLS as usize) * cell);
        for y in 0..cell {
            for x in 0..cell {
                let u = x as f32 / cell as f32;
                let v = y as f32 / cell as f32;
                let streak = vnoise(u * 40.0, v * 3.0, 77, 40);
                let rag = 0.85 + 0.15 * vnoise(u * 30.0, 0.0, 91, 30);
                if v < rag && (0.05..0.95).contains(&u) {
                    let l = 0.7 + 0.35 * streak;
                    c.px[(oy + y) * c.w + ox + x] = [l, l * 0.95, l * 0.85, 1.0];
                }
            }
        }
    }
    c
}

/// Bark texture (tileable): albedo detail near white (vertex colour tints it) and a
/// height map for the normal map.
pub fn bark(kind: Bark, size: usize) -> (Canvas, Vec<f32>) {
    let mut c = Canvas::new(size, size);
    let mut hgt = vec![0.0f32; size * size];
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let (l, h) = match kind {
                Bark::Furrowed => {
                    // Vertical ridges broken into plates.
                    let ridge = (fbm(u * 1.0, v * 0.25, 6, 3) * 2.0 - 0.5).clamp(0.0, 1.0);
                    let r = ((u * 6.0 + fbm(u, v, 3, 9) * 1.5) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    let h = (r * 0.7 + ridge * 0.3).powf(1.5);
                    (0.55 + 0.5 * h, h)
                }
                Bark::Smooth => {
                    let n = fbm(u, v, 4, 5);
                    let lent = if vnoise(u * 18.0, v * 60.0, 13, 18) > 0.82 { 0.75 } else { 1.0 };
                    (0.82 + 0.18 * n * lent, n * 0.3)
                }
                Bark::White => {
                    let n = fbm(u, v, 4, 7);
                    let mark = vnoise(u * 5.0, v * 22.0, 21, 5) > 0.78;
                    if mark { (0.25, 0.2) } else { (0.92 + 0.08 * n, 0.5 + 0.1 * n) }
                }
                Bark::Mottled => {
                    let p = fbm(u, v, 3, 31);
                    let patch = if p > 0.55 { 1.0 } else if p > 0.45 { 0.75 } else { 0.55 };
                    (patch * (0.9 + 0.1 * fbm(u, v, 8, 4)), patch * 0.3)
                }
                Bark::Plated => {
                    // Irregular plates with dark cracks between.
                    let px = u * 5.0 + fbm(u, v, 3, 2) * 0.6;
                    let py = v * 3.0 + fbm(u, v, 3, 12) * 0.6;
                    let fx = px.fract().min(1.0 - px.fract());
                    let fy = py.fract().min(1.0 - py.fract());
                    let crack = (fx.min(fy) * 10.0).min(1.0);
                    (0.25 + 0.75 * crack * (0.85 + 0.15 * fbm(u, v, 8, 6)), crack)
                }
                Bark::Stringy => {
                    let s = fbm(u * 1.0, v * 0.1, 16, 41);
                    let fiber = ((u * 22.0 + s * 4.0) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    (0.6 + 0.4 * fiber, fiber)
                }
                Bark::PalmRings => {
                    let ring = ((v * 6.0) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    let n = fbm(u, v, 6, 17);
                    (0.62 + 0.3 * ring.powf(3.0) + 0.12 * n, ring)
                }
                Bark::PalmBoots => {
                    // Criss-cross leaf bases.
                    let a = ((u + v) * 5.0).fract();
                    let b = ((u - v) * 5.0).rem_euclid(1.0);
                    let d = (a.min(1.0 - a)).min(b.min(1.0 - b));
                    let h = (d * 6.0).min(1.0);
                    (0.35 + 0.65 * h * (0.9 + 0.1 * fbm(u, v, 6, 3)), h)
                }
                Bark::PalmSmooth => {
                    let n = fbm(u, v, 4, 8);
                    let ring = if ((v * 10.0).fract() - 0.5).abs() < 0.03 { 0.85 } else { 1.0 };
                    (0.9 * ring + 0.1 * n, n * 0.2)
                }
                Bark::Cactus => {
                    let spine = if vnoise(u * 24.0, v * 24.0, 61, 24) > 0.88 { 1.3 } else { 1.0 };
                    let n = fbm(u, v, 4, 23);
                    ((0.85 + 0.15 * n) * spine, n * 0.2)
                }
            };
            c.px[y * size + x] = [l, l, l, 1.0];
            hgt[y * size + x] = h;
        }
    }
    (c, hgt)
}

/// A tangent-space normal map from a tileable height map.
pub fn normal_map(h: &[f32], size: usize, strength: f32) -> Vec<u8> {
    let at = |x: i64, y: i64| h[(y.rem_euclid(size as i64) as usize) * size + x.rem_euclid(size as i64) as usize];
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size as i64 {
        for x in 0..size as i64 {
            let dx = (at(x + 1, y) - at(x - 1, y)) * strength;
            let dy = (at(x, y + 1) - at(x, y - 1)) * strength;
            let n = Vec3::new(-dx, -dy, 1.0).normalize();
            out.extend_from_slice(&[((n.x * 0.5 + 0.5) * 255.0) as u8, ((n.y * 0.5 + 0.5) * 255.0) as u8, ((n.z * 0.5 + 0.5) * 255.0) as u8, 255]);
        }
    }
    out
}

/// A grass blade texture (one column of blades) for 2D grass cards: wide at the
/// bottom, pointed tips, alpha cut out.
pub fn grass_card(w: usize, h: usize) -> Canvas {
    let mut c = Canvas::new(w, h);
    let mut rng = Rng(0x6a55);
    let clip = (0, 0, w, h);
    for _ in 0..34 {
        let x = rng.r(0.04, 0.96) * w as f32;
        let tall = rng.r(0.45, 1.0) * h as f32;
        let lean = rng.r(-0.18, 0.18) * w as f32;
        let bw = rng.r(0.012, 0.025) * w as f32;
        let base = Vec2::new(x, h as f32);
        let tip = Vec2::new(x + lean, h as f32 - tall);
        let mid = base.lerp(tip, 0.5) + Vec2::new(lean * 0.3, 0.0);
        let tone = rng.r(0.75, 1.05);
        let pts = [base - Vec2::new(bw, 0.0), mid - Vec2::new(bw * 0.6, 0.0), tip, mid + Vec2::new(bw * 0.6, 0.0), base + Vec2::new(bw, 0.0)];
        let hh = h as f32;
        c.poly(&pts, &|p| {
            // Darker at the roots.
            let t = 1.0 - p.y / hh;
            let l = tone * (0.55 + 0.5 * t.min(1.0));
            [l, l, l * 0.92]
        }, clip);
    }
    c
}

/// Mip levels (largest first), keeping alpha-tested coverage so cut-out foliage
/// doesn't thin away into nothing with distance. Pixels are averaged with alpha
/// weighting so transparent edges don't darken.
pub fn pyramid(base: &Canvas, cutoff: f32) -> Vec<Canvas> {
    pyramid_cells(base, cutoff, base.w.max(base.h))
}

/// Like [`pyramid`], keeping coverage per `cell`-pixel block (atlases: each cell holds a
/// different shape with its own coverage).
pub fn pyramid_cells(base: &Canvas, cutoff: f32, cell: usize) -> Vec<Canvas> {
    let gx = (base.w / cell).max(1);
    let gy = (base.h / cell).max(1);
    // Coverage of each cell at full size.
    let cov_of = |c: &Canvas, cx: usize, cy: usize, k: f32| -> f32 {
        let (cw, ch) = (c.w / gx, c.h / gy);
        let mut n = 0usize;
        for y in cy * ch..(cy + 1) * ch {
            for x in cx * cw..(cx + 1) * cw {
                if c.px[y * c.w + x][3] * k >= cutoff {
                    n += 1;
                }
            }
        }
        n as f32 / (cw * ch).max(1) as f32
    };
    let targets: Vec<f32> = (0..gx * gy).map(|i| cov_of(base, i % gx, i / gx, 1.0)).collect();
    let mut levels = vec![base.clone()];
    loop {
        let cur = levels.last().unwrap();
        if cur.w <= 4 || cur.h <= 4 || cur.w / gx < 2 || cur.h / gy < 2 {
            break;
        }
        let (w, h) = (cur.w / 2, cur.h / 2);
        let mut next = Canvas::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = cur.px[(y * 2 + dy) * cur.w + x * 2 + dx];
                    for i in 0..3 {
                        acc[i] += p[i] * p[3];
                    }
                    acc[3] += p[3];
                }
                let a = acc[3] / 4.0;
                let rgb = if acc[3] > 1e-5 { [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]] } else { [0.5, 0.5, 0.5] };
                next.px[y * w + x] = [rgb[0], rgb[1], rgb[2], a];
            }
        }
        // Scale alpha, cell by cell, so the same share of pixels passes the cut-off.
        let (cw, ch) = (w / gx, h / gy);
        for cy in 0..gy {
            for cx in 0..gx {
                let target = targets[cy * gx + cx];
                if target <= 0.0 {
                    continue;
                }
                let (mut lo, mut hi) = (0.5f32, 16.0f32);
                for _ in 0..14 {
                    let mid = (lo + hi) * 0.5;
                    if cov_of(&next, cx, cy, mid) < target {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let k = (lo + hi) * 0.5;
                for y in cy * ch..(cy + 1) * ch {
                    for x in cx * cw..(cx + 1) * cw {
                        let p = &mut next.px[y * w + x];
                        p[3] = (p[3] * k).min(1.0);
                    }
                }
            }
        }
        levels.push(next);
    }
    levels
}

/// Mip chain as sRGB bytes (see [`pyramid`]), with colour bled into transparent
/// pixels so filtering at cut-out edges doesn't pull in grey.
pub fn mips(base: &Canvas, cutoff: f32) -> (Vec<u8>, u32) {
    mips_cells(base, cutoff, base.w.max(base.h))
}

/// [`mips`] for an atlas of `cell`-pixel cells.
pub fn mips_cells(base: &Canvas, cutoff: f32, cell: usize) -> (Vec<u8>, u32) {
    let levels = pyramid_cells(base, cutoff, cell);
    let n = levels.len() as u32;
    let mut out = Vec::new();
    for l in &levels {
        out.extend(bleed(l).to_rgba8());
    }
    (out, n)
}

fn bleed(c: &Canvas) -> Canvas {
    let mut out = c.clone();
    for _ in 0..2 {
        let src = out.clone();
        for y in 0..c.h {
            for x in 0..c.w {
                if src.px[y * c.w + x][3] > 0.01 {
                    continue;
                }
                let mut acc = [0.0f32; 3];
                let mut n = 0.0;
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx < 0 || yy < 0 || xx >= c.w as i64 || yy >= c.h as i64 {
                        continue;
                    }
                    let p = src.px[yy as usize * c.w + xx as usize];
                    if p[3] > 0.01 {
                        for i in 0..3 {
                            acc[i] += p[i];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    let a = out.px[y * c.w + x][3];
                    out.px[y * c.w + x] = [acc[0] / n, acc[1] / n, acc[2] / n, a];
                }
            }
        }
    }
    out
}

/// Draw a tree side-on into a `size` x `size` sprite: the trunk base at the bottom
/// centre, scaled to fit. `leaf_tex` is the leaf atlas; bark is flat colour (too small
/// to see at impostor distances). Lit from the front and above, with the inside of the
/// crown darker.
pub fn impostor(t: &TreeMesh, leaf_tex: &[Canvas], size: usize) -> (Canvas, f32) {
    let mut out = Canvas::new(size, size);
    let mut depth = vec![f32::MIN; size * size];
    // Fit: height or width, whichever is bigger, with a margin.
    let mut half_w = 0.1f32;
    let mut top = 0.1f32;
    for b in [&t.wood, &t.leaves] {
        for p in &b.pos {
            half_w = half_w.max(p[0].abs());
            top = top.max(p[1]);
        }
    }
    let extent = (top * 1.04).max(half_w * 2.08);
    let scale = size as f32 / extent;
    let to_px = |p: [f32; 3]| Vec3::new(size as f32 * 0.5 + p[0] * scale, size as f32 - p[1] * scale, p[2]);
    let light = Vec3::new(0.35, 0.75, 0.55).normalize();
    let (mut zmin, mut zmax) = (f32::MAX, f32::MIN);
    for p in &t.leaves.pos {
        zmin = zmin.min(p[2]);
        zmax = zmax.max(p[2]);
    }
    let zr = (zmax - zmin).max(0.1);
    for (bi, b) in [&t.wood, &t.leaves].into_iter().enumerate() {
        let leaves = bi == 1;
        for tri in b.idx.chunks_exact(3) {
            let v: Vec<Vec3> = tri.iter().map(|&i| to_px(b.pos[i as usize])).collect();
            let (x0, x1) = (v[0].x.min(v[1].x).min(v[2].x).floor().max(0.0) as i64, v[0].x.max(v[1].x).max(v[2].x).ceil().min(size as f32 - 1.0) as i64);
            let (y0, y1) = (v[0].y.min(v[1].y).min(v[2].y).floor().max(0.0) as i64, v[0].y.max(v[1].y).max(v[2].y).ceil().min(size as f32 - 1.0) as i64);
            let area = (v[1].x - v[0].x) * (v[2].y - v[0].y) - (v[2].x - v[0].x) * (v[1].y - v[0].y);
            if area.abs() < 1e-6 {
                continue;
            }
            // Texture level: texels per pixel across this triangle (in texture space at
            // full size vs. on screen), so small far cards use coverage-kept mips.
            let level = if leaves {
                let (u0, u1, u2) = (Vec2::from(b.uv[tri[0] as usize]), Vec2::from(b.uv[tri[1] as usize]), Vec2::from(b.uv[tri[2] as usize]));
                let tw = leaf_tex[0].w as f32;
                let th = leaf_tex[0].h as f32;
                let ta = ((u1.x - u0.x) * tw * (u2.y - u0.y) * th - (u2.x - u0.x) * tw * (u1.y - u0.y) * th).abs();
                let ratio = (ta / area.abs().max(1e-4)).sqrt();
                (ratio.max(1.0).log2().floor() as usize).min(leaf_tex.len() - 1)
            } else {
                0
            };
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((v[1].x - p.x) * (v[2].y - p.y) - (v[2].x - p.x) * (v[1].y - p.y)) / area;
                    let w1 = ((v[2].x - p.x) * (v[0].y - p.y) - (v[0].x - p.x) * (v[2].y - p.y)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = v[0].z * w0 + v[1].z * w1 + v[2].z * w2;
                    let k = y as usize * size + x as usize;
                    if z <= depth[k] {
                        continue;
                    }
                    let ii = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                    let col = |i: usize| b.col[i];
                    let c = [0, 1, 2].map(|ch| col(ii[0])[ch] * w0 + col(ii[1])[ch] * w1 + col(ii[2])[ch] * w2);
                    let n = Vec3::from(b.nrm[ii[0]]) * w0 + Vec3::from(b.nrm[ii[1]]) * w1 + Vec3::from(b.nrm[ii[2]]) * w2;
                    let mut albedo = c;
                    if leaves {
                        let uv = Vec2::from(b.uv[ii[0]]) * w0 + Vec2::from(b.uv[ii[1]]) * w1 + Vec2::from(b.uv[ii[2]]) * w2;
                        let tex = &leaf_tex[level.min(leaf_tex.len() - 1)];
                        let tx = ((uv.x * tex.w as f32) as usize).min(tex.w - 1);
                        let ty = ((uv.y * tex.h as f32) as usize).min(tex.h - 1);
                        let tp = tex.px[ty * tex.w + tx];
                        if tp[3] < 0.5 {
                            continue;
                        }
                        albedo = [c[0] * tp[0], c[1] * tp[1], c[2] * tp[2]];
                    }
                    let nn = n.normalize_or(Vec3::Z);
                    let diffuse = nn.dot(light).max(0.0);
                    // Inside the crown (further back) is darker.
                    let inner = if leaves { 0.7 + 0.3 * ((b.pos[ii[0]][2] - zmin) / zr) } else { 0.8 };
                    let shade = (0.5 + 0.7 * diffuse) * inner;
                    depth[k] = z;
                    out.px[k] = [albedo[0] * shade, albedo[1] * shade, albedo[2] * shade, 1.0];
                }
            }
        }
    }
    (out, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_and_impostor() {
        let atlas = leaf_atlas(128);
        // Every leaf cell has some leaves.
        for cell in [0u32, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 17, 18, 19] {
            let (ox, oy) = ((cell % ATLAS_COLS) as usize * 128, (cell / ATLAS_COLS) as usize * 128);
            let mut n = 0;
            for y in 0..128 {
                for x in 0..128 {
                    if atlas.px[(oy + y) * atlas.w + ox + x][3] > 0.5 {
                        n += 1;
                    }
                }
            }
            assert!(n > 300, "cell {cell} nearly empty ({n})");
        }
        let (m, levels) = mips(&atlas, 0.5);
        assert!(levels > 4 && !m.is_empty());
        let s = super::super::species::Season { leaves: 1.0, ..Default::default() };
        let t = super::super::treegen::generate(super::super::species::Sp::RedOak, 0, super::super::treegen::Stage::Mature, super::super::treegen::Detail::Full, &s);
        let (img, ext) = impostor(&t, &pyramid_cells(&atlas, 0.5, 128), 128);
        let filled = img.px.iter().filter(|p| p[3] > 0.5).count();
        assert!(filled > 1500, "{filled}");
        assert!(ext > 10.0);
        for b in [Bark::Furrowed, Bark::Plated, Bark::PalmBoots] {
            let (c, h) = bark(b, 64);
            assert_eq!(c.px.len(), 64 * 64);
            assert_eq!(normal_map(&h, 64, 3.0).len(), 64 * 64 * 4);
        }
    }
}

/// A picture of every species at every growth stage, drawn by the impostor rasteriser:
/// USX_GALLERY=out.png cargo test --profile fast tree_gallery -- --ignored
#[cfg(test)]
mod gallery {
    use super::*;
    use crate::nature::species::{Season, Sp};
    use crate::nature::treegen::{Detail, Stage, generate};

    #[test]
    #[ignore]
    fn tree_gallery() {
        let out = std::env::var("USX_GALLERY").unwrap_or_else(|_| "gallery.png".into());
        let fall: bool = std::env::var("USX_FALL").is_ok();
        let season = if fall { Season { leaves: 0.9, fall: 0.9, ..Default::default() } } else { Season { leaves: 1.0, ..Default::default() } };
        let all = [
            Sp::RedMaple, Sp::SugarMaple, Sp::RedOak, Sp::WhiteOak, Sp::LiveOak, Sp::CoastLiveOak, Sp::Elm, Sp::Cottonwood, Sp::Sycamore,
            Sp::Birch, Sp::Aspen, Sp::Ash, Sp::Pecan, Sp::TulipPoplar, Sp::Sweetgum, Sp::Magnolia, Sp::CrapeMyrtle, Sp::Dogwood,
            Sp::Mesquite, Sp::PaloVerde, Sp::Eucalyptus, Sp::HoneyLocust, Sp::Willow, Sp::Mangrove, Sp::Olive, Sp::PonderosaPine,
            Sp::LodgepolePine, Sp::LoblollyPine, Sp::WhitePine, Sp::DouglasFir, Sp::RedCedar, Sp::Spruce, Sp::Fir, Sp::Hemlock,
            Sp::Juniper, Sp::Pinyon, Sp::Redwood, Sp::BaldCypress, Sp::ItalianCypress, Sp::SabalPalm, Sp::CoconutPalm, Sp::RoyalPalm,
            Sp::WashingtoniaPalm, Sp::QueenPalm, Sp::DatePalm, Sp::Saguaro, Sp::JoshuaTree, Sp::Shrub, Sp::Sagebrush, Sp::Creosote,
            Sp::SawPalmetto,
        ];
        let leaf = pyramid_cells(&leaf_atlas(256), 0.5, 256);
        let cell = 160usize;
        let cols = 12usize;
        let rows = (all.len() * 2).div_ceil(cols);
        let mut img = Canvas::new(cell * cols, cell * rows);
        for (k, sp) in all.iter().enumerate() {
            for (j, stage) in [Stage::Young, Stage::Mature].into_iter().enumerate() {
                let t = generate(*sp, 0, stage, Detail::Full, &season);
                let (c, _) = impostor(&t, &leaf, cell);
                let idx = k * 2 + j;
                let (ox, oy) = ((idx % cols) * cell, (idx / cols) * cell);
                for y in 0..cell {
                    for x in 0..cell {
                        let p = c.px[y * cell + x];
                        let bg = [0.75, 0.82, 0.9];
                        let a = p[3];
                        img.px[(oy + y) * img.w + ox + x] = [p[0] * a + bg[0] * (1.0 - a), p[1] * a + bg[1] * (1.0 - a), p[2] * a + bg[2] * (1.0 - a), 1.0];
                    }
                }
            }
        }
        let bytes = img.to_rgba8();
        usx_core::image::save_buffer(&out, &bytes, img.w as u32, img.h as u32, usx_core::image::ColorType::Rgba8).unwrap();
    }
}
