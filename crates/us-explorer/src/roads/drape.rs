//! Lay a chunk's flat road geometry onto the ground that is on screen right now.
//! Runs on worker threads; re-run whenever the terrain under the chunk changes.

use super::build::{Built, H, Prop, solve_decks};
use crate::terrain::HeightSnap;
use crate::walkable::Strip;
use bevy::math::{DMat3, DQuat, DVec3, Vec2, Vec3};
use usx_core::geo::{EnuFrame, Geodetic};

pub struct Draped {
    pub center: DVec3,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub color: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Prop placements: (index into the chunk's props, translation, rotation), in ECEF
    /// axes relative to `center`.
    pub props: Vec<(usize, Vec3, bevy::math::Quat)>,
    /// Bridge decks and tunnel floors to walk on.
    pub strips: Vec<Strip>,
}

/// Deck heights for every bridge/tunnel node of the tile, against this snapshot.
fn deck_heights(b: &Built, snap: &HeightSnap) -> Vec<f32> {
    // From the natural ground, as the roads leading onto the decks are.
    let g: Vec<Option<f32>> = (0..b.decks.p.len())
        .map(|i| {
            let (la, lo) = b.local.ll(b.decks.p[i]);
            snap.natural(la, lo).map(|h| h as f32)
        })
        .collect();
    solve_decks(&b.decks, &|i| g[i].unwrap_or(0.0), &|i| b.decks.off[i])
}

pub fn drape(b: &Built, chunk: usize, snap: &HeightSnap) -> Option<Draped> {
    let c = &b.chunks[chunk];
    if snap.is_empty() {
        return None;
    }
    let decks = deck_heights(b, snap);
    let (cla, clo) = c.id.lat_lon_at(0.5, 0.5);
    let base_h = snap.ground(cla, clo).map(|x| x.0).unwrap_or(0.0);
    let center = Geodetic::new(cla, clo, base_h).to_ecef();
    let height = |p: Vec2, h: H| -> Option<f64> {
        let ground = |q: Vec2| {
            let (la, lo) = b.local.ll(q);
            snap.ground(la, lo)
        };
        Some(match h {
            H::Ground { off } => {
                let (g, conformed) = ground(p)?;
                g + if conformed { 0.0 } else { off as f64 }
            }
            H::GroundAt { at, off } | H::Raised { at, off, .. } => {
                let (la, lo) = b.local.ll(at);
                snap.natural(la, lo)? + off as f64
            }
            H::Terrain => ground(p)?.0,
            H::Deck { a, b: bb, t } => {
                let (ha, hb) = (decks[a as usize] as f64, decks[bb as usize] as f64);
                ha + (hb - ha) * t as f64
            }
        })
    };
    let n = c.geom.v.len();
    let mut positions = Vec::with_capacity(n);
    let mut ecef_cache: Vec<DVec3> = Vec::with_capacity(n);
    for v in &c.geom.v {
        let h = height(v.p, v.h).unwrap_or(base_h) + v.dy as f64;
        let (la, lo) = b.local.ll(v.p);
        let e = Geodetic::new(la, lo, h).to_ecef();
        ecef_cache.push(e);
        positions.push((e - center).as_vec3().to_array());
    }
    // Area-weighted normals from the triangles.
    let mut normals = vec![Vec3::ZERO; n];
    for t in c.geom.i.chunks_exact(3) {
        let (a, bb, cc) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let pa = Vec3::from(positions[a]);
        let nn = (Vec3::from(positions[bb]) - pa).cross(Vec3::from(positions[cc]) - pa);
        for k in [a, bb, cc] {
            normals[k] += nn;
        }
    }
    let up = (center / center.length()).as_vec3();
    let normals: Vec<[f32; 3]> = normals.into_iter().map(|v| v.try_normalize().unwrap_or(up).to_array()).collect();

    let mut props = Vec::with_capacity(c.props.len());
    for (i, p) in c.props.iter().enumerate() {
        if let Some((t, r)) = place(b, p, &height, center) {
            props.push((i, t, r));
        }
    }

    // Walkable strips for decks (bridges and tunnels) whose segments lie in this chunk.
    let mut strips = Vec::new();
    let owner = chunk_owner(b, chunk);
    let (w, s, e, nn) = c.id.bounds();
    for lane in &b.lanes {
        if !lane.h.iter().any(|h| matches!(h, H::Deck { .. })) {
            continue;
        }
        let half = lane.profile.half() + if lane.class.vehicle() { 2.0 } else { 0.3 };
        for k in 0..lane.pts.len().saturating_sub(1) {
            let mid = (lane.pts[k] + lane.pts[k + 1]) * 0.5;
            let (la, lo) = b.local.ll(mid);
            if la < s || la >= nn || lo < w || lo >= e {
                continue;
            }
            let (Some(ha), Some(hb)) = (height(lane.pts[k], lane.h[k]), height(lane.pts[k + 1], lane.h[k + 1])) else { continue };
            if !matches!(lane.h[k], H::Deck { .. }) && !matches!(lane.h[k + 1], H::Deck { .. }) {
                continue;
            }
            let lift = lane.class.lift() as f64;
            strips.push(Strip {
                a: b.local.ll(lane.pts[k]),
                b: b.local.ll(lane.pts[k + 1]),
                ha: ha + lift,
                hb: hb + lift,
                half_width_m: half as f64,
                owner,
                tunnel: is_tunnel_lane(b, lane),
            });
        }
    }

    let g = &c.geom;
    Some(Draped {
        center,
        positions,
        normals,
        uv0: g.v.iter().map(|v| v.uv).collect(),
        uv1: g.v.iter().map(|v| v.uv2).collect(),
        color: g.v.iter().map(|v| v.col).collect(),
        indices: g.i.clone(),
        props,
        strips,
    })
}

fn is_tunnel_lane(b: &Built, lane: &super::build::Lane) -> bool {
    lane.h.iter().any(|h| matches!(h, H::Deck { a, .. } if b.decks.tunnel.get(*a as usize).copied().unwrap_or(false)))
}

/// Stable owner id for a chunk's walkable strips.
pub fn chunk_owner(b: &Built, chunk: usize) -> u64 {
    let id = b.chunks[chunk].id;
    0x5_0000_0000_0000 | ((id.z as u64) << 44) | ((id.x as u64) << 22) | id.y as u64
}

fn place(b: &Built, p: &Prop, height: &dyn Fn(Vec2, H) -> Option<f64>, center: DVec3) -> Option<(Vec3, bevy::math::Quat)> {
    let h = height(p.p, p.h)? + p.dy as f64;
    let (la, lo) = b.local.ll(p.p);
    let g = Geodetic::new(la, lo, h);
    let e = g.to_ecef();
    let frame = EnuFrame::at(g);
    // Local (x east, y up, -z north) -> ECEF axes, then turn to face `heading`.
    let to_ecef: DMat3 = frame.ecef_to_local.transpose();
    let r = DQuat::from_mat3(&to_ecef) * DQuat::from_rotation_y(-(p.heading as f64));
    Some(((e - center).as_vec3(), r.as_quat()))
}
