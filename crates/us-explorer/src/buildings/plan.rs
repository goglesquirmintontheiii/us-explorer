//! Floor plans: rooms laid out on a half-metre grid aligned with the building, by
//! building type. Most layouts are a spine (a hall or corridor) with rooms on either
//! side; shops, restaurants and factories are a big open floor with back-of-house rooms
//! along the back wall. Every room is reachable: each gets a door to the spine or to a
//! neighbour, and the stairs and elevator sit on the spine.

use super::geom;
use super::model::{Building, EntranceKind, Kind, hash01, hash64};
use bevy::math::Vec2;

pub const CELL: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoomKind {
    Living,
    Kitchen,
    Dining,
    Bedroom,
    Bath,
    HalfBath,
    Hall,
    Laundry,
    Garage,
    Utility,
    Closet,
    Office,
    OpenOffice,
    Conference,
    Break,
    Restroom,
    Lobby,
    Sales,
    Stock,
    Seating,
    RestKitchen,
    Floor,
    Classroom,
    Patient,
    Nave,
    HotelRoom,
    Corridor,
    Stair,
    Elevator,
    Parking,
    Storage,
    Attic,
    Basement,
}

impl RoomKind {
    /// Circulation spaces, open to the rooms they serve.
    pub fn circulation(self) -> bool {
        matches!(self, RoomKind::Hall | RoomKind::Corridor | RoomKind::Stair | RoomKind::Lobby)
    }
    /// Big public rooms that don't need a wall to the hall.
    pub fn open(self) -> bool {
        matches!(
            self,
            RoomKind::Living
                | RoomKind::Lobby
                | RoomKind::Sales
                | RoomKind::Seating
                | RoomKind::OpenOffice
                | RoomKind::Floor
                | RoomKind::Parking
                | RoomKind::Basement
                | RoomKind::Attic
        )
    }
}

#[derive(Clone, Debug)]
pub struct Room {
    pub kind: RoomKind,
    /// Grid bounds (i0, j0, i1, j1), exclusive ends.
    pub rect: (usize, usize, usize, usize),
    pub cells: usize,
}

/// A doorway in an interior wall (frame coordinates: along the axis `a`, across `b`).
#[derive(Clone, Copy, Debug)]
pub struct IDoor {
    pub p: Vec2,
    /// Direction along the wall.
    pub along: Vec2,
    pub width: f32,
    pub rooms: (usize, usize),
    pub id: u32,
}

#[derive(Clone, Debug)]
pub struct FloorPlan {
    pub level: i32,
    /// Floor and ceiling heights above the ground floor.
    pub y: f32,
    pub h: f32,
    pub label: Vec<i32>,
    pub rooms: Vec<Room>,
    pub doors: Vec<IDoor>,
    /// Open to each other (no wall between).
    pub open_pairs: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, Debug)]
pub struct StairRun {
    /// Frame coordinates of the run's rectangle: from `s0` (bottom) to `s1` (top) along
    /// `dir_a` (true: along the frame's a axis), across `t0..t1`.
    pub along_a: bool,
    pub s0: f32,
    pub s1: f32,
    pub t0: f32,
    pub t1: f32,
    pub y0: f32,
    pub y1: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Lift {
    /// Frame rectangle (a0, b0, a1, b1) and the side its doors open to (unit, frame).
    pub rect: (f32, f32, f32, f32),
    pub door_dir: Vec2,
}

pub struct Plan {
    /// Grid frame: centre, long axis, cells and the frame's lower-left corner.
    pub c: Vec2,
    pub ax: Vec2,
    pub lo: Vec2,
    pub nx: usize,
    pub ny: usize,
    pub inside: Vec<bool>,
    pub floors: Vec<FloorPlan>,
    pub stairs: Vec<StairRun>,
    pub lift: Option<Lift>,
    pub attic: bool,
    pub basement: bool,
    /// Front door position (frame) and the direction into the building.
    pub entry: Option<(Vec2, Vec2)>,
}

impl Plan {
    pub fn to_local(&self, f: Vec2) -> Vec2 {
        self.c + self.ax * f.x + self.ax.perp() * f.y
    }
    pub fn to_frame(&self, p: Vec2) -> Vec2 {
        let d = p - self.c;
        Vec2::new(d.dot(self.ax), d.dot(self.ax.perp()))
    }
    pub fn cell_centre(&self, i: usize, j: usize) -> Vec2 {
        self.lo + Vec2::new((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL)
    }
    pub fn cell_of(&self, f: Vec2) -> Option<(usize, usize)> {
        let q = (f - self.lo) / CELL;
        (q.x >= 0.0 && q.y >= 0.0 && (q.x as usize) < self.nx && (q.y as usize) < self.ny).then(|| (q.x as usize, q.y as usize))
    }
    pub fn room_at(&self, fl: &FloorPlan, f: Vec2) -> Option<usize> {
        let (i, j) = self.cell_of(f)?;
        let l = fl.label[j * self.nx + i];
        (l >= 0).then_some(l as usize)
    }
}

/// Lay a list of rooms along a band: (kind, metres or 0 = share what's left).
fn band(
    label: &mut [i32],
    nx: usize,
    rooms: &mut Vec<Room>,
    cells: (usize, usize, usize, usize),
    along_i: bool,
    list: &[(RoomKind, f32)],
    inside: &[bool],
) {
    let (i0, j0, i1, j1) = cells;
    let len = if along_i { i1 - i0 } else { j1 - j0 } as f32 * CELL;
    if len <= 0.0 || i1 <= i0 || j1 <= j0 {
        return;
    }
    let fixed: f32 = list.iter().map(|r| r.1).sum();
    let flex = list.iter().filter(|r| r.1 <= 0.0).count().max(1) as f32;
    let share = ((len - fixed) / flex).max(0.0);
    let mut s = 0.0f32;
    for (k, &(kind, l)) in list.iter().enumerate() {
        let w = if l > 0.0 { l } else { share };
        let a = (s / CELL).round() as usize;
        let b = if k + 1 == list.len() { ((len / CELL).round() as usize).max(a) } else { ((s + w) / CELL).round() as usize };
        s += w;
        if b <= a {
            continue;
        }
        let r = if along_i { (i0 + a, j0, (i0 + b).min(i1), j1) } else { (i0, j0 + a, i1, (j0 + b).min(j1)) };
        paint(label, nx, rooms, r, kind, inside);
    }
}

/// Mark a rectangle of cells (inside the footprint) as a new room.
fn paint(
    label: &mut [i32],
    nx: usize,
    rooms: &mut Vec<Room>,
    r: (usize, usize, usize, usize),
    kind: RoomKind,
    inside: &[bool],
) -> Option<usize> {
    let id = rooms.len();
    let mut n = 0;
    for j in r.1..r.3 {
        for i in r.0..r.2 {
            let k = j * nx + i;
            if inside[k] {
                label[k] = id as i32;
                n += 1;
            }
        }
    }
    if n == 0 {
        return None;
    }
    rooms.push(Room { kind, rect: r, cells: n });
    Some(id)
}

/// Programmes by building kind: (spine across the short axis?, side A rooms, side B rooms).
type Prog = Vec<(RoomKind, f32)>;

fn house_floor(level: u32, levels: u32, area: f32, seed: u64) -> (Prog, Prog) {
    let r = hash01(seed ^ level as u64);
    if levels == 1 {
        let beds = if area > 170.0 { 3 } else { 2 };
        let a = vec![(RoomKind::Kitchen, 3.6), (RoomKind::Dining, 3.2), (RoomKind::Living, 0.0)];
        let mut b = vec![(RoomKind::Bedroom, 0.0), (RoomKind::Bath, 2.2)];
        for _ in 1..beds {
            b.push((RoomKind::Bedroom, 0.0));
        }
        if area > 140.0 {
            b.insert(1, (RoomKind::Bath, 2.0));
            b.push((RoomKind::Laundry, 1.8));
        }
        (a, b)
    } else if level == 0 {
        let a = vec![(RoomKind::Living, 0.0)];
        let b = if r < 0.5 {
            vec![(RoomKind::Kitchen, 0.0), (RoomKind::Dining, 3.2), (RoomKind::HalfBath, 1.6)]
        } else {
            vec![(RoomKind::Dining, 3.4), (RoomKind::Kitchen, 0.0), (RoomKind::Laundry, 1.8)]
        };
        (a, b)
    } else {
        let a = vec![(RoomKind::Bedroom, 0.0), (RoomKind::Bath, 2.2)];
        let b = vec![(RoomKind::Bedroom, 0.0), (RoomKind::Bath, 2.0), (RoomKind::Bedroom, 0.0)];
        (a, b)
    }
}

/// Doors between rooms that share enough wall: through the spine first, then a
/// spanning tree over neighbours until every room can be reached from the entry.
fn connect(fl: &mut FloorPlan, nx: usize, ny: usize, lo: Vec2, entry: Option<usize>, seed: u64, wide: bool) {
    let n = fl.rooms.len();
    if n == 0 {
        return;
    }
    // Shared wall runs between pairs: collect boundary edges.
    use std::collections::HashMap;
    let mut shared: HashMap<(usize, usize), Vec<(Vec2, Vec2)>> = HashMap::new();
    let lab = |i: usize, j: usize| fl.label[j * nx + i];
    for j in 0..ny {
        for i in 0..nx {
            let a = lab(i, j);
            if a < 0 {
                continue;
            }
            if i + 1 < nx {
                let b = lab(i + 1, j);
                if b >= 0 && b != a {
                    let key = (a.min(b) as usize, a.max(b) as usize);
                    let x = lo.x + (i + 1) as f32 * CELL;
                    shared.entry(key).or_default().push((Vec2::new(x, lo.y + j as f32 * CELL), Vec2::new(x, lo.y + (j + 1) as f32 * CELL)));
                }
            }
            if j + 1 < ny {
                let b = lab(i, j + 1);
                if b >= 0 && b != a {
                    let key = (a.min(b) as usize, a.max(b) as usize);
                    let y = lo.y + (j + 1) as f32 * CELL;
                    shared.entry(key).or_default().push((Vec2::new(lo.x + i as f32 * CELL, y), Vec2::new(lo.x + (i + 1) as f32 * CELL, y)));
                }
            }
        }
    }
    // Longest straight run of each shared boundary.
    let mut runs: HashMap<(usize, usize), (Vec2, Vec2)> = HashMap::new();
    for (k, mut edges) in shared {
        edges.sort_by(|a, b| (a.0.x, a.0.y).partial_cmp(&(b.0.x, b.0.y)).unwrap());
        let mut best: Option<(Vec2, Vec2)> = None;
        let mut cur: Option<(Vec2, Vec2)> = None;
        for e in edges {
            cur = match cur {
                Some((s, t)) if t.distance(e.0) < 1e-3 && (t - s).normalize_or_zero().dot((e.1 - e.0).normalize_or_zero()) > 0.99 => {
                    Some((s, e.1))
                }
                _ => {
                    if let Some(c) = cur
                        && best.is_none_or(|b| b.0.distance(b.1) < c.0.distance(c.1))
                    {
                        best = Some(c);
                    }
                    Some(e)
                }
            };
        }
        if let Some(c) = cur
            && best.is_none_or(|b| b.0.distance(b.1) < c.0.distance(c.1))
        {
            best = Some(c);
        }
        if let Some(b) = best {
            runs.insert(k, b);
        }
    }
    let width = if wide { 1.0 } else { 0.9 };
    let mut reached = vec![false; n];
    let start = entry.or_else(|| fl.rooms.iter().position(|r| r.kind.circulation())).unwrap_or(0);
    reached[start] = true;
    let add_door = |fl: &mut FloorPlan, a: usize, b: usize, run: (Vec2, Vec2)| {
        let len = run.0.distance(run.1);
        if len < width + 0.3 {
            return false;
        }
        let along = (run.1 - run.0) / len;
        // Not too close to the run's ends (corners, other walls).
        let t = 0.5 + (hash01(seed ^ (a * 31 + b) as u64) - 0.5) * (1.0 - (width + 0.6) / len).max(0.0);
        let p = run.0 + along * (len * t);
        let id = (hash64(seed ^ ((fl.level as u64) << 32) ^ (a * 1000 + b) as u64) & 0x7fff_ffff) as u32 | 0x8000_0000;
        fl.doors.push(IDoor { p, along, width, rooms: (a, b), id });
        true
    };
    // Open pairs need no door.
    let is_open = |fl: &FloorPlan, a: usize, b: usize| fl.open_pairs.contains(&(a.min(b), a.max(b)));
    // Rooms off circulation first.
    let mut changed = true;
    while changed {
        changed = false;
        for a in 0..n {
            if !reached[a] {
                continue;
            }
            for b in 0..n {
                if reached[b] || a == b {
                    continue;
                }
                let key = (a.min(b), a.max(b));
                let Some(&run) = runs.get(&key) else { continue };
                // Baths and closets are reached from bedrooms when they border one.
                let kb = fl.rooms[b].kind;
                let ka = fl.rooms[a].kind;
                let private_from_public = matches!(kb, RoomKind::Bath | RoomKind::Closet)
                    && !matches!(ka, RoomKind::Bedroom | RoomKind::Hall | RoomKind::Corridor | RoomKind::HotelRoom | RoomKind::Patient);
                if private_from_public
                    && (0..n)
                        .any(|c| !reached[c] && matches!(fl.rooms[c].kind, RoomKind::Bedroom) && runs.contains_key(&(c.min(b), c.max(b))))
                {
                    continue;
                }
                if is_open(fl, a, b) || add_door(fl, a, b, run) {
                    reached[b] = true;
                    changed = true;
                }
            }
        }
    }
    // Anything still cut off: open it to a reached neighbour.
    for b in 0..n {
        if reached[b] {
            continue;
        }
        if let Some(a) = (0..n).find(|&a| reached[a] && runs.contains_key(&(a.min(b), a.max(b)))) {
            fl.open_pairs.push((a.min(b), a.max(b)));
            reached[b] = true;
        }
    }
}

/// Whether a building gets a basement (houses tagged or guessed to have one).
pub fn has_basement(b: &Building) -> bool {
    b.basement && matches!(b.kind, Kind::House | Kind::Terrace)
}

/// The whole plan for a building.
pub fn plan(b: &Building, doors: &[(usize, f32, f32, EntranceKind)], garages: &[(usize, f32, f32)]) -> Option<Plan> {
    let ring = &b.outline;
    if ring.len() < 3 || b.area < 8.0 || matches!(b.kind, Kind::Canopy | Kind::Carport) {
        return None;
    }
    let (c, ax, half) = geom::obb(ring);
    let pp = ax.perp();
    let lo = -half - Vec2::splat(CELL * 0.5);
    let nx = ((half.x * 2.0 + CELL) / CELL).ceil() as usize;
    let ny = ((half.y * 2.0 + CELL) / CELL).ceil() as usize;
    if nx * ny > 250_000 {
        return None;
    }
    // Inside cells: centre inside the footprint and at least a bit away from the walls
    // (the exterior wall is 0.2 m thick).
    let mut inside = vec![false; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let f = lo + Vec2::new((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
            let p = c + ax * f.x + pp * f.y;
            inside[j * nx + i] = geom::contains(ring, p) && !b.holes.iter().any(|h| geom::contains(h, p));
        }
    }
    let n_in = inside.iter().filter(|&&x| x).count();
    if n_in < 8 {
        return None;
    }
    let mut p =
        Plan { c, ax, lo, nx, ny, inside, floors: Vec::new(), stairs: Vec::new(), lift: None, attic: false, basement: false, entry: None };
    let fh = b.kind.floor_height();
    let gf = super::exterior::ground_floor(b.kind, fh);
    let wall_top = b.parts.iter().map(|q| q.wall_top).fold(0.0, f32::max);
    let levels = b.levels.max(1);
    // Front door in frame coordinates, and the direction into the building.
    let n = ring.len();
    let main = doors.iter().find(|d| d.3 == EntranceKind::Main).or(doors.first());
    if let Some(&(e, at, _, _)) = main {
        let (a0, a1) = (ring[e], ring[(e + 1) % n]);
        let d = (a1 - a0).normalize_or(Vec2::X);
        let q = a0 + d * at;
        let into = d.perp();
        p.entry = Some((p.to_frame(q), Vec2::new(into.dot(ax), into.dot(pp))));
    }
    let entry_side_b = p.entry.map(|(_, into)| into.y.abs() > into.x.abs()).unwrap_or(true);
    // Full grid bounds.
    let full = (0usize, 0usize, nx, ny);
    let (w_a, w_b) = (nx as f32 * CELL, ny as f32 * CELL);
    let mut floors: Vec<FloorPlan> = Vec::new();
    let base_floor = |level: i32, y: f32, h: f32| FloorPlan {
        level,
        y,
        h,
        label: vec![-1; nx * ny],
        rooms: Vec::new(),
        doors: Vec::new(),
        open_pairs: Vec::new(),
    };
    // Heights.
    let floor_y = |k: u32| if k == 0 { 0.0 } else { gf + (k - 1) as f32 * fh };
    let floor_h = |k: u32| if k == 0 { gf } else { fh };
    let max_levels = levels.min(((wall_top - gf) / fh).floor() as u32 + 1).max(1);
    // Stairs (and an elevator) on multi-storey buildings: one straight run along the
    // long axis in the middle of the spine.
    let multi = max_levels > 1;
    let spine_w = match b.kind {
        Kind::House | Kind::Terrace => {
            if multi {
                2.0
            } else {
                1.2
            }
        }
        Kind::Apartments | Kind::Hotel | Kind::Hospital => 1.8,
        Kind::School => 2.6,
        _ => 2.0,
    };
    let house = matches!(b.kind, Kind::House | Kind::Terrace);
    // Houses with stairs: the hall runs across the short axis (a centre-hall plan); single
    // storey houses and everything else: along the long axis.
    let spine_across = house && multi;
    let cells = |m: f32| ((m / CELL).round() as usize).max(1);
    let (sp0, sp1) = if spine_across {
        let mid = nx / 2 + ((hash01(b.seed ^ 0x51) - 0.5) * nx as f32 * 0.25) as usize;
        let w = cells(spine_w);
        (mid.saturating_sub(w / 2), (mid + w - w / 2).min(nx))
    } else {
        // Offset toward the back so the front rooms are deeper.
        let frac = if house { 0.55 } else { 0.5 };
        let mid = (ny as f32 * if entry_side_b { frac } else { 1.0 - frac }) as usize;
        let w = cells(spine_w);
        (mid.saturating_sub(w / 2), (mid + w - w / 2).min(ny))
    };
    // Stair run inside the spine (along it), from the front end.
    let rise = |k: u32| floor_h(k);
    let stair_len = |h: f32| ((h / 0.19).ceil() * 0.26 + 0.2).max(2.0);
    let stair_w = if house { 1.0 } else { 1.4 };
    if multi {
        let len = stair_len(rise(0).max(fh));
        if spine_across {
            let (t0, t1) = (lo.x + sp0 as f32 * CELL, lo.x + sp0 as f32 * CELL + stair_w);
            let s0 = lo.y + ny as f32 * CELL * 0.5 - len * 0.5;
            for k in 0..max_levels - 1 {
                p.stairs.push(StairRun { along_a: false, s0, s1: s0 + len, t0, t1, y0: floor_y(k), y1: floor_y(k + 1) });
            }
        } else {
            let (t0, t1) = (lo.y + sp1 as f32 * CELL - stair_w, lo.y + sp1 as f32 * CELL);
            let s0 = lo.x + nx as f32 * CELL * 0.5 - len * 0.5;
            for k in 0..max_levels - 1 {
                p.stairs.push(StairRun { along_a: true, s0, s1: s0 + len, t0, t1, y0: floor_y(k), y1: floor_y(k + 1) });
            }
        }
        if !house && max_levels >= 3 && !spine_across {
            let a0 = lo.x + nx as f32 * CELL * 0.5 + stair_len(fh) * 0.5 + 0.6;
            let (b1, b0) = (lo.y + sp0 as f32 * CELL, lo.y + sp0 as f32 * CELL - 2.4);
            if b0 > lo.y + 0.5 {
                p.lift = Some(Lift { rect: (a0, b0, a0 + 2.2, b1), door_dir: Vec2::new(0.0, 1.0) });
            }
        }
    }
    // Garage: a rectangle behind each garage door, on the ground floor.
    let mut garage_rects: Vec<(usize, usize, usize, usize)> = Vec::new();
    for &(e, at, w) in garages {
        let (a0, a1) = (ring[e], ring[(e + 1) % n]);
        let d = (a1 - a0).normalize_or(Vec2::X);
        let into = d.perp();
        let q0 = p.to_frame(a0 + d * (at - w * 0.5 - 0.6));
        let q1 = p.to_frame(a0 + d * (at + w * 0.5 + 0.6) + into * 6.6);
        let (mn, mx) = (q0.min(q1), q0.max(q1));
        let i0 = ((mn.x - lo.x) / CELL).floor().max(0.0) as usize;
        let j0 = ((mn.y - lo.y) / CELL).floor().max(0.0) as usize;
        let i1 = (((mx.x - lo.x) / CELL).ceil() as usize).min(nx);
        let j1 = (((mx.y - lo.y) / CELL).ceil() as usize).min(ny);
        if i1 > i0 + 4 && j1 > j0 + 4 {
            garage_rects.push((i0, j0, i1, j1));
        }
    }
    let inside = p.inside.clone();
    for k in 0..max_levels {
        let mut fl = base_floor(k as i32, floor_y(k), floor_h(k));
        let mut rooms = Vec::new();
        let mut label = vec![-1i32; nx * ny];
        let lev_seed = b.seed ^ (k as u64 * 0x9e37);
        // Spine first.
        let spine_rect = if spine_across { (sp0, 0, sp1, ny) } else { (0, sp0, nx, sp1) };
        let side_a = if spine_across { (0, 0, sp0, ny) } else { (0, 0, nx, sp0) };
        let side_b = if spine_across { (sp1, 0, nx, ny) } else { (0, sp1, nx, ny) };
        let spine_kind = match b.kind {
            Kind::House | Kind::Terrace => RoomKind::Hall,
            _ if k == 0
                && matches!(b.kind, Kind::Office | Kind::Hotel | Kind::Civic | Kind::Apartments | Kind::Hospital | Kind::School) =>
            {
                RoomKind::Lobby
            }
            _ => RoomKind::Corridor,
        };
        // Garages take their cells first on the ground floor.
        if k == 0 {
            for &g in &garage_rects {
                paint(&mut label, nx, &mut rooms, g, RoomKind::Garage, &inside);
            }
        }
        let free_inside: Vec<bool> = inside.iter().zip(&label).map(|(&i, &l)| i && l < 0).collect();
        let use_spine = !matches!(
            b.kind,
            Kind::Retail
                | Kind::Supermarket
                | Kind::FastFood
                | Kind::Cafe
                | Kind::Restaurant
                | Kind::Bar
                | Kind::Industrial
                | Kind::Warehouse
                | Kind::Church
                | Kind::Garage
                | Kind::Shed
                | Kind::ParkingGarage
                | Kind::Other
        ) && (if spine_across { w_a } else { w_b }) > 6.5;
        if use_spine {
            let s_id = paint(&mut label, nx, &mut rooms, spine_rect, spine_kind, &free_inside);
            let _ = s_id;
            let free: Vec<bool> = inside.iter().zip(&label).map(|(&i, &l)| i && l < 0).collect();
            let along_i = !spine_across;
            let (pa, pb): (Prog, Prog) = match b.kind {
                Kind::House | Kind::Terrace => house_floor(k, max_levels, b.area, b.seed),
                Kind::Apartments => {
                    let unit = 7.5;
                    let n_units = ((w_a / unit).floor() as usize).max(1);
                    let a: Prog = (0..n_units).map(|_| (RoomKind::Living, 0.0)).collect();
                    (a.clone(), a)
                }
                Kind::Hotel => {
                    let n_rooms = ((w_a / 4.0).floor() as usize).max(1);
                    let a: Prog = (0..n_rooms).map(|_| (RoomKind::HotelRoom, 0.0)).collect();
                    (a.clone(), a)
                }
                Kind::Hospital => {
                    let n_rooms = ((w_a / 4.2).floor() as usize).max(2);
                    let mut a: Prog = (0..n_rooms).map(|_| (RoomKind::Patient, 0.0)).collect();
                    a.insert(n_rooms / 2, (RoomKind::Office, 4.0));
                    let mut bb = a.clone();
                    bb.insert(1, (RoomKind::Restroom, 3.0));
                    (a, bb)
                }
                Kind::School => {
                    let n_rooms = ((w_a / 9.0).floor() as usize).max(1);
                    let a: Prog = (0..n_rooms).map(|_| (RoomKind::Classroom, 0.0)).collect();
                    let mut bb = a.clone();
                    bb.insert(0, (RoomKind::Restroom, 3.5));
                    bb.insert(bb.len() / 2, (RoomKind::Office, 4.0));
                    (a, bb)
                }
                _ => {
                    // Offices and civic: an open office on one side, private offices,
                    // a conference room, a break room and restrooms on the other.
                    let n_off = ((w_a - 18.0) / 3.2).floor().max(1.0) as usize;
                    let a: Prog = if k == 0 && w_a > 14.0 {
                        vec![(RoomKind::Lobby, 9.0), (RoomKind::OpenOffice, 0.0)]
                    } else {
                        vec![(RoomKind::OpenOffice, 0.0)]
                    };
                    let mut bb: Prog = vec![(RoomKind::Conference, 6.0)];
                    for i in 0..n_off {
                        if i == n_off / 2 {
                            bb.push((RoomKind::Restroom, 3.0));
                            bb.push((RoomKind::Restroom, 3.0));
                            bb.push((RoomKind::Break, 4.5));
                        }
                        bb.push((RoomKind::Office, 3.2));
                    }
                    (a, bb)
                }
            };
            let (pa, pb) = if (hash01(lev_seed ^ 0xab) < 0.5) && !house { (pb, pa) } else { (pa, pb) };
            band(&mut label, nx, &mut rooms, side_a, along_i, &pa, &free);
            band(&mut label, nx, &mut rooms, side_b, along_i, &pb, &free);
            // Apartment units: split each into living/kitchen, a bedroom and a bathroom.
            if b.kind == Kind::Apartments {
                let units: Vec<usize> = (0..rooms.len()).filter(|&r| rooms[r].kind == RoomKind::Living).collect();
                for u in units {
                    let (i0, j0, i1, j1) = rooms[u].rect;
                    let (wa, wb) = (i1 - i0, j1 - j0);
                    if wa < 8 || wb < 8 {
                        continue;
                    }
                    // Bedroom: the 40% of the unit away from... along the band.
                    let cut = i0 + wa * 3 / 5;
                    if along_i {
                        let bed = paint(&mut label, nx, &mut rooms, (cut, j0, i1, j1), RoomKind::Bedroom, &inside);
                        // Bathroom against the corridor in the bedroom part.
                        let near_corr_low = j0 >= sp1;
                        let bath_rect = if near_corr_low {
                            (cut, j0, i1.min(cut + 5), j0 + 5)
                        } else {
                            (cut, j1.saturating_sub(5), i1.min(cut + 5), j1)
                        };
                        let _ = bed;
                        paint(&mut label, nx, &mut rooms, bath_rect, RoomKind::Bath, &inside);
                    }
                }
            }
        } else {
            // Open-plan buildings: back-of-house strip along the wall opposite the door.
            let back_depth = match b.kind {
                Kind::Supermarket => 8.0,
                Kind::Retail => 5.0,
                Kind::FastFood | Kind::Restaurant | Kind::Cafe | Kind::Bar => (if entry_side_b { w_b } else { w_a } * 0.35).clamp(3.5, 9.0),
                Kind::Church => 0.0,
                Kind::Industrial | Kind::Warehouse => 0.0,
                _ => 0.0,
            };
            let main_kind = match b.kind {
                Kind::Retail | Kind::Supermarket => RoomKind::Sales,
                Kind::FastFood | Kind::Cafe | Kind::Restaurant | Kind::Bar => RoomKind::Seating,
                Kind::Industrial | Kind::Warehouse => RoomKind::Floor,
                Kind::Church => RoomKind::Nave,
                Kind::Garage => RoomKind::Garage,
                Kind::ParkingGarage => RoomKind::Parking,
                _ => RoomKind::Storage,
            };
            let free: Vec<bool> = inside.iter().zip(&label).map(|(&i, &l)| i && l < 0).collect();
            if back_depth > 0.0 && k == 0 {
                // Which side is the back? Opposite the entry.
                let (into_x, into_y) = p.entry.map(|e| (e.1.x, e.1.y)).unwrap_or((0.0, 1.0));
                let d = cells(back_depth);
                let (strip, rest, along_i) = if into_y.abs() >= into_x.abs() {
                    if into_y > 0.0 {
                        ((0, ny.saturating_sub(d), nx, ny), (0, 0, nx, ny.saturating_sub(d)), true)
                    } else {
                        ((0, 0, nx, d.min(ny)), (0, d.min(ny), nx, ny), true)
                    }
                } else if into_x > 0.0 {
                    ((nx.saturating_sub(d), 0, nx, ny), (0, 0, nx.saturating_sub(d), ny), false)
                } else {
                    ((0, 0, d.min(nx), ny), (d.min(nx), 0, nx, ny), false)
                };
                let back: Prog = match b.kind {
                    Kind::FastFood | Kind::Restaurant | Kind::Cafe | Kind::Bar => {
                        vec![(RoomKind::Restroom, 2.4), (RoomKind::Restroom, 2.4), (RoomKind::RestKitchen, 0.0), (RoomKind::Storage, 2.6)]
                    }
                    _ => vec![(RoomKind::Restroom, 2.4), (RoomKind::Office, 3.0), (RoomKind::Stock, 0.0)],
                };
                band(&mut label, nx, &mut rooms, strip, along_i, &back, &free);
                let free2: Vec<bool> = inside.iter().zip(&label).map(|(&i, &l)| i && l < 0).collect();
                paint(&mut label, nx, &mut rooms, rest, main_kind, &free2);
            } else if matches!(b.kind, Kind::Industrial | Kind::Warehouse) && k == 0 && w_a > 20.0 && w_b > 14.0 {
                // A block of offices, a break room and restrooms in one corner.
                let bw = cells(12.0).min(nx / 2);
                let bd = cells(8.0).min(ny / 2);
                let block = (0, 0, bw, bd);
                band(
                    &mut label,
                    nx,
                    &mut rooms,
                    block,
                    true,
                    &[(RoomKind::Office, 0.0), (RoomKind::Break, 4.0), (RoomKind::Restroom, 2.5)],
                    &free,
                );
                let free2: Vec<bool> = inside.iter().zip(&label).map(|(&i, &l)| i && l < 0).collect();
                paint(&mut label, nx, &mut rooms, full, main_kind, &free2);
            } else {
                let kind = if k > 0 && main_kind == RoomKind::Sales { RoomKind::Sales } else { main_kind };
                paint(&mut label, nx, &mut rooms, full, kind, &free);
            }
        }
        // Stairs override what's under them (both ends of each run).
        for s in &p.stairs {
            if (s.y0 - fl.y).abs() > 0.01 && (s.y1 - fl.y).abs() > 0.01 {
                continue;
            }
            let (a0, a1, b0, b1) = if s.along_a { (s.s0, s.s1, s.t0, s.t1) } else { (s.t0, s.t1, s.s0, s.s1) };
            let r = (
                (((a0 - lo.x) / CELL).floor().max(0.0)) as usize,
                (((b0 - lo.y) / CELL).floor().max(0.0)) as usize,
                (((a1 - lo.x) / CELL).ceil() as usize).min(nx),
                (((b1 - lo.y) / CELL).ceil() as usize).min(ny),
            );
            paint(&mut label, nx, &mut rooms, r, RoomKind::Stair, &inside);
        }
        if let Some(l) = p.lift {
            let r = (
                (((l.rect.0 - lo.x) / CELL).floor().max(0.0)) as usize,
                (((l.rect.1 - lo.y) / CELL).floor().max(0.0)) as usize,
                (((l.rect.2 - lo.x) / CELL).ceil() as usize).min(nx),
                (((l.rect.3 - lo.y) / CELL).ceil() as usize).min(ny),
            );
            paint(&mut label, nx, &mut rooms, r, RoomKind::Elevator, &inside);
        }
        // Rooms that lost all their cells (painted over) or are tiny get merged into a
        // neighbour by relabelling.
        let mut counts = vec![0usize; rooms.len()];
        for &l in &label {
            if l >= 0 {
                counts[l as usize] += 1;
            }
        }
        for (r, room) in rooms.iter_mut().enumerate() {
            room.cells = counts[r];
        }
        // Fill unlabelled inside cells with the nearest room on the same row.
        for j in 0..ny {
            let mut last = -1;
            for i in 0..nx {
                let kk = j * nx + i;
                if inside[kk] && label[kk] < 0 {
                    label[kk] = last;
                } else if label[kk] >= 0 && !matches!(rooms[label[kk] as usize].kind, RoomKind::Stair | RoomKind::Elevator) {
                    last = label[kk];
                }
            }
        }
        for j in 0..ny {
            for i in (0..nx).rev() {
                let kk = j * nx + i;
                if inside[kk] && label[kk] < 0 && i + 1 < nx {
                    label[kk] = label[kk + 1];
                }
            }
        }
        // Tiny rooms (< 1.5 m2) join the neighbour that shares most edge.
        for r in 0..rooms.len() {
            let n_cells = label.iter().filter(|&&l| l == r as i32).count();
            if n_cells > 0 && n_cells < 6 && !matches!(rooms[r].kind, RoomKind::Stair | RoomKind::Elevator) {
                let mut votes = std::collections::HashMap::new();
                for j in 0..ny {
                    for i in 0..nx {
                        if label[j * nx + i] != r as i32 {
                            continue;
                        }
                        for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                            let (ii, jj) = (i as i64 + di, j as i64 + dj);
                            if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny {
                                let l = label[jj as usize * nx + ii as usize];
                                if l >= 0 && l != r as i32 {
                                    *votes.entry(l).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                }
                if let Some((&to, _)) = votes.iter().max_by_key(|(_, c)| **c) {
                    for l in label.iter_mut() {
                        if *l == r as i32 {
                            *l = to;
                        }
                    }
                }
            }
        }
        for (r, room) in rooms.iter_mut().enumerate() {
            room.cells = label.iter().filter(|&&l| l == r as i32).count();
        }
        fl.label = label;
        fl.rooms = rooms;
        // Open pairs: circulation with open rooms, stairs with everything public, and
        // open-concept living/dining/kitchen in some houses.
        let nr = fl.rooms.len();
        for a in 0..nr {
            for c in a + 1..nr {
                let (ka, kc) = (fl.rooms[a].kind, fl.rooms[c].kind);
                let pair = |x: RoomKind, y: RoomKind| (ka == x && kc == y) || (ka == y && kc == x);
                let open = (ka.circulation() && kc.open())
                    || (kc.circulation() && ka.open())
                    || (ka == RoomKind::Stair && kc.circulation())
                    || (kc == RoomKind::Stair && ka.circulation())
                    || (ka.circulation() && kc.circulation())
                    || (house
                        && (pair(RoomKind::Living, RoomKind::Dining) || pair(RoomKind::Dining, RoomKind::Kitchen))
                        && hash01(b.seed ^ 0x0be) < 0.6)
                    || pair(RoomKind::Seating, RoomKind::Stair)
                    || pair(RoomKind::Sales, RoomKind::Stair)
                    || pair(RoomKind::Floor, RoomKind::Stair)
                    || pair(RoomKind::Lobby, RoomKind::Elevator)
                    || pair(RoomKind::Corridor, RoomKind::Elevator)
                    || pair(RoomKind::OpenOffice, RoomKind::Stair)
                    || pair(RoomKind::Parking, RoomKind::Stair);
                if open {
                    fl.open_pairs.push((a, c));
                }
            }
        }
        // The room the front door opens into.
        let entry_room = if k == 0 {
            p.entry.and_then(|(q, into)| p.room_at(&fl, q + into * 0.6))
        } else {
            fl.rooms.iter().position(|r| r.kind == RoomKind::Stair)
        };
        let wide = !house;
        connect(&mut fl, nx, ny, lo, entry_room, lev_seed, wide);
        floors.push(fl);
    }
    // Basements (houses on flat lots, and buildings tagged with one) and attics (houses
    // with pitched roofs).
    p.basement = has_basement(b);
    p.attic = house && b.parts[0].roof != super::model::Roof::Flat && b.parts[0].roof_h > 1.6;
    if p.basement {
        let mut fl = base_floor(-1, -2.6, 2.45);
        let unfinished = hash01(b.seed ^ 0xbb) < 0.55;
        let mut rooms = Vec::new();
        let mut label = vec![-1i32; nx * ny];
        let inside2 = p.inside.clone();
        if unfinished {
            paint(&mut label, nx, &mut rooms, full, RoomKind::Basement, &inside2);
        } else {
            let along_i = true;
            let half_j = ny / 2;
            band(&mut label, nx, &mut rooms, (0, 0, nx, half_j), along_i, &[(RoomKind::Living, 0.0), (RoomKind::Bath, 2.2)], &inside2);
            band(
                &mut label,
                nx,
                &mut rooms,
                (0, half_j, nx, ny),
                along_i,
                &[(RoomKind::Utility, 3.0), (RoomKind::Basement, 0.0)],
                &inside2,
            );
        }
        // Basement stairs under the main stairs (or from the hall).
        let len = stair_len(2.6);
        let (s0, t0, along_a) = match p.stairs.first() {
            Some(s) => (s.s0, s.t0, s.along_a),
            None => (lo.x + nx as f32 * CELL * 0.5 - len * 0.5, lo.y + sp1 as f32 * CELL - 1.0, true),
        };
        let run = StairRun { along_a, s0, s1: s0 + len, t0, t1: t0 + 1.0, y0: -2.6, y1: 0.0 };
        p.stairs.push(run);
        let (a0, a1, b0, b1) = if along_a { (run.s0, run.s1, run.t0, run.t1) } else { (run.t0, run.t1, run.s0, run.s1) };
        let r = (
            (((a0 - lo.x) / CELL).floor().max(0.0)) as usize,
            (((b0 - lo.y) / CELL).floor().max(0.0)) as usize,
            (((a1 - lo.x) / CELL).ceil() as usize).min(nx),
            (((b1 - lo.y) / CELL).ceil() as usize).min(ny),
        );
        paint(&mut label, nx, &mut rooms, r, RoomKind::Stair, &inside2);
        // Mark the stair on the ground floor too (an opening in its floor).
        if let Some(g) = floors.first_mut() {
            let sid = g.rooms.len();
            g.rooms.push(Room { kind: RoomKind::Stair, rect: r, cells: 0 });
            for j in r.1..r.3 {
                for i in r.0..r.2 {
                    if p.inside[j * nx + i] {
                        g.label[j * nx + i] = sid as i32;
                    }
                }
            }
            for a in 0..g.rooms.len() - 1 {
                if g.rooms[a].kind.circulation() || g.rooms[a].kind.open() {
                    g.open_pairs.push((a, sid));
                }
            }
        }
        for (r_i, room) in rooms.iter_mut().enumerate() {
            room.cells = label.iter().filter(|&&l| l == r_i as i32).count();
        }
        fl.label = label;
        fl.rooms = rooms;
        let nr = fl.rooms.len();
        for a in 0..nr {
            for c in a + 1..nr {
                if fl.rooms[a].kind == RoomKind::Stair
                    || fl.rooms[c].kind == RoomKind::Stair
                    || fl.rooms[a].kind.open() && fl.rooms[c].kind.open()
                {
                    fl.open_pairs.push((a, c));
                }
            }
        }
        let start = fl.rooms.iter().position(|r| r.kind == RoomKind::Stair);
        connect(&mut fl, nx, ny, lo, start, b.seed ^ 0xbad, false);
        floors.insert(0, fl);
    }
    p.floors = floors;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buildings::model::{self, Part, Roof, RoofMat, Wall, Windows};

    fn house(w: f32, d: f32, levels: u32) -> Building {
        let outline = geom::clean(vec![Vec2::new(0.0, 0.0), Vec2::new(w, 0.0), Vec2::new(w, d), Vec2::new(0.0, d)]);
        Building {
            id: 1,
            kind: Kind::House,
            name: None,
            outline: outline.clone(),
            holes: vec![],
            parts: vec![Part {
                outer: outline,
                holes: vec![],
                min_h: 0.0,
                wall_top: 2.9 * levels as f32,
                roof: Roof::Gabled,
                roof_h: 3.0,
                ridge: None,
                wall: Wall::Siding,
                wall_col: [0.8; 3],
                roof_mat: RoofMat::Shingles,
                roof_col: None,
                windows: Windows::House,
                levels,
            }],
            levels,
            basement: true,
            height: 2.9 * levels as f32 + 3.0,
            entrances: vec![],
            indoor: vec![],
            seed: 7,
            detail: None,
            area: w * d,
            wikidata: None,
        }
    }

    #[test]
    fn two_storey_house_plan() {
        let b = house(12.0, 9.0, 2);
        let doors = super::super::exterior::doors(&b, &[0]);
        let p = plan(&b, &doors, &[]).expect("plan");
        // Basement, ground and upper floors; stairs between each.
        assert_eq!(p.floors.len(), 3);
        assert!(p.stairs.len() >= 2);
        for fl in &p.floors {
            let kinds: Vec<RoomKind> = fl.rooms.iter().filter(|r| r.cells > 0).map(|r| r.kind).collect();
            assert!(kinds.contains(&RoomKind::Stair), "{kinds:?}");
            // Every room with cells is reachable: doors or open pairs connect them.
            let n = fl.rooms.len();
            let mut adj = vec![vec![]; n];
            for d in &fl.doors {
                adj[d.rooms.0].push(d.rooms.1);
                adj[d.rooms.1].push(d.rooms.0);
            }
            for &(a, c) in &fl.open_pairs {
                adj[a].push(c);
                adj[c].push(a);
            }
            let start = fl.rooms.iter().position(|r| r.kind == RoomKind::Stair).unwrap();
            let mut seen = vec![false; n];
            let mut st = vec![start];
            seen[start] = true;
            while let Some(x) = st.pop() {
                for &y in &adj[x] {
                    if !seen[y] {
                        seen[y] = true;
                        st.push(y);
                    }
                }
            }
            for (r, room) in fl.rooms.iter().enumerate() {
                if room.cells > 0 {
                    assert!(seen[r], "level {} room {:?} unreachable", fl.level, room.kind);
                }
            }
        }
        let g = &p.floors[1];
        let kinds: Vec<RoomKind> = g.rooms.iter().map(|r| r.kind).collect();
        assert!(kinds.contains(&RoomKind::Kitchen) && kinds.contains(&RoomKind::Living), "{kinds:?}");
        let up = &p.floors[2];
        assert!(up.rooms.iter().filter(|r| r.kind == RoomKind::Bedroom).count() >= 2);
        let _ = model::hash01(0);
    }

    #[test]
    fn office_has_core() {
        let mut b = house(40.0, 24.0, 5);
        b.kind = Kind::Office;
        b.parts[0].wall_top = 3.8 * 5.0;
        b.basement = false;
        let doors = super::super::exterior::doors(&b, &[0]);
        let p = plan(&b, &doors, &[]).expect("plan");
        assert_eq!(p.floors.len(), 5);
        assert!(p.lift.is_some());
        let k: Vec<RoomKind> = p.floors[2].rooms.iter().map(|r| r.kind).collect();
        assert!(k.contains(&RoomKind::OpenOffice) && k.contains(&RoomKind::Restroom) && k.contains(&RoomKind::Break), "{k:?}");
    }
}
