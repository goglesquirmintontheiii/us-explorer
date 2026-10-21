//! 2D polygon helpers for footprints (x east, z south, metres): area and orientation,
//! oriented bounding boxes, inset outlines, triangulation, and splitting a footprint
//! into rectangles for roofs and floor plans.

use bevy::math::Vec2;

/// Signed area (positive when counter-clockwise in x/z as seen from above with z south,
/// i.e. the shoelace sign on (x, z)).
pub fn signed_area(r: &[Vec2]) -> f32 {
    let n = r.len();
    let mut a = 0.0;
    for i in 0..n {
        a += r[i].perp_dot(r[(i + 1) % n]);
    }
    a * 0.5
}

pub fn area(r: &[Vec2]) -> f32 {
    signed_area(r).abs()
}

pub fn centroid(r: &[Vec2]) -> Vec2 {
    let n = r.len();
    let (mut c, mut a) = (Vec2::ZERO, 0.0);
    for i in 0..n {
        let (p, q) = (r[i], r[(i + 1) % n]);
        let k = p.perp_dot(q);
        a += k;
        c += (p + q) * k;
    }
    if a.abs() < 1e-6 { r.iter().copied().sum::<Vec2>() / n.max(1) as f32 } else { c / (3.0 * a) }
}

pub fn contains(r: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = r.len();
    if n < 3 {
        return false;
    }
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

/// Distance from a point to a ring's edges.
pub fn dist_to_ring(r: &[Vec2], p: Vec2) -> f32 {
    let n = r.len();
    let mut d = f32::MAX;
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        d = d.min(seg_dist(a, b, p));
    }
    d
}

pub fn seg_dist(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Make a ring counter-clockwise (in the shoelace sense on x/z), drop duplicate and
/// collinear points, and the closing repeat.
pub fn clean(mut r: Vec<Vec2>) -> Vec<Vec2> {
    if r.len() > 1 && r[0].distance(r[r.len() - 1]) < 0.01 {
        r.pop();
    }
    r.dedup_by(|a, b| a.distance(*b) < 0.05);
    // Remove nearly collinear points.
    let mut changed = true;
    while changed && r.len() > 3 {
        changed = false;
        let n = r.len();
        for i in 0..n {
            let (a, b, c) = (r[(i + n - 1) % n], r[i], r[(i + 1) % n]);
            let ab = (b - a).normalize_or_zero();
            let bc = (c - b).normalize_or_zero();
            if ab.perp_dot(bc).abs() < 0.02 && ab.dot(bc) > 0.0 || b.distance(a) < 0.05 {
                r.remove(i);
                changed = true;
                break;
            }
        }
    }
    if signed_area(&r) < 0.0 {
        r.reverse();
    }
    r
}

/// Oriented bounding box: (centre, unit long axis, half extents along (axis, perp)).
/// Tries each edge direction and keeps the smallest box (good for buildings).
pub fn obb(r: &[Vec2]) -> (Vec2, Vec2, Vec2) {
    let n = r.len();
    let mut best = (f32::MAX, Vec2::ZERO, Vec2::X, Vec2::ONE);
    for i in 0..n {
        let d = (r[(i + 1) % n] - r[i]).normalize_or_zero();
        if d == Vec2::ZERO {
            continue;
        }
        let pp = d.perp();
        let (mut a0, mut a1, mut b0, mut b1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &p in r {
            let (a, b) = (p.dot(d), p.dot(pp));
            a0 = a0.min(a);
            a1 = a1.max(a);
            b0 = b0.min(b);
            b1 = b1.max(b);
        }
        let ar = (a1 - a0) * (b1 - b0);
        if ar < best.0 - 1e-3 {
            let c = d * (a0 + a1) * 0.5 + pp * (b0 + b1) * 0.5;
            let (hx, hy) = ((a1 - a0) * 0.5, (b1 - b0) * 0.5);
            // Long axis first.
            best = if hx >= hy { (ar, c, d, Vec2::new(hx, hy)) } else { (ar, c, pp, Vec2::new(hy, hx)) };
        }
    }
    (best.1, best.2, best.3)
}

/// How much of its oriented box a footprint fills (1 = a rectangle).
pub fn rectangularity(r: &[Vec2]) -> f32 {
    let (_, _, h) = obb(r);
    area(r) / (4.0 * h.x * h.y).max(1e-3)
}

/// Inset a counter-clockwise ring by `d` (negative grows it), mitred and clamped.
pub fn inset(r: &[Vec2], d: f32) -> Vec<Vec2> {
    let n = r.len();
    (0..n)
        .map(|i| {
            let (a, b, c) = (r[(i + n - 1) % n], r[i], r[(i + 1) % n]);
            let e0 = (b - a).normalize_or_zero();
            let e1 = (c - b).normalize_or_zero();
            // Inward normals for a counter-clockwise ring (x/z, z south): perp() points left.
            let n0 = e0.perp();
            let n1 = e1.perp();
            let m = (n0 + n1).normalize_or(n0);
            let k = m.dot(n0).max(0.35);
            b + m * (d / k)
        })
        .collect()
}

/// Triangulate a polygon with holes; returns flat points and triangle indices.
pub fn triangulate(outer: &[Vec2], holes: &[Vec<Vec2>]) -> (Vec<Vec2>, Vec<u32>) {
    let mut pts: Vec<Vec2> = outer.to_vec();
    let mut flat: Vec<f64> = outer.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
    let mut hi = Vec::new();
    for h in holes {
        if h.len() < 3 {
            continue;
        }
        hi.push(pts.len());
        pts.extend_from_slice(h);
        flat.extend(h.iter().flat_map(|p| [p.x as f64, p.y as f64]));
    }
    let idx = earcutr::earcut(&flat, &hi, 2).unwrap_or_default();
    (pts, idx.into_iter().map(|i| i as u32).collect())
}

/// Cover a footprint with axis-aligned (in the given frame) rectangles on a grid: the
/// pieces of an L, T or U shaped house. Returns rectangles as (min, max) in the frame
/// (a = along `axis`, b = along its perpendicular), largest first.
pub fn rect_cover(r: &[Vec2], centre: Vec2, axis: Vec2, cell: f32, max_rects: usize) -> Option<Vec<(Vec2, Vec2)>> {
    let perp = axis.perp();
    let local: Vec<Vec2> = r.iter().map(|&p| Vec2::new((p - centre).dot(axis), (p - centre).dot(perp))).collect();
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in &local {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let nx = ((hi.x - lo.x) / cell).ceil() as usize;
    let ny = ((hi.y - lo.y) / cell).ceil() as usize;
    if nx == 0 || ny == 0 || nx * ny > 40_000 {
        return None;
    }
    let mut grid = vec![false; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let c = lo + Vec2::new((i as f32 + 0.5) * cell, (j as f32 + 0.5) * cell);
            grid[j * nx + i] = contains(&local, c);
        }
    }
    let total = grid.iter().filter(|&&g| g).count();
    let mut covered = 0;
    let mut out = Vec::new();
    while covered < total && out.len() < max_rects {
        // Largest all-true rectangle (histogram method).
        let mut best = (0usize, 0usize, 0usize, 0usize, 0usize); // area, i0, j0, i1, j1 (exclusive)
        let mut heights = vec![0usize; nx];
        for j in 0..ny {
            for i in 0..nx {
                heights[i] = if grid[j * nx + i] { heights[i] + 1 } else { 0 };
            }
            for i in 0..nx {
                let h = heights[i];
                if h == 0 {
                    continue;
                }
                let mut minh = h;
                for k in i..nx {
                    minh = minh.min(heights[k]);
                    if minh == 0 {
                        break;
                    }
                    let a = minh * (k - i + 1);
                    if a > best.0 {
                        best = (a, i, j + 1 - minh, k + 1, j + 1);
                    }
                }
            }
        }
        if best.0 == 0 {
            break;
        }
        // Too small to bother with (a bay window, a chimney chase).
        if best.0 * 100 < total * 6 && !out.is_empty() {
            break;
        }
        for j in best.2..best.4 {
            for i in best.1..best.3 {
                if grid[j * nx + i] {
                    grid[j * nx + i] = false;
                    covered += 1;
                }
            }
        }
        out.push((lo + Vec2::new(best.1 as f32, best.2 as f32) * cell, lo + Vec2::new(best.3 as f32, best.4 as f32) * cell));
    }
    // Mostly covered?
    let got: f32 = out.iter().map(|(a, b)| (b.x - a.x) * (b.y - a.y)).sum();
    (got >= area(r) * 0.82).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_shape_splits_into_two() {
        let l = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(12.0, 0.0),
            Vec2::new(12.0, 6.0),
            Vec2::new(6.0, 6.0),
            Vec2::new(6.0, 12.0),
            Vec2::new(0.0, 12.0),
        ];
        let r = rect_cover(&l, Vec2::ZERO, Vec2::X, 0.5, 4).unwrap();
        assert_eq!(r.len(), 2);
        let a: f32 = r.iter().map(|(a, b)| (b.x - a.x) * (b.y - a.y)).sum();
        assert!((a - 108.0).abs() < 1.0, "{a}");
    }

    #[test]
    fn obb_of_rotated_rect() {
        let d = Vec2::from_angle(0.5);
        let p = d.perp();
        let r: Vec<Vec2> = [(-5.0, -2.0), (5.0, -2.0), (5.0, 2.0), (-5.0, 2.0)].iter().map(|&(a, b)| d * a + p * b).collect();
        let (c, ax, h) = obb(&r);
        assert!(c.length() < 1e-3);
        assert!((ax.dot(d).abs() - 1.0).abs() < 1e-3);
        assert!((h.x - 5.0).abs() < 1e-3 && (h.y - 2.0).abs() < 1e-3);
        assert!((rectangularity(&r) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn inset_square() {
        let sq = clean(vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)]);
        let i = inset(&sq, 1.0);
        assert!((area(&i) - 64.0).abs() < 0.5, "{}", area(&i));
    }
}
