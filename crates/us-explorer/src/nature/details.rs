//! Putting the built details (see `features`) on screen: draping each zoom-17 chunk on
//! the ground drawn, one mesh per material, and the moving parts: wind turbines that
//! yaw into the real wind and turn at a speed set by it, and buoys that ride the waves
//! with their lights flashing at night.

use super::conditions::Conditions;
use super::features::{self, FBuilt, FChunk, FH, FMat, ObjKind};
use super::nature_osm::Seamark;
use super::trees::{TreeAssets, VegMaterial};
use crate::globe::{GeoAnchor, Viewer};
use crate::terrain::{HeightSnap, Terrain};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::{DMat3, DQuat, DVec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::sync::Arc;
use usx_core::geo::{EnuFrame, Geodetic};
use usx_core::net::WorkPool;
use usx_core::tile::TileId;

/// Distance details are drawn to, by preset (turbines further: they're huge).
pub fn range(p: usx_core::settings::Preset) -> f64 {
    use usx_core::settings::Preset::*;
    match p {
        Low => 700.0,
        Medium => 1100.0,
        High => 1700.0,
        Ultra => 2600.0,
    }
}

#[derive(Resource)]
pub struct DetailMats {
    pub paint: Handle<StandardMaterial>,
    pub metal: Handle<StandardMaterial>,
    pub chain: Handle<StandardMaterial>,
    pub wood: Handle<StandardMaterial>,
    pub lamp: Handle<StandardMaterial>,
    pub lamp_off: Handle<StandardMaterial>,
    /// Shared turbine parts.
    pub nacelle: Handle<Mesh>,
    pub rotor: Handle<Mesh>,
}

/// A wind turbine's moving parts.
#[derive(Component)]
pub struct Turbine {
    pub head: Entity,
    pub rotor: Entity,
    pub hub: f32,
    pub angle: f32,
    pub yaw: f32,
    /// ENU->parent rotation at the tower.
    pub base_rot: Quat,
    pub base_t: Vec3,
}

/// A floating buoy.
#[derive(Component)]
pub struct Buoy {
    pub base_t: Vec3,
    pub base_rot: Quat,
    pub phase: f32,
    pub light: Option<(Entity, f32, String)>,
}

struct ChunkState {
    root: Option<Entity>,
    sig: u64,
    pending: bool,
    dist: f64,
}

struct FTile {
    built: Arc<FBuilt>,
    chunks: Vec<ChunkState>,
}

struct Draped {
    center: DVec3,
    meshes: Vec<(FMat, Mesh)>,
    /// Objects: index, translation, rotation (ECEF axes relative to centre).
    objs: Vec<(usize, Vec3, Quat)>,
}

enum Job {
    Build(TileId, Arc<super::nature_osm::NatureData>),
    Drape(Arc<FBuilt>, TileId, usize, HeightSnap, u64),
}

enum Out {
    Built(TileId, FBuilt),
    Draped(TileId, usize, u64, Option<Draped>),
}

#[derive(Resource)]
pub struct Details {
    tiles: HashMap<TileId, FTile>,
    building: std::collections::HashSet<TileId>,
    work: WorkPool<Job, Out>,
    pub enabled: bool,
    pub objects: usize,
}

pub struct DetailsPlugin;

impl Plugin for DetailsPlugin {
    fn build(&self, app: &mut App) {
        let work = WorkPool::new("details", 1, |_, job: Job| match job {
            Job::Build(id, nat) => Out::Built(id, features::build(id, &nat)),
            Job::Drape(b, id, ci, snap, sig) => Out::Draped(id, ci, sig, drape(&b.chunks[ci], &snap)),
        });
        let enabled = app.world().resource::<crate::GameSettings>().0.details;
        app.insert_resource(Details { tiles: HashMap::new(), building: Default::default(), work, enabled, objects: 0 })
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (manage, receive, animate_turbines, animate_buoys).chain().after(super::trees::receive).before(crate::globe::GlobeSet),
            );
    }
}

fn tex(images: &mut Assets<Image>, size: usize, f: impl Fn(f32, f32) -> [f32; 4]) -> Handle<Image> {
    let mut c = super::sprites::Canvas::new(size, size);
    for y in 0..size {
        for x in 0..size {
            c.px[y * size + x] = f(x as f32 / size as f32, y as f32 / size as f32);
        }
    }
    let (data, levels) = super::sprites::mips(&c, 0.5);
    let mut img = Image::new_uninit(
        Extent3d { width: size as u32, height: size as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    images.add(img)
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut mats: ResMut<Assets<StandardMaterial>>, mut meshes: ResMut<Assets<Mesh>>) {
    // Chain link: diamond wire mesh, 5 cm openings (texture repeats every 0.6 m).
    let chain_tex = tex(&mut images, 128, |u, v| {
        let n = 12.0;
        let a = ((u + v) * n).fract();
        let b = ((u - v) * n).rem_euclid(1.0);
        let wire = a < 0.12 || b < 0.12;
        if wire { [0.7, 0.72, 0.74, 1.0] } else { [0.5, 0.5, 0.5, 0.0] }
    });
    let wood_tex = tex(&mut images, 128, |u, v| {
        // Vertical boards 15 cm wide with grain.
        let boards = 4.0;
        let fu = (u * boards).fract();
        let grain = ((v * 40.0 + (u * boards).floor() * 3.7).sin() * 0.5 + 0.5) * 0.15;
        let gap = fu < 0.04;
        let l = if gap { 0.35 } else { 0.85 + grain - 0.07 };
        [l, l, l, 1.0]
    });
    let paint = mats.add(StandardMaterial { perceptual_roughness: 0.75, reflectance: 0.35, ..default() });
    let metal = mats.add(StandardMaterial { perceptual_roughness: 0.42, metallic: 0.75, ..default() });
    let chain = mats.add(StandardMaterial {
        base_color_texture: Some(chain_tex),
        alpha_mode: AlphaMode::Mask(0.5),
        cull_mode: None,
        double_sided: true,
        metallic: 0.7,
        perceptual_roughness: 0.4,
        ..default()
    });
    let wood = mats.add(StandardMaterial { base_color_texture: Some(wood_tex), perceptual_roughness: 0.85, cull_mode: None, double_sided: true, ..default() });
    let lamp = mats.add(StandardMaterial { emissive: LinearRgba::rgb(4000.0, 3600.0, 2600.0), base_color: Color::srgb(1.0, 0.95, 0.85), ..default() });
    let lamp_off = mats.add(StandardMaterial { base_color: Color::srgb(0.6, 0.6, 0.6), perceptual_roughness: 0.2, ..default() });
    // Turbine nacelle (a box with a rounded nose) and the three-bladed rotor, both
    // built for an 80 m hub; scaled per turbine.
    let nacelle = meshes.add(Cuboid::new(4.0, 3.6, 11.0).mesh().build());
    let rotor = meshes.add(rotor_mesh());
    commands.insert_resource(DetailMats { paint, metal, chain, wood, lamp, lamp_off, nacelle, rotor });
}

/// Three tapered, slightly twisted blades round a hub, in the x-y plane (axis +z),
/// for a 100 m rotor.
fn rotor_mesh() -> Mesh {
    let (mut pos, mut nrm, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    let r = 50.0f32;
    for b in 0..3 {
        let a0 = b as f32 / 3.0 * std::f32::consts::TAU;
        let segs = 8;
        let base = pos.len() as u32;
        for k in 0..=segs {
            let t = k as f32 / segs as f32;
            let rad = 1.5 + (r - 1.5) * t;
            let chord = 3.2 * (1.0 - t * 0.8) * if t < 0.15 { t / 0.15 * 0.6 + 0.4 } else { 1.0 };
            let twist = 0.35 * (1.0 - t);
            let dir = Vec3::new(a0.cos(), a0.sin(), 0.0);
            let side = Vec3::new(-a0.sin(), a0.cos(), 0.0);
            let c = dir * rad;
            let s = (side * twist.cos() + Vec3::Z * twist.sin()) * chord * 0.5;
            pos.push((c - s).to_array());
            pos.push((c + s).to_array());
            let n = s.cross(dir).normalize();
            nrm.push(n.to_array());
            nrm.push(n.to_array());
        }
        for k in 0..segs {
            let i = base + k * 2;
            idx.extend_from_slice(&[i, i + 1, i + 3, i, i + 3, i + 2]);
            idx.extend_from_slice(&[i, i + 3, i + 1, i, i + 2, i + 3]);
        }
    }
    // Hub (a short cone).
    let hb = pos.len() as u32;
    let seg = 12;
    for k in 0..=seg {
        let a = k as f32 / seg as f32 * std::f32::consts::TAU;
        pos.push([a.cos() * 1.8, a.sin() * 1.8, 0.0]);
        nrm.push([a.cos(), a.sin(), 0.3]);
    }
    pos.push([0.0, 0.0, 3.5]);
    nrm.push([0.0, 0.0, 1.0]);
    let tip = pos.len() as u32 - 1;
    for k in 0..seg {
        idx.extend_from_slice(&[hb + k, hb + k + 1, tip]);
    }
    let n = pos.len();
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.93f32, 0.93, 0.92, 1.0]; n]);
    m.insert_indices(Indices::U32(idx));
    m
}

/// A buoy: body by shape with colour bands, a topmark and a lamp. Built per buoy (they
/// are few); base at the waterline, y up.
fn buoy_mesh(s: &Seamark) -> (Mesh, Option<f32>) {
    let mut g = features::FGeom::default();
    let z = Vec2::ZERO;
    let cols: Vec<[f32; 4]> = s.colours.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect();
    let col = |i: usize| cols.get(i.min(cols.len().saturating_sub(1))).copied().unwrap_or([0.75, 0.08, 0.06, 1.0]);
    let bands = cols.len().max(1);
    // Height of the body above the water; the lamp sits on top.
    let (h, lamp_at): (f32, f32) = match s.shape.as_str() {
        "can" | "cylinder" => {
            for b in 0..bands {
                let (y0, y1) = (-0.6 + 2.2 * b as f32 / bands as f32, -0.6 + 2.2 * (b + 1) as f32 / bands as f32);
                g_cyl(&mut g, z, 0.75, 0.75, y0, y1, col(b));
            }
            (1.6, 1.6)
        }
        "conical" | "nun" => {
            for b in 0..bands {
                let (t0, t1) = (b as f32 / bands as f32, (b + 1) as f32 / bands as f32);
                g_cyl(&mut g, z, 0.85 * (1.0 - t0) + 0.05, 0.85 * (1.0 - t1) + 0.05, -0.6 + 2.6 * t0, -0.6 + 2.6 * t1, col(b));
            }
            (2.0, 2.0)
        }
        "sphere" | "spherical" => {
            for b in 0..bands {
                let (t0, t1) = (b as f32 / bands as f32, (b + 1) as f32 / bands as f32);
                let r = |t: f32| (std::f32::consts::PI * t).sin() * 0.7 + 0.05;
                g_cyl(&mut g, z, r(t0), r(t1), -0.4 + 1.5 * t0, -0.4 + 1.5 * t1, col(b));
            }
            (1.1, 1.1)
        }
        "spar" | "stake" | "pole" | "perch" => {
            g_cyl(&mut g, z, 0.25, 0.25, -0.5, 0.0, col(0));
            for b in 0..bands {
                let (y0, y1) = (3.6 * b as f32 / bands as f32, 3.6 * (b + 1) as f32 / bands as f32);
                g_cyl(&mut g, z, 0.18, 0.15, y0, y1, col(b));
            }
            (3.6, 3.6)
        }
        _ => {
            // Pillar / lighted buoy: a float with a lattice-like tower and day board.
            g_cyl(&mut g, z, 1.1, 1.1, -0.6, 0.5, col(bands.saturating_sub(1)));
            for b in 0..bands {
                let (y0, y1) = (0.5 + 3.0 * b as f32 / bands as f32, 0.5 + 3.0 * (b + 1) as f32 / bands as f32);
                g_cyl(&mut g, z, 0.55 * (1.0 - 0.3 * (b as f32 / bands as f32)), 0.55 * (1.0 - 0.3 * ((b + 1) as f32 / bands as f32)), y0, y1, col(b));
            }
            (3.5, 3.8)
        }
    };
    // Topmarks: cones, cylinders, spheres, x.
    if let Some(t) = &s.topmark {
        let c = [0.06, 0.06, 0.06, 1.0];
        let tc = if s.kind.contains("cardinal") { c } else { col(0) };
        match t.as_str() {
            "cone, point up" | "cone point up" => g_cyl(&mut g, z, 0.35, 0.0, h + 0.3, h + 0.9, tc),
            "cone, point down" | "cone point down" => g_cyl(&mut g, z, 0.0, 0.35, h + 0.3, h + 0.9, tc),
            "2 cones up" | "2 cones point up" => {
                g_cyl(&mut g, z, 0.3, 0.0, h + 0.3, h + 0.8, tc);
                g_cyl(&mut g, z, 0.3, 0.0, h + 0.85, h + 1.35, tc);
            }
            "2 cones down" | "2 cones point down" => {
                g_cyl(&mut g, z, 0.0, 0.3, h + 0.3, h + 0.8, tc);
                g_cyl(&mut g, z, 0.0, 0.3, h + 0.85, h + 1.35, tc);
            }
            "sphere" => g_cyl(&mut g, z, 0.25, 0.25, h + 0.3, h + 0.8, tc),
            "2 spheres" => {
                g_cyl(&mut g, z, 0.22, 0.22, h + 0.3, h + 0.7, tc);
                g_cyl(&mut g, z, 0.22, 0.22, h + 0.8, h + 1.2, tc);
            }
            _ => g_cyl(&mut g, z, 0.3, 0.3, h + 0.3, h + 0.7, tc),
        }
    }
    let lamp = s.light.as_ref().map(|_| lamp_at + 0.25);
    let n = g.v.len();
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, g.v.iter().map(|v| [v.p.x, v.dy, v.p.y]).collect::<Vec<_>>());
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, g.v.iter().map(|v| v.n.to_array()).collect::<Vec<_>>());
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, g.v.iter().map(|v| v.col).collect::<Vec<_>>());
    let _ = n;
    m.insert_indices(Indices::U32(g.i));
    (m, lamp)
}

fn g_cyl(g: &mut features::FGeom, c: Vec2, r0: f32, r1: f32, y0: f32, y1: f32, col: [f32; 4]) {
    // Through the public builder: a vertical cylinder at the origin.
    let seg = 14u32;
    let b = g.v.len() as u32;
    for k in 0..=seg {
        let a = k as f32 / seg as f32 * std::f32::consts::TAU;
        let d = Vec2::new(a.cos(), a.sin());
        let n = Vec3::new(d.x, (r0 - r1) / (y1 - y0).max(0.01), d.y).normalize();
        g.v.push(features::FV { p: c + d * r0, h: FH::Water, dy: y0, n, uv: Vec2::ZERO, col });
        g.v.push(features::FV { p: c + d * r1, h: FH::Water, dy: y1, n, uv: Vec2::ZERO, col });
    }
    for k in 0..seg {
        let i = b + k * 2;
        g.i.extend_from_slice(&[i, i + 3, i + 1, i, i + 2, i + 3]);
    }
    let top = g.v.len() as u32;
    g.v.push(features::FV { p: c, h: FH::Water, dy: y1, n: Vec3::Y, uv: Vec2::ZERO, col });
    for k in 0..seg {
        g.i.extend_from_slice(&[top, b + k * 2 + 3, b + k * 2 + 1]);
    }
}

fn chunk_key(id: TileId, ci: usize) -> u64 {
    ((id.x as u64) << 34) ^ ((id.y as u64) << 8) ^ ci as u64 ^ 0xde7a_0000_0000_0000
}

/// Build tiles' details as their nature data arrives; drape chunks near you.
#[allow(clippy::too_many_arguments)]
fn manage(
    mut d: ResMut<Details>,
    nat: Res<super::Nature>,
    terrain: Res<Terrain>,
    viewer: Res<Viewer>,
    settings: Res<crate::GameSettings>,
    mut commands: Commands,
    time: Res<Time>,
    mut timer: Local<f32>,
) {
    d.enabled = settings.0.details;
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.3;
    let r = range(settings.0.preset);
    // New tiles.
    let ready: Vec<(TileId, Arc<super::nature_osm::NatureData>)> = nat.ready_tiles().map(|(k, v)| (*k, v.clone())).collect();
    let keep: std::collections::HashSet<TileId> = ready.iter().map(|x| x.0).collect();
    if d.enabled {
        for (id, data) in ready {
            if d.tiles.contains_key(&id) || d.building.contains(&id) {
                continue;
            }
            d.building.insert(id);
            d.work.submit(chunk_key(id, 9999), 0.0, Job::Build(id, data));
        }
    }
    // Drop tiles whose nature data went (or everything when switched off).
    let gone: Vec<TileId> = d.tiles.keys().filter(|k| !keep.contains(k) || !d.enabled).copied().collect();
    for k in gone {
        if let Some(t) = d.tiles.remove(&k) {
            for c in t.chunks {
                if let Some(e) = c.root {
                    commands.entity(e).despawn();
                }
            }
        }
    }
    // Drapes.
    let mut submitted = 0;
    let ids: Vec<TileId> = d.tiles.keys().copied().collect();
    for id in ids {
        let built = d.tiles[&id].built.clone();
        for ci in 0..built.chunks.len() {
            let ch = &built.chunks[ci];
            let Some(cid) = ch.id else { continue };
            let dist = super::tile_distance(viewer.ecef, cid);
            let has_turbine = ch.objs.iter().any(|o| matches!(o.kind, ObjKind::Turbine { .. }));
            let reach = if has_turbine { r * 3.0 } else { r };
            let st = &mut d.tiles.get_mut(&id).unwrap().chunks[ci];
            st.dist = dist;
            if dist > reach + 200.0 {
                if let Some(e) = st.root.take() {
                    commands.entity(e).despawn();
                    st.sig = 0;
                }
                continue;
            }
            if dist > reach || st.pending || submitted >= 4 {
                continue;
            }
            let bbox = ch.bbox;
            let pad = 0.0004;
            let bb = (bbox.0 - pad, bbox.1 - pad, bbox.2 + pad, bbox.3 + pad);
            let sig = terrain.signature(bb);
            if sig == st.sig || sig == 0xcbf2_9ce4_8422_2325 {
                continue;
            }
            let snap = terrain.snapshot(bb);
            if snap.is_empty() {
                continue;
            }
            st.pending = true;
            d.work.submit(chunk_key(id, ci), dist as f32, Job::Drape(built.clone(), id, ci, snap, sig));
            submitted += 1;
        }
    }
}

/// Lay a chunk on the ground (worker thread).
fn drape(ch: &FChunk, snap: &HeightSnap) -> Option<Draped> {
    let id = ch.id?;
    let (cla, clo) = id.lat_lon_at(0.5, 0.5);
    let base_h = snap.ground(cla, clo).map(|x| x.0).unwrap_or(0.0);
    let center = Geodetic::new(cla, clo, base_h).to_ecef();
    let frame = EnuFrame::at(Geodetic::new(cla, clo, base_h));
    let to_ecef: DMat3 = frame.ecef_to_local.transpose();
    let q0 = DQuat::from_mat3(&to_ecef).as_quat();
    let local = crate::roads::build::Local::new(cla, clo);
    // The builder's local frame is the tile's; positions arrive as tile-local metres,
    // so convert through lat/lon.
    let height = |la: f64, lo: f64, h: FH| -> Option<f64> {
        Some(match h {
            FH::Ground => snap.ground(la, lo)?.0,
            FH::GroundAt(_) => snap.ground(la, lo)?.0,
            FH::Water => snap.natural(la, lo)?.max(0.0),
            FH::Surface => snap.natural(la, lo)?.max(0.0).max(snap.ground(la, lo)?.0),
            FH::WaterAt(_) => snap.natural(la, lo)?,
        })
    };
    let _ = local;
    let mut meshes = Vec::new();
    for (&m, g) in &ch.geoms {
        if g.i.is_empty() {
            continue;
        }
        let mut pos = Vec::with_capacity(g.v.len());
        let mut nrm = Vec::with_capacity(g.v.len());
        // GroundAt refers to a point in the same tile frame as the vertex.
        for v in &g.v {
            let (la, lo) = ch_ll(ch, v.p);
            let h = match v.h {
                FH::GroundAt(at) => {
                    let (a, b) = ch_ll(ch, at);
                    snap.ground(a, b).map(|x| x.0)
                }
                FH::WaterAt(at) => {
                    let (a, b) = ch_ll(ch, at);
                    snap.natural(a, b)
                }
                other => height(la, lo, other),
            }
            .unwrap_or(base_h)
                + v.dy as f64;
            let e = Geodetic::new(la, lo, h).to_ecef();
            pos.push((e - center).as_vec3().to_array());
            nrm.push((q0 * v.n).to_array());
        }
        let n = pos.len();
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, g.v.iter().map(|v| v.uv.to_array()).collect::<Vec<_>>());
        if m == FMat::Leaves {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.4f32, 0.0]; n]);
        }
        if m == FMat::Water {
            // The road material's water: tint and flags in the colour, current in uv_b.
            let pk = crate::roads::build::pack(crate::roads::build::elem::JUNCTION, 0, crate::roads::build::flag::WATER);
            let tint = [0.03f32, 0.065, 0.06];
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_UV_1,
                g.v.iter().map(|v| [crate::roads::build::encode_flow(Vec2::new(v.col[0], v.col[1])), pk.0]).collect::<Vec<_>>(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[tint[0], tint[1], pk.1, tint[2]]; n]);
        } else {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, g.v.iter().map(|v| v.col).collect::<Vec<_>>());
        }
        mesh.insert_indices(Indices::U32(g.i.clone()));
        meshes.push((m, mesh));
    }
    let mut objs = Vec::new();
    for (i, o) in ch.objs.iter().enumerate() {
        let (la, lo) = ch_ll(ch, o.p);
        let Some(h) = height(la, lo, o.h) else { continue };
        let g = Geodetic::new(la, lo, h);
        let f = EnuFrame::at(g);
        let m: DMat3 = f.ecef_to_local.transpose();
        let r = (DQuat::from_mat3(&m) * DQuat::from_rotation_y(-o.yaw as f64)).as_quat();
        objs.push((i, (g.to_ecef() - center).as_vec3(), r));
    }
    Some(Draped { center, meshes, objs })
}

/// Chunk geometry is in its tile's local frame; that frame's origin is the tile centre.
fn ch_ll(ch: &FChunk, p: Vec2) -> (f64, f64) {
    let parent = ch.tile.unwrap_or_else(|| ch.id.unwrap().ancestor(usx_core::osm::ZOOM));
    let (la, lo) = parent.lat_lon_at(0.5, 0.5);
    crate::roads::build::Local::new(la, lo).ll(p)
}

#[allow(clippy::too_many_arguments)]
fn receive(
    mut d: ResMut<Details>,
    mats: Option<Res<DetailMats>>,
    ta: Option<Res<TreeAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut anchors: Query<&mut GeoAnchor>,
    roads: Option<Res<crate::roads::Roads>>,
) {
    let (Some(mats), Some(ta)) = (mats, ta) else { return };
    let road_mat = roads.map(|r| r.material.clone());
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < 0.004 {
        let Some(out) = d.work.try_recv() else { break };
        match out {
            Out::Built(id, b) => {
                d.building.remove(&id);
                let n = b.chunks.len();
                info!("details {}: {} chunks", id.path(), n);
                d.tiles.insert(id, FTile { built: Arc::new(b), chunks: (0..n).map(|_| ChunkState { root: None, sig: 0, pending: false, dist: 1e9 }).collect() });
            }
            Out::Draped(id, ci, sig, dr) => {
                let Some(t) = d.tiles.get_mut(&id) else { continue };
                let built = t.built.clone();
                let Some(st) = t.chunks.get_mut(ci) else { continue };
                st.pending = false;
                let Some(dr) = dr else { continue };
                st.sig = sig;
                if let Some(e) = st.root.take() {
                    commands.entity(e).despawn();
                }
                let root = commands.spawn((GeoAnchor::new(dr.center), Transform::default(), Visibility::Visible)).id();
                let _ = anchors.get_mut(root);
                st.root = Some(root);
                for (m, mesh) in dr.meshes {
                    let h = meshes.add(mesh);
                    let mut e = commands.spawn((Mesh3d(h), Transform::default(), ChildOf(root)));
                    match m {
                        FMat::Paint => e.insert(MeshMaterial3d(mats.paint.clone())),
                        FMat::Metal => e.insert(MeshMaterial3d(mats.metal.clone())),
                        FMat::ChainLink => e.insert((MeshMaterial3d(mats.chain.clone()), bevy::light::NotShadowCaster)),
                        FMat::Wood => e.insert(MeshMaterial3d(mats.wood.clone())),
                        FMat::Lamp => e.insert(MeshMaterial3d(mats.lamp.clone())),
                        FMat::Leaves => e.insert(MeshMaterial3d::<VegMaterial>(ta.leaves.clone())),
                        FMat::Water => match &road_mat {
                            Some(rm) => e.insert((MeshMaterial3d(rm.clone()), bevy::light::NotShadowCaster)),
                            None => e.insert(MeshMaterial3d(mats.paint.clone())),
                        },
                    };
                }
                let ch = &built.chunks[ci];
                for (i, t, r) in dr.objs {
                    let o = &ch.objs[i];
                    match &o.kind {
                        ObjKind::Turbine { hub, rotor } => {
                            let s = hub / 80.0;
                            let head = commands
                                .spawn((Transform { translation: t + r * Vec3::Y * (*hub), rotation: r, scale: Vec3::splat(s) }, Visibility::Inherited, ChildOf(root)))
                                .id();
                            commands.spawn((Mesh3d(mats.nacelle.clone()), MeshMaterial3d(mats.paint.clone()), Transform::from_xyz(0.0, 0.0, -2.0), ChildOf(head)));
                            let rs = rotor / 100.0 / s;
                            let rotor_e = commands
                                .spawn((
                                    Mesh3d(mats.rotor.clone()),
                                    MeshMaterial3d(mats.paint.clone()),
                                    Transform::from_xyz(0.0, 0.0, 4.2).with_scale(Vec3::splat(rs)),
                                    ChildOf(head),
                                ))
                                .id();
                            commands.entity(root).with_child((
                                Turbine { head, rotor: rotor_e, hub: *hub, angle: (o.id as f32 * 0.37) % 6.28, yaw: 0.0, base_rot: r, base_t: t },
                                Transform::default(),
                            ));
                        }
                        ObjKind::Buoy(s) => {
                            let (mesh, lamp) = buoy_mesh(s);
                            let h = meshes.add(mesh);
                            let e = commands.spawn((Mesh3d(h), MeshMaterial3d(mats.paint.clone()), Transform { translation: t, rotation: r, scale: Vec3::ONE }, ChildOf(root))).id();
                            let light = match (lamp, &s.light) {
                                (Some(y), Some((col, period, ch))) => {
                                    let l = commands
                                        .spawn((
                                            Mesh3d(meshes.add(Sphere::new(0.18).mesh().build())),
                                            MeshMaterial3d(mats.lamp_off.clone()),
                                            Transform::from_xyz(0.0, y, 0.0),
                                            bevy::light::NotShadowCaster,
                                            BuoyLamp { col: *col },
                                            ChildOf(e),
                                        ))
                                        .id();
                                    Some((l, *period, ch.clone()))
                                }
                                _ => None,
                            };
                            commands.entity(e).insert(Buoy { base_t: t, base_rot: r, phase: (o.id as f32 * 0.731) % 6.28, light });
                        }
                    }
                }
                d.objects = d.tiles.values().map(|t| t.chunks.iter().filter(|c| c.root.is_some()).count()).sum();
            }
        }
    }
}

/// A buoy's lamp and its colour.
#[derive(Component)]
pub struct BuoyLamp {
    pub col: [f32; 3],
}

/// Turbines turn their heads into the wind (slowly) and spin at a speed set by it:
/// still below 3 m/s, rated speed from 12 m/s, feathered and stopped above 25 m/s.
fn animate_turbines(time: Res<Time>, c: Res<Conditions>, mut q: Query<&mut Turbine>, mut tr: Query<&mut Transform, Without<Turbine>>) {
    let dt = time.delta_secs();
    let v = c.wind_speed;
    let rpm = if !(3.0..=25.0).contains(&v) { 0.0 } else { 14.0 * ((v - 3.0) / 9.0).clamp(0.0, 1.0).sqrt() };
    // Nacelles face where the wind comes from. Local frame: x east, -z north; the
    // rotor is on the nacelle's +z side, so +z points upwind.
    let from = c.wind_from.to_radians();
    let target_yaw = std::f32::consts::PI - from;
    for mut t in &mut q {
        let mut dy = target_yaw - t.yaw;
        while dy > std::f32::consts::PI {
            dy -= std::f32::consts::TAU;
        }
        while dy < -std::f32::consts::PI {
            dy += std::f32::consts::TAU;
        }
        t.yaw += dy.clamp(-0.05 * dt, 0.05 * dt) + if t.yaw == 0.0 { dy } else { 0.0 };
        t.angle = (t.angle + rpm / 60.0 * std::f32::consts::TAU * dt) % std::f32::consts::TAU;
        if let Ok(mut h) = tr.get_mut(t.head) {
            h.rotation = t.base_rot * Quat::from_rotation_y(t.yaw);
            h.translation = t.base_t + t.base_rot * Vec3::Y * t.hub;
        }
        if let Ok(mut r) = tr.get_mut(t.rotor) {
            r.rotation = Quat::from_rotation_z(t.angle);
        }
    }
}

/// Buoys bob and lean with the waves; lit ones flash at night.
fn animate_buoys(
    time: Res<Time>,
    c: Res<Conditions>,
    day: Res<crate::sun::DayNight>,
    mats: Option<Res<DetailMats>>,
    mut q: Query<(&Buoy, &mut Transform)>,
    mut lamps: Query<&mut MeshMaterial3d<StandardMaterial>, With<BuoyLamp>>,
) {
    let Some(mats) = mats else { return };
    let t = time.elapsed_secs();
    let amp = (c.wave_height * 0.35).clamp(0.05, 1.2);
    let period = c.wave_period.clamp(3.0, 16.0);
    for (b, mut tr) in &mut q {
        let ph = t / period * std::f32::consts::TAU + b.phase;
        let heave = ph.sin() * amp;
        let roll = (ph + 0.9).cos() * 0.12 * amp.min(1.0);
        let pitch = (ph * 0.7 + 2.0).sin() * 0.1 * amp.min(1.0);
        tr.translation = b.base_t + b.base_rot * Vec3::Y * heave;
        tr.rotation = b.base_rot * Quat::from_rotation_x(pitch) * Quat::from_rotation_z(roll);
        if let Some((lamp, per, ch)) = &b.light
            && let Ok(mut m) = lamps.get_mut(*lamp)
        {
            // Flashing (Fl), quick (Q), occulting (Oc) or fixed (F) by the light's character.
            let on_dark = day.dark > 0.3;
            let phase = (t + b.phase) % per.max(0.5);
            let lit = on_dark
                && if ch.starts_with("Q") {
                    (t * 1.0).fract() < 0.3
                } else if ch.starts_with("Oc") {
                    phase > per * 0.25
                } else if ch.starts_with('F') && !ch.starts_with("Fl") {
                    true
                } else {
                    phase < 0.5
                };
            let want = if lit { &mats.lamp } else { &mats.lamp_off };
            if m.0 != *want {
                m.0 = want.clone();
            }
        }
    }
}
