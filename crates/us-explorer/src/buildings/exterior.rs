//! Building shells: walls (with real openings where the doors are), foundations down to
//! the ground, roofs of every common shape, trim, porches, chimneys, garage doors and
//! rooftop equipment. Surfaces carry what the facade shader needs to draw bricks,
//! siding, glass, shingles and windows.
//!
//! Coordinates are local metres around the building's own origin: x east, y up from
//! the ground floor, z south. Vertices marked `ground` drop to the terrain when draped.

use super::geom;
use super::model::{Building, EntranceKind, Kind, Part, Roof, RoofMat, Wall, Windows, hash01, hash64};
use bevy::math::{Vec2, Vec3};

/// Element codes (also in buildings.wgsl).
pub mod elem {
    pub const WALL: u32 = 0;
    pub const ROOF: u32 = 1;
    pub const FOUNDATION: u32 = 2;
    pub const TRIM: u32 = 3;
    pub const GLASS: u32 = 4;
    pub const DOOR: u32 = 5;
    pub const GARAGE: u32 = 6;
    pub const EQUIP: u32 = 7;
    pub const CHIMNEY: u32 = 8;
    pub const PORCH: u32 = 9;
    pub const POST: u32 = 10;
    pub const SOFFIT: u32 = 11;
    pub const SIGN: u32 = 12;
    pub const AWNING: u32 = 13;
}

#[derive(Clone, Copy, Debug)]
pub struct BV {
    pub p: Vec3,
    pub n: Vec3,
    pub uv: [f32; 2],
    pub uv2: [f32; 2],
    pub col: [f32; 4],
    /// Drop to the terrain (minus a little) when placed.
    pub ground: bool,
}

#[derive(Default, Clone)]
pub struct BGeom {
    pub v: Vec<BV>,
    pub i: Vec<u32>,
}

/// Packed surface description in uv2.y.
#[derive(Clone, Copy)]
pub struct Code {
    pub elem: u32,
    pub mat: u32,
    pub windows: u32,
    pub fh: f32,
    pub storefront: bool,
    pub gable: bool,
}

impl Code {
    pub fn pack(self) -> f32 {
        let fh = ((self.fh * 8.0).round() as u32).min(63);
        (self.elem | self.mat << 4 | self.windows << 8 | fh << 11 | (self.storefront as u32) << 17 | (self.gable as u32) << 18) as f32
    }
}

/// Colour plus the wall height and a per-building seed in alpha.
pub fn col(rgb: [f32; 3], wall_top: f32, seed: u64) -> [f32; 4] {
    let wt = ((wall_top * 4.0).round() as u32).min(16383);
    [rgb[0], rgb[1], rgb[2], (wt | ((seed & 0x3ff) as u32) << 14) as f32]
}

impl BGeom {
    fn push(&mut self, v: BV) -> u32 {
        self.v.push(v);
        (self.v.len() - 1) as u32
    }

    /// A flat quad from four corners (counter-clockwise seen from the front).
    #[allow(clippy::too_many_arguments)]
    pub fn quad(&mut self, c: [Vec3; 4], uv: [[f32; 2]; 4], uv2: [f32; 2], col: [f32; 4], ground: [bool; 4]) {
        let n = (c[1] - c[0]).cross(c[3] - c[0]).normalize_or(Vec3::Y);
        let b = self.v.len() as u32;
        for k in 0..4 {
            self.v.push(BV { p: c[k], n, uv: uv[k], uv2, col, ground: ground[k] });
        }
        self.i.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }

    pub fn tri(&mut self, c: [Vec3; 3], uv: [[f32; 2]; 3], uv2: [f32; 2], col: [f32; 4]) {
        let n = (c[1] - c[0]).cross(c[2] - c[0]).normalize_or(Vec3::Y);
        let b = self.v.len() as u32;
        for k in 0..3 {
            self.v.push(BV { p: c[k], n, uv: uv[k], uv2, col, ground: false });
        }
        self.i.extend_from_slice(&[b, b + 1, b + 2]);
    }

    /// An axis-aligned-in-plan box (`dir` along x), from y0 to y1.
    pub fn boxy(&mut self, c: Vec2, dir: Vec2, hx: f32, hz: f32, y0: f32, y1: f32, uv2: [f32; 2], col: [f32; 4]) {
        let r = dir.perp();
        let pts = [c - dir * hx - r * hz, c + dir * hx - r * hz, c + dir * hx + r * hz, c - dir * hx + r * hz];
        for k in 0..4 {
            let (a, b) = (pts[k], pts[(k + 1) % 4]);
            let l = a.distance(b);
            self.quad(
                [Vec3::new(a.x, y0, a.y), Vec3::new(b.x, y0, b.y), Vec3::new(b.x, y1, b.y), Vec3::new(a.x, y1, a.y)],
                [[0.0, y0], [l, y0], [l, y1], [0.0, y1]],
                uv2,
                col,
                [false; 4],
            );
        }
        let top: Vec<Vec3> = pts.iter().map(|p| Vec3::new(p.x, y1, p.y)).collect();
        self.quad(
            [top[3], top[2], top[1], top[0]],
            [[0.0, 0.0], [hx * 2.0, 0.0], [hx * 2.0, hz * 2.0], [0.0, hz * 2.0]],
            uv2,
            col,
            [false; 4],
        );
    }
}

/// Window layout of one facade segment: (bays, window width, sill above each floor,
/// window height). Mirrored in buildings.wgsl.
pub fn window_layout(style: Windows, len: f32, fh: f32) -> (u32, f32, f32, f32) {
    let (bay, w, sill, h): (f32, f32, f32, f32) = match style {
        Windows::House => (3.4, 0.95, 0.9, 1.35),
        Windows::Punched => (2.7, 1.25, 0.85, 1.5),
        Windows::Ribbon => (1.6, 1.48, 0.95, (fh - 1.6).max(1.2)),
        Windows::Curtain => (1.55, 1.47, 0.0, fh - 0.2),
        Windows::Clerestory => (6.0, 3.6, 0.0, 1.1),
        Windows::Tall => (3.6, 1.3, 1.0, (fh - 2.2).max(1.8)),
        Windows::None => (1.0, 0.0, 0.0, 0.0),
    };
    if style == Windows::None || len < 1.6 {
        return (0, 0.0, 0.0, 0.0);
    }
    let n = (len / bay).round().max(1.0);
    let b = len / n;
    (n as u32, w.min(b - 0.35).max(0.0), sill, h)
}

/// Ground floor height: shops have tall storefronts.
pub fn ground_floor(kind: Kind, fh: f32) -> f32 {
    if kind.storefront() { 4.6 } else { fh }
}

/// Window rectangles (u0, u1, v0, v1) on a facade segment, for glass panes and for
/// knowing where windows are inside. Mirrors the shader's layout.
pub fn window_rects(style: Windows, kind: Kind, len: f32, fh: f32, wall_top: f32, gable: bool) -> Vec<(f32, f32, f32, f32)> {
    let (n, w, sill, h) = window_layout(style, len, fh);
    let mut out = Vec::new();
    if n == 0 || w <= 0.0 || gable {
        return out;
    }
    let bay = len / n as f32;
    let gf = ground_floor(kind, fh);
    let top = wall_top - if matches!(style, Windows::House) { 0.3 } else { 0.6 };
    for b in 0..n {
        let c = (b as f32 + 0.5) * bay;
        // Ground floor storefront.
        if kind.storefront() {
            let sw = (bay - 0.3).max(0.5);
            out.push((c - sw * 0.5, c + sw * 0.5, 0.45, gf - 1.0));
        } else if style == Windows::Clerestory {
            out.push((c - w * 0.5, c + w * 0.5, (wall_top - 1.8).max(2.4), (wall_top - 0.7).max(3.4)));
            continue;
        } else if style != Windows::Curtain || gf > 0.0 {
            let (v0, v1) = (sill, (sill + h).min(gf - 0.25));
            if v1 > v0 + 0.4 {
                out.push((c - w * 0.5, c + w * 0.5, v0, v1));
            }
        }
        let mut base = gf;
        while base + fh * 0.6 < top {
            let (v0, v1) = (base + sill, (base + sill + h).min(top));
            if v1 > v0 + 0.4 {
                out.push((c - w * 0.5, c + w * 0.5, v0, v1));
            }
            base += fh;
        }
    }
    out
}

/// What to build for one building.
pub struct Options {
    /// Leave the static door panels out (the doors are real, separate objects).
    pub detailed: bool,
    /// Roof colour to use when not tagged (from imagery or the palette).
    pub roof_col: [f32; 3],
    /// Outline edges facing a street (for front doors, garages and porches).
    pub street_edges: Vec<usize>,
}

/// Where doors go on the main outline: mapped entrances, else one on the street side.
pub fn doors(b: &Building, street_edges: &[usize]) -> Vec<(usize, f32, f32, EntranceKind)> {
    let r = &b.outline;
    let n = r.len();
    let elen = |e: usize| r[e].distance(r[(e + 1) % n]);
    let mut out: Vec<(usize, f32, f32, EntranceKind)> = Vec::new();
    let width = |k: EntranceKind| match k {
        EntranceKind::Garage => 2.5,
        _ if b.kind.storefront() || matches!(b.kind, Kind::Office | Kind::School | Kind::Civic | Kind::Hospital | Kind::Hotel) => 1.8,
        _ => 0.95,
    };
    for e in &b.entrances {
        let l = elen(e.edge);
        let w = width(e.kind);
        if l > w + 0.6 {
            out.push((e.edge, e.along.clamp(w * 0.5 + 0.3, l - w * 0.5 - 0.3), w, e.kind));
        }
    }
    if out.is_empty() && !matches!(b.kind, Kind::Canopy | Kind::Carport) {
        // The longest street-facing edge, else the longest edge.
        let pick = street_edges.iter().copied().filter(|&e| elen(e) > 2.5).max_by(|&a, &c| elen(a).total_cmp(&elen(c)));
        let e = pick.unwrap_or_else(|| (0..n).max_by(|&a, &c| elen(a).total_cmp(&elen(c))).unwrap_or(0));
        let l = elen(e);
        let w = width(EntranceKind::Main);
        if l > w + 0.8 {
            // Centred on a window bay so the windows either side stay whole.
            let fh = b.kind.floor_height();
            let style = b.parts[0].windows;
            let (nb, _, _, _) = window_layout(style, l, fh);
            let bays = nb.max(1);
            let bay = l / bays as f32;
            let pref = if b.kind == Kind::House { 0.35 + 0.3 * hash01(b.seed ^ 0xd00) } else { 0.5 };
            let k = ((pref * bays as f32).floor() as u32).min(bays - 1);
            let at = ((k as f32 + 0.5) * bay).clamp(w * 0.5 + 0.3, l - w * 0.5 - 0.3);
            if b.kind == Kind::Garage {
                let gw = if l > 6.5 { 4.9 } else { 2.5f32.min(l - 0.6) };
                out.push((e, l * 0.5, gw, EntranceKind::Garage));
            } else {
                out.push((e, at, w, EntranceKind::Main));
            }
        }
        // Back or side door for bigger buildings.
        if b.area > 400.0 && n >= 4 {
            let opp = (e + n / 2) % n;
            let l2 = elen(opp);
            if l2 > 3.0 {
                out.push((opp, l2 * 0.5, 0.95, EntranceKind::Service));
            }
        }
    }
    out
}

fn wall_strip(
    g: &mut BGeom,
    a: Vec2,
    b: Vec2,
    y0: f32,
    y1: f32,
    u0: f32,
    u1: f32,
    seg_len: f32,
    code: Code,
    colour: [f32; 4],
    ground_bottom: bool,
) {
    let d = (b - a) / seg_len.max(1e-6);
    let pa = a + d * u0;
    let pb = a + d * u1;
    g.quad(
        [Vec3::new(pa.x, y0, pa.y), Vec3::new(pb.x, y0, pb.y), Vec3::new(pb.x, y1, pb.y), Vec3::new(pa.x, y1, pa.y)],
        [[u0, y0], [u1, y0], [u1, y1], [u0, y1]],
        [seg_len, code.pack()],
        colour,
        [ground_bottom, ground_bottom, false, false],
    );
}

/// Walls of one ring, outward facing, with door openings cut out of the given edges.
#[allow(clippy::too_many_arguments)]
fn walls(
    g: &mut BGeom,
    ring: &[Vec2],
    p: &Part,
    kind: Kind,
    seed: u64,
    openings: &[(usize, f32, f32, f32)],
    foundation: bool,
    detailed: bool,
    garage_doors: &[(usize, f32, f32)],
) {
    let n = ring.len();
    let fh = kind.floor_height();
    let code = Code { elem: elem::WALL, mat: p.wall as u32, windows: p.windows as u32, fh, storefront: kind.storefront(), gable: false };
    let c = col(p.wall_col, p.wall_top, seed);
    let fcode = Code { elem: elem::FOUNDATION, mat: 0, windows: Windows::None as u32, fh, storefront: false, gable: false };
    let fcol = col([0.62, 0.6, 0.56], p.wall_top, seed);
    for e in 0..n {
        let (a, b) = (ring[e], ring[(e + 1) % n]);
        let len = a.distance(b);
        if len < 0.05 {
            continue;
        }
        // Foundation from the ground up to the floor.
        if foundation {
            wall_strip(g, a, b, -0.6, 0.0, 0.0, len, len, fcode, fcol, true);
        }
        let y0 = p.min_h;
        let y1 = p.wall_top;
        let mut holes: Vec<(f32, f32, f32)> =
            openings.iter().filter(|o| o.0 == e).map(|o| (o.1 - o.2 * 0.5, o.1 + o.2 * 0.5, o.3)).collect();
        holes.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut u = 0.0;
        for &(h0, h1, hh) in &holes {
            if h0 > u {
                wall_strip(g, a, b, y0, y1, u, h0, len, code, c, false);
            }
            // Above the opening.
            wall_strip(g, a, b, hh, y1, h0, h1, len, code, c, false);
            // Frame: jambs and head, set into the opening.
            let d = (b - a) / len;
            let out = -d.perp();
            let fc = col([0.92, 0.91, 0.88], p.wall_top, seed);
            let tc = Code { elem: elem::TRIM, mat: 0, windows: Windows::None as u32, fh, storefront: false, gable: false };
            for (u_, w_) in [(h0, 0.08), (h1, -0.08)] {
                let q0 = a + d * u_;
                let q1 = a + d * (u_ + w_);
                // Reveal (the side of the opening, into the wall).
                let inn = -out * 0.2;
                g.quad(
                    [
                        Vec3::new(q0.x, y0, q0.y),
                        Vec3::new((q0 + inn).x, y0, (q0 + inn).y),
                        Vec3::new((q0 + inn).x, hh, (q0 + inn).y),
                        Vec3::new(q0.x, hh, q0.y),
                    ],
                    [[0.0, 0.0], [0.2, 0.0], [0.2, hh], [0.0, hh]],
                    [0.2, tc.pack()],
                    fc,
                    [false; 4],
                );
                let _ = q1;
            }
            // Head of the opening.
            let (qa, qb) = (a + d * h0, a + d * h1);
            let inn = -out * 0.2;
            g.quad(
                [
                    Vec3::new(qa.x, hh, qa.y),
                    Vec3::new(qb.x, hh, qb.y),
                    Vec3::new((qb + inn).x, hh, (qb + inn).y),
                    Vec3::new((qa + inn).x, hh, (qa + inn).y),
                ],
                [[0.0, 0.0], [h1 - h0, 0.0], [h1 - h0, 0.2], [0.0, 0.2]],
                [h1 - h0, tc.pack()],
                fc,
                [false; 4],
            );
            // A threshold step on the outside.
            let st = col([0.66, 0.65, 0.62], p.wall_top, seed);
            let mid = (qa + qb) * 0.5 + out * 0.35;
            g.boxy(mid, d, (h1 - h0) * 0.5 + 0.15, 0.35, -0.6, -0.02, [0.0, fcode.pack()], st);
            if !detailed {
                // Closed door, drawn in the opening (the real door replaces it up close).
                let dc = Code { elem: elem::DOOR, mat: 0, windows: Windows::None as u32, fh, storefront: kind.storefront(), gable: false };
                let pa = qa + inn * 0.4;
                let pb = qb + inn * 0.4;
                let dcol = col(door_colour(kind, seed), p.wall_top, seed ^ e as u64);
                g.quad(
                    [Vec3::new(pa.x, 0.0, pa.y), Vec3::new(pb.x, 0.0, pb.y), Vec3::new(pb.x, hh, pb.y), Vec3::new(pa.x, hh, pa.y)],
                    [[0.0, 0.0], [h1 - h0, 0.0], [h1 - h0, hh], [0.0, hh]],
                    [h1 - h0, dc.pack()],
                    dcol,
                    [false; 4],
                );
            }
            u = h1;
        }
        if u < len {
            wall_strip(g, a, b, y0, y1, u, len, len, code, c, false);
        }
        // Garage doors: panels just proud of the wall.
        for &(ge, at, w) in garage_doors.iter().filter(|gd| gd.0 == e) {
            let d = (b - a) / len;
            let out = -d.perp() * 0.04;
            let (qa, qb) = (a + d * (at - w * 0.5) + out, a + d * (at + w * 0.5) + out);
            let gc = Code { elem: elem::GARAGE, mat: 0, windows: Windows::None as u32, fh, storefront: false, gable: false };
            let _ = ge;
            g.quad(
                [Vec3::new(qa.x, 0.0, qa.y), Vec3::new(qb.x, 0.0, qb.y), Vec3::new(qb.x, 2.15, qb.y), Vec3::new(qa.x, 2.15, qa.y)],
                [[0.0, 0.0], [w, 0.0], [w, 2.15], [0.0, 2.15]],
                [w, gc.pack()],
                col([0.93, 0.93, 0.91], p.wall_top, seed),
                [false; 4],
            );
        }
    }
}

/// Front doors: painted wood or fibreglass for houses (many colours), glass and metal
/// for shops and offices.
pub fn door_colour(kind: Kind, seed: u64) -> [f32; 3] {
    let opts: &[[f32; 3]] = if kind.residential() {
        &[
            [0.55, 0.1, 0.08],
            [0.1, 0.18, 0.32],
            [0.08, 0.08, 0.08],
            [0.15, 0.3, 0.2],
            [0.92, 0.92, 0.9],
            [0.45, 0.3, 0.18],
            [0.6, 0.45, 0.2],
            [0.3, 0.38, 0.45],
        ]
    } else {
        &[[0.2, 0.21, 0.22], [0.7, 0.72, 0.73], [0.12, 0.12, 0.13]]
    };
    opts[(hash64(seed ^ 0xd0) % opts.len() as u64) as usize]
}

fn roof_code(p: &Part) -> Code {
    Code { elem: elem::ROOF, mat: p.roof_mat as u32, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false }
}

/// A sloped roof plane given its corners (eave first, counter-clockwise from above);
/// uv: (along the eave, up the slope) in metres.
fn slope(g: &mut BGeom, pts: &[Vec3], eave_dir: Vec2, rc: Code, c: [f32; 4]) {
    if pts.len() < 3 {
        return;
    }
    let o = pts[0];
    let ed = Vec3::new(eave_dir.x, 0.0, eave_dir.y);
    let nrm = (pts[1] - pts[0]).cross(pts[2] - pts[0]).normalize_or(Vec3::Y);
    let up = nrm.cross(ed).normalize_or(Vec3::Y);
    let uv = |p: Vec3| [(p - o).dot(ed), (p - o).dot(up)];
    let b = g.v.len() as u32;
    for &p in pts {
        g.v.push(BV { p, n: nrm, uv: uv(p), uv2: [0.0, rc.pack()], col: c, ground: false });
    }
    for k in 1..pts.len() as u32 - 1 {
        g.i.extend_from_slice(&[b, b + k, b + k + 1]);
    }
}

/// Gabled, hipped and related roofs on a rectangle in a frame (centre, axis = ridge
/// direction), with eaves overhanging the walls by `over`.
#[allow(clippy::too_many_arguments)]
fn rect_roof(g: &mut BGeom, p: &Part, kind: Kind, seed: u64, c: Vec2, ax: Vec2, hl: f32, hw: f32, y: f32, rh: f32, shape: Roof, over: f32) {
    let pp = ax.perp();
    let rc = roof_code(p);
    let rcol = col(p.roof_col.unwrap_or([0.3, 0.3, 0.3]), p.wall_top, seed);
    let wcode =
        Code { elem: elem::WALL, mat: p.wall as u32, windows: p.windows as u32, fh: kind.floor_height(), storefront: false, gable: true };
    let wcol = col(p.wall_col, p.wall_top, seed);
    let at = |a: f32, b: f32, h: f32| {
        let q = c + ax * a + pp * b;
        Vec3::new(q.x, h, q.y)
    };
    // Eave height drops by the overhang at the roof's pitch.
    let pitch = rh / hw.max(0.5);
    let ye = y - over * pitch;
    let (l, w) = (hl + over, hw + over);
    match shape {
        Roof::Gabled | Roof::Gambrel | Roof::HalfHipped => {
            let (r0, r1) = (at(-l, 0.0, y + rh), at(l, 0.0, y + rh));
            if shape == Roof::Gambrel {
                // Steep lower slopes then shallow upper ones.
                let (kb, kh) = (0.6, 0.65);
                for s in [-1.0f32, 1.0] {
                    let lo = [
                        at(-l, s * w, ye),
                        at(l, s * w, ye),
                        at(l, s * w * (1.0 - kb), y + rh * kh),
                        at(-l, s * w * (1.0 - kb), y + rh * kh),
                    ];
                    let hi = [lo[3], lo[2], r1, r0];
                    let (lo, hi) = if s > 0.0 { (lo, hi) } else { ([lo[1], lo[0], lo[3], lo[2]], [hi[1], hi[0], hi[3], hi[2]]) };
                    slope(g, &lo, if s > 0.0 { ax } else { -ax }, rc, rcol);
                    slope(g, &hi, if s > 0.0 { ax } else { -ax }, rc, rcol);
                }
                for e in [-1.0f32, 1.0] {
                    let pts = [
                        at(e * hl, -hw, y),
                        at(e * hl, -hw * (1.0 - kb), y + rh * kh),
                        at(e * hl, 0.0, y + rh),
                        at(e * hl, hw * (1.0 - kb), y + rh * kh),
                        at(e * hl, hw, y),
                    ];
                    gable_wall(g, &pts, e < 0.0, wcode, wcol);
                }
            } else {
                let hip = if shape == Roof::HalfHipped { 0.35 } else { 0.0 };
                // Ridge ends pulled in for half-hips.
                let (r0h, r1h) = (at(-l + w * hip * 1.2, 0.0, y + rh), at(l - w * hip * 1.2, 0.0, y + rh));
                let (ra, rb) = if hip > 0.0 { (r0h, r1h) } else { (r0, r1) };
                slope(g, &[at(-l, w, ye), at(l, w, ye), rb, ra], ax, rc, rcol);
                slope(g, &[at(l, -w, ye), at(-l, -w, ye), ra, rb], -ax, rc, rcol);
                if hip > 0.0 {
                    let yh = y + rh * (1.0 - hip);
                    for e in [-1.0f32, 1.0] {
                        let ra_ = if e < 0.0 { ra } else { rb };
                        let (p1, p2) = (at(e * l, w * hip, yh), at(e * l, -w * hip, yh));
                        let tri = if e > 0.0 { [p2, p1, ra_] } else { [p1, p2, ra_] };
                        slope(g, &tri, if e > 0.0 { -pp } else { pp }, rc, rcol);
                    }
                }
                for e in [-1.0f32, 1.0] {
                    let top = if hip > 0.0 { y + rh * (1.0 - hip) } else { y + rh };
                    let pts = if hip > 0.0 {
                        vec![at(e * hl, -hw, y), at(e * hl, -w * hip, top), at(e * hl, w * hip, top), at(e * hl, hw, y)]
                    } else {
                        vec![at(e * hl, -hw, y), at(e * hl, 0.0, top), at(e * hl, hw, y)]
                    };
                    gable_wall(g, &pts, e < 0.0, wcode, wcol);
                }
            }
        }
        Roof::Hipped | Roof::Pyramidal | Roof::Mansard => {
            let ridge = if shape == Roof::Pyramidal { 0.0 } else { (hl - hw).max(0.0) };
            if shape == Roof::Mansard {
                let k = 0.75;
                let (il, iw) = (l - w * k * 0.6, w * (1.0 - k * 0.6));
                let yt = y + rh;
                let lo = [at(-l, w, ye), at(l, w, ye), at(l, -w, ye), at(-l, -w, ye)];
                let hi = [at(-il, iw, yt), at(il, iw, yt), at(il, -iw, yt), at(-il, -iw, yt)];
                for k in 0..4 {
                    let (a0, a1, b1, b0) = (lo[k], lo[(k + 1) % 4], hi[(k + 1) % 4], hi[k]);
                    let e = Vec2::new(a1.x - a0.x, a1.z - a0.z).normalize_or(ax);
                    slope(g, &[a0, a1, b1, b0], e, rc, rcol);
                }
                slope(g, &[hi[3], hi[2], hi[1], hi[0]], ax, rc, rcol);
                return;
            }
            let (r0, r1) = (at(-ridge, 0.0, y + rh), at(ridge, 0.0, y + rh));
            slope(g, &[at(-l, w, ye), at(l, w, ye), r1, r0], ax, rc, rcol);
            slope(g, &[at(l, -w, ye), at(-l, -w, ye), r0, r1], -ax, rc, rcol);
            slope(g, &[at(l, w, ye), at(l, -w, ye), r1], -pp, rc, rcol);
            slope(g, &[at(-l, -w, ye), at(-l, w, ye), r0], pp, rc, rcol);
        }
        Roof::Skillion => {
            let (lo, hi) = (y - over * pitch * 0.5, y + rh);
            slope(g, &[at(-l, w, lo), at(l, w, lo), at(l, -w, hi), at(-l, -w, hi)], ax, rc, rcol);
            for e in [-1.0f32, 1.0] {
                let pts = [at(e * hl, hw, y), at(e * hl, -hw, y), at(e * hl, -hw, y + rh * (2.0 * hw) / (2.0 * w))];
                gable_wall(g, &pts, e > 0.0, wcode, wcol);
            }
        }
        Roof::Round => {
            let n = 10;
            let prof: Vec<(f32, f32)> = (0..=n)
                .map(|k| {
                    let t = std::f32::consts::PI * k as f32 / n as f32;
                    (w * t.cos(), rh * t.sin())
                })
                .collect();
            for k in 0..n {
                let ((b0, h0), (b1, h1)) = (prof[k], prof[k + 1]);
                slope(g, &[at(-l, b0, y + h0), at(l, b0, y + h0), at(l, b1, y + h1), at(-l, b1, y + h1)], ax, rc, rcol);
            }
            for e in [-1.0f32, 1.0] {
                let pts: Vec<Vec3> = prof.iter().map(|&(b, h)| at(e * hl, b * hw / w, y + h)).collect();
                gable_wall(g, &pts, e > 0.0, wcode, wcol);
            }
        }
        _ => {}
    }
    // Soffits under the eaves and a fascia board.
    if over > 0.05 && matches!(shape, Roof::Gabled | Roof::Hipped | Roof::HalfHipped | Roof::Gambrel | Roof::Pyramidal) {
        let sc = Code { elem: elem::SOFFIT, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
        let scol = col([0.9, 0.9, 0.88], p.wall_top, seed);
        for s in [-1.0f32, 1.0] {
            let (a0, a1) = (at(-hl, s * hw, y), at(hl, s * hw, y));
            let (b0, b1) = (at(-l, s * w, ye), at(l, s * w, ye));
            let q = if s > 0.0 { [a1, a0, b0, b1] } else { [a0, a1, b1, b0] };
            g.quad(q, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], [0.0, sc.pack()], scol, [false; 4]);
            // Fascia.
            let f = [b0, b1, b1 + Vec3::Y * 0.18, b0 + Vec3::Y * 0.18];
            let f = if s > 0.0 { [f[1], f[0], f[3], f[2]] } else { f };
            g.quad(f, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], [0.0, sc.pack()], scol, [false; 4]);
        }
    }
}

/// A gable end wall (polygon in a vertical plane), facing out of the end it closes.
fn gable_wall(g: &mut BGeom, pts: &[Vec3], flip: bool, code: Code, c: [f32; 4]) {
    let pts: Vec<Vec3> = if flip { pts.iter().rev().copied().collect() } else { pts.to_vec() };
    let a = pts[0];
    let b = *pts.last().unwrap();
    let d = Vec2::new(b.x - a.x, b.z - a.z);
    let len = d.length().max(1e-3);
    let dir = d / len;
    let nrm = (pts[1] - pts[0]).cross(pts[pts.len() - 1] - pts[0]).normalize_or(Vec3::Y);
    let base = g.v.len() as u32;
    for &p in &pts {
        let u = Vec2::new(p.x - a.x, p.z - a.z).dot(dir);
        g.v.push(BV { p, n: -nrm, uv: [u, p.y], uv2: [len, code.pack()], col: c, ground: false });
    }
    for k in 1..pts.len() as u32 - 1 {
        g.i.extend_from_slice(&[base, base + k + 1, base + k]);
    }
}

/// Domes, onions and cones over any footprint: rings shrinking toward the centre.
fn radial_roof(g: &mut BGeom, p: &Part, seed: u64, ring: &[Vec2], y: f32, rh: f32, shape: Roof) {
    let c = geom::centroid(ring);
    let rc = roof_code(p);
    let rcol = col(p.roof_col.unwrap_or([0.45, 0.5, 0.5]), p.wall_top, seed);
    let steps = if shape == Roof::Cone { 1 } else { 8 };
    let prof = |t: f32| -> (f32, f32) {
        match shape {
            Roof::Cone | Roof::Pyramidal => (1.0 - t, t),
            Roof::Onion => {
                let a = t * std::f32::consts::PI;
                ((1.0 + 0.35 * (a).sin()) * (1.0 - t).powf(0.7), t)
            }
            _ => {
                let a = t * std::f32::consts::FRAC_PI_2;
                (a.cos(), a.sin())
            }
        }
    };
    let n = ring.len();
    for k in 0..steps {
        let (t0, t1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
        let ((s0, h0), (s1, h1)) = (prof(t0), prof(t1));
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let pa0 = c + (a - c) * s0;
            let pb0 = c + (b - c) * s0;
            let pa1 = c + (a - c) * s1;
            let pb1 = c + (b - c) * s1;
            let e = (b - a).normalize_or(Vec2::X);
            slope(
                g,
                &[
                    Vec3::new(pa0.x, y + rh * h0, pa0.y),
                    Vec3::new(pb0.x, y + rh * h0, pb0.y),
                    Vec3::new(pb1.x, y + rh * h1, pb1.y),
                    Vec3::new(pa1.x, y + rh * h1, pa1.y),
                ],
                e,
                rc,
                rcol,
            );
        }
    }
}

/// A flat roof: the slab, a parapet for commercial buildings, rooftop units.
fn flat_roof(g: &mut BGeom, p: &Part, kind: Kind, seed: u64, ring: &[Vec2], holes: &[Vec<Vec2>]) {
    let rc = roof_code(p);
    let default = match p.roof_mat {
        RoofMat::Gravel => [0.5, 0.49, 0.46],
        _ => [0.62, 0.62, 0.6],
    };
    let rcol = col(p.roof_col.unwrap_or(default), p.wall_top, seed);
    let parapet = if kind.residential() || matches!(kind, Kind::Canopy | Kind::Carport | Kind::Shed | Kind::Garage) { 0.0 } else { 0.8 };
    let y = p.wall_top - parapet;
    let (pts, idx) = geom::triangulate(ring, holes);
    let b = g.v.len() as u32;
    for q in &pts {
        g.v.push(BV { p: Vec3::new(q.x, y, q.y), n: Vec3::Y, uv: [q.x, q.y], uv2: [0.0, rc.pack()], col: rcol, ground: false });
    }
    for t in idx.chunks_exact(3) {
        g.i.extend_from_slice(&[b + t[0], b + t[2], b + t[1]]);
    }
    if parapet > 0.0 {
        let inner = geom::inset(ring, 0.25);
        let wcode = Code { elem: elem::WALL, mat: p.wall as u32, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: true };
        let wcol = col(p.wall_col, p.wall_top, seed);
        let n = ring.len();
        let tc = Code { elem: elem::TRIM, mat: 1, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
        let tcol = col([0.7, 0.69, 0.66], p.wall_top, seed);
        for i in 0..n {
            let (a, bb) = (inner[i], inner[(i + 1) % n]);
            let l = a.distance(bb);
            // Inner face of the parapet (faces into the roof).
            g.quad(
                [Vec3::new(bb.x, y, bb.y), Vec3::new(a.x, y, a.y), Vec3::new(a.x, p.wall_top, a.y), Vec3::new(bb.x, p.wall_top, bb.y)],
                [[l, y], [0.0, y], [0.0, p.wall_top], [l, p.wall_top]],
                [l, wcode.pack()],
                wcol,
                [false; 4],
            );
            // Coping on top.
            let (oa, ob) = (ring[i], ring[(i + 1) % n]);
            g.quad(
                [
                    Vec3::new(oa.x, p.wall_top, oa.y),
                    Vec3::new(ob.x, p.wall_top, ob.y),
                    Vec3::new(bb.x, p.wall_top, bb.y),
                    Vec3::new(a.x, p.wall_top, a.y),
                ],
                [[0.0, 0.0], [l, 0.0], [l, 0.25], [0.0, 0.25]],
                [l, tc.pack()],
                tcol,
                [false; 4],
            );
        }
    }
    // Rooftop units on commercial and apartment roofs, a penthouse on tall ones.
    if !matches!(kind, Kind::House | Kind::Garage | Kind::Shed | Kind::Carport | Kind::Canopy) {
        let area = geom::area(ring);
        let (c, ax, half) = geom::obb(ring);
        let ec = Code { elem: elem::EQUIP, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
        let ecol = col([0.72, 0.73, 0.72], p.wall_top, seed);
        let count = ((area / 250.0) as u32).clamp(1, 8);
        for k in 0..count {
            let r1 = hash01(seed ^ (k as u64 * 7 + 1)) * 2.0 - 1.0;
            let r2 = hash01(seed ^ (k as u64 * 7 + 2)) * 2.0 - 1.0;
            let q = c + ax * r1 * (half.x - 3.0).max(0.0) + ax.perp() * r2 * (half.y - 3.0).max(0.0);
            if !geom::contains(ring, q) || geom::dist_to_ring(ring, q) < 2.0 {
                continue;
            }
            let s = 0.8 + hash01(seed ^ (k as u64 * 7 + 3)) * 1.2;
            g.boxy(q, ax, s, s * 0.7, y, y + 1.1 + s * 0.3, [0.0, ec.pack()], ecol);
        }
        if p.levels >= 8 && half.x > 8.0 && half.y > 8.0 {
            let pc = Code { elem: elem::WALL, mat: p.wall as u32, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: true };
            g.boxy(c, ax, half.x * 0.3, half.y * 0.3, y, y + 4.0, [half.x * 0.6, pc.pack()], col(p.wall_col, y + 4.0, seed));
        }
    }
}

/// Garage doors (edge, along, width): mapped ones, or an attached garage at one end of
/// the street side of bigger houses.
pub fn garages(b: &Building, doors_all: &[(usize, f32, f32, EntranceKind)]) -> Vec<(usize, f32, f32)> {
    let mut garage: Vec<(usize, f32, f32)> = doors_all.iter().filter(|d| d.3 == EntranceKind::Garage).map(|d| (d.0, d.1, d.2)).collect();
    if b.kind == Kind::House && garage.is_empty() && b.area > 150.0 && hash01(b.seed ^ 0x6a) < 0.55 {
        if let Some(&(e, at, _, _)) = doors_all.first() {
            let n = b.outline.len();
            let l = b.outline[e].distance(b.outline[(e + 1) % n]);
            let w = if l > 15.0 { 4.9 } else { 2.5 };
            let end = if at > l * 0.5 { w * 0.5 + 0.6 } else { l - w * 0.5 - 0.6 };
            if (end - at).abs() > w * 0.5 + 1.5 && l > w + 4.0 {
                garage.push((e, end, w));
            }
        }
    }
    if b.kind == Kind::Garage && garage.is_empty() {
        if let Some(&(e, at, _, _)) = doors_all.first() {
            let n = b.outline.len();
            let l = b.outline[e].distance(b.outline[(e + 1) % n]);
            if l > 2.9 {
                garage.push((e, at.clamp(1.4, l - 1.4), if l > 6.0 { 4.9f32.min(l - 0.8) } else { 2.5 }));
            }
        }
    }
    garage
}

/// Everything outside of one building.
pub fn shell(b: &Building, opt: &Options) -> BGeom {
    let mut g = BGeom::default();
    let doors_all = doors(b, &opt.street_edges);
    let openings: Vec<(usize, f32, f32, f32)> =
        doors_all.iter().filter(|d| d.3 != EntranceKind::Garage).map(|d| (d.0, d.1, d.2, if d.2 > 1.5 { 2.5 } else { 2.15 })).collect();
    let garage = garages(b, &doors_all);
    for (pi, p) in b.parts.iter().enumerate() {
        let seed = b.seed ^ pi as u64;
        let mut p = p.clone();
        if p.roof_col.is_none() {
            p.roof_col = Some(opt.roof_col);
        }
        let main = pi == 0 && b.parts.len() == 1 || p.min_h < 0.5;
        let my_openings: Vec<(usize, f32, f32, f32)> = if b.parts.len() == 1 { openings.clone() } else { Vec::new() };
        let my_garage: Vec<(usize, f32, f32)> = if b.parts.len() == 1 { garage.clone() } else { Vec::new() };
        if matches!(b.kind, Kind::Canopy | Kind::Carport) && p.min_h < 0.5 {
            canopy(&mut g, &p, b.kind, seed);
            continue;
        }
        walls(&mut g, &p.outer, &p, b.kind, seed, &my_openings, main && p.min_h < 0.5, opt.detailed, &my_garage);
        for h in &p.holes {
            let rev: Vec<Vec2> = h.iter().rev().copied().collect();
            walls(&mut g, &rev, &p, b.kind, seed, &[], main && p.min_h < 0.5, opt.detailed, &[]);
        }
        // Underside of raised parts (overhangs, skybridges).
        if p.min_h > 0.5 {
            let (pts, idx) = geom::triangulate(&p.outer, &p.holes);
            let base = g.v.len() as u32;
            let c = col([0.7, 0.7, 0.68], p.wall_top, seed);
            let sc = Code { elem: elem::SOFFIT, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
            for q in &pts {
                g.v.push(BV { p: Vec3::new(q.x, p.min_h, q.y), n: -Vec3::Y, uv: [q.x, q.y], uv2: [0.0, sc.pack()], col: c, ground: false });
            }
            for t in idx.chunks_exact(3) {
                g.i.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
            }
        }
        roof(&mut g, &p, b.kind, seed);
    }
    if b.parts.len() == 1 {
        house_details(&mut g, b, &doors_all, opt);
    }
    g
}

fn roof(g: &mut BGeom, p: &Part, kind: Kind, seed: u64) {
    let y = p.wall_top;
    match p.roof {
        Roof::Flat => flat_roof(g, p, kind, seed, &p.outer, &p.holes),
        Roof::Dome | Roof::Onion | Roof::Cone => radial_roof(g, p, seed, &p.outer, y, p.roof_h, p.roof),
        shape => {
            let rect = geom::rectangularity(&p.outer);
            let over = if kind.residential() || matches!(kind, Kind::Garage | Kind::Shed | Kind::Church) { 0.4 } else { 0.15 };
            let (c, ax, half) = geom::obb(&p.outer);
            if rect > 0.86 && p.holes.is_empty() {
                // Ridge along the long axis (or as tagged).
                let (ax, hl, hw) = match p.ridge {
                    Some(r) if r.dot(ax).abs() < 0.5 => (ax.perp(), half.y, half.x),
                    _ => (ax, half.x, half.y),
                };
                rect_roof(g, p, kind, seed, c, ax, hl, hw, y, p.roof_h.min(hw * 1.4), shape, over);
            } else if let Some(rects) = geom::rect_cover(&p.outer, c, ax, 0.5, 4) {
                // L, T and U shapes: a roof per wing, hipped so they meet cleanly.
                let pp = ax.perp();
                let full = half.y.max(0.5);
                for (lo, hi) in rects {
                    let rc = (lo + hi) * 0.5;
                    let centre = c + ax * rc.x + pp * rc.y;
                    let (ex, ey) = ((hi.x - lo.x) * 0.5, (hi.y - lo.y) * 0.5);
                    let (rax, hl, hw) = if ex >= ey { (ax, ex, ey) } else { (pp, ey, ex) };
                    let rh = p.roof_h * (hw / full).clamp(0.4, 1.0);
                    let wing = if shape == Roof::Gabled { Roof::Hipped } else { shape };
                    rect_roof(g, p, kind, seed, centre, rax, hl, hw, y, rh.min(hw * 1.4), wing, over);
                }
            } else if geom::area(&p.outer) < 600.0 {
                radial_roof(g, p, seed, &p.outer, y, p.roof_h.min(half.y), Roof::Pyramidal);
            } else {
                flat_roof(g, p, kind, seed, &p.outer, &p.holes);
            }
        }
    }
}

/// Canopies and carports: posts and a roof slab.
fn canopy(g: &mut BGeom, p: &Part, kind: Kind, seed: u64) {
    let pc = Code { elem: elem::POST, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
    let ccol = col([0.85, 0.86, 0.86], p.wall_top, seed);
    let h = if kind == Kind::Canopy { p.wall_top.max(4.8) } else { p.wall_top.clamp(2.4, 3.2) };
    let (c, ax, half) = geom::obb(&p.outer);
    let nx = ((half.x * 2.0) / 6.0).ceil().max(1.0) as i32;
    let ny = ((half.y * 2.0) / 8.0).ceil().max(1.0) as i32;
    for i in 0..=nx {
        for j in 0..=ny {
            if !(i == 0 || i == nx || kind == Kind::Canopy) || !(j == 0 || j == ny || kind == Kind::Canopy) {
                continue;
            }
            let q = c
                + ax * (-half.x * 0.85 + 1.7 * half.x * i as f32 / nx as f32)
                + ax.perp() * (-half.y * 0.85 + 1.7 * half.y * j as f32 / ny as f32);
            g.boxy(q, ax, 0.15, 0.15, -0.3, h, [0.0, pc.pack()], ccol);
        }
    }
    let tc = Code { elem: elem::TRIM, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
    let ring = &p.outer;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        let l = a.distance(b);
        g.quad(
            [Vec3::new(a.x, h, a.y), Vec3::new(b.x, h, b.y), Vec3::new(b.x, h + 0.7, b.y), Vec3::new(a.x, h + 0.7, a.y)],
            [[0.0, 0.0], [l, 0.0], [l, 0.7], [0.0, 0.7]],
            [l, tc.pack()],
            ccol,
            [false; 4],
        );
    }
    let (pts, idx) = geom::triangulate(ring, &[]);
    let b0 = g.v.len() as u32;
    let rc = roof_code(p);
    for q in &pts {
        g.v.push(BV { p: Vec3::new(q.x, h + 0.7, q.y), n: Vec3::Y, uv: [q.x, q.y], uv2: [0.0, rc.pack()], col: ccol, ground: false });
    }
    for t in idx.chunks_exact(3) {
        g.i.extend_from_slice(&[b0 + t[0], b0 + t[2], b0 + t[1]]);
    }
    let b1 = g.v.len() as u32;
    let sc = Code { elem: elem::SOFFIT, mat: 1, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
    for q in &pts {
        g.v.push(BV { p: Vec3::new(q.x, h, q.y), n: -Vec3::Y, uv: [q.x, q.y], uv2: [0.0, sc.pack()], col: ccol, ground: false });
    }
    for t in idx.chunks_exact(3) {
        g.i.extend_from_slice(&[b1 + t[0], b1 + t[1], b1 + t[2]]);
    }
}

/// Porches, chimneys, awnings and sign bands: the things that make a facade read.
fn house_details(g: &mut BGeom, b: &Building, doors: &[(usize, f32, f32, EntranceKind)], _opt: &Options) {
    let p = &b.parts[0];
    let r = &b.outline;
    let n = r.len();
    let seed = b.seed;
    let Some(&(e, at, w, _)) = doors.iter().find(|d| d.3 == EntranceKind::Main) else { return };
    let (a, bb) = (r[e], r[(e + 1) % n]);
    let len = a.distance(bb);
    let d = (bb - a) / len.max(1e-3);
    let out = -d.perp();
    match b.kind {
        Kind::House | Kind::Terrace => {
            // Front porch on about half of houses: deck, posts, a small roof.
            if hash01(seed ^ 0x9c) < 0.5 {
                let pw = (w + 2.6 + hash01(seed ^ 0x9d) * 3.0).min(len - 0.4);
                let depth = 1.8 + hash01(seed ^ 0x9e) * 0.8;
                let c = a + d * at + out * depth * 0.5;
                let pc = Code { elem: elem::PORCH, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
                let wood = col([0.55, 0.45, 0.35], p.wall_top, seed);
                g.boxy(c, d, pw * 0.5, depth * 0.5, -0.6, -0.05, [0.0, pc.pack()], wood);
                let post = Code { elem: elem::POST, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
                let white = col([0.93, 0.93, 0.9], p.wall_top, seed);
                let top = 2.7f32.min(p.wall_top);
                for s in [-1.0f32, 1.0] {
                    g.boxy(
                        c + d * s * (pw * 0.5 - 0.15) + out * (depth * 0.5 - 0.15),
                        d,
                        0.09,
                        0.09,
                        -0.05,
                        top,
                        [0.0, post.pack()],
                        white,
                    );
                }
                // Porch roof sloping away from the house.
                let rc = roof_code(p);
                let rcol = col(p.roof_col.unwrap_or([0.3, 0.3, 0.3]), p.wall_top, seed);
                let q = |u: f32, v: f32, y: f32| {
                    let z = a + d * (at + u) + out * v;
                    Vec3::new(z.x, y, z.y)
                };
                slope(
                    g,
                    &[
                        q(pw * 0.5, depth + 0.3, top),
                        q(-pw * 0.5, depth + 0.3, top),
                        q(-pw * 0.5, 0.0, top + 0.6),
                        q(pw * 0.5, 0.0, top + 0.6),
                    ],
                    -d,
                    rc,
                    rcol,
                );
                let sc = Code { elem: elem::SOFFIT, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
                g.quad(
                    [q(-pw * 0.5, depth + 0.3, top), q(pw * 0.5, depth + 0.3, top), q(pw * 0.5, 0.0, top), q(-pw * 0.5, 0.0, top)],
                    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    [0.0, sc.pack()],
                    white,
                    [false; 4],
                );
            } else {
                // Stoop steps.
                let c = a + d * at + out * 0.6;
                let st = col([0.62, 0.6, 0.57], p.wall_top, seed);
                let fc = Code { elem: elem::FOUNDATION, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
                g.boxy(c, d, w * 0.5 + 0.4, 0.6, -0.6, -0.3, [0.0, fc.pack()], st);
            }
            // Chimney on many houses.
            if p.roof != Roof::Flat && hash01(seed ^ 0xc4) < 0.45 {
                let (c, ax, half) = geom::obb(&p.outer);
                let q = c + ax * half.x * (0.4 + 0.3 * hash01(seed ^ 0xc5)) * if hash01(seed ^ 0xc6) < 0.5 { 1.0 } else { -1.0 };
                let cc = Code {
                    elem: elem::CHIMNEY,
                    mat: Wall::Brick as u32,
                    windows: Windows::None as u32,
                    fh: 3.0,
                    storefront: false,
                    gable: false,
                };
                let bc = col([0.55, 0.28, 0.2], p.wall_top, seed);
                g.boxy(q, ax, 0.45, 0.35, p.wall_top - 0.5, p.wall_top + p.roof_h + 0.9, [0.9, cc.pack()], bc);
            }
        }
        k if k.storefront() => {
            // Awning over the storefront, and a blank sign band (no branding).
            let aw = (len - 0.6).min(14.0);
            let c0 = a + d * (len * 0.5);
            let q = |u: f32, v: f32, y: f32| {
                let z = c0 + d * u + out * v;
                Vec3::new(z.x, y, z.y)
            };
            let gf = ground_floor(b.kind, b.kind.floor_height());
            if hash01(seed ^ 0xa1) < 0.6 {
                let ac = Code { elem: elem::AWNING, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
                let palette = [[0.55, 0.1, 0.1], [0.12, 0.28, 0.16], [0.15, 0.2, 0.35], [0.2, 0.2, 0.2], [0.75, 0.62, 0.3]];
                let acol = col(palette[(hash64(seed ^ 0xa2) % 5) as usize], p.wall_top, seed);
                g.quad(
                    [q(aw * 0.5, 1.4, gf - 1.2), q(-aw * 0.5, 1.4, gf - 1.2), q(-aw * 0.5, 0.0, gf - 0.4), q(aw * 0.5, 0.0, gf - 0.4)],
                    [[0.0, 0.0], [aw, 0.0], [aw, 1.6], [0.0, 1.6]],
                    [aw, ac.pack()],
                    acol,
                    [false; 4],
                );
                g.quad(
                    [q(-aw * 0.5, 1.4, gf - 1.2), q(aw * 0.5, 1.4, gf - 1.2), q(aw * 0.5, 0.0, gf - 0.4), q(-aw * 0.5, 0.0, gf - 0.4)],
                    [[0.0, 0.0], [aw, 0.0], [aw, 1.6], [0.0, 1.6]],
                    [aw, ac.pack()],
                    acol,
                    [false; 4],
                );
            }
            let sc = Code { elem: elem::SIGN, mat: 0, windows: Windows::None as u32, fh: 3.0, storefront: false, gable: false };
            let scol = col([0.18, 0.2, 0.22], p.wall_top, seed);
            let sw = (len * 0.6).min(10.0);
            g.quad(
                [q(-sw * 0.5, 0.06, gf - 0.35), q(sw * 0.5, 0.06, gf - 0.35), q(sw * 0.5, 0.06, gf + 0.45), q(-sw * 0.5, 0.06, gf + 0.45)],
                [[0.0, 0.0], [sw, 0.0], [sw, 0.8], [0.0, 0.8]],
                [sw, sc.pack()],
                scol,
                [false; 4],
            );
        }
        _ => {}
    }
}
