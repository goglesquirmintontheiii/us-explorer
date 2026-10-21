//! Parked cars: a handful of body styles (sedan, SUV, pickup, hatchback, minivan) in
//! the colours American cars come in, each a single small mesh with vertex colours so
//! a whole lot of them draws in a few batches.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

pub const BODIES: u8 = 5;

/// Paint colours (sRGB) by how common they are on US roads.
const PAINTS: [([f32; 3], f32); 12] = [
    ([0.92, 0.92, 0.9], 0.24),   // white
    ([0.03, 0.03, 0.035], 0.2),  // black
    ([0.36, 0.37, 0.38], 0.17),  // grey
    ([0.68, 0.69, 0.7], 0.12),   // silver
    ([0.06, 0.14, 0.38], 0.08),  // blue
    ([0.55, 0.05, 0.04], 0.08),  // red
    ([0.18, 0.19, 0.2], 0.04),   // charcoal
    ([0.62, 0.56, 0.45], 0.02),  // beige
    ([0.08, 0.2, 0.12], 0.02),   // green
    ([0.35, 0.55, 0.75], 0.01),  // light blue
    ([0.85, 0.45, 0.05], 0.01),  // orange
    ([0.3, 0.12, 0.08], 0.01),   // brown
];

pub const PAINT_COUNT: u8 = PAINTS.len() as u8;

/// A paint picked by popularity from a 0..1 number.
pub fn paint_for(u: f32) -> u8 {
    let mut acc = 0.0;
    for (i, (_, w)) in PAINTS.iter().enumerate() {
        acc += w;
        if u < acc {
            return i as u8;
        }
    }
    0
}

/// A body style picked by how common it is (SUVs and pickups lead in the US).
pub fn body_for(u: f32) -> u8 {
    match u {
        u if u < 0.3 => 1,  // SUV
        u if u < 0.55 => 0, // sedan
        u if u < 0.75 => 2, // pickup
        u if u < 0.9 => 3,  // hatchback
        _ => 4,             // minivan
    }
}

/// Length of a body style (for spacing along a curb).
pub fn length(body: u8) -> f32 {
    [4.75, 4.8, 5.7, 4.25, 5.1][body.min(4) as usize]
}

fn lin(c: [f32; 3]) -> [f32; 4] {
    let f = |x: f32| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

#[derive(Default)]
struct B {
    p: Vec<[f32; 3]>,
    n: Vec<[f32; 3]>,
    c: Vec<[f32; 4]>,
    i: Vec<u32>,
}

impl B {
    fn quad(&mut self, q: [Vec3; 4], col: [f32; 4]) {
        let n = (q[1] - q[0]).cross(q[3] - q[0]).normalize_or_zero();
        let b = self.p.len() as u32;
        for v in q {
            self.p.push(v.to_array());
            self.n.push(n.to_array());
            self.c.push(col);
        }
        self.i.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    /// A six-sided block from its 8 corners: bottom (fl, fr, rr, rl) then top, with
    /// front at -z; `top` colours the upper face and `sides` the rest.
    fn hexa(&mut self, c: [Vec3; 8], sides: [f32; 4], top: [f32; 4]) {
        let [b0, b1, b2, b3, t0, t1, t2, t3] = c;
        self.quad([t0, t3, t2, t1], top);
        self.quad([b0, b1, b2, b3], sides);
        self.quad([b0, t0, t1, b1], sides); // front
        self.quad([b2, t2, t3, b3], sides); // back
        self.quad([b1, t1, t2, b2], sides); // right
        self.quad([b3, t3, t0, b0], sides); // left
    }
    /// Block with a tapered top: bottom z0..z1 and half-width w, top z2..z3 and half-width tw.
    #[allow(clippy::too_many_arguments)]
    fn block(&mut self, y0: f32, y1: f32, z0: f32, z1: f32, w: f32, z2: f32, z3: f32, tw: f32, sides: [f32; 4], top: [f32; 4]) {
        self.hexa(
            [
                Vec3::new(-w, y0, z0),
                Vec3::new(w, y0, z0),
                Vec3::new(w, y0, z1),
                Vec3::new(-w, y0, z1),
                Vec3::new(-tw, y1, z2),
                Vec3::new(tw, y1, z2),
                Vec3::new(tw, y1, z3),
                Vec3::new(-tw, y1, z3),
            ],
            sides,
            top,
        );
    }
    fn wheel(&mut self, x: f32, z: f32, r: f32, w: f32, col: [f32; 4], hub: [f32; 4]) {
        let n = 10;
        let (xa, xb) = (x - w * 0.5, x + w * 0.5);
        let pt = |k: usize, xx: f32| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            Vec3::new(xx, r + a.sin() * r, z + a.cos() * r)
        };
        for k in 0..n {
            self.quad([pt(k, xa), pt(k, xb), pt(k + 1, xb), pt(k + 1, xa)], col);
        }
        // Outer faces: a fan as thin quads.
        let outer = if x > 0.0 { xb } else { xa };
        let c = Vec3::new(outer, r, z);
        for k in 0..n {
            let (a, b) = (pt(k, outer), pt(k + 1, outer));
            let q = if x > 0.0 { [c, b, a, a] } else { [c, a, b, b] };
            self.quad(q, hub);
        }
    }
}

/// The mesh for a body style in a paint, front towards -z, wheels on y = 0.
pub fn mesh(body: u8, paint: u8) -> Mesh {
    let mut b = B::default();
    let p = lin(PAINTS[paint.min(PAINT_COUNT - 1) as usize].0);
    let glass = lin([0.05, 0.07, 0.085]);
    let trim = lin([0.06, 0.06, 0.065]);
    let tyre = lin([0.035, 0.035, 0.035]);
    let hub = lin([0.55, 0.56, 0.58]);
    let lamp = lin([0.92, 0.92, 0.88]);
    let tail = lin([0.6, 0.04, 0.03]);
    let l = length(body);
    let h = l * 0.5;
    // (half width, body bottom, body top, wheel radius, cabin top, cabin bottom z0..z1, top z2..z3)
    let (w, y0, y1, r, yc, cz0, cz1, cz2, cz3) = match body {
        0 => (0.9, 0.3, 0.86, 0.33, 1.45, -0.75, 1.35, -0.15, 0.95),
        1 => (0.96, 0.38, 1.05, 0.37, 1.74, -1.0, 2.2, -0.45, 2.15),
        2 => (0.99, 0.45, 1.1, 0.39, 1.86, -1.05, 0.55, -0.75, 0.45),
        3 => (0.88, 0.3, 0.85, 0.32, 1.48, -0.55, 1.95, -0.05, 1.7),
        _ => (0.98, 0.34, 1.0, 0.35, 1.78, -1.35, 2.45, -0.75, 2.45),
    };
    // Lower body, a little narrower at the shoulders, nose and tail rounded off.
    b.block(y0, y1, -h, h, w, -h + 0.12, h - 0.08, w - 0.06, p, p);
    // Bumpers.
    b.block(y0 - 0.02, y0 + 0.2, -h - 0.04, -h + 0.25, w - 0.02, -h - 0.04, -h + 0.25, w - 0.02, trim, trim);
    b.block(y0 - 0.02, y0 + 0.2, h - 0.25, h + 0.04, w - 0.02, h - 0.25, h + 0.04, w - 0.02, trim, trim);
    // Cabin: glass all round, painted roof.
    b.block(y1, yc - 0.06, cz0, cz1, w - 0.08, cz2, cz3, w - 0.2, glass, glass);
    b.block(yc - 0.06, yc, cz2, cz3, w - 0.2, cz2 + 0.04, cz3 - 0.04, w - 0.23, p, p);
    if body == 2 {
        // Pickup bed: floor, sides and tailgate.
        let (z0, z1) = (0.7, h - 0.05);
        b.block(y1 - 0.25, y1 - 0.2, z0, z1, w - 0.12, z0, z1, w - 0.12, trim, trim);
        for s in [-1.0f32, 1.0] {
            let x0 = s * (w - 0.06);
            let x1 = s * (w - 0.14);
            let (xa, xb) = if s < 0.0 { (x0, x1) } else { (x1, x0) };
            b.hexa(
                [
                    Vec3::new(xa, y1, z0),
                    Vec3::new(xb, y1, z0),
                    Vec3::new(xb, y1, z1),
                    Vec3::new(xa, y1, z1),
                    Vec3::new(xa, y1 + 0.35, z0),
                    Vec3::new(xb, y1 + 0.35, z0),
                    Vec3::new(xb, y1 + 0.35, z1),
                    Vec3::new(xa, y1 + 0.35, z1),
                ],
                p,
                p,
            );
        }
        b.block(y1, y1 + 0.35, z1 - 0.08, z1, w - 0.06, z1 - 0.08, z1, w - 0.06, p, p);
        b.block(y1, y1 + 0.3, z0 - 0.05, z0, w - 0.1, z0 - 0.05, z0, w - 0.1, p, p);
    }
    // Lamps.
    for s in [-1.0f32, 1.0] {
        let x = s * (w - 0.22);
        b.hexa(
            [
                Vec3::new(x - 0.17, y1 - 0.2, -h + 0.1),
                Vec3::new(x + 0.17, y1 - 0.2, -h + 0.1),
                Vec3::new(x + 0.17, y1 - 0.2, -h + 0.16),
                Vec3::new(x - 0.17, y1 - 0.2, -h + 0.16),
                Vec3::new(x - 0.17, y1 - 0.06, -h + 0.1),
                Vec3::new(x + 0.17, y1 - 0.06, -h + 0.1),
                Vec3::new(x + 0.17, y1 - 0.06, -h + 0.16),
                Vec3::new(x - 0.17, y1 - 0.06, -h + 0.16),
            ]
            .map(|v| v - Vec3::Z * 0.11),
            lamp,
            lamp,
        );
        b.hexa(
            [
                Vec3::new(x - 0.15, y1 - 0.24, h - 0.02),
                Vec3::new(x + 0.15, y1 - 0.24, h - 0.02),
                Vec3::new(x + 0.15, y1 - 0.24, h + 0.02),
                Vec3::new(x - 0.15, y1 - 0.24, h + 0.02),
                Vec3::new(x - 0.15, y1 - 0.08, h - 0.02),
                Vec3::new(x + 0.15, y1 - 0.08, h - 0.02),
                Vec3::new(x + 0.15, y1 - 0.08, h + 0.02),
                Vec3::new(x - 0.15, y1 - 0.08, h + 0.02),
            ],
            tail,
            tail,
        );
    }
    // Wheels.
    let axle = h - if body == 2 { 1.0 } else { 0.85 };
    for s in [-1.0f32, 1.0] {
        for z in [-axle + 0.05, axle] {
            b.wheel(s * (w - 0.11), z, r, 0.24, tyre, hub);
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.p)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.n)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, b.c)
        .with_inserted_indices(Indices::U32(b.i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::primitives::MeshAabb;

    #[test]
    fn bodies_and_paints() {
        for body in 0..BODIES {
            let m = mesh(body, 0);
            let n = m.count_vertices();
            assert!(n > 100 && n < 2000, "{body}: {n}");
            let aabb = m.compute_aabb().unwrap();
            let size = aabb.half_extents * 2.0;
            assert!((size.z - length(body)).abs() < 0.3, "{body}: {size:?}");
            assert!(size.y > 1.3 && size.y < 2.0 && size.x < 2.1);
            assert!(aabb.min().y.abs() < 0.02);
        }
        let total: f32 = PAINTS.iter().map(|p| p.1).sum();
        assert!((total - 1.0).abs() < 0.01, "{total}");
        assert_eq!(paint_for(0.0), 0);
        assert_eq!(paint_for(0.999), PAINT_COUNT - 1);
    }
}
