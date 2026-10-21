//! Furniture, fixtures and appliances built from shapes, merged into one mesh per
//! material per interior (so a furnished house is a handful of draw calls). Things you
//! can use (doors, appliance doors, seats, taps, mirrors) are described as specs and
//! become their own entities.

use super::textures::M;
use bevy::asset::RenderAssetUsages;
use bevy::math::{Affine3A, Quat, Vec2, Vec3};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct MeshB {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub col: Vec<[f32; 4]>,
    pub idx: Vec<u32>,
}

/// Shapes collected per material.
#[derive(Default, Clone)]
pub struct Kit {
    pub parts: HashMap<M, MeshB>,
}

pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

pub fn rgb(r: f32, g: f32, b: f32) -> [f32; 4] {
    [r, g, b, 1.0]
}

impl Kit {
    fn mb(&mut self, m: M) -> &mut MeshB {
        self.parts.entry(m).or_default()
    }

    /// A quad from four corners (counter-clockwise seen from the front), textured in
    /// metres along its edges.
    pub fn quad(&mut self, m: M, c: [Vec3; 4], col: [f32; 4]) {
        let s = m.size();
        let ex = (c[1] - c[0]).normalize_or(Vec3::X);
        let n = (c[1] - c[0]).cross(c[3] - c[0]).normalize_or(Vec3::Y);
        let ey = n.cross(ex);
        let b = self.mb(m);
        let base = b.pos.len() as u32;
        for p in c {
            b.pos.push(p.to_array());
            b.nrm.push(n.to_array());
            b.uv.push([(p - c[0]).dot(ex) / s, -(p - c[0]).dot(ey) / s]);
            b.col.push(col);
        }
        b.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A horizontal quad textured in world metres (floors and ceilings line up).
    pub fn floor_quad(&mut self, m: M, c: [Vec3; 4], col: [f32; 4], up: bool) {
        let s = m.size();
        let n = if up { Vec3::Y } else { -Vec3::Y };
        let b = self.mb(m);
        let base = b.pos.len() as u32;
        for p in c {
            b.pos.push(p.to_array());
            b.nrm.push(n.to_array());
            b.uv.push([p.x / s, p.z / s]);
            b.col.push(col);
        }
        if up {
            b.idx.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        } else {
            b.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }

    /// Triangles of a horizontal polygon at height y, textured in world metres.
    pub fn polygon(&mut self, m: M, pts: &[Vec2], tris: &[u32], y: f32, up: bool, col: [f32; 4]) {
        let s = m.size();
        let n = if up { Vec3::Y } else { -Vec3::Y };
        let b = self.mb(m);
        let base = b.pos.len() as u32;
        for p in pts {
            b.pos.push([p.x, y, p.y]);
            b.nrm.push(n.to_array());
            b.uv.push([p.x / s, p.y / s]);
            b.col.push(col);
        }
        for t in tris.chunks_exact(3) {
            if up {
                b.idx.extend_from_slice(&[base + t[0], base + t[2], base + t[1]]);
            } else {
                b.idx.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
            }
        }
    }

    /// A box of `size` centred at the transform's origin.
    pub fn cube(&mut self, m: M, xf: Affine3A, size: Vec3, col: [f32; 4]) {
        let h = size * 0.5;
        let p = |x: f32, y: f32, z: f32| xf.transform_point3(Vec3::new(x * h.x, y * h.y, z * h.z));
        let faces = [
            [p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)],
            [p(1.0, -1.0, -1.0), p(-1.0, -1.0, -1.0), p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0)],
            [p(1.0, -1.0, 1.0), p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0), p(1.0, 1.0, 1.0)],
            [p(-1.0, -1.0, -1.0), p(-1.0, -1.0, 1.0), p(-1.0, 1.0, 1.0), p(-1.0, 1.0, -1.0)],
            [p(-1.0, 1.0, 1.0), p(1.0, 1.0, 1.0), p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0)],
            [p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0), p(1.0, -1.0, 1.0), p(-1.0, -1.0, 1.0)],
        ];
        for f in faces {
            self.quad(m, f, col);
        }
    }

    /// A vertical cylinder of radius r and height h, base at the transform's origin.
    pub fn cyl(&mut self, m: M, xf: Affine3A, r: f32, h: f32, seg: u32, col: [f32; 4], caps: bool) {
        self.frustum(m, xf, r, r, h, seg, col, caps);
    }

    /// A cylinder tapering from r0 at the base to r1 at the top.
    #[allow(clippy::too_many_arguments)]
    pub fn frustum(&mut self, m: M, xf: Affine3A, r0: f32, r1: f32, h: f32, seg: u32, col: [f32; 4], caps: bool) {
        let s = m.size();
        let b = self.mb(m);
        let base = b.pos.len() as u32;
        for k in 0..=seg {
            let a = k as f32 / seg as f32 * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            for (r, y) in [(r0, 0.0), (r1, h)] {
                b.pos.push(xf.transform_point3(Vec3::new(c * r, y, sn * r)).to_array());
                b.nrm.push(xf.transform_vector3(Vec3::new(c, (r0 - r1) / h.max(1e-3), sn)).normalize_or(Vec3::Y).to_array());
                b.uv.push([a * r.max(0.01) / s, y / s]);
                b.col.push(col);
            }
        }
        for k in 0..seg {
            let i = base + k * 2;
            b.idx.extend_from_slice(&[i, i + 1, i + 3, i, i + 3, i + 2]);
        }
        if caps {
            for (r, y, up) in [(r0, 0.0, false), (r1, h, true)] {
                if r <= 0.0 {
                    continue;
                }
                let c0 = b.pos.len() as u32;
                let n = xf.transform_vector3(if up { Vec3::Y } else { -Vec3::Y }).normalize();
                b.pos.push(xf.transform_point3(Vec3::new(0.0, y, 0.0)).to_array());
                b.nrm.push(n.to_array());
                b.uv.push([0.0, 0.0]);
                b.col.push(col);
                for k in 0..=seg {
                    let a = k as f32 / seg as f32 * std::f32::consts::TAU;
                    b.pos.push(xf.transform_point3(Vec3::new(a.cos() * r, y, a.sin() * r)).to_array());
                    b.nrm.push(n.to_array());
                    b.uv.push([a.cos() * r / s, a.sin() * r / s]);
                    b.col.push(col);
                }
                for k in 0..seg {
                    if up {
                        b.idx.extend_from_slice(&[c0, c0 + k + 2, c0 + k + 1]);
                    } else {
                        b.idx.extend_from_slice(&[c0, c0 + k + 1, c0 + k + 2]);
                    }
                }
            }
        }
    }

    /// An ellipsoid (sphere scaled by `size` / 2), centred.
    pub fn ball(&mut self, m: M, xf: Affine3A, size: Vec3, col: [f32; 4]) {
        let (rings, seg) = (6u32, 10u32);
        let b = self.mb(m);
        let base = b.pos.len() as u32;
        for i in 0..=rings {
            let t = i as f32 / rings as f32 * std::f32::consts::PI;
            for k in 0..=seg {
                let a = k as f32 / seg as f32 * std::f32::consts::TAU;
                let d = Vec3::new(t.sin() * a.cos(), t.cos(), t.sin() * a.sin());
                b.pos.push(xf.transform_point3(d * size * 0.5).to_array());
                b.nrm.push(xf.transform_vector3(d / size.max(Vec3::splat(1e-3))).normalize_or(Vec3::Y).to_array());
                b.uv.push([k as f32 / seg as f32, i as f32 / rings as f32]);
                b.col.push(col);
            }
        }
        for i in 0..rings {
            for k in 0..seg {
                let a = base + i * (seg + 1) + k;
                let c = a + seg + 1;
                b.idx.extend_from_slice(&[a, a + 1, c + 1, a, c + 1, c]);
            }
        }
    }

    pub fn merge(&mut self, other: Kit) {
        for (m, o) in other.parts {
            let b = self.mb(m);
            let base = b.pos.len() as u32;
            b.pos.extend(o.pos);
            b.nrm.extend(o.nrm);
            b.uv.extend(o.uv);
            b.col.extend(o.col);
            b.idx.extend(o.idx.into_iter().map(|i| i + base));
        }
    }

    pub fn into_meshes(self) -> Vec<(M, Mesh)> {
        self.parts
            .into_iter()
            .filter(|(_, b)| !b.idx.is_empty())
            .map(|(m, b)| {
                let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, b.pos);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.col);
                mesh.insert_indices(Indices::U32(b.idx));
                if mesh.generate_tangents().is_err() {
                    // Flat surfaces without a normal map don't need them.
                }
                (m, mesh)
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.values().all(|b| b.idx.is_empty())
    }
}

/// What kind of hinged thing (for the interaction prompt and sounds later).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HingeKind {
    Door,
    ExteriorDoor,
    Fridge,
    Freezer,
    Oven,
    Microwave,
    Dishwasher,
    Cabinet,
    Washer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixtureKind {
    Sink,
    KitchenSink,
    Shower,
    Tub,
    TubShower,
    Toilet,
}

/// Things that become their own entities, in interior-local coordinates.
pub enum Spec {
    /// Rotates about `hinge` (vertical axis unless `flip_down`) by up to `angle`.
    Hinged {
        kit: Kit,
        hinge: Vec3,
        angle: f32,
        kind: HingeKind,
        half: Vec3,
        centre: Vec3,
        flip_down: bool,
        door_id: Option<u32>,
        front: Vec3,
    },
    Seat {
        at: Vec3,
        yaw: f32,
        half: Vec3,
        centre: Vec3,
        lie: bool,
    },
    Fixture {
        kind: FixtureKind,
        spout: Vec3,
        basin_centre: Vec3,
        basin_half: Vec3,
        fill_h: f32,
        half: Vec3,
        centre: Vec3,
        yaw: f32,
    },
    Mirror {
        centre: Vec3,
        yaw: f32,
        w: f32,
        h: f32,
    },
    Light {
        at: Vec3,
        lumens: f32,
    },
    Screen {
        centre: Vec3,
        yaw: f32,
        w: f32,
        h: f32,
    },
    Conveyor {
        from: Vec3,
        to: Vec3,
        width: f32,
    },
}

pub fn spec_offset(s: &mut Spec, by: Affine3A) {
    let tp = |v: &mut Vec3| *v = by.transform_point3(*v);
    match s {
        Spec::Hinged { hinge, centre, .. } => {
            tp(hinge);
            tp(centre);
        }
        Spec::Seat { at, centre, .. } => {
            tp(at);
            tp(centre);
        }
        Spec::Fixture { spout, basin_centre, centre, .. } => {
            tp(spout);
            tp(basin_centre);
            tp(centre);
        }
        Spec::Mirror { centre, .. } | Spec::Screen { centre, .. } => tp(centre),
        Spec::Light { at, .. } => tp(at),
        Spec::Conveyor { from, to, .. } => {
            tp(from);
            tp(to);
        }
    }
}

/// Furniture and fixtures, each a few dozen shapes at real sizes. Local frame: +x to
/// the right, +y up, -z forward (the front faces -z, like props); origin on the floor
/// at the item's back-centre for wall pieces, centre for free-standing ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Bed { width: f32 },
    Nightstand,
    Dresser,
    Wardrobe,
    Desk,
    OfficeChair,
    DiningTable { seats: u32 },
    DiningChair,
    Sofa { seats: u32 },
    Armchair,
    CoffeeTable,
    TvStand,
    Bookshelf,
    KitchenRun { len: f32, sink_at: Option<f32>, stove_at: Option<f32> },
    Fridge,
    Microwave,
    Dishwasher,
    Toilet,
    Vanity,
    Tub,
    Shower,
    TubShower,
    Washer,
    Dryer,
    WaterHeater,
    Furnace,
    Boxes { n: u32 },
    Workbench,
    Cubicle,
    Printer,
    Scanner,
    FilingCabinet,
    ConferenceTable { len: f32 },
    CoffeeMaker,
    WaterCooler,
    VendingMachine,
    Gondola { len: f32 },
    Checkout,
    ProduceBin,
    FridgeCase { len: f32 },
    Counter { len: f32 },
    Booth,
    CafeTable,
    BarStool,
    RestaurantTable { cloth: bool },
    EspressoMachine,
    Fryer,
    Grill,
    PrepTable,
    Machine,
    PalletRack { len: f32 },
    Locker { n: u32 },
    Pew { len: f32 },
    SchoolDesk,
    HospitalBed,
    Plant,
    Rug { w: f32, d: f32 },
    FloorLamp,
    Lamp,
    Painting { w: f32, h: f32 },
    Outlet,
    Ethernet,
    LightSwitch,
    Pipe { len: f32, vertical: bool },
    Shelf { len: f32 },
}

fn t(x: f32, y: f32, z: f32) -> Affine3A {
    Affine3A::from_translation(Vec3::new(x, y, z))
}

fn ty(x: f32, y: f32, z: f32, yaw: f32) -> Affine3A {
    Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(x, y, z))
}

/// A hash for picking colours.
fn pick<T: Copy>(opts: &[T], seed: u64) -> T {
    opts[(super::model::hash64(seed) % opts.len() as u64) as usize]
}

pub fn fabric_colour(seed: u64) -> [f32; 4] {
    pick(
        &[
            rgb(0.35, 0.37, 0.42),
            rgb(0.55, 0.5, 0.42),
            rgb(0.18, 0.25, 0.38),
            rgb(0.42, 0.18, 0.15),
            rgb(0.6, 0.6, 0.58),
            rgb(0.25, 0.32, 0.25),
            rgb(0.12, 0.12, 0.13),
        ],
        seed,
    )
}

/// Build an item into `kit` (its static parts) and `specs` (usable parts), at `at`.
pub fn build(item: Item, at: Affine3A, seed: u64, kit: &mut Kit, specs: &mut Vec<Spec>) {
    let mut k = Kit::default();
    let mut s: Vec<Spec> = Vec::new();
    let fab = fabric_colour(seed);
    match item {
        Item::Bed { width } => {
            let l = 2.05;
            // Frame and headboard.
            k.cube(M::Wood, t(0.0, 0.18, -l * 0.5), Vec3::new(width + 0.08, 0.3, l), WHITE);
            k.cube(M::Wood, t(0.0, 0.55, -0.03), Vec3::new(width + 0.1, 1.1, 0.06), WHITE);
            // Mattress, duvet, pillows.
            k.cube(M::Fabric, t(0.0, 0.43, -l * 0.5 - 0.02), Vec3::new(width, 0.24, l - 0.08), rgb(0.92, 0.92, 0.9));
            k.cube(M::Fabric, t(0.0, 0.57, -l * 0.5 - 0.3), Vec3::new(width + 0.06, 0.06, l * 0.7), fab);
            let np = if width > 1.2 { 2 } else { 1 };
            for i in 0..np {
                let x = if np == 1 { 0.0 } else { (i as f32 - 0.5) * width * 0.5 };
                k.ball(M::Fabric, t(x, 0.62, -0.28), Vec3::new(width / np as f32 * 0.8, 0.14, 0.38), rgb(0.95, 0.95, 0.93));
            }
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.55, -l * 0.5),
                yaw: 0.0,
                half: Vec3::new(width * 0.5, 0.3, l * 0.5),
                centre: Vec3::new(0.0, 0.4, -l * 0.5),
                lie: true,
            });
        }
        Item::Nightstand => {
            k.cube(M::Wood, t(0.0, 0.3, -0.22), Vec3::new(0.48, 0.6, 0.42), WHITE);
            k.cube(M::Metal, t(0.0, 0.42, -0.44), Vec3::new(0.12, 0.02, 0.02), WHITE);
            // Lamp.
            k.cyl(M::Ceramic, t(0.0, 0.6, -0.22), 0.07, 0.25, 12, rgb(0.8, 0.78, 0.72), true);
            k.frustum(M::Fabric, t(0.0, 0.82, -0.22), 0.16, 0.11, 0.2, 14, rgb(0.95, 0.92, 0.85), false);
        }
        Item::Dresser => {
            k.cube(M::Wood, t(0.0, 0.42, -0.25), Vec3::new(1.4, 0.84, 0.48), WHITE);
            for r in 0..3 {
                for c in [-0.33f32, 0.33] {
                    k.cube(M::Wood, t(c, 0.16 + r as f32 * 0.26, -0.495), Vec3::new(0.62, 0.22, 0.02), rgb(0.92, 0.9, 0.88));
                    k.cube(M::Metal, t(c, 0.16 + r as f32 * 0.26, -0.51), Vec3::new(0.12, 0.02, 0.02), WHITE);
                }
            }
        }
        Item::Wardrobe => {
            k.cube(M::Wood, t(0.0, 1.0, -0.3), Vec3::new(1.2, 2.0, 0.6), WHITE);
            k.cube(M::Metal, t(-0.05, 1.05, -0.61), Vec3::new(0.02, 0.25, 0.02), WHITE);
            k.cube(M::Metal, t(0.05, 1.05, -0.61), Vec3::new(0.02, 0.25, 0.02), WHITE);
        }
        Item::Desk => {
            k.cube(M::Wood, t(0.0, 0.74, -0.35), Vec3::new(1.4, 0.04, 0.7), WHITE);
            for x in [-0.66f32, 0.66] {
                k.cube(M::Wood, t(x, 0.37, -0.35), Vec3::new(0.04, 0.72, 0.66), WHITE);
            }
            computer(&mut k, &mut s, t(0.0, 0.76, -0.25));
            phone(&mut k, t(0.5, 0.76, -0.2));
        }
        Item::OfficeChair => {
            let c = rgb(0.1, 0.1, 0.11);
            for i in 0..5 {
                let a = i as f32 * std::f32::consts::TAU / 5.0;
                k.cube(M::Plastic, ty(a.cos() * 0.15, 0.06, a.sin() * 0.15, -a), Vec3::new(0.3, 0.04, 0.05), c);
                k.ball(M::Rubber, t(a.cos() * 0.29, 0.03, a.sin() * 0.29), Vec3::splat(0.06), c);
            }
            k.cyl(M::Chrome, t(0.0, 0.06, 0.0), 0.025, 0.36, 10, WHITE, false);
            k.cube(M::Fabric, t(0.0, 0.47, 0.0), Vec3::new(0.5, 0.08, 0.48), c);
            k.cube(M::Fabric, t(0.0, 0.82, 0.22), Vec3::new(0.46, 0.6, 0.07), c);
            for x in [-0.25f32, 0.25] {
                k.cube(M::Plastic, t(x, 0.64, 0.02), Vec3::new(0.05, 0.03, 0.3), c);
            }
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.5, 0.05),
                yaw: 0.0,
                half: Vec3::new(0.28, 0.5, 0.28),
                centre: Vec3::new(0.0, 0.5, 0.0),
                lie: false,
            });
        }
        Item::DiningTable { seats } => {
            let l = 0.6 * (seats as f32 / 2.0).max(1.0) + 0.3;
            k.cube(M::Wood, t(0.0, 0.74, 0.0), Vec3::new(l, 0.04, 0.9), WHITE);
            for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                k.cube(M::Wood, t(x * (l * 0.5 - 0.06), 0.36, z * 0.39), Vec3::new(0.06, 0.72, 0.06), WHITE);
            }
            let per = (seats / 2).max(1);
            for i in 0..per {
                let x = (i as f32 + 0.5) / per as f32 * l - l * 0.5;
                for (z, yaw) in [(-0.62f32, std::f32::consts::PI), (0.62, 0.0)] {
                    let mut sub = Kit::default();
                    let mut ss = Vec::new();
                    build(Item::DiningChair, ty(x, 0.0, z, yaw), seed ^ i as u64, &mut sub, &mut ss);
                    k.merge(sub);
                    s.extend(ss);
                }
            }
        }
        Item::DiningChair => {
            k.cube(M::Wood, t(0.0, 0.45, 0.0), Vec3::new(0.44, 0.04, 0.42), WHITE);
            for (x, z) in [(-0.19f32, -0.18f32), (0.19, -0.18), (-0.19, 0.18), (0.19, 0.18)] {
                k.cube(M::Wood, t(x, 0.22, z), Vec3::new(0.035, 0.44, 0.035), WHITE);
            }
            for x in [-0.19f32, 0.19] {
                k.cube(M::Wood, t(x, 0.68, 0.19), Vec3::new(0.035, 0.46, 0.035), WHITE);
            }
            for y in [0.62f32, 0.86] {
                k.cube(M::Wood, t(0.0, y, 0.19), Vec3::new(0.4, 0.06, 0.02), WHITE);
            }
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.47, 0.0),
                yaw: 0.0,
                half: Vec3::new(0.24, 0.45, 0.24),
                centre: Vec3::new(0.0, 0.45, 0.0),
                lie: false,
            });
        }
        Item::Sofa { seats } => {
            let w = 0.7 * seats as f32 + 0.4;
            k.cube(M::Fabric, t(0.0, 0.22, -0.45), Vec3::new(w, 0.3, 0.88), fab);
            k.cube(M::Fabric, t(0.0, 0.62, -0.1), Vec3::new(w, 0.55, 0.2), fab);
            for x in [-1.0f32, 1.0] {
                k.cube(M::Fabric, t(x * (w * 0.5 - 0.1), 0.45, -0.45), Vec3::new(0.2, 0.42, 0.88), fab);
            }
            for i in 0..seats {
                let x = (i as f32 + 0.5) / seats as f32 * (w - 0.4) - (w - 0.4) * 0.5;
                k.cube(M::Fabric, t(x, 0.44, -0.5), Vec3::new((w - 0.4) / seats as f32 - 0.02, 0.14, 0.7), fab);
                s.push(Spec::Seat {
                    at: Vec3::new(x, 0.5, -0.5),
                    yaw: 0.0,
                    half: Vec3::new(0.3, 0.3, 0.4),
                    centre: Vec3::new(x, 0.4, -0.45),
                    lie: false,
                });
            }
            for x in [-1.0f32, 1.0] {
                k.cube(M::DarkWood, t(x * (w * 0.5 - 0.1), 0.04, -0.45), Vec3::new(0.05, 0.08, 0.05), WHITE);
            }
        }
        Item::Armchair => {
            k.cube(M::Leather, t(0.0, 0.22, -0.45), Vec3::new(0.85, 0.3, 0.85), fab);
            k.cube(M::Leather, t(0.0, 0.62, -0.1), Vec3::new(0.85, 0.55, 0.2), fab);
            for x in [-1.0f32, 1.0] {
                k.cube(M::Leather, t(x * 0.33, 0.47, -0.45), Vec3::new(0.18, 0.4, 0.85), fab);
            }
            k.cube(M::Leather, t(0.0, 0.42, -0.5), Vec3::new(0.5, 0.12, 0.7), fab);
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.48, -0.5),
                yaw: 0.0,
                half: Vec3::new(0.4, 0.4, 0.4),
                centre: Vec3::new(0.0, 0.4, -0.45),
                lie: false,
            });
        }
        Item::CoffeeTable => {
            k.cube(M::Wood, t(0.0, 0.42, 0.0), Vec3::new(1.1, 0.04, 0.6), WHITE);
            k.cube(M::Wood, t(0.0, 0.12, 0.0), Vec3::new(1.0, 0.02, 0.5), WHITE);
            for (x, z) in [(-0.5f32, -0.25f32), (0.5, -0.25), (-0.5, 0.25), (0.5, 0.25)] {
                k.cube(M::Wood, t(x, 0.21, z), Vec3::new(0.04, 0.42, 0.04), WHITE);
            }
        }
        Item::TvStand => {
            k.cube(M::DarkWood, t(0.0, 0.25, -0.22), Vec3::new(1.6, 0.5, 0.42), WHITE);
            k.cube(M::Plastic, t(0.0, 0.52, -0.2), Vec3::new(0.3, 0.03, 0.2), WHITE);
            k.cube(M::Plastic, t(0.0, 0.62, -0.2), Vec3::new(0.05, 0.2, 0.04), WHITE);
            k.cube(M::Plastic, t(0.0, 1.06, -0.2), Vec3::new(1.45, 0.84, 0.05), WHITE);
            s.push(Spec::Screen { centre: Vec3::new(0.0, 1.06, -0.23), yaw: 0.0, w: 1.4, h: 0.79 });
        }
        Item::Bookshelf => {
            k.cube(M::Wood, t(0.0, 1.0, -0.17), Vec3::new(0.9, 2.0, 0.02), WHITE);
            for x in [-0.44f32, 0.44] {
                k.cube(M::Wood, t(x, 1.0, -0.17), Vec3::new(0.02, 2.0, 0.34), WHITE);
            }
            for r in 0..6 {
                let y = 0.02 + r as f32 * 0.38;
                k.cube(M::Wood, t(0.0, y, -0.17), Vec3::new(0.88, 0.02, 0.34), WHITE);
                if r < 5 {
                    // Books.
                    let mut x = -0.4;
                    let mut i = 0;
                    while x < 0.38 {
                        let w = 0.025 + super::model::hash01(seed ^ (r * 50 + i)) * 0.035;
                        let h = 0.2 + super::model::hash01(seed ^ (r * 50 + i + 7)) * 0.12;
                        let c = pick(
                            &[
                                rgb(0.5, 0.1, 0.1),
                                rgb(0.1, 0.2, 0.4),
                                rgb(0.15, 0.35, 0.2),
                                rgb(0.8, 0.75, 0.6),
                                rgb(0.1, 0.1, 0.1),
                                rgb(0.6, 0.4, 0.15),
                            ],
                            seed ^ (r * 50 + i + 3),
                        );
                        k.cube(M::Paint, t(x + w * 0.5, y + 0.01 + h * 0.5, -0.19), Vec3::new(w - 0.003, h, 0.22), c);
                        x += w;
                        i += 1;
                    }
                }
            }
        }
        Item::KitchenRun { len, sink_at, stove_at } => {
            let top = pick(&[rgb(0.85, 0.84, 0.8), rgb(0.2, 0.2, 0.21), rgb(0.6, 0.55, 0.48)], seed ^ 1);
            let cab = pick(&[rgb(0.95, 0.95, 0.93), rgb(0.85, 0.82, 0.75), rgb(0.3, 0.38, 0.45), rgb(1.0, 1.0, 1.0)], seed ^ 2);
            let cm = if cab[0] > 0.9 { M::Paint } else { M::Wood };
            // Base cabinets and countertop.
            k.cube(cm, t(len * 0.5, 0.45, -0.3), Vec3::new(len, 0.8, 0.58), cab);
            k.cube(M::Rubber, t(len * 0.5, 0.05, -0.55), Vec3::new(len, 0.1, 0.06), rgb(0.1, 0.1, 0.1));
            k.cube(M::Marble, t(len * 0.5, 0.88, -0.31), Vec3::new(len + 0.02, 0.04, 0.64), top);
            // Backsplash.
            k.quad(
                M::SubwayTile,
                [Vec3::new(0.0, 0.9, -0.005), Vec3::new(len, 0.9, -0.005), Vec3::new(len, 1.45, -0.005), Vec3::new(0.0, 1.45, -0.005)],
                WHITE,
            );
            // Upper cabinets.
            k.cube(cm, t(len * 0.5, 1.85, -0.17), Vec3::new(len, 0.75, 0.34), cab);
            let doors = (len / 0.6).floor().max(1.0) as u32;
            for i in 0..doors {
                let x = (i as f32 + 0.5) * len / doors as f32;
                k.cube(M::Metal, t(x + 0.2, 0.75, -0.6), Vec3::new(0.02, 0.12, 0.02), WHITE);
                k.cube(M::Metal, t(x + 0.2, 1.6, -0.345), Vec3::new(0.02, 0.12, 0.02), WHITE);
                k.cube(cm, t(x, 0.45, -0.595), Vec3::new(len / doors as f32 - 0.01, 0.72, 0.01), cab);
            }
            if let Some(sx) = sink_at {
                // Double-bowl sink and a gooseneck faucet.
                k.cube(M::Chrome, t(sx, 0.86, -0.32), Vec3::new(0.8, 0.02, 0.48), WHITE);
                k.cube(M::Metal, t(sx, 0.72, -0.32), Vec3::new(0.74, 0.28, 0.42), rgb(0.6, 0.6, 0.62));
                k.cyl(M::Chrome, t(sx, 0.9, -0.08), 0.02, 0.32, 10, WHITE, false);
                k.cube(M::Chrome, t(sx, 1.21, -0.16), Vec3::new(0.025, 0.025, 0.18), WHITE);
                s.push(Spec::Fixture {
                    kind: FixtureKind::KitchenSink,
                    spout: Vec3::new(sx, 1.19, -0.25),
                    basin_centre: Vec3::new(sx, 0.62, -0.32),
                    basin_half: Vec3::new(0.36, 0.0, 0.2),
                    fill_h: 0.2,
                    half: Vec3::new(0.4, 0.25, 0.3),
                    centre: Vec3::new(sx, 0.95, -0.3),
                    yaw: 0.0,
                });
            }
            if let Some(ox) = stove_at {
                // Range: cooktop with burners, oven door that opens.
                k.cube(M::Metal, t(ox, 0.45, -0.32), Vec3::new(0.76, 0.9, 0.62), rgb(0.85, 0.86, 0.87));
                k.cube(M::Plastic, t(ox, 0.91, -0.32), Vec3::new(0.76, 0.02, 0.6), rgb(0.05, 0.05, 0.05));
                for (bx, bz) in [(-0.18f32, -0.15f32), (0.18, -0.15), (-0.18, -0.45), (0.18, -0.45)] {
                    k.cyl(M::Metal, t(ox + bx, 0.92, bz), 0.09, 0.01, 12, rgb(0.2, 0.2, 0.2), true);
                }
                k.cube(M::Metal, t(ox, 1.0, -0.05), Vec3::new(0.76, 0.18, 0.06), rgb(0.85, 0.86, 0.87));
                let mut door = Kit::default();
                door.cube(M::Metal, t(0.0, 0.3, 0.0), Vec3::new(0.7, 0.6, 0.04), rgb(0.85, 0.86, 0.87));
                door.cube(M::Glass, t(0.0, 0.32, -0.021), Vec3::new(0.45, 0.25, 0.005), WHITE);
                door.cube(M::Chrome, t(0.0, 0.56, -0.06), Vec3::new(0.55, 0.025, 0.025), WHITE);
                s.push(Spec::Hinged {
                    kit: door,
                    hinge: Vec3::new(ox, 0.12, -0.63),
                    angle: 1.4,
                    kind: HingeKind::Oven,
                    half: Vec3::new(0.35, 0.3, 0.05),
                    centre: Vec3::new(ox, 0.42, -0.64),
                    flip_down: true,
                    door_id: None,
                    front: Vec3::NEG_Z,
                });
                // Range hood.
                k.frustum(M::Metal, t(ox, 1.55, -0.25), 0.4, 0.15, 0.45, 4, rgb(0.8, 0.8, 0.82), false);
            }
        }
        Item::Fridge => {
            let c = pick(&[rgb(0.85, 0.86, 0.87), rgb(0.95, 0.95, 0.94), rgb(0.12, 0.12, 0.12)], seed);
            k.cube(M::Metal, t(0.0, 0.88, -0.36), Vec3::new(0.86, 1.76, 0.68), c);
            // Inside: white liner and shelves (seen when the doors open).
            k.cube(M::WhitePlastic, t(0.0, 0.88, -0.705), Vec3::new(0.8, 1.7, 0.01), WHITE);
            for y in [0.5f32, 0.85, 1.2] {
                k.cube(M::Glass, t(0.0, y, -0.4), Vec3::new(0.78, 0.01, 0.55), WHITE);
            }
            for (i, (y, h, kind)) in [(1.15f32, 1.1f32, HingeKind::Fridge), (0.32, 0.6, HingeKind::Freezer)].into_iter().enumerate() {
                let mut door = Kit::default();
                door.cube(M::Metal, t(-0.43, 0.0, 0.0), Vec3::new(0.86, h - 0.02, 0.06), c);
                door.cube(M::Chrome, t(-0.8, if i == 0 { -h * 0.3 } else { h * 0.3 }, -0.06), Vec3::new(0.03, 0.4, 0.03), WHITE);
                s.push(Spec::Hinged {
                    kit: door,
                    hinge: Vec3::new(0.43, y, -0.73),
                    angle: 1.9,
                    kind,
                    half: Vec3::new(0.43, h * 0.5, 0.04),
                    centre: Vec3::new(0.0, y, -0.74),
                    flip_down: false,
                    door_id: None,
                    front: Vec3::NEG_Z,
                });
            }
        }
        Item::Microwave => {
            k.cube(M::Metal, t(0.0, 0.15, -0.2), Vec3::new(0.5, 0.3, 0.36), rgb(0.15, 0.15, 0.16));
            k.cube(M::WhitePlastic, t(-0.05, 0.15, -0.21), Vec3::new(0.34, 0.24, 0.32), rgb(0.9, 0.9, 0.85));
            let mut door = Kit::default();
            door.cube(M::Plastic, t(0.17, 0.0, 0.0), Vec3::new(0.36, 0.28, 0.02), rgb(0.08, 0.08, 0.09));
            door.cube(M::Glass, t(0.17, 0.0, -0.011), Vec3::new(0.3, 0.2, 0.002), WHITE);
            s.push(Spec::Hinged {
                kit: door,
                hinge: Vec3::new(-0.23, 0.15, -0.39),
                angle: 1.6,
                kind: HingeKind::Microwave,
                half: Vec3::new(0.25, 0.15, 0.04),
                centre: Vec3::new(0.0, 0.15, -0.39),
                flip_down: false,
                door_id: None,
                front: Vec3::NEG_Z,
            });
        }
        Item::Dishwasher => {
            k.cube(M::Metal, t(0.0, 0.43, -0.3), Vec3::new(0.6, 0.82, 0.58), rgb(0.85, 0.86, 0.87));
            let mut door = Kit::default();
            door.cube(M::Metal, t(0.0, 0.38, 0.0), Vec3::new(0.6, 0.76, 0.03), rgb(0.85, 0.86, 0.87));
            door.cube(M::Chrome, t(0.0, 0.72, -0.04), Vec3::new(0.45, 0.025, 0.025), WHITE);
            s.push(Spec::Hinged {
                kit: door,
                hinge: Vec3::new(0.0, 0.06, -0.6),
                angle: 1.5,
                kind: HingeKind::Dishwasher,
                half: Vec3::new(0.3, 0.4, 0.03),
                centre: Vec3::new(0.0, 0.45, -0.6),
                flip_down: true,
                door_id: None,
                front: Vec3::NEG_Z,
            });
        }
        Item::Toilet => {
            k.cube(M::Ceramic, t(0.0, 0.62, -0.1), Vec3::new(0.48, 0.38, 0.18), WHITE);
            k.cube(M::Ceramic, t(0.0, 0.83, -0.1), Vec3::new(0.5, 0.04, 0.2), WHITE);
            k.frustum(M::Ceramic, t(0.0, 0.0, -0.42), 0.15, 0.2, 0.38, 14, WHITE, false);
            k.ball(M::Ceramic, t(0.0, 0.38, -0.45), Vec3::new(0.38, 0.06, 0.5), WHITE);
            k.cube(M::Chrome, t(-0.2, 0.75, -0.2), Vec3::new(0.06, 0.02, 0.02), WHITE);
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.42, -0.42),
                yaw: 0.0,
                half: Vec3::new(0.22, 0.4, 0.35),
                centre: Vec3::new(0.0, 0.4, -0.38),
                lie: false,
            });
            s.push(Spec::Fixture {
                kind: FixtureKind::Toilet,
                spout: Vec3::new(0.0, 0.35, -0.45),
                basin_centre: Vec3::new(0.0, 0.3, -0.45),
                basin_half: Vec3::new(0.13, 0.0, 0.18),
                fill_h: 0.0,
                half: Vec3::new(0.1, 0.06, 0.06),
                centre: Vec3::new(-0.2, 0.75, -0.2),
                yaw: 0.0,
            });
        }
        Item::Vanity => {
            let cab = pick(&[rgb(0.95, 0.95, 0.93), rgb(0.35, 0.25, 0.18), rgb(0.3, 0.38, 0.45)], seed);
            k.cube(M::Wood, t(0.0, 0.42, -0.27), Vec3::new(0.9, 0.82, 0.52), cab);
            k.cube(M::Marble, t(0.0, 0.85, -0.28), Vec3::new(0.94, 0.04, 0.56), rgb(0.92, 0.9, 0.88));
            k.ball(M::Ceramic, t(0.0, 0.83, -0.3), Vec3::new(0.42, 0.1, 0.32), WHITE);
            k.cyl(M::Chrome, t(0.0, 0.87, -0.06), 0.02, 0.18, 10, WHITE, true);
            k.cube(M::Chrome, t(0.0, 1.04, -0.12), Vec3::new(0.025, 0.025, 0.12), WHITE);
            s.push(Spec::Fixture {
                kind: FixtureKind::Sink,
                spout: Vec3::new(0.0, 1.02, -0.17),
                basin_centre: Vec3::new(0.0, 0.8, -0.3),
                basin_half: Vec3::new(0.18, 0.0, 0.13),
                fill_h: 0.06,
                half: Vec3::new(0.25, 0.15, 0.2),
                centre: Vec3::new(0.0, 0.92, -0.25),
                yaw: 0.0,
            });
            // Mirror and a light bar above.
            k.cube(M::Chrome, t(0.0, 1.55, -0.012), Vec3::new(0.8, 0.9, 0.02), WHITE);
            s.push(Spec::Mirror { centre: Vec3::new(0.0, 1.55, -0.025), yaw: 0.0, w: 0.76, h: 0.86 });
            k.cube(M::LightPanel, t(0.0, 2.08, -0.06), Vec3::new(0.6, 0.06, 0.08), WHITE);
        }
        Item::Tub | Item::TubShower => {
            k.cube(M::Ceramic, t(0.0, 0.28, -0.38), Vec3::new(1.6, 0.56, 0.76), WHITE);
            k.cube(M::Ceramic, t(0.0, 0.45, -0.38), Vec3::new(1.45, 0.3, 0.62), rgb(0.88, 0.9, 0.92));
            k.cyl(M::Chrome, ty(-0.7, 0.75, -0.15, 0.0), 0.02, 0.06, 10, WHITE, true);
            k.cube(M::Chrome, t(-0.7, 0.78, -0.22), Vec3::new(0.03, 0.03, 0.14), WHITE);
            let tub_shower = matches!(item, Item::TubShower);
            if tub_shower {
                // Tile surround, shower head and a curtain rod with curtain.
                for (a, b) in [(Vec3::new(-0.8, 0.56, -0.005), Vec3::new(0.8, 2.0, -0.005))] {
                    k.quad(
                        M::SubwayTile,
                        [Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, b.y, a.z), Vec3::new(a.x, b.y, a.z)],
                        WHITE,
                    );
                }
                k.cyl(M::Chrome, ty(-0.7, 1.85, -0.04, 0.0), 0.015, 0.1, 8, WHITE, true);
                k.frustum(M::Chrome, ty(-0.7, 1.82, -0.12, 0.0), 0.06, 0.03, 0.05, 12, WHITE, true);
                k.cyl(
                    M::Chrome,
                    Affine3A::from_rotation_translation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), Vec3::new(0.8, 2.0, -0.74)),
                    0.012,
                    1.6,
                    8,
                    WHITE,
                    false,
                );
                let cur = fabric_colour(seed ^ 99);
                k.quad(
                    M::Fabric,
                    [Vec3::new(0.3, 0.6, -0.73), Vec3::new(0.8, 0.6, -0.73), Vec3::new(0.8, 1.98, -0.73), Vec3::new(0.3, 1.98, -0.73)],
                    cur,
                );
                k.quad(
                    M::Fabric,
                    [Vec3::new(0.8, 0.6, -0.73), Vec3::new(0.3, 0.6, -0.73), Vec3::new(0.3, 1.98, -0.73), Vec3::new(0.8, 1.98, -0.73)],
                    cur,
                );
            }
            s.push(Spec::Fixture {
                kind: if tub_shower { FixtureKind::TubShower } else { FixtureKind::Tub },
                spout: if tub_shower { Vec3::new(-0.7, 1.78, -0.16) } else { Vec3::new(-0.7, 0.77, -0.28) },
                basin_centre: Vec3::new(0.0, 0.3, -0.38),
                basin_half: Vec3::new(0.7, 0.0, 0.29),
                fill_h: 0.32,
                half: Vec3::new(0.8, 0.4, 0.38),
                centre: Vec3::new(0.0, 0.5, -0.38),
                yaw: 0.0,
            });
        }
        Item::Shower => {
            k.cube(M::TileFloor, t(0.0, 0.05, -0.45), Vec3::new(0.9, 0.1, 0.9), WHITE);
            for (a, b) in [(Vec3::new(-0.45, 0.1, -0.005), Vec3::new(0.45, 2.05, -0.005))] {
                k.quad(
                    M::SubwayTile,
                    [Vec3::new(a.x, a.y, a.z), Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, b.y, a.z), Vec3::new(a.x, b.y, a.z)],
                    WHITE,
                );
            }
            k.cube(M::Glass, t(0.0, 1.05, -0.9), Vec3::new(0.9, 1.9, 0.01), WHITE);
            k.cube(M::Glass, t(0.45, 1.05, -0.45), Vec3::new(0.01, 1.9, 0.9), WHITE);
            k.cyl(M::Chrome, ty(0.0, 1.9, -0.04, 0.0), 0.015, 0.1, 8, WHITE, true);
            k.frustum(M::Chrome, t(0.0, 1.87, -0.14), 0.07, 0.03, 0.05, 12, WHITE, true);
            k.cyl(M::Chrome, t(0.0, 0.1, -0.45), 0.04, 0.005, 10, WHITE, true);
            s.push(Spec::Fixture {
                kind: FixtureKind::Shower,
                spout: Vec3::new(0.0, 1.84, -0.18),
                basin_centre: Vec3::new(0.0, 0.1, -0.45),
                basin_half: Vec3::new(0.4, 0.0, 0.4),
                fill_h: 0.0,
                half: Vec3::new(0.45, 1.0, 0.45),
                centre: Vec3::new(0.0, 1.0, -0.45),
                yaw: 0.0,
            });
        }
        Item::Washer | Item::Dryer => {
            k.cube(M::WhitePlastic, t(0.0, 0.45, -0.32), Vec3::new(0.68, 0.9, 0.64), WHITE);
            k.cube(M::WhitePlastic, t(0.0, 0.95, -0.08), Vec3::new(0.68, 0.12, 0.08), WHITE);
            let mut door = Kit::default();
            door.cyl(
                M::Chrome,
                Affine3A::from_rotation_translation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::new(0.2, 0.0, 0.0)),
                0.21,
                0.04,
                20,
                WHITE,
                true,
            );
            door.cyl(
                M::Glass,
                Affine3A::from_rotation_translation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), Vec3::new(0.2, 0.0, -0.045)),
                0.16,
                0.01,
                20,
                WHITE,
                true,
            );
            s.push(Spec::Hinged {
                kit: door,
                hinge: Vec3::new(-0.2, 0.48, -0.66),
                angle: 1.6,
                kind: HingeKind::Washer,
                half: Vec3::new(0.22, 0.22, 0.04),
                centre: Vec3::new(0.0, 0.48, -0.66),
                flip_down: false,
                door_id: None,
                front: Vec3::NEG_Z,
            });
        }
        Item::WaterHeater => {
            k.cyl(M::WhitePlastic, t(0.0, 0.05, -0.32), 0.28, 1.45, 18, rgb(0.85, 0.85, 0.82), true);
            k.cyl(M::Copper, t(-0.1, 1.5, -0.32), 0.02, 0.9, 8, WHITE, false);
            k.cyl(M::Copper, t(0.1, 1.5, -0.32), 0.02, 0.9, 8, WHITE, false);
            k.cyl(M::Metal, t(0.0, 1.5, -0.32), 0.05, 0.9, 10, rgb(0.6, 0.6, 0.62), false);
            k.cube(M::Metal, t(0.0, 0.3, -0.6), Vec3::new(0.18, 0.12, 0.06), rgb(0.3, 0.3, 0.3));
        }
        Item::Furnace => {
            k.cube(M::Metal, t(0.0, 0.75, -0.38), Vec3::new(0.6, 1.5, 0.75), rgb(0.62, 0.64, 0.66));
            k.cube(M::Metal, t(0.0, 1.8, -0.38), Vec3::new(0.5, 0.6, 0.5), rgb(0.7, 0.72, 0.74));
            k.cube(M::Metal, t(0.0, 2.25, -0.2), Vec3::new(0.4, 0.3, 1.5), rgb(0.7, 0.72, 0.74));
        }
        Item::Boxes { n } => {
            for i in 0..n {
                let r = |s: u64| super::model::hash01(seed ^ (i as u64 * 13 + s));
                let (w, h, d) = (0.35 + r(1) * 0.3, 0.25 + r(2) * 0.35, 0.3 + r(3) * 0.25);
                let x = (i % 3) as f32 * 0.65 - 0.65;
                let y = (i / 3) as f32;
                k.cube(M::Cardboard, ty(x + r(4) * 0.1, h * 0.5 + y * 0.6, -0.4 + r(5) * 0.1, r(6) * 0.4 - 0.2), Vec3::new(w, h, d), WHITE);
            }
        }
        Item::Workbench => {
            k.cube(M::Wood, t(0.0, 0.9, -0.35), Vec3::new(1.8, 0.05, 0.7), WHITE);
            k.cube(M::Wood, t(0.0, 0.3, -0.35), Vec3::new(1.7, 0.03, 0.6), WHITE);
            for (x, z) in [(-0.85f32, -0.05f32), (0.85, -0.05), (-0.85, -0.65), (0.85, -0.65)] {
                k.cube(M::Wood, t(x, 0.45, z), Vec3::new(0.07, 0.9, 0.07), WHITE);
            }
            k.cube(M::Wood, t(0.0, 1.5, -0.02), Vec3::new(1.8, 1.0, 0.02), rgb(0.75, 0.62, 0.45));
            for i in 0..6 {
                k.cube(M::Metal, t(-0.7 + i as f32 * 0.28, 1.5, -0.05), Vec3::new(0.03, 0.25, 0.03), rgb(0.4, 0.4, 0.42));
            }
        }
        Item::Cubicle => {
            // Partition panels on two sides, a desk with a computer, a chair.
            let pc = rgb(0.55, 0.55, 0.57);
            k.cube(M::Fabric, t(0.0, 0.7, -0.02), Vec3::new(1.8, 1.4, 0.05), pc);
            k.cube(M::Fabric, t(-0.88, 0.7, -0.8), Vec3::new(0.05, 1.4, 1.6), pc);
            k.cube(M::WhitePlastic, t(0.0, 0.73, -0.38), Vec3::new(1.7, 0.03, 0.7), rgb(0.85, 0.84, 0.8));
            k.cube(M::Metal, t(0.75, 0.36, -0.38), Vec3::new(0.4, 0.7, 0.6), rgb(0.5, 0.5, 0.52));
            computer(&mut k, &mut s, t(-0.1, 0.745, -0.25));
            phone(&mut k, t(0.45, 0.745, -0.2));
            let mut sub = Kit::default();
            build(Item::OfficeChair, ty(-0.1, 0.0, -1.05, std::f32::consts::PI), seed ^ 5, &mut sub, &mut s);
            k.merge(sub);
        }
        Item::Printer => {
            k.cube(M::WhitePlastic, t(0.0, 0.35, -0.3), Vec3::new(0.6, 0.7, 0.55), rgb(0.85, 0.85, 0.83));
            k.cube(M::WhitePlastic, t(0.0, 1.0, -0.3), Vec3::new(0.6, 0.6, 0.55), rgb(0.88, 0.88, 0.86));
            k.cube(M::Plastic, t(0.0, 1.33, -0.3), Vec3::new(0.58, 0.06, 0.5), rgb(0.25, 0.25, 0.27));
            k.cube(M::WhitePlastic, t(0.0, 0.85, -0.6), Vec3::new(0.4, 0.02, 0.15), WHITE);
            k.cube(M::Screen, t(0.2, 1.25, -0.58), Vec3::new(0.15, 0.08, 0.01), WHITE);
        }
        Item::Scanner => {
            k.cube(M::Plastic, t(0.0, 0.79, -0.25), Vec3::new(0.45, 0.08, 0.32), rgb(0.15, 0.15, 0.16));
            k.cube(M::Glass, t(0.0, 0.835, -0.25), Vec3::new(0.38, 0.005, 0.26), WHITE);
        }
        Item::FilingCabinet => {
            k.cube(M::Metal, t(0.0, 0.66, -0.3), Vec3::new(0.46, 1.32, 0.6), rgb(0.5, 0.52, 0.55));
            for r in 0..4 {
                k.cube(M::Chrome, t(0.0, 0.2 + r as f32 * 0.32, -0.61), Vec3::new(0.15, 0.03, 0.03), WHITE);
            }
        }
        Item::ConferenceTable { len } => {
            k.cube(M::DarkWood, t(0.0, 0.74, 0.0), Vec3::new(len, 0.05, 1.2), WHITE);
            k.cube(M::Metal, t(-len * 0.3, 0.36, 0.0), Vec3::new(0.12, 0.72, 0.8), rgb(0.3, 0.3, 0.32));
            k.cube(M::Metal, t(len * 0.3, 0.36, 0.0), Vec3::new(0.12, 0.72, 0.8), rgb(0.3, 0.3, 0.32));
            let per = ((len / 0.9) as u32).max(1);
            for i in 0..per {
                let x = (i as f32 + 0.5) / per as f32 * len - len * 0.5;
                for (z, yaw) in [(-0.85f32, std::f32::consts::PI), (0.85, 0.0)] {
                    let mut sub = Kit::default();
                    build(Item::OfficeChair, ty(x, 0.0, z, yaw), seed ^ i as u64, &mut sub, &mut s);
                    k.merge(sub);
                }
            }
        }
        Item::CoffeeMaker => {
            k.cube(M::Plastic, t(0.0, 0.18, -0.15), Vec3::new(0.22, 0.36, 0.25), rgb(0.08, 0.08, 0.09));
            k.cyl(M::Glass, t(0.0, 0.0, -0.08), 0.07, 0.15, 12, WHITE, true);
        }
        Item::WaterCooler => {
            k.cube(M::WhitePlastic, t(0.0, 0.5, -0.17), Vec3::new(0.32, 1.0, 0.32), WHITE);
            k.cyl(M::Glass, t(0.0, 1.0, -0.17), 0.14, 0.45, 14, rgb(0.6, 0.75, 0.95), true);
        }
        Item::VendingMachine => {
            k.cube(M::Metal, t(0.0, 0.92, -0.42), Vec3::new(0.95, 1.84, 0.85), rgb(0.2, 0.2, 0.22));
            k.cube(M::Packaging, t(-0.1, 1.05, -0.84), Vec3::new(0.6, 1.3, 0.02), WHITE);
            k.cube(M::Glass, t(-0.1, 1.05, -0.86), Vec3::new(0.62, 1.32, 0.01), WHITE);
            k.cube(M::LightPanel, t(-0.1, 1.73, -0.85), Vec3::new(0.6, 0.04, 0.02), WHITE);
        }
        Item::Gondola { len } => {
            // Double-sided store shelving with products on four shelves each side.
            k.cube(M::Metal, t(0.0, 0.9, 0.0), Vec3::new(len, 1.8, 0.06), rgb(0.85, 0.86, 0.86));
            k.cube(M::Metal, t(0.0, 0.07, 0.0), Vec3::new(len, 0.14, 1.0), rgb(0.85, 0.86, 0.86));
            for side in [-1.0f32, 1.0] {
                for r in 0..4 {
                    let y = 0.2 + r as f32 * 0.42;
                    k.cube(M::Metal, t(0.0, y, side * 0.25), Vec3::new(len, 0.02, 0.45), rgb(0.85, 0.86, 0.86));
                    // Products: a packaging strip, varying depth.
                    k.quad(
                        M::Packaging,
                        if side > 0.0 {
                            [
                                Vec3::new(-len * 0.5, y + 0.01, side * 0.46),
                                Vec3::new(len * 0.5, y + 0.01, side * 0.46),
                                Vec3::new(len * 0.5, y + 0.32, side * 0.46),
                                Vec3::new(-len * 0.5, y + 0.32, side * 0.46),
                            ]
                        } else {
                            [
                                Vec3::new(len * 0.5, y + 0.01, side * 0.46),
                                Vec3::new(-len * 0.5, y + 0.01, side * 0.46),
                                Vec3::new(-len * 0.5, y + 0.32, side * 0.46),
                                Vec3::new(len * 0.5, y + 0.32, side * 0.46),
                            ]
                        },
                        WHITE,
                    );
                    k.cube(M::Packaging, t(0.0, y + 0.165, side * 0.26), Vec3::new(len - 0.02, 0.31, 0.38), rgb(0.7, 0.7, 0.7));
                }
            }
        }
        Item::Checkout => {
            k.cube(M::Metal, t(0.0, 0.45, 0.0), Vec3::new(2.4, 0.9, 0.7), rgb(0.6, 0.6, 0.62));
            k.cube(M::Rubber, t(-0.3, 0.91, 0.0), Vec3::new(1.6, 0.02, 0.5), rgb(0.1, 0.1, 0.1));
            k.cube(M::Glass, t(0.65, 0.92, 0.0), Vec3::new(0.35, 0.02, 0.35), rgb(0.5, 0.2, 0.2));
            // Register: screen on a pole, card reader, cash drawer.
            k.cyl(M::Metal, t(0.95, 0.9, 0.25), 0.02, 0.35, 8, WHITE, false);
            k.cube(M::Plastic, ty(0.95, 1.3, 0.25, 0.3), Vec3::new(0.35, 0.25, 0.04), rgb(0.1, 0.1, 0.1));
            s.push(Spec::Screen { centre: Vec3::new(0.95, 1.3, 0.228), yaw: 0.3, w: 0.32, h: 0.22 });
            k.cube(M::Plastic, t(0.95, 0.96, 0.3), Vec3::new(0.4, 0.12, 0.4), rgb(0.15, 0.15, 0.15));
            k.cube(M::Plastic, t(0.6, 1.0, -0.3), Vec3::new(0.1, 0.18, 0.06), rgb(0.12, 0.12, 0.12));
            k.cyl(M::Metal, t(1.4, 0.0, 0.0), 0.04, 1.4, 8, WHITE, false);
            k.cube(M::LightPanel, t(1.4, 1.5, 0.0), Vec3::new(0.12, 0.2, 0.12), WHITE);
        }
        Item::ProduceBin => {
            k.cube(M::Wood, t(0.0, 0.4, 0.0), Vec3::new(1.4, 0.8, 1.0), WHITE);
            k.frustum(M::Wood, t(0.0, 0.8, 0.0), 0.75, 0.6, 0.15, 4, WHITE, false);
            for i in 0..18 {
                let r = |s: u64| super::model::hash01(seed ^ (i * 7 + s));
                let c = pick(
                    &[rgb(0.8, 0.1, 0.08), rgb(0.95, 0.6, 0.1), rgb(0.2, 0.55, 0.15), rgb(0.9, 0.85, 0.2), rgb(0.4, 0.1, 0.35)],
                    seed ^ (i / 6),
                );
                k.ball(M::Plastic, t((r(1) - 0.5) * 1.1, 0.88 + r(2) * 0.06, (r(3) - 0.5) * 0.7), Vec3::splat(0.1 + r(4) * 0.05), c);
            }
        }
        Item::FridgeCase { len } => {
            k.cube(M::Metal, t(0.0, 1.0, -0.4), Vec3::new(len, 2.0, 0.8), rgb(0.2, 0.2, 0.22));
            for r in 0..5 {
                let y = 0.3 + r as f32 * 0.35;
                k.cube(M::Packaging, t(0.0, y + 0.13, -0.45), Vec3::new(len - 0.1, 0.26, 0.6), WHITE);
            }
            k.cube(M::Glass, t(0.0, 1.05, -0.81), Vec3::new(len, 1.8, 0.02), WHITE);
            let doors = (len / 0.75).floor().max(1.0) as u32;
            for i in 0..=doors {
                k.cube(
                    M::Metal,
                    t(-len * 0.5 + i as f32 * len / doors as f32, 1.05, -0.82),
                    Vec3::new(0.04, 1.8, 0.03),
                    rgb(0.2, 0.2, 0.22),
                );
            }
            k.cube(M::LightPanel, t(0.0, 1.98, -0.75), Vec3::new(len - 0.1, 0.03, 0.05), WHITE);
        }
        Item::Counter { len } => {
            k.cube(M::Wood, t(0.0, 0.5, 0.0), Vec3::new(len, 1.0, 0.7), rgb(0.85, 0.8, 0.72));
            k.cube(M::Marble, t(0.0, 1.02, 0.0), Vec3::new(len + 0.05, 0.04, 0.75), rgb(0.3, 0.3, 0.3));
            k.cube(M::Plastic, ty(len * 0.3, 1.2, 0.0, 0.3), Vec3::new(0.35, 0.25, 0.04), rgb(0.1, 0.1, 0.1));
            s.push(Spec::Screen { centre: Vec3::new(len * 0.3, 1.2, -0.022), yaw: 0.3, w: 0.32, h: 0.22 });
        }
        Item::Booth => {
            let c = fabric_colour(seed ^ 0xb0);
            for (z, yaw) in [(-0.75f32, 0.0f32), (0.75, std::f32::consts::PI)] {
                let b = ty(0.0, 0.0, z, yaw);
                k.cube(M::Leather, b * t(0.0, 0.25, 0.0), Vec3::new(1.4, 0.5, 0.5), c);
                k.cube(M::Leather, b * t(0.0, 0.8, 0.25), Vec3::new(1.4, 0.7, 0.12), c);
                s.push(Spec::Seat {
                    at: b.transform_point3(Vec3::new(-0.3, 0.52, -0.05)),
                    yaw,
                    half: Vec3::new(0.7, 0.4, 0.3),
                    centre: b.transform_point3(Vec3::new(0.0, 0.4, 0.0)),
                    lie: false,
                });
            }
            k.cube(M::Marble, t(0.0, 0.74, 0.0), Vec3::new(1.2, 0.04, 0.75), rgb(0.85, 0.85, 0.82));
            k.cyl(M::Metal, t(0.0, 0.0, 0.0), 0.05, 0.72, 10, WHITE, false);
        }
        Item::CafeTable => {
            k.cyl(M::Wood, t(0.0, 0.73, 0.0), 0.35, 0.03, 18, WHITE, true);
            k.cyl(M::Metal, t(0.0, 0.0, 0.0), 0.03, 0.73, 8, rgb(0.15, 0.15, 0.15), false);
            k.cyl(M::Metal, t(0.0, 0.0, 0.0), 0.22, 0.02, 12, rgb(0.15, 0.15, 0.15), true);
            for a in [0.0f32, std::f32::consts::PI] {
                let mut sub = Kit::default();
                build(Item::DiningChair, ty(a.sin() * 0.55, 0.0, a.cos() * 0.55, a), seed ^ 31, &mut sub, &mut s);
                k.merge(sub);
            }
        }
        Item::BarStool => {
            k.cyl(M::Leather, t(0.0, 0.72, 0.0), 0.18, 0.06, 14, rgb(0.15, 0.1, 0.08), true);
            k.cyl(M::Chrome, t(0.0, 0.02, 0.0), 0.025, 0.72, 8, WHITE, false);
            k.cyl(M::Chrome, t(0.0, 0.0, 0.0), 0.2, 0.02, 14, WHITE, true);
            k.cyl(M::Chrome, t(0.0, 0.28, 0.0), 0.15, 0.015, 14, WHITE, false);
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.76, 0.0),
                yaw: 0.0,
                half: Vec3::new(0.2, 0.4, 0.2),
                centre: Vec3::new(0.0, 0.5, 0.0),
                lie: false,
            });
        }
        Item::RestaurantTable { cloth } => {
            if cloth {
                k.cube(M::Fabric, t(0.0, 0.5, 0.0), Vec3::new(1.0, 0.52, 1.0), rgb(0.95, 0.95, 0.93));
                k.cube(M::Fabric, t(0.0, 0.76, 0.0), Vec3::new(1.04, 0.02, 1.04), rgb(0.97, 0.97, 0.95));
                k.cyl(M::Glass, t(0.0, 0.77, 0.0), 0.03, 0.15, 10, WHITE, true);
                for (x, z) in [(-0.25f32, 0.0f32), (0.25, 0.0)] {
                    k.cyl(M::Ceramic, t(x, 0.77, z), 0.13, 0.015, 16, WHITE, true);
                }
            } else {
                k.cube(M::Wood, t(0.0, 0.74, 0.0), Vec3::new(0.9, 0.04, 0.9), WHITE);
                k.cyl(M::Metal, t(0.0, 0.0, 0.0), 0.04, 0.72, 8, rgb(0.15, 0.15, 0.15), false);
            }
            for i in 0..4 {
                let a = i as f32 * std::f32::consts::FRAC_PI_2;
                let mut sub = Kit::default();
                build(Item::DiningChair, ty(a.sin() * 0.68, 0.0, a.cos() * 0.68, a), seed ^ 41, &mut sub, &mut s);
                k.merge(sub);
            }
        }
        Item::EspressoMachine => {
            k.cube(M::Chrome, t(0.0, 0.25, -0.25), Vec3::new(0.7, 0.45, 0.5), WHITE);
            k.cube(M::Metal, t(0.0, 0.5, -0.25), Vec3::new(0.7, 0.04, 0.5), rgb(0.2, 0.2, 0.2));
            for x in [-0.2f32, 0.0, 0.2] {
                k.cyl(M::Chrome, t(x, 0.18, -0.52), 0.035, 0.05, 10, WHITE, true);
                k.cube(M::Plastic, t(x, 0.15, -0.58), Vec3::new(0.03, 0.03, 0.12), rgb(0.1, 0.1, 0.1));
            }
            k.cube(M::Metal, t(0.6, 0.18, -0.25), Vec3::new(0.25, 0.36, 0.35), rgb(0.1, 0.1, 0.1));
        }
        Item::Fryer => {
            k.cube(M::Metal, t(0.0, 0.45, -0.4), Vec3::new(0.6, 0.9, 0.8), rgb(0.7, 0.72, 0.74));
            k.cube(M::Plastic, t(0.0, 0.88, -0.4), Vec3::new(0.5, 0.02, 0.5), rgb(0.5, 0.35, 0.1));
        }
        Item::Grill => {
            k.cube(M::Metal, t(0.0, 0.45, -0.4), Vec3::new(1.0, 0.9, 0.8), rgb(0.7, 0.72, 0.74));
            k.cube(M::Metal, t(0.0, 0.91, -0.4), Vec3::new(0.95, 0.02, 0.75), rgb(0.15, 0.15, 0.15));
            k.cube(M::Metal, t(0.0, 1.15, -0.78), Vec3::new(1.0, 0.5, 0.04), rgb(0.7, 0.72, 0.74));
        }
        Item::PrepTable => {
            k.cube(M::Metal, t(0.0, 0.9, -0.38), Vec3::new(1.5, 0.04, 0.76), WHITE);
            k.cube(M::Metal, t(0.0, 0.25, -0.38), Vec3::new(1.45, 0.02, 0.7), WHITE);
            for (x, z) in [(-0.72f32, -0.03f32), (0.72, -0.03), (-0.72, -0.73), (0.72, -0.73)] {
                k.cyl(M::Metal, t(x, 0.0, z), 0.02, 0.9, 8, WHITE, false);
            }
        }
        Item::Machine => {
            // A generic industrial machine: a base, a housing with a window, a control box.
            let c = pick(&[rgb(0.3, 0.45, 0.35), rgb(0.85, 0.85, 0.82), rgb(0.25, 0.32, 0.45), rgb(0.85, 0.55, 0.15)], seed);
            k.cube(M::Metal, t(0.0, 0.2, 0.0), Vec3::new(2.4, 0.4, 1.6), rgb(0.3, 0.3, 0.32));
            k.cube(M::Metal, t(0.0, 1.2, 0.0), Vec3::new(2.2, 1.6, 1.5), c);
            k.cube(M::Glass, t(0.0, 1.3, -0.76), Vec3::new(1.2, 0.7, 0.01), WHITE);
            k.cube(M::Metal, t(1.5, 1.2, -0.3), Vec3::new(0.5, 0.9, 0.3), rgb(0.85, 0.85, 0.82));
            k.cube(M::Screen, t(1.5, 1.35, -0.46), Vec3::new(0.3, 0.2, 0.01), WHITE);
            k.cyl(M::Metal, t(-0.6, 2.0, 0.3), 0.15, 1.2, 12, rgb(0.6, 0.6, 0.6), false);
        }
        Item::PalletRack { len } => {
            let bays = (len / 2.7).floor().max(1.0) as u32;
            let orange = rgb(0.9, 0.45, 0.1);
            let blue = rgb(0.15, 0.3, 0.6);
            for i in 0..=bays {
                let x = -len * 0.5 + i as f32 * len / bays as f32;
                for z in [-0.5f32, 0.5] {
                    k.cube(M::Metal, t(x, 3.0, z), Vec3::new(0.08, 6.0, 0.08), blue);
                }
            }
            for lv in 0..4 {
                let y = 0.15 + lv as f32 * 1.6;
                for z in [-0.5f32, 0.5] {
                    k.cube(M::Metal, t(0.0, y, z), Vec3::new(len, 0.1, 0.06), orange);
                }
                for i in 0..bays {
                    let x = -len * 0.5 + (i as f32 + 0.5) * len / bays as f32;
                    for j in 0..2 {
                        if super::model::hash01(seed ^ (lv * 100 + i * 2 + j) as u64) < 0.75 {
                            let px = x + (j as f32 - 0.5) * 1.25;
                            k.cube(M::Wood, t(px, y + 0.12, 0.0), Vec3::new(1.2, 0.12, 1.0), rgb(0.7, 0.6, 0.45));
                            k.cube(M::Cardboard, t(px, y + 0.7, 0.0), Vec3::new(1.15, 1.0, 0.95), WHITE);
                        }
                    }
                }
            }
        }
        Item::Locker { n } => {
            for i in 0..n {
                let x = (i as f32 - (n as f32 - 1.0) * 0.5) * 0.31;
                k.cube(M::Metal, t(x, 0.92, -0.23), Vec3::new(0.3, 1.84, 0.46), rgb(0.35, 0.45, 0.6));
                for y in [1.55f32, 1.6, 1.65] {
                    k.cube(M::Plastic, t(x, y, -0.465), Vec3::new(0.15, 0.01, 0.005), rgb(0.1, 0.1, 0.1));
                }
            }
        }
        Item::Pew { len } => {
            k.cube(M::DarkWood, t(0.0, 0.44, 0.0), Vec3::new(len, 0.05, 0.42), WHITE);
            k.cube(M::DarkWood, t(0.0, 0.7, 0.22), Vec3::new(len, 0.55, 0.05), WHITE);
            for x in [-0.5f32, 0.5] {
                k.cube(M::DarkWood, t(x * len, 0.45, 0.05), Vec3::new(0.06, 0.9, 0.55), WHITE);
            }
            let n = (len / 0.6) as u32;
            for i in 0..n {
                let x = (i as f32 + 0.5) / n as f32 * len - len * 0.5;
                s.push(Spec::Seat {
                    at: Vec3::new(x, 0.47, 0.0),
                    yaw: 0.0,
                    half: Vec3::new(0.3, 0.4, 0.25),
                    centre: Vec3::new(x, 0.45, 0.0),
                    lie: false,
                });
            }
        }
        Item::SchoolDesk => {
            k.cube(M::Wood, t(0.0, 0.72, -0.25), Vec3::new(0.65, 0.03, 0.48), rgb(0.85, 0.75, 0.55));
            k.cyl(M::Metal, t(0.0, 0.0, -0.25), 0.025, 0.72, 8, rgb(0.3, 0.3, 0.32), false);
            let mut sub = Kit::default();
            build(Item::DiningChair, ty(0.0, 0.0, 0.25, 0.0), seed, &mut sub, &mut s);
            k.merge(sub);
        }
        Item::HospitalBed => {
            k.cube(M::Metal, t(0.0, 0.5, -1.05), Vec3::new(1.0, 0.2, 2.1), rgb(0.85, 0.85, 0.85));
            k.cube(M::Fabric, t(0.0, 0.65, -1.05), Vec3::new(0.9, 0.15, 2.0), rgb(0.92, 0.94, 0.95));
            k.cube(M::Metal, t(0.0, 0.85, -0.05), Vec3::new(1.0, 0.7, 0.05), rgb(0.85, 0.85, 0.85));
            k.cyl(M::Chrome, t(0.6, 0.0, -0.2), 0.02, 1.9, 8, WHITE, false);
            s.push(Spec::Seat {
                at: Vec3::new(0.0, 0.72, -1.05),
                yaw: 0.0,
                half: Vec3::new(0.5, 0.4, 1.05),
                centre: Vec3::new(0.0, 0.6, -1.05),
                lie: true,
            });
        }
        Item::Plant => {
            k.frustum(
                M::Ceramic,
                t(0.0, 0.0, 0.0),
                0.16,
                0.2,
                0.38,
                14,
                pick(&[rgb(0.75, 0.4, 0.25), rgb(0.95, 0.95, 0.93), rgb(0.15, 0.15, 0.15)], seed),
                true,
            );
            for i in 0..7 {
                let a = i as f32 * 0.9 + super::model::hash01(seed ^ i) * 0.5;
                k.ball(
                    M::Plant,
                    Affine3A::from_rotation_translation(
                        Quat::from_rotation_z(0.4 * a.cos()) * Quat::from_rotation_x(0.4 * a.sin()),
                        Vec3::new(a.cos() * 0.12, 0.65 + (i % 3) as f32 * 0.12, a.sin() * 0.12),
                    ),
                    Vec3::new(0.18, 0.5, 0.18),
                    WHITE,
                );
            }
        }
        Item::Rug { w, d } => {
            let c = fabric_colour(seed ^ 0x52);
            k.floor_quad(
                M::Carpet,
                [
                    Vec3::new(-w * 0.5, 0.008, -d * 0.5),
                    Vec3::new(w * 0.5, 0.008, -d * 0.5),
                    Vec3::new(w * 0.5, 0.008, d * 0.5),
                    Vec3::new(-w * 0.5, 0.008, d * 0.5),
                ],
                c,
                true,
            );
        }
        Item::FloorLamp => {
            k.cyl(M::Metal, t(0.0, 0.0, 0.0), 0.15, 0.02, 14, rgb(0.15, 0.15, 0.15), true);
            k.cyl(M::Metal, t(0.0, 0.02, 0.0), 0.015, 1.5, 8, rgb(0.15, 0.15, 0.15), false);
            k.frustum(M::Fabric, t(0.0, 1.45, 0.0), 0.22, 0.15, 0.28, 14, rgb(0.95, 0.92, 0.85), false);
            s.push(Spec::Light { at: Vec3::new(0.0, 1.55, 0.0), lumens: 800.0 });
        }
        Item::Lamp => {
            k.cyl(M::Ceramic, t(0.0, 0.0, 0.0), 0.08, 0.3, 12, rgb(0.6, 0.65, 0.7), true);
            k.frustum(M::Fabric, t(0.0, 0.28, 0.0), 0.17, 0.12, 0.22, 14, rgb(0.95, 0.92, 0.85), false);
        }
        Item::Painting { w, h } => {
            k.cube(M::DarkWood, t(0.0, 0.0, -0.02), Vec3::new(w + 0.08, h + 0.08, 0.03), WHITE);
            // Abstract canvas: a few colour fields.
            let c1 = fabric_colour(seed ^ 1);
            let c2 = fabric_colour(seed ^ 2);
            k.quad(
                M::Paint,
                [
                    Vec3::new(-w * 0.5, -h * 0.5, -0.036),
                    Vec3::new(w * 0.5, -h * 0.5, -0.036),
                    Vec3::new(w * 0.5, 0.0, -0.036),
                    Vec3::new(-w * 0.5, 0.0, -0.036),
                ],
                c1,
            );
            k.quad(
                M::Paint,
                [
                    Vec3::new(-w * 0.5, 0.0, -0.036),
                    Vec3::new(w * 0.5, 0.0, -0.036),
                    Vec3::new(w * 0.5, h * 0.5, -0.036),
                    Vec3::new(-w * 0.5, h * 0.5, -0.036),
                ],
                c2,
            );
        }
        Item::Outlet => {
            k.cube(M::WhitePlastic, t(0.0, 0.0, -0.004), Vec3::new(0.07, 0.115, 0.008), WHITE);
            for y in [-0.022f32, 0.022] {
                k.cube(M::Plastic, t(0.0, y, -0.009), Vec3::new(0.035, 0.035, 0.002), rgb(0.8, 0.8, 0.78));
            }
        }
        Item::LightSwitch => {
            k.cube(M::WhitePlastic, t(0.0, 0.0, -0.004), Vec3::new(0.075, 0.12, 0.008), WHITE);
            k.cube(M::WhitePlastic, t(0.0, 0.0, -0.011), Vec3::new(0.022, 0.04, 0.008), rgb(0.95, 0.95, 0.93));
        }
        Item::Ethernet => {
            k.cube(M::WhitePlastic, t(0.0, 0.0, -0.004), Vec3::new(0.07, 0.115, 0.008), WHITE);
            k.cube(M::Plastic, t(0.0, 0.0, -0.009), Vec3::new(0.018, 0.015, 0.002), rgb(0.1, 0.25, 0.6));
        }
        Item::Pipe { len, vertical } => {
            let xf = if vertical {
                t(0.0, 0.0, 0.0)
            } else {
                Affine3A::from_rotation_translation(Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2), Vec3::ZERO)
            };
            k.cyl(M::Copper, xf, 0.012, len, 8, WHITE, false);
            let xf2 = xf * t(0.06, 0.0, 0.0);
            k.cyl(M::WhitePlastic, xf2, 0.025, len, 8, rgb(0.95, 0.95, 0.93), false);
            let xf3 = xf * t(0.12, 0.0, 0.02);
            k.cyl(M::Plastic, xf3, 0.006, len, 6, rgb(0.85, 0.75, 0.3), false);
        }
        Item::Shelf { len } => {
            let c = rgb(0.6, 0.62, 0.64);
            for x in [-0.5f32, 0.5] {
                k.cube(M::Metal, t(x * len, 0.9, -0.2), Vec3::new(0.04, 1.8, 0.4), c);
            }
            for r in 0..5 {
                k.cube(M::Metal, t(0.0, 0.05 + r as f32 * 0.43, -0.2), Vec3::new(len, 0.02, 0.4), c);
                if super::model::hash01(seed ^ r) < 0.7 {
                    k.cube(
                        M::Cardboard,
                        t((super::model::hash01(seed ^ (r + 9)) - 0.5) * len * 0.5, 0.2 + r as f32 * 0.43, -0.2),
                        Vec3::new(0.4, 0.28, 0.32),
                        WHITE,
                    );
                }
            }
        }
    }
    // Into the caller's frame.
    for (m, mb) in k.parts {
        let b = kit.mb(m);
        let base = b.pos.len() as u32;
        b.pos.extend(mb.pos.into_iter().map(|p| at.transform_point3(Vec3::from(p)).to_array()));
        b.nrm.extend(mb.nrm.into_iter().map(|n| at.transform_vector3(Vec3::from(n)).normalize_or(Vec3::Y).to_array()));
        b.uv.extend(mb.uv);
        b.col.extend(mb.col);
        b.idx.extend(mb.idx.into_iter().map(|i| i + base));
    }
    for mut sp in s {
        spec_offset(&mut sp, at);
        if let Spec::Hinged { kit: hk, front, .. } = &mut sp {
            // Hinged kits stay in their own frame (relative to the hinge); only rotate.
            let rot = Affine3A::from_quat(Quat::from_mat3a(&at.matrix3));
            *front = rot.transform_vector3(*front);
            let mut out = Kit::default();
            for (m, mb) in std::mem::take(&mut hk.parts) {
                let b = out.mb(m);
                b.pos = mb.pos.into_iter().map(|p| rot.transform_point3(Vec3::from(p)).to_array()).collect();
                b.nrm = mb.nrm.into_iter().map(|n| rot.transform_vector3(Vec3::from(n)).to_array()).collect();
                b.uv = mb.uv;
                b.col = mb.col;
                b.idx = mb.idx;
            }
            *hk = out;
        }
        match &mut sp {
            Spec::Seat { yaw, .. } | Spec::Fixture { yaw, .. } | Spec::Mirror { yaw, .. } | Spec::Screen { yaw, .. } => {
                *yaw += yaw_of(at);
            }
            _ => {}
        }
        specs.push(sp);
    }
}

pub fn yaw_of(a: Affine3A) -> f32 {
    let f = a.transform_vector3(Vec3::NEG_Z);
    (-f.x).atan2(-f.z)
}

/// A desk phone: base with keypad and display, handset in its cradle, cord.
fn phone(k: &mut Kit, at: Affine3A) {
    let mut c = Kit::default();
    let dark = rgb(0.09, 0.09, 0.1);
    c.cube(
        M::Plastic,
        Affine3A::from_rotation_translation(Quat::from_rotation_x(0.25), Vec3::new(0.0, 0.03, 0.0)),
        Vec3::new(0.2, 0.05, 0.19),
        dark,
    );
    c.cube(
        M::Screen,
        Affine3A::from_rotation_translation(Quat::from_rotation_x(0.25), Vec3::new(0.03, 0.058, -0.03)),
        Vec3::new(0.08, 0.005, 0.04),
        WHITE,
    );
    for i in 0..4 {
        for j in 0..3 {
            c.cube(
                M::Plastic,
                Affine3A::from_rotation_translation(
                    Quat::from_rotation_x(0.25),
                    Vec3::new(0.0 + j as f32 * 0.022, 0.06, 0.02 + i as f32 * 0.018),
                ),
                Vec3::new(0.016, 0.006, 0.012),
                rgb(0.3, 0.3, 0.32),
            );
        }
    }
    c.cube(M::Plastic, t(-0.065, 0.075, 0.0), Vec3::new(0.05, 0.04, 0.2), dark);
    c.cube(M::Plastic, t(-0.065, 0.07, -0.085), Vec3::new(0.055, 0.05, 0.05), dark);
    c.cube(M::Plastic, t(-0.065, 0.07, 0.085), Vec3::new(0.055, 0.05, 0.05), dark);
    c.cube(M::Plastic, t(-0.11, 0.01, 0.06), Vec3::new(0.012, 0.012, 0.12), dark);
    for (m, mb) in c.parts {
        let b = k.mb(m);
        let base = b.pos.len() as u32;
        b.pos.extend(mb.pos.into_iter().map(|p| at.transform_point3(Vec3::from(p)).to_array()));
        b.nrm.extend(mb.nrm.into_iter().map(|n| at.transform_vector3(Vec3::from(n)).to_array()));
        b.uv.extend(mb.uv);
        b.col.extend(mb.col);
        b.idx.extend(mb.idx.into_iter().map(|i| i + base));
    }
}

/// A monitor, keyboard and mouse on a desk top (origin at the desk surface).
fn computer(k: &mut Kit, s: &mut Vec<Spec>, at: Affine3A) {
    let mut c = Kit::default();
    c.cube(M::Plastic, t(0.0, 0.01, 0.0), Vec3::new(0.22, 0.02, 0.18), rgb(0.1, 0.1, 0.1));
    c.cube(M::Plastic, t(0.0, 0.15, 0.03), Vec3::new(0.04, 0.26, 0.03), rgb(0.1, 0.1, 0.1));
    c.cube(M::Plastic, t(0.0, 0.33, 0.0), Vec3::new(0.56, 0.34, 0.03), rgb(0.08, 0.08, 0.09));
    c.cube(M::Plastic, t(0.0, 0.01, -0.28), Vec3::new(0.44, 0.02, 0.14), rgb(0.12, 0.12, 0.13));
    c.cube(M::Plastic, t(0.3, 0.015, -0.28), Vec3::new(0.06, 0.03, 0.1), rgb(0.12, 0.12, 0.13));
    // Mini tower beside it.
    c.cube(M::Plastic, t(-0.45, 0.2, 0.0), Vec3::new(0.18, 0.4, 0.4), rgb(0.1, 0.1, 0.11));
    for (m, mb) in c.parts {
        let b = k.mb(m);
        let base = b.pos.len() as u32;
        b.pos.extend(mb.pos.into_iter().map(|p| at.transform_point3(Vec3::from(p)).to_array()));
        b.nrm.extend(mb.nrm.into_iter().map(|n| at.transform_vector3(Vec3::from(n)).to_array()));
        b.uv.extend(mb.uv);
        b.col.extend(mb.col);
        b.idx.extend(mb.idx.into_iter().map(|i| i + base));
    }
    s.push(Spec::Screen { centre: at.transform_point3(Vec3::new(0.0, 0.33, -0.016)), yaw: 0.0, w: 0.52, h: 0.3 });
}
