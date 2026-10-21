//! Surface parking lots (OpenStreetMap `amenity=parking`): paved surface, double rows
//! of striped stalls laid along the mapped aisles (or the lot's long axis), accessible
//! stalls with hatched access aisles and signs nearest the buildings, landscaped islands
//! at row ends and medians between rows, wheel stops and light poles.

use super::build::{Geom, H, Local, Prop, PropKind, V, clip_rect, elem, flag, hash01, hash64, heading_of, pack, point_in_ring, prism};
use bevy::math::Vec2;
use usx_core::osm::{OsmTile, Tags};

/// Stall width and depth, aisle width (two-way, 90 degree stalls).
const STALL_W: f32 = 2.7;
const STALL_D: f32 = 5.5;
const AISLE: f32 = 7.3;
const MODULE: f32 = AISLE + 2.0 * STALL_D;
/// Surface cells (so the lot follows the ground).
const CELL: f32 = 12.0;

/// Paint and surface kinds in `uv2[0]` for lot geometry (see road.wgsl).
pub mod kind {
    pub const SURFACE: f32 = 0.0;
    pub const WHITE: f32 = -2.0;
    pub const ACCESSIBLE: f32 = -3.0;
    pub const HATCH: f32 = -4.0;
    pub const GRASS: f32 = -6.0;
}

pub struct LotOut {
    /// Geometry with a point to choose its chunk by.
    pub pieces: Vec<(Vec2, Geom)>,
    pub props: Vec<Prop>,
    pub stalls: usize,
}

fn surface_code(t: &Tags) -> u32 {
    match t.get("surface") {
        Some("concrete" | "concrete:plates") => 1,
        Some("paving_stones" | "sett" | "bricks" | "cobblestone") => 2,
        Some("gravel" | "fine_gravel" | "compacted" | "pebblestone") => 3,
        Some("dirt" | "ground" | "grass" | "earth" | "unpaved" | "sand") => 4,
        _ => 0,
    }
}

/// Accessible stalls for a lot of `n` stalls (2010 ADA Standards, 208.2).
pub fn accessible_needed(n: usize) -> usize {
    match n {
        0 => 0,
        1..=25 => 1,
        26..=50 => 2,
        51..=75 => 3,
        76..=100 => 4,
        101..=150 => 5,
        151..=200 => 6,
        201..=300 => 7,
        301..=400 => 8,
        401..=500 => 9,
        501..=1000 => n.div_ceil(50),
        _ => 20 + (n - 1000).div_ceil(100),
    }
}

struct Stall {
    /// Front centre (on the aisle side) and the direction into the stall.
    front: Vec2,
    into: Vec2,
    along: Vec2,
    accessible: bool,
    hatch: bool,
    dist_road: f32,
}

#[allow(clippy::too_many_arguments)]
fn vtx(p: Vec2, dy: f32, uv: [f32; 2], k: f32, surface: u32) -> V {
    let pk = pack(elem::JUNCTION, 0, flag::LOT | (surface << flag::SURFACE_SHIFT));
    V { p, h: H::Ground { off: 0.0 }, dy, uv, uv2: [k, pk.0], col: [0.0, 0.0, pk.1, 0.0] }
}

/// A flat quad on the lot (a, b, c, d counter-clockwise from above) with local uvs.
fn flat(g: &mut Geom, q: [Vec2; 4], uv: [[f32; 2]; 4], dy: f32, k: f32) {
    let ids: Vec<u32> = (0..4).map(|i| g.push(vtx(q[i], dy, uv[i], k, 0))).collect();
    g.tri(ids[0], ids[2], ids[1]);
    g.tri(ids[0], ids[3], ids[2]);
}

/// A painted line between two points.
fn line(g: &mut Geom, a: Vec2, b: Vec2, w: f32, dy: f32) {
    let d = (b - a).normalize_or(Vec2::X);
    let n = Vec2::new(-d.y, d.x) * w * 0.5;
    let len = a.distance(b);
    flat(g, [a - n, b - n, b + n, a + n], [[0.0, 0.0], [len, 0.0], [len, w], [0.0, w]], dy, kind::WHITE);
}

/// Every lot in the tile.
pub fn lots(
    osm: &OsmTile,
    local: &Local,
    bounds: (f32, f32, f32, f32),
    in_tile: &dyn Fn(Vec2) -> bool,
    nearest_road: &dyn Fn(Vec2) -> Option<(Vec2, f32, Vec2)>,
) -> LotOut {
    let mut out = LotOut { pieces: Vec::new(), props: Vec::new(), stalls: 0 };
    // Mapped aisles, to line the rows up with.
    let aisles: Vec<Vec<Vec2>> = osm
        .ways
        .iter()
        .filter(|w| w.tags.is("highway", "service") && w.tags.is("service", "parking_aisle"))
        .map(|w| w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect())
        .collect();
    for area in osm.areas.iter().filter(|a| usx_core::osm::is_parking(&a.tags)) {
        let t = &area.tags;
        if t.get("building").is_some()
            || matches!(t.get("parking"), Some("multi-storey" | "underground" | "rooftop" | "garage_boxes" | "carports"))
        {
            continue;
        }
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
        let area_m2: f32 = outers.iter().map(|r| ring_area(r)).sum::<f32>() - inners.iter().map(|r| ring_area(r)).sum::<f32>();
        if area_m2 < 60.0 {
            continue;
        }
        // Only the tile that holds the lot's first vertex builds it.
        if !in_tile(outers[0][0]) {
            continue;
        }
        let seed = hash64(area.id as u64);
        let surface = surface_code(t);
        let inside = |p: Vec2| outers.iter().any(|r| point_in_ring(r, p)) && !inners.iter().any(|r| point_in_ring(r, p));
        let mut g = Geom::default();
        surface_cells(&mut g, &outers, &inners, bounds, surface);
        let centre = outers[0].iter().copied().sum::<Vec2>() / outers[0].len() as f32;
        // Paint only on paved lots big enough for a row.
        let paved = surface <= 2;
        if paved && area_m2 > 250.0 {
            let stalls = layout(&mut g, &mut out.props, &outers, &inners, &aisles, &inside, nearest_road, seed, area.id);
            out.stalls += stalls;
        }
        out.pieces.push((centre, g));
    }
    out
}

fn ring_area(r: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for i in 0..r.len() {
        let (p, q) = (r[i], r[(i + 1) % r.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    (a * 0.5).abs()
}

/// The paved surface in cells clipped to the lot.
fn surface_cells(g: &mut Geom, outers: &[Vec<Vec2>], inners: &[Vec<Vec2>], bounds: (f32, f32, f32, f32), surface: u32) {
    let (x0, x1, z0, z1) = bounds;
    let (mut bx0, mut bx1, mut bz0, mut bz1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for p in outers.iter().flatten() {
        bx0 = bx0.min(p.x);
        bx1 = bx1.max(p.x);
        bz0 = bz0.min(p.y);
        bz1 = bz1.max(p.y);
    }
    // Lots are built whole by one tile, a little past its edge.
    let pad = 400.0;
    let (bx0, bx1, bz0, bz1) = (bx0.max(x0 - pad), bx1.min(x1 + pad), bz0.max(z0 - pad), bz1.min(z1 + pad));
    if bx0 >= bx1 || bz0 >= bz1 {
        return;
    }
    let (c0x, c0z) = ((bx0 / CELL).floor() as i32, (bz0 / CELL).floor() as i32);
    let (c1x, c1z) = ((bx1 / CELL).floor() as i32, (bz1 / CELL).floor() as i32);
    for cz in c0z..=c1z {
        for cx in c0x..=c1x {
            let (qx0, qz0, qx1, qz1) = (cx as f32 * CELL, cz as f32 * CELL, (cx + 1) as f32 * CELL, (cz + 1) as f32 * CELL);
            let mut polys: Vec<Vec<Vec2>> = Vec::new();
            for r in outers {
                let c = clip_rect(r, qx0, qz0, qx1, qz1);
                if c.len() >= 3 {
                    polys.push(c);
                }
            }
            if polys.is_empty() {
                continue;
            }
            let holes: Vec<Vec<Vec2>> = inners.iter().map(|r| clip_rect(r, qx0, qz0, qx1, qz1)).filter(|c| c.len() >= 3).collect();
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
                for k in 0..flat.len() / 2 {
                    let p = Vec2::new(flat[2 * k], flat[2 * k + 1]);
                    g.v.push(vtx(p, 0.05, [p.x, p.y], kind::SURFACE, surface));
                }
                for t in tris.chunks_exact(3) {
                    // Facing up means clockwise in (x, z) numbers (z points south).
                    let p = |k: usize| Vec2::new(flat[2 * t[k]], flat[2 * t[k] + 1]);
                    let cw = (p(1) - p(0)).perp_dot(p(2) - p(0)) < 0.0;
                    if cw {
                        g.tri(base + t[0] as u32, base + t[1] as u32, base + t[2] as u32);
                    } else {
                        g.tri(base + t[0] as u32, base + t[2] as u32, base + t[1] as u32);
                    }
                }
            }
        }
    }
}

/// Runs (u0, u1) along `u` where `ok(u)` holds, sampled every half metre.
fn runs(u0: f32, u1: f32, ok: &dyn Fn(f32) -> bool) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut start: Option<f32> = None;
    let mut u = u0;
    while u <= u1 + 0.25 {
        let good = u <= u1 && ok(u);
        match (good, start) {
            (true, None) => start = Some(u),
            (false, Some(s)) => {
                out.push((s, u - 0.5));
                start = None;
            }
            _ => {}
        }
        u += 0.5;
    }
    out
}

/// Rows of stalls, paint, islands, medians, wheel stops, lights and signs.
#[allow(clippy::too_many_arguments)]
fn layout(
    g: &mut Geom,
    props: &mut Vec<Prop>,
    outers: &[Vec<Vec2>],
    inners: &[Vec<Vec2>],
    aisles: &[Vec<Vec2>],
    inside: &dyn Fn(Vec2) -> bool,
    nearest_road: &dyn Fn(Vec2) -> Option<(Vec2, f32, Vec2)>,
    seed: u64,
    id: i64,
) -> usize {
    let _ = inners;
    // Orientation: the longest mapped aisle inside the lot, else the lot's long axis.
    let mut best: Option<(f32, Vec2, Vec2)> = None;
    for a in aisles {
        for s in a.windows(2) {
            let mid = (s[0] + s[1]) * 0.5;
            let l = s[0].distance(s[1]);
            if l > 8.0 && inside(mid) && best.is_none_or(|b| l > b.0) {
                best = Some((l, (s[1] - s[0]) / l, mid));
            }
        }
    }
    let ring = &outers[0];
    let (ax, anchor) = match best {
        Some((_, d, m)) => (d, Some(m)),
        None => {
            // Longest edge direction (lots are usually drawn along their streets).
            let mut b = (0.0f32, Vec2::X);
            for i in 0..ring.len() {
                let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                let l = p.distance(q);
                if l > b.0 {
                    b = (l, (q - p) / l);
                }
            }
            (b.1, None)
        }
    };
    let nrm = Vec2::new(-ax.y, ax.x);
    let (mut u0, mut u1, mut t0, mut t1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for p in outers.iter().flatten() {
        u0 = u0.min(p.dot(ax));
        u1 = u1.max(p.dot(ax));
        t0 = t0.min(p.dot(nrm));
        t1 = t1.max(p.dot(nrm));
    }
    // Aisle centres: through the mapped aisle, or so the first row starts at the edge.
    let first = match anchor {
        Some(m) => {
            let c = m.dot(nrm);
            c - ((c - t0) / MODULE).floor() * MODULE
        }
        None => t0 + 0.4 + STALL_D + AISLE * 0.5,
    };
    let at = |u: f32, t: f32| ax * u + nrm * t;
    // Is a point on a mapped road (other than the lot's own aisles, which are fine to
    // park beside)?
    let on_road = |p: Vec2, margin: f32| nearest_road(p).is_some_and(|(_, d, _)| d < margin);
    let mut stalls: Vec<Stall> = Vec::new();
    let mut bands: Vec<(f32, i32, Vec<(f32, f32)>)> = Vec::new(); // aisle centre, side, runs
    let mut tc = first;
    while tc - AISLE * 0.5 < t1 {
        for side in [-1.0f32, 1.0] {
            let front = tc + side * AISLE * 0.5;
            let back = tc + side * (AISLE * 0.5 + STALL_D);
            let ok = |u: f32| {
                inside(at(u, tc))
                    && inside(at(u, front))
                    && inside(at(u, back - side * 0.3))
                    && inside(at(u, (front + back) * 0.5))
                    && !on_road(at(u, (front + back) * 0.5), 0.6)
                    && !on_road(at(u, back - side * 0.4), 0.2)
            };
            let rs: Vec<(f32, f32)> = runs(u0, u1, &ok).into_iter().filter(|r| r.1 - r.0 >= STALL_W * 2.0 + 0.6).collect();
            bands.push((tc, side as i32, rs.clone()));
            for (r0, r1) in rs {
                let island = r1 - r0 > STALL_W * 9.0;
                let (s0, s1) = if island { (r0 + 2.4, r1 - 2.4) } else { (r0 + 0.3, r1 - 0.3) };
                let n = ((s1 - s0) / STALL_W).floor() as i32;
                if n < 1 {
                    continue;
                }
                let start = (s0 + s1) * 0.5 - n as f32 * STALL_W * 0.5;
                // Stripes between stalls, and the back line.
                for k in 0..=n {
                    let u = start + k as f32 * STALL_W;
                    line(g, at(u, front), at(u, back), 0.1, 0.075);
                }
                line(g, at(start, back - side * 0.05), at(start + n as f32 * STALL_W, back - side * 0.05), 0.1, 0.075);
                for k in 0..n {
                    let u = start + (k as f32 + 0.5) * STALL_W;
                    let f = at(u, front);
                    let d = nearest_road(f).map(|x| x.1).unwrap_or(1e4);
                    stalls.push(Stall { front: f, into: nrm * side, along: ax, accessible: false, hatch: false, dist_road: d });
                }
                // Islands at the row ends: curbed, planted, with a light at one end.
                if island {
                    for (k, uc) in [r0 + 1.1, r1 - 1.1].into_iter().enumerate() {
                        let c = at(uc, (front + back) * 0.5);
                        prism(g, c, nrm, STALL_D * 0.5 - 0.15, 0.95, (H::Ground { off: 0.0 }, 0.0), (H::Ground { off: 0.0 }, 0.16), None);
                        flat(
                            g,
                            island_quad(c, nrm, ax, STALL_D * 0.5 - 0.3, 0.8),
                            [[0.0, 0.0], [1.6, 0.0], [1.6, 5.0], [0.0, 5.0]],
                            0.165,
                            kind::GRASS,
                        );
                        if k == 0 && side > 0.0 && hash01(seed ^ (r0 as i64 as u64)) < 0.85 {
                            props.push(Prop {
                                kind: PropKind::Lamp { arm: 0.6 },
                                p: at(uc, back - side * 0.8),
                                h: H::Ground { off: 0.0 },
                                heading: heading_of(-nrm * side),
                                id: hash64(seed ^ 0x1a ^ (uc as i64 as u64)),
                                dy: 0.16,
                            });
                        }
                    }
                }
            }
        }
        tc += MODULE;
    }
    // Medians between back-to-back rows in some lots: a planted curb strip where both
    // rows run.
    if hash01(seed ^ 0x3ed) < 0.35 {
        for i in 0..bands.len() {
            let (tc, side, ref ra) = bands[i];
            if side != 1 {
                continue;
            }
            let Some((_, _, rb)) = bands.iter().find(|b| (b.0 - (tc + MODULE)).abs() < 0.01 && b.1 == -1) else { continue };
            let back = tc + AISLE * 0.5 + STALL_D;
            for &(a0, a1) in ra {
                for &(b0, b1) in rb {
                    let (m0, m1) = (a0.max(b0) + 2.4, a1.min(b1) - 2.4);
                    if m1 - m0 > 6.0 {
                        let c = at((m0 + m1) * 0.5, back);
                        prism(g, c, ax, (m1 - m0) * 0.5, 0.6, (H::Ground { off: 0.0 }, 0.0), (H::Ground { off: 0.0 }, 0.15), None);
                        flat(
                            g,
                            island_quad(c, ax, nrm, (m1 - m0) * 0.5 - 0.15, 0.45),
                            [[0.0, 0.0], [0.9, 0.0], [0.9, m1 - m0], [0.0, m1 - m0]],
                            0.155,
                            kind::GRASS,
                        );
                    }
                }
            }
        }
    }
    // Accessible stalls: those farthest from the street (nearest the buildings), in
    // pairs sharing a hatched access aisle.
    let need = accessible_needed(stalls.len());
    if need > 0 {
        let mut order: Vec<usize> = (0..stalls.len()).collect();
        order.sort_by(|&a, &b| stalls[b].dist_road.total_cmp(&stalls[a].dist_road));
        let mut placed = 0;
        for &i in &order {
            if placed >= need {
                break;
            }
            if stalls[i].accessible || stalls[i].hatch {
                continue;
            }
            // A neighbour in the same row becomes the access aisle.
            let nb = (0..stalls.len()).find(|&j| {
                j != i
                    && !stalls[j].accessible
                    && !stalls[j].hatch
                    && stalls[j].into == stalls[i].into
                    && stalls[j].front.distance(stalls[i].front) < STALL_W * 1.1
            });
            let Some(j) = nb else { continue };
            stalls[i].accessible = true;
            stalls[j].hatch = true;
            placed += 1;
            // The stall on the hatch's other side shares it.
            if placed < need
                && let Some(k) = (0..stalls.len()).find(|&k| {
                    k != i
                        && k != j
                        && !stalls[k].accessible
                        && !stalls[k].hatch
                        && stalls[k].into == stalls[j].into
                        && stalls[k].front.distance(stalls[j].front) < STALL_W * 1.1
                })
            {
                stalls[k].accessible = true;
                placed += 1;
            }
        }
    }
    let wheel_stops = hash01(seed ^ 0x5709) < 0.4;
    for (si, s) in stalls.iter().enumerate() {
        let hw = STALL_W * 0.5 - 0.06;
        let q = [
            s.front - s.along * hw,
            s.front + s.along * hw,
            s.front + s.along * hw + s.into * (STALL_D - 0.1),
            s.front - s.along * hw + s.into * (STALL_D - 0.1),
        ];
        // Counter-clockwise from above whichever way the stall faces.
        let ccw = (q[1] - q[0]).perp_dot(q[3] - q[0]) > 0.0;
        let (q, uv) = if ccw {
            (q, [[0.0, 0.0], [STALL_W, 0.0], [STALL_W, STALL_D], [0.0, STALL_D]])
        } else {
            ([q[1], q[0], q[3], q[2]], [[STALL_W, 0.0], [0.0, 0.0], [0.0, STALL_D], [STALL_W, STALL_D]])
        };
        if s.accessible {
            flat(g, q, uv, 0.07, kind::ACCESSIBLE);
            // The sign at the head of the stall, facing the aisle.
            let p = s.front + s.into * (STALL_D + 0.35);
            props.push(Prop {
                kind: PropKind::ParkingSign,
                p,
                h: H::Ground { off: 0.0 },
                heading: heading_of(-s.into),
                id: hash64(seed ^ 0xacc ^ si as u64 ^ (id as u64) << 20),
                dy: 0.0,
            });
        } else if s.hatch {
            flat(g, q, uv, 0.07, kind::HATCH);
        }
        if wheel_stops && !s.hatch {
            let c = s.front + s.into * (STALL_D - 0.75);
            prism(
                g,
                c,
                s.along,
                0.9,
                0.08,
                (H::Ground { off: 0.0 }, 0.0),
                (H::Ground { off: 0.0 }, 0.12),
                Some([0.85, 0.82, 0.3]).filter(|_| s.accessible),
            );
        }
    }
    stalls.len()
}

/// A quad centred on `c`, `hl` along `d` and `hw` across.
fn island_quad(c: Vec2, d: Vec2, across: Vec2, hl: f32, hw: f32) -> [Vec2; 4] {
    let q = [c - d * hl - across * hw, c - d * hl + across * hw, c + d * hl + across * hw, c + d * hl - across * hw];
    if (q[1] - q[0]).perp_dot(q[3] - q[0]) > 0.0 { q } else { [q[1], q[0], q[3], q[2]] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ada_counts() {
        assert_eq!(accessible_needed(10), 1);
        assert_eq!(accessible_needed(60), 3);
        assert_eq!(accessible_needed(450), 9);
        assert_eq!(accessible_needed(800), 16);
        assert_eq!(accessible_needed(1500), 25);
    }

    #[test]
    fn rectangular_lot_gets_rows() {
        let local = Local::new(39.7, -105.0);
        // An 80 x 60 m lot.
        let pts = [(0.0f32, 0.0f32), (80.0, 0.0), (80.0, 60.0), (0.0, 60.0), (0.0, 0.0)];
        let ring: Vec<(f64, f64)> = pts.iter().map(|&(x, z)| local.ll(Vec2::new(x, z))).collect();
        let tags = Tags(vec![("amenity".into(), "parking".into())]);
        let osm = OsmTile { areas: vec![usx_core::osm::Area { id: 7, outer: vec![ring], inner: vec![], tags }], ..Default::default() };
        let out = lots(&osm, &local, (-1000.0, 1000.0, -1000.0, 1000.0), &|_| true, &|_| None);
        assert_eq!(out.pieces.len(), 1);
        // Three modules of two rows, ~28 stalls a row.
        assert!(out.stalls > 120 && out.stalls < 200, "{}", out.stalls);
        assert!(out.props.iter().filter(|p| p.kind == PropKind::ParkingSign).count() >= 5);
    }
}
