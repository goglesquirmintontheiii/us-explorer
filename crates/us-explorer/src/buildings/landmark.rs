//! Landmark models: drop a glTF (`.glb` or `.gltf`) named by a building's OpenStreetMap
//! id into the landmarks folder (`way123.glb`, `relation456.glb`, or just `123.glb` for a
//! way) and it replaces that building's procedural version, set on the ground at the
//! footprint's centre and scaled to the building's height. Models use glTF axes: +Y
//! up, -Z north.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    usx_core::settings::data_dir().join("landmarks")
}

/// Building ids (relations negative) with a model file, and the file's name.
pub fn scan() -> HashMap<i64, String> {
    let mut out = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir()) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let Some(stem) = lower.strip_suffix(".glb").or_else(|| lower.strip_suffix(".gltf")) else { continue };
        let id = if let Some(n) = stem.strip_prefix("relation").or_else(|| stem.strip_prefix('r')) {
            n.trim_start_matches(['_', '-']).parse::<i64>().ok().map(|x| -x)
        } else {
            stem.strip_prefix("way").or_else(|| stem.strip_prefix('w')).unwrap_or(stem).trim_start_matches(['_', '-']).parse::<i64>().ok()
        };
        if let Some(id) = id {
            out.insert(id, name);
        }
    }
    out
}

/// A placed landmark model waiting to be fitted to its building.
#[derive(Component)]
pub struct Landmark {
    /// Height to scale to, and whether the fit is done.
    pub height: f32,
    pub fitted: bool,
    pub tries: u32,
}

/// Once the model's meshes have loaded, scale it to the building's height and stand
/// it on the ground at the footprint centre.
pub fn fit(
    mut q: Query<(Entity, &mut Landmark, &mut Transform)>,
    children: Query<&Children>,
    parts: Query<(&GlobalTransform, &Aabb)>,
    globals: Query<&GlobalTransform>,
) {
    for (e, mut lm, mut tr) in &mut q {
        if lm.fitted {
            continue;
        }
        lm.tries += 1;
        if lm.tries % 15 != 0 {
            continue;
        }
        let Ok(root_g) = globals.get(e) else { continue };
        let inv = root_g.affine().inverse();
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut any = false;
        for d in children.iter_descendants(e) {
            let Ok((g, aabb)) = parts.get(d) else { continue };
            let (c, h) = (Vec3::from(aabb.center), Vec3::from(aabb.half_extents));
            for k in 0..8 {
                let s = Vec3::new(
                    if k & 1 == 0 { -1.0 } else { 1.0 },
                    if k & 2 == 0 { -1.0 } else { 1.0 },
                    if k & 4 == 0 { -1.0 } else { 1.0 },
                );
                let p = inv.transform_point3(g.transform_point(c + h * s));
                lo = lo.min(p);
                hi = hi.max(p);
                any = true;
            }
        }
        if !any {
            continue;
        }
        // Bounds are in the model's own (unscaled) frame.
        let size = hi - lo;
        let s = if size.y > 0.5 && lm.height > 1.0 { lm.height / size.y } else { 1.0 };
        let centre = (lo + hi) * 0.5;
        tr.scale = Vec3::splat(s);
        tr.translation = Vec3::new(-centre.x * s, -lo.y * s, -centre.z * s);
        lm.fitted = true;
        info!("landmark fitted: {:.1} x {:.1} x {:.1} m model scaled {s:.3}", size.x, size.y, size.z);
    }
}
