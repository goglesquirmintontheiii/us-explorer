//! Building interiors: floors and ceilings, the inside of the outer walls with real
//! window openings, partition walls with doors, stairs, elevators, and furnished rooms,
//! built on a worker thread from the floor plan and only for buildings you're close to
//! and can see into (or are inside).

use super::build::Built;
use super::exterior;
use super::furniture::{self, Item, Kit, Spec, WHITE, rgb};
use super::geom;
use super::model::{Building, Kind, hash01, hash64};
use super::plan::{self, CELL, FloorPlan, Plan, RoomKind};
use super::textures::M;
use super::{Buildings, PlacedRef};
use crate::globe::{GeoAnchor, Viewer};
use crate::roads::build::Local;
use crate::walkable::{SFloor, SRamp, SWall, Solid, Walkables};
use bevy::math::{Affine3A, DVec3, Quat, Vec2, Vec3};
use bevy::prelude::*;
use std::sync::Arc;
use usx_core::geo::{EnuFrame, Geodetic};
use usx_core::net::WorkPool;
use usx_core::tile::TileId;

/// The building the player is inside, if any.
#[derive(Resource, Default)]
pub struct Inside(pub Option<(TileId, u32)>);

/// Everything an interior needs, in building-local metres (x east, y up from the
/// ground floor, z south, origin at the footprint's centroid).
pub struct InteriorData {
    pub kit: Kit,
    pub specs: Vec<Spec>,
    pub walls: Vec<SWall>,
    pub floors: Vec<SFloor>,
    pub ramps: Vec<SRamp>,
    pub lift: Option<LiftData>,
    pub doors: Vec<DoorData>,
    /// Height (above the ground floor) the floors were chosen around.
    pub near_y: f32,
    /// More floors than were built.
    pub partial: bool,
}

/// An interior door: where its hinge is, how wide, which way it swings.
pub struct DoorData {
    pub hinge: Vec3,
    pub along: Vec3,
    pub width: f32,
    pub id: u32,
    pub kind: DoorStyle,
    pub colour: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DoorStyle {
    Panel,
    Glass,
    Metal,
}

pub struct LiftData {
    /// Car footprint centre and size (local), door direction (local, unit).
    pub centre: Vec2,
    pub size: Vec2,
    pub door_dir: Vec2,
    pub floors: Vec<f32>,
    pub id: u32,
    /// Middle of the shaft's doorway and the direction along it (local).
    pub door_mid: Vec2,
    pub door_along: Vec2,
    /// Hall call buttons: position (local) and the floor they call to.
    pub calls: Vec<(Vec3, usize)>,
}

/// Exterior door ids, shared by the door entities and the walls they open.
pub fn ext_door_id(key: (TileId, u32), edge: usize, at: f32) -> u32 {
    (hash64(super::solid_key(key) ^ (edge as u64) << 20 ^ (at * 10.0) as u64) & 0x7fff_ffff) as u32
}

fn local_frame(b: &Built, bld: &Building) -> (Vec2, Local) {
    let cen = geom::centroid(&bld.outline);
    let (la, lo) = b.local.ll(cen);
    (cen, Local::new(la, lo))
}

/// Collision for a building seen from outside: the walls (doorways open with their
/// doors), the ground floor, and a flat roof to stand on.
pub fn exterior_solid(b: &Built, bi: u32, p: &PlacedRef) -> Solid {
    let bld = &b.buildings[bi as usize];
    let (cen, local) = local_frame(b, bld);
    let ring: Vec<Vec2> = bld.outline.iter().map(|q| *q - cen).collect();
    let base = p.base;
    let top = bld.parts.iter().map(|q| q.wall_top).fold(0.0f32, f32::max) as f64;
    let doors = if bld.parts.len() == 1 { exterior::doors(bld, &b.street[bi as usize]) } else { Vec::new() };
    let key = (p.tile, bi);
    let mut walls = Vec::new();
    let n = ring.len();
    for e in 0..n {
        let (a, c) = (ring[e], ring[(e + 1) % n]);
        let len = a.distance(c);
        let d = (c - a) / len.max(1e-3);
        let mut cuts: Vec<(f32, f32, u32)> = doors
            .iter()
            .filter(|q| q.0 == e && q.3 != super::model::EntranceKind::Garage)
            .map(|q| (q.1 - q.2 * 0.5, q.1 + q.2 * 0.5, ext_door_id(key, e, q.1)))
            .collect();
        cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut u = 0.0;
        for (u0, u1, id) in cuts {
            if u0 > u {
                walls.push(SWall { a: a + d * u, b: a + d * u0, h0: base - 3.0, h1: base + top, door: None });
            }
            walls.push(SWall { a: a + d * u0, b: a + d * u1, h0: base - 0.5, h1: base + 2.3, door: Some(id) });
            walls.push(SWall { a: a + d * u0, b: a + d * u1, h0: base + 2.3, h1: base + top, door: None });
            u = u1;
        }
        if u < len {
            walls.push(SWall { a: a + d * u, b: c, h0: base - 3.0, h1: base + top, door: None });
        }
    }
    let mut floors = vec![SFloor { ring: ring.clone(), holes: Vec::new(), h: base, lift: None }];
    if bld.parts.iter().all(|q| q.roof == super::model::Roof::Flat) {
        let parapet = if bld.kind.residential() { 0.0 } else { 0.8 };
        floors.push(SFloor { ring: ring.clone(), holes: Vec::new(), h: base + top - parapet, lift: None });
    }
    let (mut s, mut w, mut nn, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for q in &ring {
        let (la, lo) = local.ll(*q);
        s = s.min(la);
        nn = nn.max(la);
        w = w.min(lo);
        e = e.max(lo);
    }
    // Steps up to each doorway from the ground outside.
    let mut ramps = Vec::new();
    for d in doors.iter().filter(|d| d.3 != super::model::EntranceKind::Garage) {
        let (a, c) = (ring[d.0], ring[(d.0 + 1) % n]);
        let dir = (c - a).normalize_or(Vec2::X);
        let out = -dir.perp();
        let at = a + dir * d.1;
        ramps.push(SRamp { c: at + out * 0.75, dir: -out, hl: 0.75, hw: d.2 * 0.5 + 0.2, h0: p.low.min(base - 0.05), h1: base });
    }
    Solid { local, bbox: (s, w, nn, e), walls, floors, ramps }
}

// ------------------------------------------------------------------ realise

struct Ctx<'a> {
    b: &'a Building,
    plan: &'a Plan,
    cen: Vec2,
    base: f64,
    kit: Kit,
    specs: Vec<Spec>,
    walls: Vec<SWall>,
    floors: Vec<SFloor>,
    ramps: Vec<SRamp>,
    doors: Vec<DoorData>,
    room_col: Vec<Vec<[f32; 4]>>,
    house: bool,
    /// Exterior doorways (edge, along, width, kind), as the shell cuts them.
    ext_doors: Vec<(usize, f32, f32, super::model::EntranceKind)>,
}

impl Ctx<'_> {
    /// Frame (a, b) to building-local (x, z).
    fn l(&self, f: Vec2) -> Vec2 {
        self.plan.to_local(f) - self.cen
    }
    fn v3(&self, f: Vec2, y: f32) -> Vec3 {
        let q = self.l(f);
        Vec3::new(q.x, y, q.y)
    }
    /// Local direction for a frame direction.
    fn dir(&self, d: Vec2) -> Vec2 {
        self.plan.ax * d.x + self.plan.ax.perp() * d.y
    }
}

fn paint_colour(kind: RoomKind, house: bool, seed: u64) -> [f32; 4] {
    let pick = |o: &[[f32; 4]]| o[(hash64(seed) % o.len() as u64) as usize];
    match kind {
        RoomKind::Bath | RoomKind::HalfBath | RoomKind::Restroom => {
            pick(&[rgb(0.95, 0.95, 0.93), rgb(0.8, 0.86, 0.9), rgb(0.88, 0.9, 0.86)])
        }
        RoomKind::Garage
        | RoomKind::Stock
        | RoomKind::Floor
        | RoomKind::Utility
        | RoomKind::Storage
        | RoomKind::Parking
        | RoomKind::Basement
        | RoomKind::Attic => rgb(0.78, 0.77, 0.74),
        _ if house => pick(&[
            rgb(0.95, 0.94, 0.9),
            rgb(0.86, 0.84, 0.78),
            rgb(0.78, 0.82, 0.84),
            rgb(0.8, 0.84, 0.77),
            rgb(0.93, 0.88, 0.8),
            rgb(0.72, 0.76, 0.82),
            rgb(0.9, 0.9, 0.9),
        ]),
        _ => pick(&[rgb(0.93, 0.93, 0.91), rgb(0.88, 0.88, 0.86), rgb(0.9, 0.9, 0.88)]),
    }
}

fn floor_finish(kind: RoomKind, bk: Kind, seed: u64) -> (M, [f32; 4]) {
    let r = hash01(seed);
    match kind {
        RoomKind::Kitchen => {
            if r < 0.5 {
                (M::TileFloor, WHITE)
            } else {
                (M::WoodFloor, WHITE)
            }
        }
        RoomKind::Bath | RoomKind::HalfBath | RoomKind::Laundry | RoomKind::Restroom | RoomKind::RestKitchen => (M::TileFloor, WHITE),
        RoomKind::Bedroom | RoomKind::HotelRoom => {
            if r < 0.55 {
                (M::Carpet, furniture::fabric_colour(seed ^ 7))
            } else {
                (M::WoodFloor, WHITE)
            }
        }
        RoomKind::Living | RoomKind::Dining | RoomKind::Hall => {
            if bk.residential() {
                (M::WoodFloor, WHITE)
            } else {
                (M::Carpet, rgb(0.45, 0.45, 0.48))
            }
        }
        RoomKind::Garage
        | RoomKind::Stock
        | RoomKind::Floor
        | RoomKind::Utility
        | RoomKind::Storage
        | RoomKind::Parking
        | RoomKind::Basement => (M::Concrete, WHITE),
        RoomKind::Attic => (M::Insulation, WHITE),
        RoomKind::Office | RoomKind::OpenOffice | RoomKind::Conference => (M::Carpet, rgb(0.42, 0.44, 0.48)),
        RoomKind::Lobby => (M::Marble, WHITE),
        RoomKind::Sales | RoomKind::Classroom | RoomKind::Patient | RoomKind::Break => (M::Linoleum, WHITE),
        RoomKind::Seating => {
            if r < 0.5 {
                (M::WoodFloor, WHITE)
            } else {
                (M::TileFloor, rgb(0.8, 0.75, 0.7))
            }
        }
        RoomKind::Corridor => {
            if matches!(bk, Kind::School | Kind::Hospital) {
                (M::Linoleum, WHITE)
            } else {
                (M::Carpet, rgb(0.4, 0.38, 0.42))
            }
        }
        RoomKind::Nave => (M::WoodFloor, WHITE),
        _ => (M::Concrete, WHITE),
    }
}

/// Rectangles (frame) covering a room's cells: runs per row merged downward.
fn room_rects(p: &Plan, fl: &FloorPlan, r: usize) -> Vec<(Vec2, Vec2)> {
    let (nx, ny) = (p.nx, p.ny);
    let mut runs: Vec<(usize, usize, usize, usize)> = Vec::new(); // i0, i1, j0, j1
    let mut open: Vec<(usize, usize, usize)> = Vec::new(); // i0, i1, j0
    for j in 0..=ny {
        let mut row: Vec<(usize, usize)> = Vec::new();
        if j < ny {
            let mut i = 0;
            while i < nx {
                if fl.label[j * nx + i] == r as i32 {
                    let s = i;
                    while i < nx && fl.label[j * nx + i] == r as i32 {
                        i += 1;
                    }
                    row.push((s, i));
                } else {
                    i += 1;
                }
            }
        }
        let mut next = Vec::new();
        for &(i0, i1, j0) in &open {
            if row.contains(&(i0, i1)) {
                next.push((i0, i1, j0));
            } else {
                runs.push((i0, i1, j0, j));
            }
        }
        for &(i0, i1) in &row {
            if !open.iter().any(|o| o.0 == i0 && o.1 == i1) {
                next.push((i0, i1, j));
            }
        }
        open = next;
    }
    runs.into_iter()
        .map(|(i0, i1, j0, j1)| {
            (p.lo + Vec2::new(i0 as f32 * CELL, j0 as f32 * CELL), p.lo + Vec2::new(i1 as f32 * CELL, j1 as f32 * CELL))
        })
        .collect()
}

/// The biggest rectangle inside a room (frame).
fn room_box(p: &Plan, fl: &FloorPlan, r: usize) -> Option<(Vec2, Vec2)> {
    let (nx, ny) = (p.nx, p.ny);
    let mut best = (0usize, (0usize, 0usize, 0usize, 0usize));
    let mut h = vec![0usize; nx];
    for j in 0..ny {
        for i in 0..nx {
            h[i] = if fl.label[j * nx + i] == r as i32 { h[i] + 1 } else { 0 };
        }
        for i in 0..nx {
            if h[i] == 0 {
                continue;
            }
            let mut m = h[i];
            for k in i..nx {
                m = m.min(h[k]);
                if m == 0 {
                    break;
                }
                let a = m * (k - i + 1);
                if a > best.0 {
                    best = (a, (i, j + 1 - m, k + 1, j + 1));
                }
            }
        }
    }
    (best.0 > 0).then(|| {
        let (i0, j0, i1, j1) = best.1;
        (p.lo + Vec2::new(i0 as f32 * CELL, j0 as f32 * CELL), p.lo + Vec2::new(i1 as f32 * CELL, j1 as f32 * CELL))
    })
}

/// Floor slabs, finishes, ceilings.
fn slabs(cx: &mut Ctx, fi: usize) {
    let fl = &cx.plan.floors[fi];
    let ring: Vec<Vec2> = geom::inset(&cx.b.outline, 0.15).iter().map(|q| *q - cx.cen).collect();
    // Holes: stairs arriving at this floor from below, and the lift shaft above the bottom.
    let mut holes: Vec<Vec<Vec2>> = Vec::new();
    let rect_ring = |cx: &Ctx, a0: f32, b0: f32, a1: f32, b1: f32| -> Vec<Vec2> {
        let mut r = vec![cx.l(Vec2::new(a0, b0)), cx.l(Vec2::new(a1, b0)), cx.l(Vec2::new(a1, b1)), cx.l(Vec2::new(a0, b1))];
        if geom::signed_area(&r) < 0.0 {
            r.reverse();
        }
        r
    };
    for s in &cx.plan.stairs {
        if (s.y1 - fl.y).abs() < 0.01 {
            let (a0, a1, b0, b1) = if s.along_a { (s.s0, s.s1, s.t0, s.t1) } else { (s.t0, s.t1, s.s0, s.s1) };
            holes.push(rect_ring(cx, a0, b0, a1, b1));
        }
    }
    let lowest = cx.plan.floors.first().map(|f| f.level) == Some(fl.level);
    if let Some(l) = cx.plan.lift
        && !lowest
    {
        holes.push(rect_ring(cx, l.rect.0, l.rect.1, l.rect.2, l.rect.3));
    }
    let (pts, tris) = geom::triangulate(&ring, &holes);
    let default_m = if cx.house {
        M::WoodFloor
    } else if matches!(cx.b.kind, Kind::Industrial | Kind::Warehouse | Kind::Garage | Kind::Shed | Kind::ParkingGarage) {
        M::Concrete
    } else {
        M::Linoleum
    };
    let base_m = if fl.level < 0 { M::Concrete } else { default_m };
    cx.kit.polygon(base_m, &pts, &tris, fl.y, true, WHITE);
    // The slab's underside is the ceiling below (drawn by that floor), and its edge.
    cx.floors.push(SFloor { ring: ring.clone(), holes: holes.clone(), h: cx.base + fl.y as f64, lift: None });
    // Finishes per room (5 mm up).
    for (r, room) in fl.rooms.iter().enumerate() {
        if room.cells == 0 || matches!(room.kind, RoomKind::Stair | RoomKind::Elevator) {
            continue;
        }
        let (m, c) = floor_finish(room.kind, cx.b.kind, cx.b.seed ^ (fi * 97 + r) as u64);
        if m == base_m && c == WHITE {
            continue;
        }
        for (a, b) in room_rects(cx.plan, fl, r) {
            let q = [
                cx.v3(Vec2::new(a.x, a.y), fl.y + 0.005),
                cx.v3(Vec2::new(b.x, a.y), fl.y + 0.005),
                cx.v3(Vec2::new(b.x, b.y), fl.y + 0.005),
                cx.v3(Vec2::new(a.x, b.y), fl.y + 0.005),
            ];
            // Orientation: keep the quad facing up.
            let n = (q[1] - q[0]).cross(q[3] - q[0]);
            let q = if n.y < 0.0 { [q[0], q[3], q[2], q[1]] } else { q };
            cx.kit.floor_quad(m, [q[0], q[3], q[2], q[1]], c, true);
        }
    }
    // Ceiling (stair openings going up from this floor, and the lift shaft).
    let ceil_y = fl.y + fl.h - if fl.level < 0 { 0.15 } else { 0.3 };
    let mut up_holes: Vec<Vec<Vec2>> = Vec::new();
    for s in &cx.plan.stairs {
        if (s.y0 - fl.y).abs() < 0.01 {
            let (a0, a1, b0, b1) = if s.along_a { (s.s0, s.s1, s.t0, s.t1) } else { (s.t0, s.t1, s.s0, s.s1) };
            up_holes.push(rect_ring(cx, a0, b0, a1, b1));
        }
    }
    let top = cx.plan.floors.last().map(|f| f.level) == Some(fl.level);
    if let Some(l) = cx.plan.lift
        && !top
    {
        up_holes.push(rect_ring(cx, l.rect.0, l.rect.1, l.rect.2, l.rect.3));
    }
    let (cp, ct) = geom::triangulate(&ring, &up_holes);
    let cm = if cx.house || fl.level < 0 { M::Ceiling } else { M::CeilingTile };
    let unfinished = fl.level < 0 && fl.rooms.iter().all(|r| r.kind == RoomKind::Basement || r.kind == RoomKind::Stair);
    if !unfinished {
        cx.kit.polygon(cm, &cp, &ct, ceil_y, false, WHITE);
    } else {
        // Exposed joists and the subfloor above.
        cx.kit.polygon(M::Wood, &cp, &ct, fl.y + fl.h - 0.02, false, rgb(0.8, 0.7, 0.55));
        let (c, ax, half) = geom::obb(&ring);
        let n = (half.x * 2.0 / 0.4) as i32;
        for k in 0..n {
            let x = -half.x + (k as f32 + 0.5) * 0.4;
            let q = c + ax * x;
            let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-ax.y.atan2(ax.x)), Vec3::new(q.x, fl.y + fl.h - 0.14, q.y));
            cx.kit.cube(M::Wood, xf, Vec3::new(0.05, 0.24, half.y * 2.0 - 0.2), rgb(0.85, 0.72, 0.55));
        }
    }
}

/// The inside face of the outer walls, with window openings, reveals, glass and the
/// exterior doorways.
fn outer_walls(cx: &mut Ctx, fi: usize) {
    let fl = cx.plan.floors[fi].clone();
    let b = cx.b;
    let outer: Vec<Vec2> = b.outline.iter().map(|q| *q - cx.cen).collect();
    let inner: Vec<Vec2> = geom::inset(&b.outline, 0.2).iter().map(|q| *q - cx.cen).collect();
    let n = outer.len();
    let fh = b.kind.floor_height();
    let style = b.parts[0].windows;
    let wall_top = b.parts.iter().map(|q| q.wall_top).fold(0.0f32, f32::max);
    let y0 = fl.y;
    let y1 = fl.y + fl.h - if fl.level < 0 { 0.15 } else { 0.3 };
    let doors = cx.ext_doors.clone();
    let basement = fl.level < 0;
    for e in 0..n {
        let (oa, ob) = (outer[e], outer[(e + 1) % n]);
        let (ia, ib) = (inner[e], inner[(e + 1) % n]);
        let len = oa.distance(ob);
        if len < 0.3 {
            continue;
        }
        let mut open: Vec<(f32, f32, f32, f32, bool)> = Vec::new(); // u0, u1, v0, v1, is_door
        if !basement {
            for w in exterior::window_rects(style, b.kind, len, fh, wall_top, false) {
                if w.3 > y0 + 0.05 && w.2 < y1 - 0.05 {
                    open.push((w.0, w.1, w.2.max(y0 + 0.02), w.3.min(y1 - 0.02), false));
                }
            }
            if fl.level == 0 {
                for d in doors.iter().filter(|d| d.0 == e && d.3 != super::model::EntranceKind::Garage) {
                    let h = if d.2 > 1.5 { 2.5 } else { 2.15 };
                    // Drop windows the door overlaps.
                    open.retain(|w| w.1 < d.1 - d.2 * 0.5 - 0.05 || w.0 > d.1 + d.2 * 0.5 + 0.05 || w.2 > h);
                    open.push((d.1 - d.2 * 0.5, d.1 + d.2 * 0.5, 0.0, h, true));
                }
                for g in exterior::garages(b, &doors).iter().filter(|g| g.0 == e) {
                    open.retain(|w| w.1 < g.1 - g.2 * 0.5 || w.0 > g.1 + g.2 * 0.5);
                }
            }
        }
        open.sort_by(|a, c| a.0.total_cmp(&c.0));
        // Colour by the room behind each stretch.
        let at = |u: f32| -> Vec2 { ia + (ib - ia) * (u / len) };
        let inward = (ib - ia).normalize_or(Vec2::X).perp();
        let colour_at = |cx: &Ctx, u: f32| -> ([f32; 4], M) {
            let p = at(u) + inward * 0.45 + cx.cen;
            let f = cx.plan.to_frame(p);
            match cx.plan.room_at(&fl, f) {
                Some(r) => {
                    let k = fl.rooms[r].kind;
                    if basement && matches!(k, RoomKind::Basement | RoomKind::Utility)
                        || matches!(k, RoomKind::Garage | RoomKind::Floor | RoomKind::Parking) && !cx.house
                    {
                        (WHITE, M::Concrete)
                    } else {
                        (cx.room_col[fi][r], M::Paint)
                    }
                }
                None => (rgb(0.9, 0.9, 0.88), M::Paint),
            }
        };
        let wall_piece = |cx: &mut Ctx, u0: f32, u1: f32, v0: f32, v1: f32| {
            if u1 - u0 < 0.01 || v1 - v0 < 0.01 {
                return;
            }
            // Split long pieces at room changes (every metre).
            let steps = ((u1 - u0) / 1.0).ceil().max(1.0) as usize;
            for k in 0..steps {
                let (s0, s1) = (u0 + (u1 - u0) * k as f32 / steps as f32, u0 + (u1 - u0) * (k + 1) as f32 / steps as f32);
                let (col, m) = colour_at(cx, (s0 + s1) * 0.5);
                let (p0, p1) = (at(s0), at(s1));
                cx.kit.quad(
                    m,
                    [Vec3::new(p1.x, v0, p1.y), Vec3::new(p0.x, v0, p0.y), Vec3::new(p0.x, v1, p0.y), Vec3::new(p1.x, v1, p1.y)],
                    col,
                );
            }
        };
        let mut u = 0.0;
        for &(u0, u1, v0, v1, door) in &open {
            if u0 > u {
                wall_piece(cx, u, u0, y0, y1);
            }
            wall_piece(cx, u0.max(u), u1, y0, v0);
            wall_piece(cx, u0.max(u), u1, v1, y1);
            // Reveals: sill, head and jambs between the inner face and the outer face.
            let (q0, q1) = (at(u0), at(u1));
            let (o0, o1) = (oa + (ob - oa) * (u0 / len), oa + (ob - oa) * (u1 / len));
            let rc = if cx.house { rgb(0.95, 0.95, 0.93) } else { rgb(0.85, 0.85, 0.84) };
            let rm = M::Paint;
            if !door {
                cx.kit.quad(
                    rm,
                    [Vec3::new(q0.x, v0, q0.y), Vec3::new(q1.x, v0, q1.y), Vec3::new(o1.x, v0, o1.y), Vec3::new(o0.x, v0, o0.y)],
                    rc,
                );
                // Window sill board inside.
                let sill_c = (q0 + q1) * 0.5 + inward * 0.05;
                let d = (q1 - q0).normalize_or(Vec2::X);
                let xf =
                    Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(sill_c.x, v0 - 0.01, sill_c.y));
                cx.kit.cube(M::Paint, xf, Vec3::new(u1 - u0 + 0.12, 0.025, 0.12), rc);
                // Glass, a little in from the outer face.
                let (g0, g1) = (o0 + (q0 - o0) * 0.3, o1 + (q1 - o1) * 0.3);
                cx.kit.quad(
                    M::Glass,
                    [Vec3::new(g0.x, v0, g0.y), Vec3::new(g1.x, v0, g1.y), Vec3::new(g1.x, v1, g1.y), Vec3::new(g0.x, v1, g0.y)],
                    WHITE,
                );
            }
            cx.kit.quad(
                rm,
                [Vec3::new(q1.x, v1, q1.y), Vec3::new(q0.x, v1, q0.y), Vec3::new(o0.x, v1, o0.y), Vec3::new(o1.x, v1, o1.y)],
                rc,
            );
            cx.kit.quad(
                rm,
                [Vec3::new(q0.x, v0, q0.y), Vec3::new(o0.x, v0, o0.y), Vec3::new(o0.x, v1, o0.y), Vec3::new(q0.x, v1, q0.y)],
                rc,
            );
            cx.kit.quad(
                rm,
                [Vec3::new(o1.x, v0, o1.y), Vec3::new(q1.x, v0, q1.y), Vec3::new(q1.x, v1, q1.y), Vec3::new(o1.x, v1, o1.y)],
                rc,
            );
            u = u1;
        }
        if u < len {
            wall_piece(cx, u, len, y0, y1);
        }
        // Baseboard.
        if !basement {
            let d = (ib - ia).normalize_or(Vec2::X);
            let mut s = 0.0;
            let mut cuts: Vec<(f32, f32)> = open.iter().filter(|o| o.4 || o.2 < y0 + 0.12).map(|o| (o.0, o.1)).collect();
            cuts.push((len, len));
            for (c0, c1) in cuts {
                if c0 > s + 0.05 {
                    let m = at((s + c0) * 0.5) + inward * 0.008;
                    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(m.x, y0 + 0.05, m.y));
                    cx.kit.cube(M::Paint, xf, Vec3::new(c0 - s, 0.1, 0.016), rgb(0.96, 0.96, 0.94));
                }
                s = c1;
            }
        }
    }
}

/// Partition walls between rooms, with doors.
fn partitions(cx: &mut Ctx, fi: usize) {
    let fl = cx.plan.floors[fi].clone();
    let p = cx.plan;
    let (nx, ny) = (p.nx, p.ny);
    let y0 = fl.y;
    let y1 = fl.y + fl.h - if fl.level < 0 { 0.15 } else { 0.3 };
    let lab = |i: i64, j: i64| -> i32 {
        if i < 0 || j < 0 || i as usize >= nx || j as usize >= ny { -1 } else { fl.label[j as usize * nx + i as usize] }
    };
    let open = |a: i32, b: i32| fl.open_pairs.contains(&(a.min(b) as usize, a.max(b) as usize));
    // Runs along grid lines: (fixed coordinate, start, end, left room, right room, vertical?)
    let mut runs: Vec<(f32, f32, f32, i32, i32, bool)> = Vec::new();
    for i in 1..nx as i64 {
        let x = p.lo.x + i as f32 * CELL;
        let mut j = 0i64;
        while j < ny as i64 {
            let (l, r) = (lab(i - 1, j), lab(i, j));
            if l >= 0 && r >= 0 && l != r && !open(l, r) {
                let s = j;
                while j < ny as i64 && lab(i - 1, j) == l && lab(i, j) == r {
                    j += 1;
                }
                runs.push((x, p.lo.y + s as f32 * CELL, p.lo.y + j as f32 * CELL, l, r, true));
            } else {
                j += 1;
            }
        }
    }
    for j in 1..ny as i64 {
        let y = p.lo.y + j as f32 * CELL;
        let mut i = 0i64;
        while i < nx as i64 {
            let (l, r) = (lab(i, j - 1), lab(i, j));
            if l >= 0 && r >= 0 && l != r && !open(l, r) {
                let s = i;
                while i < nx as i64 && lab(i, j - 1) == l && lab(i, j) == r {
                    i += 1;
                }
                runs.push((y, p.lo.x + s as f32 * CELL, p.lo.x + i as f32 * CELL, l, r, false));
            } else {
                i += 1;
            }
        }
    }
    let t = 0.06;
    for (fixed, s0, s1, l, r, vertical) in runs {
        // Frame points along the run, and the side normal (from l to r).
        let pt = |s: f32| if vertical { Vec2::new(fixed, s) } else { Vec2::new(s, fixed) };
        let nrm = if vertical { Vec2::new(1.0, 0.0) } else { Vec2::new(0.0, 1.0) };
        // Doors on this run.
        let mut gaps: Vec<(f32, f32, u32, Vec2)> = Vec::new();
        for d in &fl.doors {
            let dp = if vertical {
                (d.p.x - fixed).abs() < 0.01 && d.p.y > s0 && d.p.y < s1
            } else {
                (d.p.y - fixed).abs() < 0.01 && d.p.x > s0 && d.p.x < s1
            };
            if dp {
                let c = if vertical { d.p.y } else { d.p.x };
                gaps.push((c - d.width * 0.5, c + d.width * 0.5, d.id, d.along));
            }
        }
        gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (cl, cr) = (cx.room_col[fi][l as usize], cx.room_col[fi][r as usize]);
        let ml = if matches!(fl.rooms[l as usize].kind, RoomKind::Bath | RoomKind::Restroom) && cx.house { M::Paint } else { M::Paint };
        let piece = |cx: &mut Ctx, a: f32, b: f32, v0: f32, v1: f32| {
            if b - a < 0.01 {
                return;
            }
            let (pa, pb) = (pt(a), pt(b));
            let off = nrm * t;
            // Face toward l (at -t) and toward r (at +t).
            let fa = [cx.v3(pa - off, v0), cx.v3(pb - off, v0), cx.v3(pb - off, v1), cx.v3(pa - off, v1)];
            let fb = [cx.v3(pb + off, v0), cx.v3(pa + off, v0), cx.v3(pa + off, v1), cx.v3(pb + off, v1)];
            // Make each face point into its room: check against the normal direction.
            let into_l = cx.dir(-nrm);
            let fix = |q: [Vec3; 4], into: Vec2| {
                let n = (q[1] - q[0]).cross(q[3] - q[0]);
                if n.x * into.x + n.z * into.y < 0.0 { [q[1], q[0], q[3], q[2]] } else { q }
            };
            cx.kit.quad(ml, fix(fa, into_l), cl);
            cx.kit.quad(M::Paint, fix(fb, -into_l), cr);
            // Top cap (seen from a stairwell) and baseboards.
            if v0 < y0 + 0.01 {
                for (side, col) in [(-1.0f32, cl), (1.0, cr)] {
                    let _ = col;
                    let m = (pa + pb) * 0.5 + nrm * side * (t + 0.008);
                    let d = cx.dir(pb - pa).normalize_or(Vec2::X);
                    let c = cx.l(m);
                    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(c.x, y0 + 0.05, c.y));
                    cx.kit.cube(M::Paint, xf, Vec3::new(b - a, 0.1, 0.016), rgb(0.96, 0.96, 0.94));
                }
            }
            cx.walls.push(SWall { a: cx.l(pa), b: cx.l(pb), h0: cx.base + v0 as f64, h1: cx.base + v1 as f64, door: None });
        };
        let mut s = s0;
        for &(g0, g1, id, _) in &gaps {
            piece(cx, s, g0, y0, y1);
            piece(cx, g0, g1, y0 + 2.1, y1);
            // Door leaf, hinged at g0, and a casing.
            let hinge_f = pt(g0);
            let along_f = (pt(g1) - pt(g0)).normalize_or(Vec2::X);
            let hl = cx.l(hinge_f);
            let al = cx.dir(along_f);
            let style =
                if matches!(fl.rooms[l as usize].kind, RoomKind::Stock | RoomKind::RestKitchen | RoomKind::Utility | RoomKind::Floor) {
                    DoorStyle::Metal
                } else {
                    DoorStyle::Panel
                };
            let colour = if cx.house { rgb(0.96, 0.96, 0.94) } else { rgb(0.6, 0.45, 0.3) };
            cx.doors.push(DoorData {
                hinge: Vec3::new(hl.x, y0, hl.y),
                along: Vec3::new(al.x, 0.0, al.y),
                width: g1 - g0,
                id,
                kind: style,
                colour,
            });
            cx.walls.push(SWall {
                a: cx.l(pt(g0)),
                b: cx.l(pt(g1)),
                h0: cx.base + y0 as f64,
                h1: cx.base + (y0 + 2.1) as f64,
                door: Some(id),
            });
            // Casing around the opening on both sides.
            for side in [-1.0f32, 1.0] {
                for (u, w) in [(g0 - 0.04, 0.08), (g1 + 0.04, 0.08)] {
                    let c = cx.l(pt(u) + nrm * side * (t + 0.01));
                    let d = cx.dir(along_f);
                    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(c.x, y0 + 1.07, c.y));
                    cx.kit.cube(M::Paint, xf, Vec3::new(w, 2.14, 0.02), rgb(0.97, 0.97, 0.95));
                }
                let c = cx.l((pt(g0) + pt(g1)) * 0.5 + nrm * side * (t + 0.01));
                let d = cx.dir(along_f);
                let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(c.x, y0 + 2.14, c.y));
                cx.kit.cube(M::Paint, xf, Vec3::new(g1 - g0 + 0.16, 0.08, 0.02), rgb(0.97, 0.97, 0.95));
            }
            s = g1;
        }
        piece(cx, s, s1, y0, y1);
    }
}

/// Stair flights with treads, stringers and handrails; guard rails round the
/// openings above.
fn stairs(cx: &mut Ctx) {
    let plan = cx.plan;
    for s in &plan.stairs {
        let rise = s.y1 - s.y0;
        let n = (rise / 0.19).ceil().max(2.0) as usize;
        let (len, w) = (s.s1 - s.s0, s.t1 - s.t0);
        let tread = len / n as f32;
        let fr = |sv: f32, tv: f32| if s.along_a { Vec2::new(sv, tv) } else { Vec2::new(tv, sv) };
        let dir_f = if s.along_a { Vec2::X } else { Vec2::Y };
        let d = cx.dir(dir_f);
        let yaw = -d.y.atan2(d.x);
        let m = if cx.house { M::WoodFloor } else { M::Concrete };
        for k in 0..n {
            let sc = s.s0 + (k as f32 + 0.5) * tread;
            let c = cx.l(fr(sc, (s.t0 + s.t1) * 0.5));
            let top = s.y0 + rise * (k + 1) as f32 / n as f32;
            let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(c.x, (s.y0 + top) * 0.5, c.y));
            cx.kit.cube(m, xf, Vec3::new(tread, top - s.y0, w), WHITE);
            // A nosing strip on each tread.
            let nc = cx.l(fr(s.s0 + k as f32 * tread + 0.02, (s.t0 + s.t1) * 0.5));
            let xn = Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(nc.x, top + 0.005, nc.y));
            cx.kit.cube(M::Rubber, xn, Vec3::new(0.04, 0.01, w), rgb(0.2, 0.2, 0.2));
        }
        // Handrails on both sides: posts and a sloped rail.
        for tv in [s.t0 + 0.05, s.t1 - 0.05] {
            for k in 0..=2 {
                let sv = s.s0 + len * k as f32 / 2.0;
                let y = s.y0 + rise * k as f32 / 2.0;
                let c = cx.l(fr(sv.clamp(s.s0 + 0.05, s.s1 - 0.05), tv));
                cx.kit.cyl(M::Metal, Affine3A::from_translation(Vec3::new(c.x, y, c.y)), 0.02, 0.9, 8, rgb(0.3, 0.3, 0.3), false);
            }
            let a = cx.l(fr(s.s0, tv));
            let b = cx.l(fr(s.s1, tv));
            let mid = (a + b) * 0.5;
            let slope = rise.atan2(len);
            let xf = Affine3A::from_rotation_translation(
                Quat::from_rotation_y(yaw) * Quat::from_rotation_z(slope),
                Vec3::new(mid.x, (s.y0 + s.y1) * 0.5 + 0.9, mid.y),
            );
            cx.kit.cube(M::Wood, xf, Vec3::new((len * len + rise * rise).sqrt(), 0.05, 0.05), WHITE);
        }
        let c = cx.l(fr((s.s0 + s.s1) * 0.5, (s.t0 + s.t1) * 0.5));
        let ramp_dir = cx.dir(dir_f).normalize_or(Vec2::X);
        cx.ramps.push(SRamp { c, dir: ramp_dir, hl: len * 0.5, hw: w * 0.5, h0: cx.base + s.y0 as f64, h1: cx.base + s.y1 as f64 });
        // Guard rail round the opening on the upper floor (not across the top step).
        let yt = s.y1;
        let edges = [(fr(s.s0, s.t0), fr(s.s1, s.t0)), (fr(s.s0, s.t1), fr(s.s1, s.t1)), (fr(s.s0, s.t0), fr(s.s0, s.t1))];
        for (a, b) in edges {
            let (la, lb) = (cx.l(a), cx.l(b));
            let dd = (lb - la).normalize_or(Vec2::X);
            let mid = (la + lb) * 0.5;
            let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-dd.y.atan2(dd.x)), Vec3::new(mid.x, yt + 0.95, mid.y));
            cx.kit.cube(M::Wood, xf, Vec3::new(la.distance(lb), 0.05, 0.06), WHITE);
            let np = (la.distance(lb) / 0.12) as usize;
            for k in 0..=np {
                let q = la + (lb - la) * (k as f32 / np.max(1) as f32);
                cx.kit.cube(
                    M::Paint,
                    Affine3A::from_translation(Vec3::new(q.x, yt + 0.47, q.y)),
                    Vec3::new(0.025, 0.94, 0.025),
                    rgb(0.95, 0.95, 0.93),
                );
            }
            // These rails keep you from walking into the opening.
            cx.walls.push(SWall { a: la, b: lb, h0: cx.base + yt as f64 - 0.2, h1: cx.base + yt as f64 + 1.0, door: None });
        }
    }
}

/// Ceiling lights and the spots for real lights.
fn lights(cx: &mut Ctx, fi: usize) {
    let fl = cx.plan.floors[fi].clone();
    let ceil = fl.y + fl.h - if fl.level < 0 { 0.15 } else { 0.3 };
    for (r, room) in fl.rooms.iter().enumerate() {
        if room.cells < 6 || matches!(room.kind, RoomKind::Stair | RoomKind::Elevator) {
            continue;
        }
        let Some((a, b)) = room_box(cx.plan, &fl, r) else { continue };
        let area = (b.x - a.x) * (b.y - a.y);
        let commercial = !cx.house && !matches!(room.kind, RoomKind::Bath | RoomKind::Restroom);
        let (nx, ny) = if commercial {
            (((b.x - a.x) / 4.0).round().max(1.0) as i32, ((b.y - a.y) / 4.0).round().max(1.0) as i32)
        } else {
            (1, if area > 30.0 { 2 } else { 1 })
        };
        let mut real = 0;
        for i in 0..nx {
            for j in 0..ny {
                let f = Vec2::new(a.x + (b.x - a.x) * (i as f32 + 0.5) / nx as f32, a.y + (b.y - a.y) * (j as f32 + 0.5) / ny as f32);
                let c = cx.l(f);
                if commercial {
                    let d = cx.dir(Vec2::X);
                    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)), Vec3::new(c.x, ceil - 0.01, c.y));
                    cx.kit.cube(M::LightPanel, xf, Vec3::new(1.2, 0.02, 0.6), WHITE);
                } else {
                    cx.kit.cyl(M::LightPanel, Affine3A::from_translation(Vec3::new(c.x, ceil - 0.07, c.y)), 0.17, 0.07, 16, WHITE, true);
                }
                // Real light sources: one or two per room.
                if real < 2 && (i + j) % 2 == 0 {
                    cx.specs.push(Spec::Light { at: Vec3::new(c.x, ceil - 0.2, c.y), lumens: if commercial { 2400.0 } else { 1200.0 } });
                    real += 1;
                }
            }
        }
    }
}

/// Wall slots in a room's box: places items against walls and on the floor without
/// blocking doors.
struct Room2<'a> {
    a: Vec2,
    b: Vec2,
    /// Blocked stretches per wall (south b=a.y, north b=b.y, west a=a.x, east a=b.x).
    used: [Vec<(f32, f32)>; 4],
    keepout: Vec<(Vec2, f32)>,
    floor_used: Vec<(Vec2, Vec2)>,
    _p: std::marker::PhantomData<&'a ()>,
}

impl Room2<'_> {
    fn wall_len(&self, w: usize) -> f32 {
        if w < 2 { self.b.x - self.a.x } else { self.b.y - self.a.y }
    }
    /// Frame point on wall `w` at distance `s` from its start, and the direction into
    /// the room.
    fn wall_point(&self, w: usize, s: f32, depth: f32) -> (Vec2, Vec2) {
        match w {
            0 => (Vec2::new(self.a.x + s, self.a.y + depth), Vec2::Y),
            1 => (Vec2::new(self.b.x - s, self.b.y - depth), -Vec2::Y),
            2 => (Vec2::new(self.a.x + depth, self.b.y - s), Vec2::X),
            _ => (Vec2::new(self.b.x - depth, self.a.y + s), -Vec2::X),
        }
    }
    /// Find room on wall `w` for an item `width` wide and `depth` deep, preferring
    /// `pref` (0 = start, 0.5 = middle, 1 = end).
    fn fit(&self, w: usize, width: f32, depth: f32, pref: f32) -> Option<f32> {
        let len = self.wall_len(w);
        if width > len - 0.1 {
            return None;
        }
        let mut best: Option<(f32, f32)> = None;
        let steps = ((len - width) / 0.1).ceil().max(1.0) as i32;
        for k in 0..=steps {
            let s = (len - width) * k as f32 / steps as f32;
            if self.used[w].iter().any(|&(u0, u1)| s < u1 + 0.05 && s + width > u0 - 0.05) {
                continue;
            }
            // Keep doorways clear.
            let ok = (0..=4).all(|q| {
                let (pt, into) = self.wall_point(w, s + width * q as f32 / 4.0, depth * 0.5);
                let _ = into;
                self.keepout.iter().all(|(c, r)| pt.distance(*c) > *r)
            });
            if !ok {
                continue;
            }
            let score = (s / (len - width).max(0.01) - pref).abs();
            if best.is_none_or(|b| score < b.1) {
                best = Some((s, score));
            }
        }
        best.map(|b| b.0)
    }
    fn take(&mut self, w: usize, s: f32, width: f32) {
        self.used[w].push((s, s + width));
    }
}

fn place_wall(
    cx: &mut Ctx,
    rm: &mut Room2,
    fl: &FloorPlan,
    item: Item,
    width: f32,
    depth: f32,
    walls: &[usize],
    pref: f32,
    seed: u64,
) -> Option<(usize, f32)> {
    for &w in walls {
        if let Some(s) = rm.fit(w, width, depth, pref) {
            rm.take(w, s, width);
            let (pt, into) = rm.wall_point(w, s + width * 0.5, 0.07);
            let c = cx.l(pt);
            let d = cx.dir(into);
            let yaw = (-d.x).atan2(-d.y);
            let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(c.x, fl.y, c.y));
            furniture::build(item, xf, seed, &mut cx.kit, &mut cx.specs);
            // The floor in front of it is used.
            let (p0, _) = rm.wall_point(w, s, 0.0);
            let (p1, _) = rm.wall_point(w, s + width, depth);
            rm.floor_used.push((p0.min(p1), p0.max(p1)));
            return Some((w, s));
        }
    }
    None
}

fn place_free(cx: &mut Ctx, rm: &mut Room2, fl: &FloorPlan, item: Item, size: Vec2, at: Vec2, yaw_f: Vec2, seed: u64) -> bool {
    let (lo, hi) = (at - size * 0.5, at + size * 0.5);
    if lo.x < rm.a.x + 0.1 || lo.y < rm.a.y + 0.1 || hi.x > rm.b.x - 0.1 || hi.y > rm.b.y - 0.1 {
        return false;
    }
    if rm.floor_used.iter().any(|(a, b)| lo.x < b.x && hi.x > a.x && lo.y < b.y && hi.y > a.y) {
        return false;
    }
    if rm.keepout.iter().any(|(c, r)| c.x > lo.x - r && c.x < hi.x + r && c.y > lo.y - r && c.y < hi.y + r) {
        return false;
    }
    rm.floor_used.push((lo, hi));
    let c = cx.l(at);
    let d = cx.dir(yaw_f);
    let yaw = (-d.x).atan2(-d.y);
    let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(c.x, fl.y, c.y));
    furniture::build(item, xf, seed, &mut cx.kit, &mut cx.specs);
    true
}

/// Furniture, fixtures and fittings by room.
fn furnish(cx: &mut Ctx, fi: usize) {
    let fl = cx.plan.floors[fi].clone();
    let b = cx.b;
    for (r, room) in fl.rooms.iter().enumerate() {
        if room.cells < 4 {
            continue;
        }
        let Some((a, bb)) = room_box(cx.plan, &fl, r) else { continue };
        let mut rm = Room2 { a, b: bb, used: Default::default(), keepout: Vec::new(), floor_used: Vec::new(), _p: Default::default() };
        // Keep 1 m clear in front of doors and the stairs.
        for d in fl.doors.iter().filter(|d| d.rooms.0 == r || d.rooms.1 == r) {
            rm.keepout.push((d.p, d.width * 0.5 + 0.75));
        }
        for s in &cx.plan.stairs {
            if (s.y0 - fl.y).abs() < 0.01 || (s.y1 - fl.y).abs() < 0.01 {
                let (a0, a1, b0, b1) = if s.along_a { (s.s0, s.s1, s.t0, s.t1) } else { (s.t0, s.t1, s.s0, s.s1) };
                rm.floor_used.push((Vec2::new(a0 - 0.8, b0 - 0.8), Vec2::new(a1 + 0.8, b1 + 0.8)));
                let c = Vec2::new((a0 + a1) * 0.5, (b0 + b1) * 0.5);
                rm.keepout.push((c, (a1 - a0).max(b1 - b0) * 0.5 + 0.8));
            }
        }
        if let Some((q, into)) = cx.plan.entry
            && fl.level == 0
        {
            rm.keepout.push((q + into * 0.5, 1.4));
        }
        if let Some(l) = cx.plan.lift {
            rm.floor_used.push((Vec2::new(l.rect.0 - 1.5, l.rect.1 - 1.5), Vec2::new(l.rect.2 + 1.5, l.rect.3 + 1.5)));
        }
        let seed = b.seed ^ (fi as u64 * 1000 + r as u64);
        let (w, d) = (bb.x - a.x, bb.y - a.y);
        let centre = (a + bb) * 0.5;
        let long_x = w >= d;
        let all = [0usize, 1, 2, 3];
        // Walls ordered longest first.
        let mut by_len = all;
        by_len.sort_by(|&x, &y| rm.wall_len(y).total_cmp(&rm.wall_len(x)));
        match room.kind {
            RoomKind::Living | RoomKind::Lobby if cx.house || room.kind == RoomKind::Living => {
                let seats = if w.max(d) > 4.5 { 3 } else { 2 };
                if let Some((sw, _)) =
                    place_wall(cx, &mut rm, &fl, Item::Sofa { seats }, 0.7 * seats as f32 + 0.4, 0.95, &by_len, 0.5, seed)
                {
                    let opp = [1usize, 0, 3, 2][sw];
                    place_wall(cx, &mut rm, &fl, Item::TvStand, 1.6, 0.45, &[opp], 0.5, seed ^ 1);
                    let (sp, into) = rm.wall_point(sw, rm.wall_len(sw) * 0.5, 1.6);
                    place_free(cx, &mut rm, &fl, Item::CoffeeTable, Vec2::new(1.1, 0.6), sp, into, seed ^ 2);
                    place_free(cx, &mut rm, &fl, Item::Rug { w: 2.4, d: 1.7 }, Vec2::ZERO, sp, into, seed ^ 3);
                }
                place_wall(cx, &mut rm, &fl, Item::Armchair, 0.9, 0.9, &by_len, 0.1, seed ^ 4);
                place_wall(cx, &mut rm, &fl, Item::Bookshelf, 0.9, 0.36, &by_len, 0.9, seed ^ 5);
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 1.0, seed ^ 6);
                place_wall(cx, &mut rm, &fl, Item::FloorLamp, 0.35, 0.35, &by_len, 0.0, seed ^ 7);
            }
            RoomKind::Lobby => {
                if place_free(
                    cx,
                    &mut rm,
                    &fl,
                    Item::Counter { len: 3.0 },
                    Vec2::new(3.2, 1.0),
                    centre + Vec2::new(0.0, d * 0.2),
                    Vec2::new(0.0, -1.0),
                    seed,
                ) {}
                for k in 0..3 {
                    place_wall(cx, &mut rm, &fl, Item::Armchair, 0.9, 0.9, &by_len, 0.15 + k as f32 * 0.1, seed ^ k);
                }
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 0.0, seed ^ 9);
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 1.0, seed ^ 10);
            }
            RoomKind::Kitchen => {
                let wall = by_len.iter().copied().find(|&wi| rm.wall_len(wi) > 2.2).unwrap_or(by_len[0]);
                let len = (rm.wall_len(wall) - 1.2).clamp(1.2, 4.8);
                let sink = Some(len * 0.55);
                let stove = if len > 2.6 { Some(len * 0.2 + 0.4) } else { None };
                if let Some(s) = rm.fit(wall, len, 0.65, 0.3) {
                    rm.take(wall, s, len);
                    let (pt, into) = rm.wall_point(wall, s, 0.07);
                    let c = cx.l(pt);
                    let dd = cx.dir(into);
                    let along = cx.dir(match wall {
                        0 => Vec2::X,
                        1 => -Vec2::X,
                        2 => -Vec2::Y,
                        _ => Vec2::Y,
                    });
                    // KitchenRun's origin is its left end (x from 0 to len): face into the room.
                    let yaw = (-dd.x).atan2(-dd.y);
                    let rot = Quat::from_rotation_y(yaw);
                    let left = rot * Vec3::X;
                    let start = if (left.x * along.x + left.z * along.y) > 0.0 { c } else { c + along * len };
                    let xf = Affine3A::from_rotation_translation(rot, Vec3::new(start.x, fl.y, start.y));
                    furniture::build(Item::KitchenRun { len, sink_at: sink, stove_at: stove }, xf, seed, &mut cx.kit, &mut cx.specs);
                    // Microwave on the counter near the end.
                    let mw = xf * Affine3A::from_translation(Vec3::new(len - 0.45, 0.9, 0.0));
                    furniture::build(Item::Microwave, mw, seed ^ 11, &mut cx.kit, &mut cx.specs);
                    let cm = xf * Affine3A::from_translation(Vec3::new(0.35, 0.9, 0.0));
                    furniture::build(Item::CoffeeMaker, cm, seed ^ 12, &mut cx.kit, &mut cx.specs);
                    let (p0, _) = rm.wall_point(wall, s, 0.0);
                    let (p1, _) = rm.wall_point(wall, s + len, 1.2);
                    rm.floor_used.push((p0.min(p1), p0.max(p1)));
                }
                place_wall(cx, &mut rm, &fl, Item::Fridge, 0.9, 0.75, &by_len, 0.0, seed ^ 13);
                place_wall(cx, &mut rm, &fl, Item::Dishwasher, 0.62, 0.62, &[wall], 0.9, seed ^ 14);
                if w > 3.4 && d > 3.4 {
                    place_free(
                        cx,
                        &mut rm,
                        &fl,
                        Item::DiningTable { seats: 2 },
                        Vec2::new(1.4, 2.0),
                        centre,
                        Vec2::new(0.0, -1.0),
                        seed ^ 15,
                    );
                }
            }
            RoomKind::Dining => {
                let seats = if w.min(d) > 3.4 { 6 } else { 4 };
                let dir = if long_x { Vec2::Y } else { Vec2::X };
                if !place_free(
                    cx,
                    &mut rm,
                    &fl,
                    Item::DiningTable { seats },
                    if long_x { Vec2::new(2.3, 2.2) } else { Vec2::new(2.2, 2.3) },
                    centre,
                    dir,
                    seed,
                ) {
                    place_free(cx, &mut rm, &fl, Item::DiningTable { seats: 2 }, Vec2::new(1.4, 2.0), centre, dir, seed);
                }
                place_wall(cx, &mut rm, &fl, Item::Dresser, 1.4, 0.5, &by_len, 0.5, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 0.0, seed ^ 2);
            }
            RoomKind::Bedroom | RoomKind::HotelRoom => {
                let bw = if w.min(d) > 3.4 {
                    1.55
                } else if w.min(d) > 2.8 {
                    1.4
                } else {
                    1.0
                };
                // Head against the wall that isn't the door's, longest first.
                if let Some((bwall, bs)) = place_wall(cx, &mut rm, &fl, Item::Bed { width: bw }, bw + 1.1, 2.1, &by_len, 0.5, seed) {
                    // Nightstands either side (inside the reserved span).
                    for (off, sd) in [(0.25f32, 1u64), (bw + 1.1 - 0.25, 2)] {
                        let (pt, into) = rm.wall_point(bwall, bs + off, 0.07);
                        let c = cx.l(pt);
                        let dd = cx.dir(into);
                        let xf =
                            Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y, c.y));
                        furniture::build(Item::Nightstand, xf, seed ^ sd, &mut cx.kit, &mut cx.specs);
                    }
                    // A painting above the bed.
                    let (pt, into) = rm.wall_point(bwall, bs + (bw + 1.1) * 0.5, 0.07);
                    let c = cx.l(pt);
                    let dd = cx.dir(into);
                    let xf =
                        Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y + 1.55, c.y));
                    furniture::build(Item::Painting { w: 0.9, h: 0.6 }, xf, seed ^ 3, &mut cx.kit, &mut cx.specs);
                }
                place_wall(cx, &mut rm, &fl, Item::Dresser, 1.4, 0.5, &by_len, 0.5, seed ^ 4);
                place_wall(cx, &mut rm, &fl, Item::Wardrobe, 1.2, 0.62, &by_len, 1.0, seed ^ 5);
                if room.kind == RoomKind::HotelRoom {
                    place_wall(cx, &mut rm, &fl, Item::TvStand, 1.6, 0.45, &by_len, 0.5, seed ^ 6);
                    place_wall(cx, &mut rm, &fl, Item::Toilet, 0.5, 0.7, &by_len, 0.0, seed ^ 7);
                    place_wall(cx, &mut rm, &fl, Item::Vanity, 0.95, 0.56, &by_len, 0.0, seed ^ 8);
                    place_wall(cx, &mut rm, &fl, Item::TubShower, 1.6, 0.78, &by_len, 0.0, seed ^ 9);
                } else if w.min(d) > 3.0 {
                    if place_wall(cx, &mut rm, &fl, Item::Desk, 1.4, 0.72, &by_len, 0.2, seed ^ 10).is_some() {}
                }
            }
            RoomKind::Bath | RoomKind::HalfBath => {
                place_wall(cx, &mut rm, &fl, Item::Toilet, 0.5, 0.7, &by_len, 0.1, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Vanity, 0.95, 0.56, &by_len, 0.6, seed ^ 2);
                if room.kind == RoomKind::Bath {
                    let short: Vec<usize> = by_len.iter().rev().copied().collect();
                    if place_wall(cx, &mut rm, &fl, Item::TubShower, 1.6, 0.78, &short, 0.0, seed ^ 3).is_none() {
                        place_wall(cx, &mut rm, &fl, Item::Shower, 0.92, 0.92, &by_len, 1.0, seed ^ 4);
                    }
                }
            }
            RoomKind::Laundry | RoomKind::Utility => {
                place_wall(cx, &mut rm, &fl, Item::Washer, 0.7, 0.68, &by_len, 0.0, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Dryer, 0.7, 0.68, &by_len, 0.0, seed ^ 2);
                if !cx.plan.basement || fl.level < 0 {
                    place_wall(cx, &mut rm, &fl, Item::WaterHeater, 0.6, 0.6, &by_len, 1.0, seed ^ 3);
                }
                place_wall(cx, &mut rm, &fl, Item::Shelf { len: 1.2 }, 1.2, 0.4, &by_len, 0.5, seed ^ 4);
            }
            RoomKind::Garage => {
                place_wall(cx, &mut rm, &fl, Item::Workbench, 1.8, 0.72, &by_len, 0.0, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Shelf { len: 1.8 }, 1.8, 0.42, &by_len, 1.0, seed ^ 2);
                let n = 2 + (hash01(seed ^ 3) * 6.0) as u32;
                place_wall(cx, &mut rm, &fl, Item::Boxes { n }, 2.0, 0.9, &by_len, 0.8, seed ^ 4);
                if !cx.plan.basement && hash01(seed ^ 5) < 0.5 {
                    place_wall(cx, &mut rm, &fl, Item::WaterHeater, 0.6, 0.6, &by_len, 0.0, seed ^ 6);
                }
            }
            RoomKind::Office => {
                if let Some((dw, _)) = place_wall(cx, &mut rm, &fl, Item::Desk, 1.4, 0.72, &by_len, 0.5, seed) {
                    let (pt, into) = rm.wall_point(dw, rm.wall_len(dw) * 0.5, 1.05);
                    place_free(cx, &mut rm, &fl, Item::OfficeChair, Vec2::new(0.6, 0.6), pt, -into, seed ^ 1);
                }
                place_wall(cx, &mut rm, &fl, Item::FilingCabinet, 0.46, 0.62, &by_len, 0.0, seed ^ 2);
                place_wall(cx, &mut rm, &fl, Item::Bookshelf, 0.9, 0.36, &by_len, 1.0, seed ^ 3);
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 1.0, seed ^ 4);
            }
            RoomKind::OpenOffice => {
                // Rows of cubicles with aisles.
                let (cw, cd) = (1.85, 1.75);
                let mut y = a.y + 1.2;
                let mut k = 0u64;
                while y + cd < bb.y - 1.0 {
                    let mut x = a.x + 1.2;
                    while x + cw < bb.x - 1.0 {
                        place_free(
                            cx,
                            &mut rm,
                            &fl,
                            Item::Cubicle,
                            Vec2::new(cw, cd),
                            Vec2::new(x + cw * 0.5, y + cd * 0.5),
                            Vec2::new(0.0, 1.0),
                            seed ^ k,
                        );
                        x += cw;
                        k += 1;
                    }
                    y += cd * 2.0 + 1.3;
                }
                place_wall(cx, &mut rm, &fl, Item::Printer, 0.6, 0.62, &by_len, 0.0, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Scanner, 0.5, 0.4, &by_len, 0.1, seed ^ 2);
                place_wall(cx, &mut rm, &fl, Item::FilingCabinet, 0.46, 0.62, &by_len, 0.2, seed ^ 3);
                place_wall(cx, &mut rm, &fl, Item::Plant, 0.45, 0.45, &by_len, 1.0, seed ^ 4);
            }
            RoomKind::Conference => {
                let len = (w.max(d) - 2.4).clamp(1.8, 5.0);
                place_free(
                    cx,
                    &mut rm,
                    &fl,
                    Item::ConferenceTable { len },
                    if long_x { Vec2::new(len + 0.2, 2.9) } else { Vec2::new(2.9, len + 0.2) },
                    centre,
                    if long_x { Vec2::Y } else { Vec2::X },
                    seed,
                );
                place_wall(cx, &mut rm, &fl, Item::TvStand, 1.6, 0.45, &[by_len[2], by_len[3]], 0.5, seed ^ 1);
            }
            RoomKind::Break => {
                let wall = by_len[0];
                let len = (rm.wall_len(wall) - 1.0).clamp(1.2, 3.6);
                if let Some(s) = rm.fit(wall, len, 0.65, 0.0) {
                    rm.take(wall, s, len);
                    let (pt, into) = rm.wall_point(wall, s, 0.07);
                    let c = cx.l(pt);
                    let dd = cx.dir(into);
                    let rot = Quat::from_rotation_y((-dd.x).atan2(-dd.y));
                    let along = cx.dir(match wall {
                        0 => Vec2::X,
                        1 => -Vec2::X,
                        2 => -Vec2::Y,
                        _ => Vec2::Y,
                    });
                    let left = rot * Vec3::X;
                    let start = if (left.x * along.x + left.z * along.y) > 0.0 { c } else { c + along * len };
                    let xf = Affine3A::from_rotation_translation(rot, Vec3::new(start.x, fl.y, start.y));
                    furniture::build(
                        Item::KitchenRun { len, sink_at: Some(len * 0.5), stove_at: None },
                        xf,
                        seed,
                        &mut cx.kit,
                        &mut cx.specs,
                    );
                    furniture::build(
                        Item::Microwave,
                        xf * Affine3A::from_translation(Vec3::new(0.4, 0.9, 0.0)),
                        seed ^ 1,
                        &mut cx.kit,
                        &mut cx.specs,
                    );
                    furniture::build(
                        Item::CoffeeMaker,
                        xf * Affine3A::from_translation(Vec3::new(len - 0.3, 0.9, 0.0)),
                        seed ^ 2,
                        &mut cx.kit,
                        &mut cx.specs,
                    );
                }
                place_wall(cx, &mut rm, &fl, Item::Fridge, 0.9, 0.75, &by_len, 1.0, seed ^ 3);
                place_wall(cx, &mut rm, &fl, Item::VendingMachine, 0.95, 0.88, &by_len, 0.0, seed ^ 4);
                place_wall(cx, &mut rm, &fl, Item::WaterCooler, 0.35, 0.35, &by_len, 1.0, seed ^ 5);
                place_free(cx, &mut rm, &fl, Item::CafeTable, Vec2::new(0.8, 1.7), centre + Vec2::new(0.0, 0.4), Vec2::X, seed ^ 6);
            }
            RoomKind::Restroom => {
                let n = ((w.max(d) - 1.5) / 0.95).floor().clamp(1.0, 4.0) as u32;
                let wall = by_len[0];
                for k in 0..n {
                    if let Some((ww, s)) =
                        place_wall(cx, &mut rm, &fl, Item::Toilet, 0.9, 1.5, &[wall], k as f32 / n.max(1) as f32, seed ^ k as u64)
                    {
                        // Stall partitions.
                        for e in [s, s + 0.9] {
                            let (p0, into) = rm.wall_point(ww, e, 0.75);
                            let c = cx.l(p0);
                            let dd = cx.dir(into);
                            let xf = Affine3A::from_rotation_translation(
                                Quat::from_rotation_y((-dd.x).atan2(-dd.y)),
                                Vec3::new(c.x, fl.y + 1.0, c.y),
                            );
                            cx.kit.cube(M::Metal, xf, Vec3::new(0.025, 1.6, 1.45), rgb(0.55, 0.6, 0.62));
                        }
                    }
                }
                let opp = [1usize, 0, 3, 2][wall];
                for k in 0..2 {
                    place_wall(
                        cx,
                        &mut rm,
                        &fl,
                        Item::Vanity,
                        0.95,
                        0.56,
                        &[opp, by_len[2], by_len[3]],
                        0.3 + k as f32 * 0.4,
                        seed ^ (10 + k),
                    );
                }
            }
            RoomKind::Sales => sales(cx, &mut rm, &fl, seed),
            RoomKind::Stock | RoomKind::Storage => {
                for k in 0..3 {
                    place_wall(cx, &mut rm, &fl, Item::Shelf { len: 1.8 }, 1.8, 0.42, &by_len, k as f32 * 0.4, seed ^ k);
                }
                place_wall(cx, &mut rm, &fl, Item::Boxes { n: 6 }, 2.0, 0.9, &by_len, 0.9, seed ^ 7);
            }
            RoomKind::Seating => seating(cx, &mut rm, &fl, seed),
            RoomKind::RestKitchen => {
                for (k, (item, wdt, dep)) in [
                    (Item::Grill, 1.0, 0.8),
                    (Item::Fryer, 0.6, 0.8),
                    (Item::Fryer, 0.6, 0.8),
                    (Item::PrepTable, 1.5, 0.76),
                    (Item::Fridge, 0.9, 0.75),
                    (Item::Shelf { len: 1.2 }, 1.2, 0.42),
                ]
                .into_iter()
                .enumerate()
                {
                    place_wall(cx, &mut rm, &fl, item, wdt, dep, &by_len, k as f32 / 6.0, seed ^ k as u64);
                }
                place_free(cx, &mut rm, &fl, Item::PrepTable, Vec2::new(1.6, 0.9), centre, Vec2::Y, seed ^ 20);
            }
            RoomKind::Floor => {
                if b.kind == Kind::Warehouse {
                    let mut y = a.y + 2.0;
                    let mut k = 0;
                    while y + 1.2 < bb.y - 2.0 {
                        let len = (w - 6.0).max(3.0);
                        place_free(
                            cx,
                            &mut rm,
                            &fl,
                            Item::PalletRack { len },
                            Vec2::new(len, 1.2),
                            Vec2::new(centre.x, y + 0.6),
                            Vec2::Y,
                            seed ^ k,
                        );
                        y += 4.6;
                        k += 1;
                    }
                } else {
                    let mut y = a.y + 3.0;
                    let mut k = 0u64;
                    while y + 2.0 < bb.y - 2.5 {
                        let mut x = a.x + 3.0;
                        while x + 2.6 < bb.x - 3.0 {
                            place_free(
                                cx,
                                &mut rm,
                                &fl,
                                Item::Machine,
                                Vec2::new(2.6, 1.8),
                                Vec2::new(x + 1.3, y + 0.9),
                                Vec2::Y,
                                seed ^ k,
                            );
                            x += 5.5;
                            k += 1;
                        }
                        // A conveyor along the row.
                        let (p0, p1) = (cx.l(Vec2::new(a.x + 2.5, y + 2.6)), cx.l(Vec2::new(bb.x - 2.5, y + 2.6)));
                        conveyor(cx, p0, p1, fl.y);
                        y += 6.0;
                    }
                    place_wall(cx, &mut rm, &fl, Item::Locker { n: 6 }, 1.9, 0.48, &by_len, 0.0, seed ^ 99);
                    place_wall(cx, &mut rm, &fl, Item::Workbench, 1.8, 0.72, &by_len, 1.0, seed ^ 98);
                }
            }
            RoomKind::Classroom => {
                let (front, rows_dir) = if long_x { (3usize, Vec2::new(-1.0, 0.0)) } else { (1usize, Vec2::new(0.0, -1.0)) };
                place_wall(cx, &mut rm, &fl, Item::Desk, 1.4, 0.72, &[front], 0.5, seed);
                let mut k = 0u64;
                for i in 0..6 {
                    for j in 0..5 {
                        let f = if long_x {
                            Vec2::new(bb.x - 2.8 - i as f32 * 1.2, a.y + 1.0 + j as f32 * (d - 2.0) / 4.0)
                        } else {
                            Vec2::new(a.x + 1.0 + j as f32 * (w - 2.0) / 4.0, bb.y - 2.8 - i as f32 * 1.2)
                        };
                        place_free(cx, &mut rm, &fl, Item::SchoolDesk, Vec2::new(0.7, 1.0), f, -rows_dir, seed ^ k);
                        k += 1;
                    }
                }
                // Whiteboard on the front wall.
                let (pt, into) = rm.wall_point(front, rm.wall_len(front) * 0.5, 0.02);
                let c = cx.l(pt);
                let dd = cx.dir(into);
                let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y + 1.4, c.y));
                cx.kit.cube(M::WhitePlastic, xf, Vec3::new(3.0, 1.2, 0.03), WHITE);
            }
            RoomKind::Patient => {
                place_wall(cx, &mut rm, &fl, Item::HospitalBed, 1.0, 2.1, &by_len, 0.5, seed);
                place_wall(cx, &mut rm, &fl, Item::Armchair, 0.9, 0.9, &by_len, 0.1, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Nightstand, 0.5, 0.45, &by_len, 0.9, seed ^ 2);
            }
            RoomKind::Nave => {
                let (along, n_rows) = if long_x { (Vec2::X, ((w - 6.0) / 1.0) as i32) } else { (Vec2::Y, ((d - 6.0) / 1.0) as i32) };
                let pew_len = ((if long_x { d } else { w }) * 0.5 - 1.5).clamp(1.5, 6.0);
                for i in 0..n_rows.max(0) {
                    for side in [-1.0f32, 1.0] {
                        let f = if long_x {
                            Vec2::new(a.x + 2.0 + i as f32, centre.y + side * (pew_len * 0.5 + 0.8))
                        } else {
                            Vec2::new(centre.x + side * (pew_len * 0.5 + 0.8), a.y + 2.0 + i as f32)
                        };
                        place_free(
                            cx,
                            &mut rm,
                            &fl,
                            Item::Pew { len: pew_len },
                            if long_x { Vec2::new(0.6, pew_len) } else { Vec2::new(pew_len, 0.6) },
                            f,
                            -along,
                            seed ^ (i as u64 * 2 + (side > 0.0) as u64),
                        );
                    }
                }
                let alt = if long_x { Vec2::new(bb.x - 1.5, centre.y) } else { Vec2::new(centre.x, bb.y - 1.5) };
                place_free(cx, &mut rm, &fl, Item::CoffeeTable, Vec2::new(1.2, 0.7), alt, -along, seed ^ 77);
            }
            RoomKind::Basement => {
                place_wall(cx, &mut rm, &fl, Item::Furnace, 0.62, 0.78, &by_len, 0.5, seed);
                place_wall(cx, &mut rm, &fl, Item::WaterHeater, 0.6, 0.6, &by_len, 0.6, seed ^ 1);
                place_wall(cx, &mut rm, &fl, Item::Washer, 0.7, 0.68, &by_len, 0.0, seed ^ 2);
                place_wall(cx, &mut rm, &fl, Item::Dryer, 0.7, 0.68, &by_len, 0.0, seed ^ 3);
                place_wall(cx, &mut rm, &fl, Item::Shelf { len: 1.8 }, 1.8, 0.42, &by_len, 1.0, seed ^ 4);
                place_wall(cx, &mut rm, &fl, Item::Boxes { n: 5 }, 2.0, 0.9, &by_len, 0.9, seed ^ 5);
                // Exposed pipes and wiring along the ceiling.
                let ceil = fl.y + fl.h - 0.3;
                for k in 0..3 {
                    let f0 = Vec2::new(a.x + 0.3, a.y + d * (0.25 + 0.25 * k as f32));
                    let f1 = Vec2::new(bb.x - 0.3, f0.y);
                    let (p0, p1) = (cx.l(f0), cx.l(f1));
                    let dd = (p1 - p0).normalize_or(Vec2::X);
                    let xf = Affine3A::from_rotation_translation(
                        Quat::from_rotation_y(-dd.y.atan2(dd.x)),
                        Vec3::new(p0.x, ceil - 0.05 * k as f32, p0.y),
                    );
                    furniture::build(Item::Pipe { len: p0.distance(p1), vertical: false }, xf, seed ^ (20 + k), &mut cx.kit, &mut cx.specs);
                }
            }
            RoomKind::Parking => {
                // Columns on a grid and painted stalls.
                let mut x = a.x + 2.7;
                while x < bb.x - 1.0 {
                    let mut y = a.y + 5.5;
                    while y < bb.y - 1.0 {
                        let c = cx.l(Vec2::new(x, y));
                        cx.kit.cube(
                            M::Concrete,
                            Affine3A::from_translation(Vec3::new(c.x, fl.y + fl.h * 0.5, c.y)),
                            Vec3::new(0.5, fl.h, 0.5),
                            WHITE,
                        );
                        y += 16.0;
                    }
                    for y in [a.y + 0.2, bb.y - 5.5] {
                        let (p0, p1) = (cx.v3(Vec2::new(x - 0.05, y), fl.y + 0.01), cx.v3(Vec2::new(x + 0.05, y + 5.3), fl.y + 0.01));
                        cx.kit.floor_quad(M::Paint, [p0, Vec3::new(p1.x, p0.y, p0.z), p1, Vec3::new(p0.x, p0.y, p1.z)], WHITE, true);
                    }
                    x += 2.7;
                }
            }
            _ => {}
        }
        // A light switch beside each door into the room, on the room's side.
        if !matches!(room.kind, RoomKind::Stair | RoomKind::Elevator | RoomKind::Attic) {
            for d in fl.doors.iter().filter(|d| d.rooms.0 == r || d.rooms.1 == r) {
                let nrm = d.along.perp();
                let c0 = (a + bb) * 0.5;
                let side = if (c0 - d.p).dot(nrm) >= 0.0 { 1.0 } else { -1.0 };
                let f = d.p + d.along * (d.width * 0.5 + 0.18) + nrm * side * 0.065;
                let c = cx.l(f);
                let dd = cx.dir(nrm * side);
                let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y + 1.2, c.y));
                furniture::build(Item::LightSwitch, xf, seed, &mut cx.kit, &mut cx.specs);
            }
        }
        // Outlets along the walls, ethernet in offices and bedrooms.
        if !matches!(room.kind, RoomKind::Stair | RoomKind::Elevator | RoomKind::Corridor | RoomKind::Attic) && fl.level >= -1 {
            for wi in 0..4 {
                let len = rm.wall_len(wi);
                let n = (len / 3.0).floor() as i32;
                for k in 0..n {
                    let s = (k as f32 + 0.5) * len / n as f32;
                    let (pt, into) = rm.wall_point(wi, s, -0.0);
                    if rm.keepout.iter().any(|(c, r)| pt.distance(*c) < *r * 0.8) {
                        continue;
                    }
                    let c = cx.l(pt);
                    let dd = cx.dir(into);
                    let xf =
                        Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y + 0.32, c.y));
                    let item = if k == 0
                        && matches!(
                            room.kind,
                            RoomKind::Office | RoomKind::Bedroom | RoomKind::Living | RoomKind::OpenOffice | RoomKind::Conference
                        ) {
                        Item::Ethernet
                    } else {
                        Item::Outlet
                    };
                    furniture::build(item, xf, seed, &mut cx.kit, &mut cx.specs);
                }
            }
        }
    }
}

fn sales(cx: &mut Ctx, rm: &mut Room2, fl: &FloorPlan, seed: u64) {
    let (a, bb) = (rm.a, rm.b);
    let (w, d) = (bb.x - a.x, bb.y - a.y);
    let supermarket = cx.b.kind == Kind::Supermarket;
    // The entry side: checkouts there; aisles run from front to back.
    let (q, into) = cx.plan.entry.unwrap_or((Vec2::new((a.x + bb.x) * 0.5, a.y), Vec2::Y));
    let front_is_y = into.y.abs() >= into.x.abs();
    let mut by_len = [0usize, 1, 2, 3];
    by_len.sort_by(|&x, &y| rm.wall_len(y).total_cmp(&rm.wall_len(x)));
    if supermarket {
        // Fridge cases along the side walls, produce near the entry.
        let side_walls = if front_is_y { [2usize, 3] } else { [0usize, 1] };
        for sw in side_walls {
            let len = (rm.wall_len(sw) - 3.0).clamp(2.0, 12.0);
            place_wall(cx, rm, fl, Item::FridgeCase { len }, len, 0.85, &[sw], 0.6, seed ^ sw as u64);
        }
    } else {
        for wi in by_len {
            for k in 0..3 {
                place_wall(cx, rm, fl, Item::Shelf { len: 1.8 }, 1.8, 0.42, &[wi], 0.2 + k as f32 * 0.3, seed ^ (wi * 10 + k) as u64);
            }
        }
    }
    // Checkouts in a row just inside the front.
    let n_check = if supermarket { ((if front_is_y { w } else { d }) / 4.0).floor().clamp(1.0, 8.0) as i32 } else { 1 };
    for k in 0..n_check {
        let t = (k as f32 + 1.0) / (n_check as f32 + 1.0);
        let f = if front_is_y {
            let y = if into.y > 0.0 { a.y + 3.0 } else { bb.y - 3.0 };
            Vec2::new(a.x + w * t, y)
        } else {
            let x = if into.x > 0.0 { a.x + 3.0 } else { bb.x - 3.0 };
            Vec2::new(x, a.y + d * t)
        };
        if (f - q).length() < 2.0 {
            continue;
        }
        let along = if front_is_y { Vec2::Y } else { Vec2::X };
        place_free(
            cx,
            rm,
            fl,
            Item::Checkout,
            if front_is_y { Vec2::new(0.9, 2.6) } else { Vec2::new(2.6, 0.9) },
            f,
            along,
            seed ^ (40 + k as u64),
        );
    }
    // Aisles: gondolas running front to back, 2.6 m apart.
    let (span, depth) = if front_is_y { (w, d) } else { (d, w) };
    let glen = (depth - 9.0).clamp(2.0, 18.0);
    let n_aisles = ((span - 3.0) / 2.7).floor() as i32;
    for k in 0..n_aisles.max(0) {
        let s = 1.8 + (k as f32 + 0.5) * (span - 3.6) / n_aisles as f32;
        let f = if front_is_y {
            Vec2::new(a.x + s, (a.y + bb.y) * 0.5 + if into.y > 0.0 { 2.0 } else { -2.0 })
        } else {
            Vec2::new((a.x + bb.x) * 0.5 + if into.x > 0.0 { 2.0 } else { -2.0 }, a.y + s)
        };
        let (size, dir) = if front_is_y { (Vec2::new(1.0, glen), Vec2::X) } else { (Vec2::new(glen, 1.0), Vec2::Y) };
        place_free(cx, rm, fl, Item::Gondola { len: glen }, size, f, dir, seed ^ (100 + k as u64));
    }
    if supermarket {
        for k in 0..3 {
            let f = q + into * 6.0 + into.perp() * (k as f32 - 1.0) * 2.0;
            place_free(cx, rm, fl, Item::ProduceBin, Vec2::new(1.5, 1.1), f, into, seed ^ (200 + k));
        }
    }
}

fn seating(cx: &mut Ctx, rm: &mut Room2, fl: &FloorPlan, seed: u64) {
    let (a, bb) = (rm.a, rm.b);
    let (w, d) = (bb.x - a.x, bb.y - a.y);
    let mut by_len = [0usize, 1, 2, 3];
    by_len.sort_by(|&x, &y| rm.wall_len(y).total_cmp(&rm.wall_len(x)));
    let (q, into) = cx.plan.entry.unwrap_or((Vec2::new((a.x + bb.x) * 0.5, a.y), Vec2::Y));
    // The counter (and the kitchen behind) is on the wall opposite the entry.
    let back = if into.y.abs() >= into.x.abs() {
        if into.y > 0.0 { 1usize } else { 0 }
    } else if into.x > 0.0 {
        3
    } else {
        2
    };
    let fine =
        cx.b.detail
            .as_deref()
            .is_some_and(|c| ["french", "italian", "steak_house", "seafood", "fine_dining", "japanese"].iter().any(|k| c.contains(k)));
    match cx.b.kind {
        Kind::FastFood => {
            let len = (rm.wall_len(back) * 0.6).clamp(2.0, 6.0);
            let (pt, inw) = rm.wall_point(back, rm.wall_len(back) * 0.5, 1.4);
            place_free(cx, rm, fl, Item::Counter { len }, Vec2::new(len, 0.8).max(Vec2::splat(0.8)), pt, -inw, seed);
            for (k, wi) in by_len.iter().enumerate() {
                if *wi == back {
                    continue;
                }
                for j in 0..3 {
                    place_wall(cx, rm, fl, Item::Booth, 1.6, 2.0, &[*wi], 0.15 + j as f32 * 0.35, seed ^ (k * 10 + j) as u64);
                }
            }
        }
        Kind::Cafe => {
            let (pt, inw) = rm.wall_point(back, rm.wall_len(back) * 0.5, 1.2);
            let len = (rm.wall_len(back) * 0.5).clamp(2.0, 5.0);
            if place_free(cx, rm, fl, Item::Counter { len }, Vec2::new(len, 0.8), pt, -inw, seed) {
                let c = cx.l(pt - inw * 0.0);
                let dd = cx.dir(-inw);
                let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y((-dd.x).atan2(-dd.y)), Vec3::new(c.x, fl.y + 1.04, c.y));
                furniture::build(
                    Item::EspressoMachine,
                    xf * Affine3A::from_translation(Vec3::new(-len * 0.25, 0.0, 0.3)),
                    seed ^ 1,
                    &mut cx.kit,
                    &mut cx.specs,
                );
            }
            for wi in by_len {
                place_wall(cx, rm, fl, Item::Armchair, 0.9, 0.9, &[wi], 0.2, seed ^ (wi as u64 + 30));
            }
        }
        Kind::Bar => {
            let len = (rm.wall_len(by_len[0]) - 2.0).clamp(2.0, 10.0);
            let (pt, inw) = rm.wall_point(by_len[0], rm.wall_len(by_len[0]) * 0.5, 1.6);
            if place_free(cx, rm, fl, Item::Counter { len }, Vec2::new(len, 0.8), pt, inw, seed) {
                let n = (len / 0.65) as i32;
                for k in 0..n {
                    let along = rm.wall_point(by_len[0], 0.0, 0.0).1.perp();
                    let f = pt + inw * 0.75 + along * ((k as f32 + 0.5) / n as f32 - 0.5) * len;
                    let c = cx.l(f);
                    furniture::build(
                        Item::BarStool,
                        Affine3A::from_translation(Vec3::new(c.x, fl.y, c.y)),
                        seed ^ k as u64,
                        &mut cx.kit,
                        &mut cx.specs,
                    );
                }
            }
            for j in 0..3 {
                place_wall(cx, rm, fl, Item::Booth, 1.6, 2.0, &[by_len[1]], 0.15 + j as f32 * 0.35, seed ^ (50 + j));
            }
        }
        _ => {
            // Sit-down restaurant: a small bar along one side.
            if w.max(d) > 10.0 {
                let len = 3.0;
                let (pt, inw) = rm.wall_point(by_len[0], 1.5 + len * 0.5, 1.4);
                place_free(cx, rm, fl, Item::Counter { len }, Vec2::new(len, 0.8), pt, inw, seed ^ 3);
            }
        }
    }
    // Tables on a grid in what's left.
    let step = if fine { 2.6 } else { 2.2 };
    let mut k = 0u64;
    let mut y = a.y + 1.6;
    while y < bb.y - 1.4 {
        let mut x = a.x + 1.6;
        while x < bb.x - 1.4 {
            let item = match cx.b.kind {
                Kind::Cafe => Item::CafeTable,
                Kind::FastFood | Kind::Bar => Item::RestaurantTable { cloth: false },
                _ => Item::RestaurantTable { cloth: fine || hash01(seed ^ 0xc1) < 0.3 },
            };
            let size = if matches!(item, Item::CafeTable) { Vec2::new(0.9, 1.7) } else { Vec2::splat(1.9) };
            place_free(cx, rm, fl, item, size, Vec2::new(x, y), Vec2::Y, seed ^ k);
            x += step;
            k += 1;
        }
        y += step;
    }
    let _ = q;
}

/// A roller conveyor between two points (local).
fn conveyor(cx: &mut Ctx, p0: Vec2, p1: Vec2, y: f32) {
    let d = (p1 - p0).normalize_or(Vec2::X);
    let len = p0.distance(p1);
    let mid = (p0 + p1) * 0.5;
    let rot = Quat::from_rotation_y(-d.y.atan2(d.x));
    cx.kit.cube(
        M::Rubber,
        Affine3A::from_rotation_translation(rot, Vec3::new(mid.x, y + 0.86, mid.y)),
        Vec3::new(len, 0.03, 0.6),
        rgb(0.12, 0.12, 0.12),
    );
    for side in [-1.0f32, 1.0] {
        let s = mid + d.perp() * side * 0.33;
        cx.kit.cube(
            M::Metal,
            Affine3A::from_rotation_translation(rot, Vec3::new(s.x, y + 0.82, s.y)),
            Vec3::new(len, 0.14, 0.05),
            rgb(0.6, 0.62, 0.65),
        );
    }
    let n = (len / 2.0) as i32;
    for k in 0..=n {
        let q = p0 + d * (len * k as f32 / n.max(1) as f32);
        cx.kit.cube(
            M::Metal,
            Affine3A::from_rotation_translation(rot, Vec3::new(q.x, y + 0.4, q.y)),
            Vec3::new(0.06, 0.8, 0.62),
            rgb(0.35, 0.37, 0.4),
        );
    }
    // Some boxes riding on it.
    for k in 0..(len / 3.0) as i32 {
        let q = p0 + d * (1.5 + k as f32 * 3.0);
        cx.kit.cube(
            M::Cardboard,
            Affine3A::from_rotation_translation(rot, Vec3::new(q.x, y + 1.05, q.y)),
            Vec3::new(0.45, 0.35, 0.4),
            WHITE,
        );
    }
}

/// Attic: insulation between joists, a hatch ladder, ducts, pipes and wiring.
fn attic(cx: &mut Ctx) {
    let b = cx.b;
    let p = &b.parts[0];
    let y = p.wall_top;
    let ring: Vec<Vec2> = geom::inset(&b.outline, 0.25).iter().map(|q| *q - cx.cen).collect();
    let (pts, tris) = geom::triangulate(&ring, &[]);
    cx.kit.polygon(M::Insulation, &pts, &tris, y + 0.08, true, WHITE);
    cx.floors.push(SFloor { ring: ring.clone(), holes: Vec::new(), h: cx.base + (y + 0.08) as f64, lift: None });
    // Walk boards down the middle.
    let (c, ax, half) = geom::obb(&ring);
    let rot = Quat::from_rotation_y(-ax.y.atan2(ax.x));
    cx.kit.cube(
        M::Wood,
        Affine3A::from_rotation_translation(rot, Vec3::new(c.x, y + 0.1, c.y)),
        Vec3::new(half.x * 1.6, 0.025, 0.6),
        rgb(0.85, 0.75, 0.6),
    );
    // Rafters under the roof slope (simple gable pairs).
    let rh = p.roof_h;
    let n = (half.x * 2.0 / 0.6) as i32;
    for k in 0..n {
        let x = -half.x + (k as f32 + 0.5) * 0.6;
        for side in [-1.0f32, 1.0] {
            let q = c + ax * x + ax.perp() * side * half.y * 0.5;
            let slope = (rh / half.y).atan();
            let xf =
                Affine3A::from_rotation_translation(rot * Quat::from_rotation_x(side * slope), Vec3::new(q.x, y + rh * 0.5 - 0.15, q.y));
            cx.kit.cube(M::Wood, xf, Vec3::new(0.05, 0.18, half.y * 1.05 / slope.cos().max(0.3)), rgb(0.8, 0.68, 0.5));
        }
    }
    // HVAC, a duct run, pipes and wires along the floor.
    let a0 = c - ax * half.x * 0.6;
    furniture::build(
        Item::Furnace,
        Affine3A::from_rotation_translation(rot, Vec3::new(a0.x, y + 0.08, a0.y)),
        b.seed,
        &mut cx.kit,
        &mut cx.specs,
    );
    if !cx.plan.basement {
        let a1 = c + ax * half.x * 0.5;
        furniture::build(
            Item::WaterHeater,
            Affine3A::from_rotation_translation(rot, Vec3::new(a1.x, y + 0.08, a1.y)),
            b.seed ^ 1,
            &mut cx.kit,
            &mut cx.specs,
        );
    }
    let duct = Affine3A::from_rotation_translation(rot, Vec3::new(c.x, y + 0.35, c.y));
    cx.kit.cube(M::Metal, duct, Vec3::new(half.x * 1.5, 0.3, 0.4), rgb(0.75, 0.76, 0.78));
    let pp = c + ax.perp() * half.y * 0.4 - ax * half.x * 0.8;
    furniture::build(
        Item::Pipe { len: half.x * 1.6, vertical: false },
        Affine3A::from_rotation_translation(rot, Vec3::new(pp.x, y + 0.15, pp.y)),
        b.seed ^ 2,
        &mut cx.kit,
        &mut cx.specs,
    );
    let bx = c - ax.perp() * half.y * 0.4;
    furniture::build(
        Item::Boxes { n: 4 },
        Affine3A::from_rotation_translation(rot, Vec3::new(bx.x, y + 0.1, bx.y)),
        b.seed ^ 3,
        &mut cx.kit,
        &mut cx.specs,
    );
    // Hatch ladder from the upper hall: a steep folding ladder.
    if let Some(top) = cx.plan.floors.last() {
        let hall = top.rooms.iter().position(|r| matches!(r.kind, RoomKind::Hall | RoomKind::Corridor));
        if let Some(h) = hall
            && let Some((ha, hb)) = room_box(cx.plan, top, h)
        {
            let along_a = hb.x - ha.x > hb.y - ha.y;
            let mid = (ha + hb) * 0.5 + if along_a { Vec2::new(1.4, 0.0) } else { Vec2::new(0.0, 1.4) };
            let run = 1.6;
            let (s0, s1) = if along_a { (mid.x - run * 0.5, mid.x + run * 0.5) } else { (mid.y - run * 0.5, mid.y + run * 0.5) };
            let (t0, t1) = if along_a { (mid.y - 0.3, mid.y + 0.3) } else { (mid.x - 0.3, mid.x + 0.3) };
            let s = plan::StairRun { along_a, s0, s1, t0, t1, y0: top.y, y1: y + 0.08 };
            // Ladder rails and rungs.
            let fr = |sv: f32, tv: f32| if along_a { Vec2::new(sv, tv) } else { Vec2::new(tv, sv) };
            for tv in [t0, t1] {
                let (pa, pb) = (cx.l(fr(s0, tv)), cx.l(fr(s1, tv)));
                let mid3 = Vec3::new((pa.x + pb.x) * 0.5, (s.y0 + s.y1) * 0.5, (pa.y + pb.y) * 0.5);
                let d = (pb - pa).normalize_or(Vec2::X);
                let ang = (s.y1 - s.y0).atan2(run);
                let xf = Affine3A::from_rotation_translation(Quat::from_rotation_y(-d.y.atan2(d.x)) * Quat::from_rotation_z(ang), mid3);
                cx.kit.cube(M::Wood, xf, Vec3::new((run * run + (s.y1 - s.y0).powi(2)).sqrt(), 0.08, 0.04), WHITE);
            }
            let n = ((s.y1 - s.y0) / 0.28) as i32;
            for k in 1..n {
                let t = k as f32 / n as f32;
                let q = cx.l(fr(s0 + run * t, (t0 + t1) * 0.5));
                let d = cx.dir(if along_a { Vec2::Y } else { Vec2::X });
                let xf = Affine3A::from_rotation_translation(
                    Quat::from_rotation_y(-d.y.atan2(d.x)),
                    Vec3::new(q.x, s.y0 + (s.y1 - s.y0) * t, q.y),
                );
                cx.kit.cube(M::Wood, xf, Vec3::new(0.6, 0.03, 0.1), WHITE);
            }
            let c2 = cx.l(fr((s0 + s1) * 0.5, (t0 + t1) * 0.5));
            let dir = cx.dir(if along_a { Vec2::X } else { Vec2::Y });
            cx.ramps.push(SRamp { c: c2, dir, hl: run * 0.5, hw: 0.3, h0: cx.base + s.y0 as f64, h1: cx.base + s.y1 as f64 });
        }
    }
}

/// Indoor mapping from OpenStreetMap (rooms and corridors per level), where the
/// building has it: these replace the generated rooms' walls on that level.
fn mapped_rooms(cx: &mut Ctx) {
    let b = cx.b;
    if b.indoor.is_empty() {
        return;
    }
    let fh = b.kind.floor_height();
    for area in &b.indoor {
        if area.corridor {
            continue;
        }
        let y0 = area.level.max(0) as f32 * fh;
        let y1 = y0 + fh - 0.3;
        let ring: Vec<Vec2> = area.ring.iter().map(|q| *q - cx.cen).collect();
        let n = ring.len();
        let col = paint_colour(RoomKind::Office, false, hash64(area.ring.len() as u64));
        for e in 0..n {
            let (a, c) = (ring[e], ring[(e + 1) % n]);
            let len = a.distance(c);
            if len < 0.3 {
                continue;
            }
            // A doorway in the middle of the longest walls.
            let door = len > 2.5;
            let d = (c - a) / len;
            let pieces: Vec<(f32, f32, f32, f32)> = if door {
                let m = len * 0.5;
                vec![(0.0, m - 0.5, y0, y1), (m + 0.5, len, y0, y1), (m - 0.5, m + 0.5, y0 + 2.1, y1)]
            } else {
                vec![(0.0, len, y0, y1)]
            };
            for (u0, u1, v0, v1) in pieces {
                let (p0, p1) = (a + d * u0, a + d * u1);
                cx.kit.quad(
                    M::Paint,
                    [Vec3::new(p0.x, v0, p0.y), Vec3::new(p1.x, v0, p1.y), Vec3::new(p1.x, v1, p1.y), Vec3::new(p0.x, v1, p0.y)],
                    col,
                );
                cx.kit.quad(
                    M::Paint,
                    [Vec3::new(p1.x, v0, p1.y), Vec3::new(p0.x, v0, p0.y), Vec3::new(p0.x, v1, p0.y), Vec3::new(p1.x, v1, p1.y)],
                    col,
                );
                cx.walls.push(SWall { a: p0, b: p1, h0: cx.base + v0 as f64, h1: cx.base + v1 as f64, door: None });
            }
        }
    }
}

/// Elevator shaft walls and the car's data.
fn elevator(cx: &mut Ctx, key: (TileId, u32)) -> Option<LiftData> {
    let l = cx.plan.lift?;
    let (a0, b0, a1, b1) = l.rect;
    let corners = [Vec2::new(a0, b0), Vec2::new(a1, b0), Vec2::new(a1, b1), Vec2::new(a0, b1)];
    let floors: Vec<f32> = cx.plan.floors.iter().filter(|f| f.level >= 0).map(|f| f.y).collect();
    let top = cx.plan.floors.last().map(|f| f.y + f.h).unwrap_or(3.0);
    let bottom = cx.plan.floors.iter().find(|f| f.level >= 0).map(|f| f.y).unwrap_or(0.0);
    // The shaft side its doors are on.
    let dd = l.door_dir;
    let door_side = if dd.y.abs() >= dd.x.abs() {
        if dd.y > 0.0 { 2usize } else { 0 }
    } else if dd.x > 0.0 {
        1
    } else {
        3
    };
    let id = (hash64(super::solid_key(key) ^ 0x11f7) & 0x7fff_ffff) as u32;
    let mut calls: Vec<(Vec3, usize)> = Vec::new();
    let (mut door_mid, mut door_along) = (Vec2::ZERO, Vec2::X);
    for k in 0..4 {
        let (p, q) = (corners[k], corners[(k + 1) % 4]);
        let (lp, lq) = (cx.l(p), cx.l(q));
        if k == door_side {
            // Openings on each floor, 1.0 m wide in the middle; walls between.
            let m = (lp + lq) * 0.5;
            let d = (lq - lp).normalize_or(Vec2::X);
            door_mid = m;
            door_along = d;
            for (i, &fy) in floors.iter().enumerate() {
                let next = floors.get(i + 1).copied().unwrap_or(top);
                cx.walls.push(SWall { a: lp, b: m - d * 0.55, h0: cx.base + fy as f64, h1: cx.base + next as f64, door: None });
                cx.walls.push(SWall { a: m + d * 0.55, b: lq, h0: cx.base + fy as f64, h1: cx.base + next as f64, door: None });
                cx.walls.push(SWall {
                    a: m - d * 0.55,
                    b: m + d * 0.55,
                    h0: cx.base + fy as f64 - 0.5,
                    h1: cx.base + next as f64,
                    door: Some(id ^ (i as u32 + 1)),
                });
                for (u0, u1) in [(0.0f32, lp.distance(m) - 0.55), (lp.distance(m) + 0.55, lp.distance(lq))] {
                    let (s0, s1) = (lp + d * u0, lp + d * u1);
                    cx.kit.quad(
                        M::Metal,
                        [
                            Vec3::new(s0.x, fy, s0.y),
                            Vec3::new(s1.x, fy, s1.y),
                            Vec3::new(s1.x, next - 0.3, s1.y),
                            Vec3::new(s0.x, next - 0.3, s0.y),
                        ],
                        rgb(0.7, 0.72, 0.74),
                    );
                }
                let (h0, h1) = (m - d * 0.55, m + d * 0.55);
                cx.kit.quad(
                    M::Metal,
                    [
                        Vec3::new(h0.x, fy + 2.2, h0.y),
                        Vec3::new(h1.x, fy + 2.2, h1.y),
                        Vec3::new(h1.x, next - 0.3, h1.y),
                        Vec3::new(h0.x, next - 0.3, h0.y),
                    ],
                    rgb(0.7, 0.72, 0.74),
                );
                // Call button panel beside the door.
                let cb = m + d * 0.8;
                let n_out = cx.dir(dd);
                cx.kit.cube(
                    M::Chrome,
                    Affine3A::from_translation(Vec3::new(cb.x + n_out.x * 0.02, fy + 1.1, cb.y + n_out.y * 0.02)),
                    Vec3::new(0.08, 0.16, 0.02),
                    WHITE,
                );
                calls.push((Vec3::new(cb.x + n_out.x * 0.035, fy + 1.1, cb.y + n_out.y * 0.035), i));
            }
        } else {
            cx.walls.push(SWall { a: lp, b: lq, h0: cx.base + bottom as f64 - 0.5, h1: cx.base + top as f64, door: None });
            let len = lp.distance(lq);
            let _ = len;
            for side in [lp, lq] {
                let _ = side;
            }
            cx.kit.quad(
                M::Concrete,
                [Vec3::new(lp.x, bottom, lp.y), Vec3::new(lq.x, bottom, lq.y), Vec3::new(lq.x, top, lq.y), Vec3::new(lp.x, top, lp.y)],
                WHITE,
            );
            cx.kit.quad(
                M::Concrete,
                [Vec3::new(lq.x, bottom, lq.y), Vec3::new(lp.x, bottom, lp.y), Vec3::new(lp.x, top, lp.y), Vec3::new(lq.x, top, lq.y)],
                WHITE,
            );
        }
    }
    let centre = cx.l(Vec2::new((a0 + a1) * 0.5, (b0 + b1) * 0.5));
    let door_dir = cx.dir(l.door_dir).normalize_or(Vec2::Y);
    let ring = vec![cx.l(corners[0]), cx.l(corners[1]), cx.l(corners[2]), cx.l(corners[3])];
    let ring = if geom::signed_area(&ring) < 0.0 { ring.into_iter().rev().collect() } else { ring };
    cx.floors.push(SFloor { ring, holes: Vec::new(), h: cx.base + bottom as f64, lift: Some(id) });
    Some(LiftData { centre, size: Vec2::new(a1 - a0, b1 - b0), door_dir, floors, id, door_mid, door_along, calls })
}

/// Build an interior (worker thread). Very tall buildings get the floors near `near_y`.
pub fn realise(b: &Built, bi: u32, p: &PlacedRef, near_y: f32) -> Option<InteriorData> {
    let bld = &b.buildings[bi as usize];
    let doors = exterior::doors(bld, &b.street[bi as usize]);
    let garages = exterior::garages(bld, &doors);
    let mut plan = plan::plan(bld, &doors, &garages)?;
    let mut partial = plan.floors.len() > 6;
    // Keep at most six floors: the ones around where you are.
    if partial {
        plan.floors.sort_by(|a, c| (a.y - near_y).abs().total_cmp(&(c.y - near_y).abs()));
        plan.floors.truncate(6);
        plan.floors.sort_by(|a, c| a.y.total_cmp(&c.y));
        let ys: Vec<f32> = plan.floors.iter().map(|f| f.y).collect();
        plan.stairs.retain(|s| ys.iter().any(|y| (y - s.y0).abs() < 0.01) && ys.iter().any(|y| (y - s.y1).abs() < 0.01));
    }
    let cen = geom::centroid(&bld.outline);
    let house = matches!(bld.kind, Kind::House | Kind::Terrace);
    let room_col: Vec<Vec<[f32; 4]>> = plan
        .floors
        .iter()
        .enumerate()
        .map(|(fi, f)| {
            f.rooms.iter().enumerate().map(|(r, room)| paint_colour(room.kind, house, bld.seed ^ (fi * 131 + r) as u64)).collect()
        })
        .collect();
    let mut cx = Ctx {
        b: bld,
        plan: &plan,
        cen,
        base: p.base,
        kit: Kit::default(),
        specs: Vec::new(),
        walls: Vec::new(),
        floors: Vec::new(),
        ramps: Vec::new(),
        doors: Vec::new(),
        room_col,
        house,
        ext_doors: if bld.parts.len() == 1 { doors.clone() } else { Vec::new() },
    };
    // Big buildings are furnished on your floor and the ones next to it (a furnished
    // office floor is a lot of triangles); the rest get walls and lights.
    let fh = bld.kind.floor_height();
    let here = plan.floors.iter().map(|f| f.y).min_by(|a, c| (a - near_y).abs().total_cmp(&(c - near_y).abs())).unwrap_or(0.0);
    let furnish_all = house || plan.floors.len() <= 3;
    for fi in 0..plan.floors.len() {
        slabs(&mut cx, fi);
        outer_walls(&mut cx, fi);
        if bld.indoor.iter().any(|a| a.level.max(0) == plan.floors[fi].level) {
            continue;
        }
        partitions(&mut cx, fi);
        lights(&mut cx, fi);
        if furnish_all || (plan.floors[fi].y - here).abs() < fh * 1.2 {
            furnish(&mut cx, fi);
        } else {
            partial = true;
        }
    }
    stairs(&mut cx);
    mapped_rooms(&mut cx);
    if plan.attic {
        attic(&mut cx);
    }
    let lift = elevator(&mut cx, (p.tile, bi));
    // Exterior door frames inside are part of the shell; doors themselves are entities.
    Some(InteriorData {
        kit: cx.kit,
        specs: cx.specs,
        walls: cx.walls,
        floors: cx.floors,
        ramps: cx.ramps,
        lift,
        doors: cx.doors,
        near_y,
        partial,
    })
}

// ------------------------------------------------------------------ systems

type Job = (Arc<Built>, u32, PlacedRef, f32, (TileId, u32));

#[derive(Resource)]
struct InteriorPool {
    pool: WorkPool<Job, ((TileId, u32), Option<InteriorData>)>,
    pending: std::collections::HashSet<(TileId, u32)>,
}

/// The parent entity of an interior, in the building's local frame (x east, y up,
/// z south), so its children use local metres.
#[derive(Component)]
pub struct InteriorRoot {
    pub key: (TileId, u32),
    pub lights: Vec<Vec3>,
}

pub fn plugin(app: &mut App) {
    let pool = WorkPool::new("interior", 2, |_, (b, bi, p, near_y, key): Job| (key, realise(&b, bi, &p, near_y)));
    app.init_resource::<Inside>()
        .insert_resource(InteriorPool { pool, pending: Default::default() })
        .add_systems(Update, (track_inside, choose_interiors, receive_interiors).chain().after(super::manage_detail));
}

/// Rotation from the building's local frame (x east, y up, z south) to ECEF axes.
pub fn local_to_ecef(b: &Built, bi: u32, p: &PlacedRef) -> Quat {
    let bld = &b.buildings[bi as usize];
    let cen = geom::centroid(&bld.outline);
    let (la, lo) = b.local.ll(cen);
    let m = EnuFrame::at(Geodetic::new(la, lo, p.base)).ecef_to_local.transpose();
    bevy::math::DQuat::from_mat3(&m).as_quat()
}

fn track_inside(bl: Option<Res<Buildings>>, player: Res<crate::player::Player>, mut inside: ResMut<Inside>) {
    let Some(bl) = bl else { return };
    let g = player.geo();
    let mut found = None;
    for (k, d) in &bl.detail {
        let bld = &d.built.buildings[k.1 as usize];
        let q = d.built.local.xz(g.lat, g.lon);
        if geom::contains(&bld.outline, q) && g.h > d.placed.base - 4.0 && g.h < d.placed.base + bld.height as f64 + 1.0 {
            found = Some(*k);
        }
    }
    if inside.0 != found {
        inside.0 = found;
    }
}

/// Load interiors for detailed buildings you can see into (or are inside).
#[allow(clippy::too_many_arguments)]
fn choose_interiors(
    mut bl: Option<ResMut<Buildings>>,
    mut pool: ResMut<InteriorPool>,
    viewer: Res<Viewer>,
    inside: Res<Inside>,
    settings: Res<crate::GameSettings>,
    mut commands: Commands,
    mut mats: ResMut<Assets<super::material::BuildingMaterial>>,
    mut walk_mut: ResMut<Walkables>,
    mut timer: bevy::prelude::Local<f32>,
    time: Res<Time>,
) {
    let Some(bl) = bl.as_deref_mut() else { return };
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.3;
    let max = match settings.0.preset {
        usx_core::settings::Preset::Low => 1,
        usx_core::settings::Preset::Medium => 2,
        usx_core::settings::Preset::High => 3,
        usx_core::settings::Preset::Ultra => 4,
    };
    let me = viewer.ecef;
    let g = viewer.geo();
    // Score detailed buildings.
    let mut scored: Vec<(f64, (TileId, u32))> = Vec::new();
    let outlines: Vec<((TileId, u32), Vec<Vec2>, Arc<Built>)> =
        bl.detail.iter().map(|(k, d)| (*k, d.built.buildings[k.1 as usize].outline.clone(), d.built.clone())).collect();
    for (k, d) in &bl.detail {
        let bld = &d.built.buildings[k.1 as usize];
        let dist = super::footprint_distance(&d.built, bld, &d.placed, me);
        let mine = inside.0 == Some(*k);
        let door_open =
            exterior::doors(bld, &d.built.street[k.1 as usize]).iter().any(|q| walk_mut.open_doors.contains(&ext_door_id(*k, q.0, q.1)));
        if !(mine || door_open || dist < 32.0) {
            continue;
        }
        // Line of sight: from the viewer to the nearest point of the footprint, not
        // through another nearby building.
        let q = d.built.local.xz(g.lat, g.lon);
        let n = bld.outline.len();
        let mut target = q;
        let mut best = f32::MAX;
        for i in 0..n {
            let (a, c) = (bld.outline[i], bld.outline[(i + 1) % n]);
            let ab = c - a;
            let t = ((q - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let pnt = a + ab * t;
            if pnt.distance(q) < best {
                best = pnt.distance(q);
                target = pnt;
            }
        }
        let blocked = !mine
            && outlines.iter().any(|(ok, ring, built)| {
                if *ok == *k || built.tile != d.built.tile {
                    return false;
                }
                seg_hits_ring(q, target, ring)
            });
        if blocked && !door_open {
            continue;
        }
        scored.push((if mine { -1.0 } else { dist }, *k));
    }
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    scored.truncate(max);
    let want: std::collections::HashSet<(TileId, u32)> = scored.iter().map(|s| s.1).collect();
    // Unload others.
    let keys: Vec<(TileId, u32)> = bl.detail.keys().copied().collect();
    for k in keys {
        let d = bl.detail.get_mut(&k).unwrap();
        if d.interior.is_some() && !want.contains(&k) {
            if let Some(e) = d.interior.take() {
                commands.entity(e).despawn();
            }
            if let Some(mut m) = mats.get_mut(&d.material) {
                m.extension.params.cutout = 0.0;
            }
            walk_mut.add_solid(super::solid_key(k), exterior_solid(&d.built, k.1, &d.placed));
        }
    }
    // Request new ones.
    for (_, k) in scored {
        let Some(d) = bl.detail.get(&k) else { continue };
        let near_y = (g.h - d.placed.base) as f32;
        let stale = d.interior_partial && (near_y - d.interior_y).abs() > 5.0;
        if d.interior.is_some() && !stale || pool.pending.contains(&k) {
            continue;
        }
        pool.pending.insert(k);
        pool.pool.submit(super::solid_key(k), 0.0, (d.built.clone(), k.1, d.placed, near_y, k));
    }
}

fn seg_hits_ring(a: Vec2, b: Vec2, ring: &[Vec2]) -> bool {
    let n = ring.len();
    for i in 0..n {
        let (c, d) = (ring[i], ring[(i + 1) % n]);
        let r = b - a;
        let s = d - c;
        let den = r.perp_dot(s);
        if den.abs() < 1e-9 {
            continue;
        }
        let t = (c - a).perp_dot(s) / den;
        let u = (c - a).perp_dot(r) / den;
        if t > 0.02 && t < 0.98 && (0.0..=1.0).contains(&u) {
            return true;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn receive_interiors(
    mut bl: Option<ResMut<Buildings>>,
    mut pool: ResMut<InteriorPool>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    imats: Option<Res<super::textures::InteriorMats>>,
    mut mats: ResMut<Assets<super::material::BuildingMaterial>>,
    um: Option<Res<super::interact::UseMats>>,
    mut walk: ResMut<Walkables>,
) {
    let (Some(bl), Some(imats), Some(um)) = (bl.as_deref_mut(), imats, um) else { return };
    while let Some((k, data)) = pool.pool.try_recv() {
        pool.pending.remove(&k);
        let Some(d) = bl.detail.get_mut(&k) else { continue };
        let Some(data) = data else { continue };
        // A reload for another stretch of floors replaces the old one.
        if let Some(old) = d.interior.take() {
            commands.entity(old).despawn();
        }
        d.interior_y = data.near_y;
        d.interior_partial = data.partial;
        let rot = local_to_ecef(&d.built, k.1, &d.placed);
        let root = commands.spawn((GeoAnchor::new(d.placed.origin), Transform::default(), Visibility::Visible)).id();
        let frame = commands
            .spawn((
                InteriorRoot {
                    key: k,
                    lights: data.specs.iter().filter_map(|s| if let Spec::Light { at, .. } = s { Some(*at) } else { None }).collect(),
                },
                Transform::from_rotation(rot),
                Visibility::Inherited,
                ChildOf(root),
            ))
            .id();
        for (m, mesh) in data.kit.into_meshes() {
            let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(imats.get(m)), Transform::default(), ChildOf(frame)));
            // Glass and water don't block the sun.
            if matches!(m, M::Glass | M::Water) {
                e.insert(bevy::light::NotShadowCaster);
            }
        }
        let um = &*um;
        super::interact::spawn_specs(
            &mut commands,
            &mut meshes,
            &imats,
            um,
            frame,
            data.specs,
            &data.doors,
            data.lift.as_ref(),
            d.placed.origin,
            rot,
            d.placed.base,
            data.near_y,
        );
        d.interior = Some(root);
        if let Some(mut m) = mats.get_mut(&d.material) {
            m.extension.params.cutout = 1.0;
        }
        // Collision: exterior plus everything inside.
        let mut solid = exterior_solid(&d.built, k.1, &d.placed);
        solid.walls.extend(data.walls);
        solid.floors.extend(data.floors);
        solid.ramps.extend(data.ramps);
        if let Some(l) = &data.lift {
            walk.lifts.entry(l.id).or_insert(d.placed.base + l.floors.first().copied().unwrap_or(0.0) as f64);
        }
        walk.add_solid(super::solid_key(k), solid);
    }
}

/// World position (ECEF) of a point in an interior's local frame.
pub fn local_point_ecef(origin: DVec3, rot: Quat, p: Vec3) -> DVec3 {
    origin + (rot * p).as_dvec3()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_cover_cells() {
        let outline = geom::clean(vec![Vec2::new(0.0, 0.0), Vec2::new(12.0, 0.0), Vec2::new(12.0, 9.0), Vec2::new(0.0, 9.0)]);
        let mut fl = FloorPlan { level: 0, y: 0.0, h: 2.9, label: vec![-1; 24 * 18], rooms: vec![], doors: vec![], open_pairs: vec![] };
        for j in 0..18 {
            for i in 0..24 {
                fl.label[j * 24 + i] = if i < 12 { 0 } else { 1 };
            }
        }
        let p = Plan {
            c: Vec2::new(6.0, 4.5),
            ax: Vec2::X,
            lo: Vec2::new(-6.0, -4.5),
            nx: 24,
            ny: 18,
            inside: vec![true; 24 * 18],
            floors: vec![],
            stairs: vec![],
            lift: None,
            attic: false,
            basement: false,
            entry: None,
        };
        let r = room_rects(&p, &fl, 0);
        let a: f32 = r.iter().map(|(a, b)| (b.x - a.x) * (b.y - a.y)).sum();
        assert!((a - 54.0).abs() < 0.01, "{a}");
        let bx = room_box(&p, &fl, 1).unwrap();
        assert!((bx.1.x - bx.0.x - 6.0).abs() < 0.01);
        let _ = outline;
    }
}
