//! Grass around you: 3D blades on the default preset and up, flat tufts below it.
//! A 128 m map centred on you (0.5 m texels) holds where grass grows, its colour and
//! height: from the vegetation cells (imagery and land cover), with roads, paths,
//! buildings, parking and water cut out precisely from the map data. Patches of blades
//! on a 2 m grid follow you; the shader folds away blades where the map says none.

use super::cover::{Cover, GRASS_N, tile_uv};
use super::species::{self, Biome, Season};
use super::trees::TreeAssets;
use super::{Nature, ranges, three_d};
use crate::globe::{GeoAnchor, Viewer, WorldOrigin};
use crate::roads::build::{Corridor, Lake};
use crate::terrain::Terrain;
use bevy::asset::RenderAssetUsages;
use bevy::math::{DMat3, DQuat, DVec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use std::sync::Arc;
use usx_core::geo::{EnuFrame, Geodetic};
use usx_core::tile::TileId;

/// Map: 256 texels of 0.5 m.
const MAP_N: usize = 256;
const TEXEL: f32 = 0.5;
const PATCH: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct GrassParams {
    pub centre: Vec4,
    pub east: Vec4,
    pub south: Vec4,
    pub wind: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct GrassExt {
    #[uniform(100)]
    pub params: GrassParams,
    #[texture(101)]
    pub map: Handle<Image>,
}

impl MaterialExtension for GrassExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://us_explorer/nature/grass.wgsl".into()
    }
}

pub type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassExt>;

/// What the map was built around.
struct MapJob {
    lat: f64,
    lon: f64,
    cells: Vec<Arc<super::cover::CellData>>,
    corridors: Vec<Corridor>,
    areas: Vec<Lake>,
    buildings: Vec<crate::buildings::Footprint>,
    biome: Biome,
    season: Season,
}

#[derive(Resource)]
struct Grass {
    map: Handle<Image>,
    blades: Handle<GrassMaterial>,
    tufts: Handle<GrassMaterial>,
    /// Patch meshes: dense, medium, sparse (blades), and tufts.
    meshes: [Handle<Mesh>; 4],
    /// The map's centre (lat, lon, ECEF) once built.
    centre: Option<(f64, f64, DVec3)>,
    job: Option<crossbeam_channel::Receiver<(f64, f64, Vec<u8>)>>,
    root: Option<Entity>,
    patches: HashMap<(i32, i32), Entity>,
    /// The patch grid's frame: centre ECEF and local->ECEF rotation.
    grid: Option<(DVec3, DQuat, f64, f64)>,
    mode_3d: bool,
}

pub struct GrassPlugin;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "grass.wgsl");
        app.add_plugins(MaterialPlugin::<GrassMaterial>::default())
            .add_systems(Startup, setup)
            .add_systems(Update, (rebuild_map, receive_map, place_patches, update_params).chain().after(super::trees::receive));
    }
}

fn hash(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x27d4_eb2d) ^ b.wrapping_mul(0x85eb_ca6b).wrapping_add(0x9e37_79b9);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

/// A 2 m patch of blades, each built 1 m tall (the shader scales them).
fn blade_patch(blades: usize, width: f32, seed: u32) -> Mesh {
    let (mut pos, mut nrm, mut uv, mut uvb, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..blades {
        let r = |k: u32| hash(i as u32 * 7 + k, seed);
        let root = Vec2::new(r(1) * PATCH - PATCH * 0.5, r(2) * PATCH - PATCH * 0.5);
        let yaw = r(3) * std::f32::consts::TAU;
        let side = Vec3::new(yaw.cos(), 0.0, yaw.sin());
        let face = Vec3::new(-yaw.sin(), 0.0, yaw.cos());
        // A slight curve: the blade leans over along its face.
        let lean = (r(4) - 0.3) * 0.35;
        let w = width * (0.7 + 0.6 * r(5));
        let base = pos.len() as u32;
        let segs = 3;
        for s in 0..=segs {
            let t = s as f32 / segs as f32;
            let half = w * 0.5 * (1.0 - t).powf(0.7);
            let c = Vec3::new(root.x, 0.0, root.y) + Vec3::Y * t + face * lean * t * t;
            for (k, sign) in [(0.0f32, -1.0f32), (1.0, 1.0)] {
                if s == segs && k > 0.5 {
                    break;
                }
                let p = if s == segs { c } else { c + side * half * sign };
                pos.push(p.to_array());
                nrm.push(face.to_array());
                uv.push([k, t]);
                uvb.push([root.x, root.y]);
                col.push([r(6), r(7), r(8), r(9)]);
            }
        }
        // Two vertices per level, one at the tip.
        for s in 0..segs - 1 {
            let a = base + s * 2;
            idx.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        let a = base + (segs - 1) * 2;
        idx.extend_from_slice(&[a, a + 1, a + 2]);
    }
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvb);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Flat tufts (crossed cards with a grass texture) for the lower presets.
fn tuft_patch(n: usize, seed: u32) -> Mesh {
    let (mut pos, mut nrm, mut uv, mut uvb, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..n {
        let r = |k: u32| hash(i as u32 * 11 + k, seed);
        let root = Vec2::new(r(1) * PATCH - PATCH * 0.5, r(2) * PATCH - PATCH * 0.5);
        let yaw = r(3) * std::f32::consts::PI;
        for q in 0..2 {
            let a = yaw + q as f32 * std::f32::consts::FRAC_PI_2;
            let side = Vec3::new(a.cos(), 0.0, a.sin()) * 0.3;
            let face = Vec3::new(-a.sin(), 0.0, a.cos());
            let c = Vec3::new(root.x, 0.0, root.y);
            let b = pos.len() as u32;
            for (p, u, v) in [(c - side, 0.0, 1.0), (c + side, 1.0, 1.0), (c + side + Vec3::Y, 1.0, 0.0), (c - side + Vec3::Y, 0.0, 0.0)] {
                pos.push(p.to_array());
                nrm.push(face.to_array());
                // v runs down the card texture; the shader's height uses uv.y as "up".
                uv.push([u, 1.0 - v]);
                uvb.push([root.x, root.y]);
                col.push([r(6), r(7), r(8), r(9)]);
            }
            idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvb);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<GrassMaterial>>,
    settings: Res<crate::GameSettings>,
) {
    let mut img = Image::new_fill(
        Extent3d { width: MAP_N as u32, height: MAP_N as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.sampler = bevy::image::ImageSampler::nearest();
    let map = images.add(img);
    // Tuft texture.
    let card = super::sprites::grass_card(128, 128);
    let (data, levels) = super::sprites::mips(&card, 0.5);
    let mut cimg = Image::new_uninit(
        Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    cimg.data = Some(data);
    cimg.texture_descriptor.mip_level_count = levels;
    let card_tex = images.add(cimg);
    let blades = mats.add(GrassMaterial {
        base: StandardMaterial {
            perceptual_roughness: 0.85,
            reflectance: 0.25,
            cull_mode: None,
            double_sided: true,
            diffuse_transmission: 0.25,
            ..default()
        },
        extension: GrassExt { params: GrassParams::default(), map: map.clone() },
    });
    let tufts = mats.add(GrassMaterial {
        base: StandardMaterial {
            base_color_texture: Some(card_tex),
            alpha_mode: AlphaMode::Mask(0.5),
            perceptual_roughness: 0.85,
            reflectance: 0.25,
            cull_mode: None,
            double_sided: true,
            ..default()
        },
        extension: GrassExt { params: GrassParams { wind: Vec4::new(0.0, 0.0, 0.0, 1.0), ..default() }, map: map.clone() },
    });
    let meshes = [
        meshes.add(blade_patch(420, 0.012, 1)),
        meshes.add(blade_patch(160, 0.02, 2)),
        meshes.add(blade_patch(60, 0.035, 3)),
        meshes.add(tuft_patch(10, 4)),
    ];
    commands.insert_resource(Grass {
        map,
        blades,
        tufts,
        meshes,
        centre: None,
        job: None,
        root: None,
        patches: HashMap::new(),
        grid: None,
        mode_3d: three_d(settings.0.preset),
    });
}

/// Start a new map when you've moved a quarter of the way to its edge.
fn rebuild_map(mut g: ResMut<Grass>, nat: Res<Nature>, viewer: Res<Viewer>, terrain: Res<Terrain>, ta: Option<Res<TreeAssets>>, settings: Res<crate::GameSettings>) {
    if g.job.is_some() || !nat.enabled {
        return;
    }
    let p = viewer.geo();
    let ground = terrain.height_at(p.lat, p.lon).unwrap_or(0.0);
    if p.h - ground > 150.0 {
        return;
    }
    let half = MAP_N as f64 * TEXEL as f64 * 0.5;
    if let Some((la, lo, _)) = g.centre {
        let dy = (p.lat - la) * 111_000.0;
        let dx = (p.lon - lo) * 111_000.0 * la.to_radians().cos();
        if (dx * dx + dy * dy).sqrt() < half * 0.25 {
            return;
        }
    }
    // Cells and map data around.
    let dlat = half / 111_000.0;
    let dlon = half / (111_000.0 * p.lat.to_radians().cos().max(0.05));
    let mut cells = Vec::new();
    for id in usx_core::tile::tiles_in_box(p.lat - dlat, p.lon - dlon, p.lat + dlat, p.lon + dlon, super::cover::CELL_ZOOM) {
        if let Some(c) = nat.cell(id) {
            cells.push(c.clone());
        }
    }
    let mut corridors = Vec::new();
    let mut areas = Vec::new();
    let mut buildings = Vec::new();
    for id in usx_core::tile::tiles_in_box(p.lat - dlat, p.lon - dlon, p.lat + dlat, p.lon + dlon, usx_core::osm::ZOOM) {
        if let Some(c) = crate::roads::corridors_for(id) {
            corridors.extend(c.0.iter().cloned());
            areas.extend(c.1.iter().cloned());
        }
        if let Some(b) = crate::buildings::footprints_for(id) {
            buildings.extend(b.iter().cloned());
        }
    }
    if cells.is_empty() && settings.0.vegetation {
        return;
    }
    let elev = crate::terrain::load::dem_height(p.lat, p.lon).unwrap_or(ground);
    let biome = species::biome_at(p.lat, p.lon, elev);
    let season = ta.map(|t| t.season).unwrap_or_default();
    let job = MapJob { lat: p.lat, lon: p.lon, cells, corridors, areas, buildings, biome, season };
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let (la, lo) = (job.lat, job.lon);
        let _ = tx.send((la, lo, build_map(&job)));
    });
    g.job = Some(rx);
}

/// Height class (0..15) for a grass height in metres.
fn hclass(h: f32) -> u8 {
    ((h / 0.06).max(1.0).ln() / 1.26f32.ln()).round().clamp(0.0, 15.0) as u8
}

/// The grass map (worker thread): RGBA, rgb = colour, a = density * 16 + height class.
fn build_map(j: &MapJob) -> Vec<u8> {
    let n = MAP_N;
    let local = crate::roads::build::Local::new(j.lat, j.lon);
    let half = n as f32 * TEXEL * 0.5;
    let at = |i: usize, k: usize| Vec2::new((i as f32 + 0.5) * TEXEL - half, (k as f32 + 0.5) * TEXEL - half);
    // Masks at full resolution: roads (with sidewalks), buildings, lots, water.
    let mut blocked = vec![false; n * n];
    let to_px = |la: f64, lo: f64| {
        let p = local.xz(la, lo);
        Vec2::new((p.x + half) / TEXEL, (p.y + half) / TEXEL)
    };
    for c in &j.corridors {
        let pts: Vec<Vec2> = c.pts.iter().map(|&(a, b)| to_px(a, b)).collect();
        let hw = (c.half + 0.2) / TEXEL;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let lo = a.min(b) - Vec2::splat(hw + 1.0);
            let hi = a.max(b) + Vec2::splat(hw + 1.0);
            if hi.x < 0.0 || hi.y < 0.0 || lo.x > n as f32 || lo.y > n as f32 {
                continue;
            }
            let ab = b - a;
            let l2 = ab.length_squared().max(1e-6);
            for y in (lo.y.max(0.0) as usize)..(hi.y.min(n as f32) as usize) {
                for x in (lo.x.max(0.0) as usize)..(hi.x.min(n as f32) as usize) {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                    if p.distance(a + ab * t) <= hw {
                        blocked[y * n + x] = true;
                    }
                }
            }
        }
    }
    let mut fill = |rings: Vec<Vec<Vec2>>| {
        let (mut y0, mut y1) = (f32::MAX, f32::MIN);
        for p in rings.iter().flatten() {
            y0 = y0.min(p.y);
            y1 = y1.max(p.y);
        }
        if y1 < 0.0 || y0 > n as f32 {
            return;
        }
        let mut xs = Vec::new();
        for y in (y0.floor().max(0.0) as usize)..=(y1.ceil().min(n as f32 - 1.0).max(0.0) as usize) {
            let yc = y as f32 + 0.5;
            xs.clear();
            for r in &rings {
                for i in 0..r.len() {
                    let (a, b) = (r[i], r[(i + 1) % r.len()]);
                    if (a.y > yc) != (b.y > yc) {
                        xs.push(a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x));
                    }
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pr in xs.chunks_exact(2) {
                let xa = (pr[0] - 0.5).ceil().max(0.0) as i64;
                let xb = ((pr[1] - 0.5).floor() as i64).min(n as i64 - 1);
                for x in xa..=xb {
                    blocked[y * n + x as usize] = true;
                }
            }
        }
    };
    for f in &j.buildings {
        fill(vec![f.ring.iter().map(|&(a, b)| to_px(a, b)).collect()]);
    }
    for l in &j.areas {
        let mut rings: Vec<Vec<Vec2>> = l.outer.iter().map(|r| r.iter().map(|&(a, b)| to_px(a, b)).collect()).collect();
        rings.extend(l.inner.iter().map(|r| r.iter().map(|&(a, b)| to_px(a, b)).collect::<Vec<_>>()));
        fill(rings);
    }
    let base = species::grass_colour(j.biome, &j.season);
    let mut out = vec![0u8; n * n * 4];
    for k in 0..n {
        for i in 0..n {
            let o = (k * n + i) * 4;
            if blocked[k * n + i] {
                continue;
            }
            let (la, lo) = local.ll(at(i, k));
            let mut sample = None;
            for c in &j.cells {
                if let Some(s) = c.sample(la, lo) {
                    sample = Some(s);
                    break;
                }
            }
            let Some((cover, img)) = sample else { continue };
            let dens = cover.grass();
            if dens <= 0.0 {
                continue;
            }
            let noise = hash(i as u32 / 6, k as u32 / 6);
            let (h, dmul) = match cover {
                Cover::Grass => (0.1 + 0.08 * noise, 1.0),
                Cover::DryGrass => (0.28 + 0.2 * noise, 0.8),
                Cover::Canopy => (0.12, 0.6),
                Cover::Shrub => (0.3, 0.5),
                Cover::Crop => {
                    // Rows: crops grow in lines.
                    let row = ((at(i, k).x * 1.3).sin() * 0.5 + 0.5) > 0.35;
                    let grown = (1.0 - j.season.dormant) * (1.0 - j.season.dry * 0.5);
                    (0.25 + 1.0 * grown, if row { 1.0 } else { 0.15 })
                }
                Cover::Wetland => (0.7 + 0.3 * noise, 0.9),
                _ => (0.1, 1.0),
            };
            // Colour: the imagery's colour pulled towards the region's grass colour for
            // the season, and a little more saturated.
            let im = [img[0] as f32 / 255.0, img[1] as f32 / 255.0, img[2] as f32 / 255.0];
            let has_img = img != [0, 0, 0];
            let lin = |c: f32| c.powf(1.0 / 2.2);
            let target = [lin(base[0]), lin(base[1]), lin(base[2])];
            let mix = if has_img { 0.45 } else { 1.0 };
            let mut c = [0.0f32; 3];
            for q in 0..3 {
                c[q] = im[q] + (target[q] - im[q]) * mix;
            }
            let l = (c[0] + c[1] + c[2]) / 3.0;
            for q in c.iter_mut() {
                *q = (l + (*q - l) * 1.25).clamp(0.0, 1.0);
            }
            let d = ((dens * dmul).clamp(0.0, 1.0) * 15.0).round() as u8;
            out[o] = (c[0] * 255.0) as u8;
            out[o + 1] = (c[1] * 255.0) as u8;
            out[o + 2] = (c[2] * 255.0) as u8;
            out[o + 3] = (d << 4) | hclass(h);
        }
    }
    out
}

fn receive_map(mut g: ResMut<Grass>, mut images: ResMut<Assets<Image>>) {
    let Some(rx) = &g.job else { return };
    let Ok((la, lo, data)) = rx.try_recv() else { return };
    g.job = None;
    if let Some(mut img) = images.get_mut(&g.map) {
        img.data = Some(data);
    }
    g.centre = Some((la, lo, Geodetic::new(la, lo, 0.0).to_ecef()));
}

/// Keep a grid of patches around you, on the ground.
#[allow(clippy::too_many_arguments)]
fn place_patches(
    mut g: ResMut<Grass>,
    nat: Res<Nature>,
    viewer: Res<Viewer>,
    terrain: Res<Terrain>,
    settings: Res<crate::GameSettings>,
    mut commands: Commands,
    mut anchors: Query<&mut GeoAnchor>,
    mut moved_at: bevy::prelude::Local<Option<(i32, i32)>>,
) {
    let mode_3d = three_d(settings.0.preset);
    let r = ranges(settings.0.preset).grass;
    let p = viewer.geo();
    let ground = terrain.height_at(p.lat, p.lon).unwrap_or(0.0);
    let off = !nat.enabled || g.centre.is_none() || p.h - ground > 120.0;
    if off || mode_3d != g.mode_3d {
        if let Some(root) = g.root.take() {
            commands.entity(root).despawn();
        }
        g.patches.clear();
        g.grid = None;
        *moved_at = None;
        g.mode_3d = mode_3d;
        if off {
            return;
        }
    }
    // A fixed frame for the grid, renewed when you've gone a long way from it.
    let need_frame = g.grid.is_none_or(|(_, _, la, lo)| {
        let dy = (p.lat - la) * 111_000.0;
        let dx = (p.lon - lo) * 111_000.0 * la.to_radians().cos();
        dx * dx + dy * dy > 400.0 * 400.0
    });
    if need_frame {
        if let Some(root) = g.root.take() {
            commands.entity(root).despawn();
        }
        g.patches.clear();
        let base = Geodetic::new(p.lat, p.lon, ground);
        let frame = EnuFrame::at(base);
        let to_ecef: DMat3 = frame.ecef_to_local.transpose();
        let q = DQuat::from_mat3(&to_ecef);
        let e = base.to_ecef();
        let root = commands.spawn((GeoAnchor::new(e), Transform::default(), Visibility::Visible)).id();
        g.root = Some(root);
        g.grid = Some((e, q, p.lat, p.lon));
        *moved_at = None;
    }
    let Some((ge, gq, gla, glo)) = g.grid else { return };
    let Some(root) = g.root else { return };
    let _ = anchors.get_mut(root);
    let local = crate::roads::build::Local::new(gla, glo);
    let me = local.xz(p.lat, p.lon);
    let cell = ((me.x / PATCH).floor() as i32, (me.y / PATCH).floor() as i32);
    if *moved_at == Some(cell) {
        return;
    }
    *moved_at = Some(cell);
    let reach = (r / PATCH).ceil() as i32 + 1;
    let mut want: HashMap<(i32, i32), usize> = HashMap::new();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let c = (cell.0 + dx, cell.1 + dz);
            let centre = Vec2::new((c.0 as f32 + 0.5) * PATCH, (c.1 as f32 + 0.5) * PATCH);
            let d = centre.distance(me);
            if d > r {
                continue;
            }
            let lod = if !mode_3d {
                3
            } else if d < r * 0.3 {
                0
            } else if d < r * 0.62 {
                1
            } else {
                2
            };
            want.insert(c, lod);
        }
    }
    // Drop patches no longer wanted (or at another detail).
    let keys: Vec<(i32, i32)> = g.patches.keys().copied().collect();
    for k in keys {
        if !want.contains_key(&k) {
            if let Some(e) = g.patches.remove(&k) {
                commands.entity(e).despawn();
            }
        }
    }
    let gq32 = gq.as_quat();
    for (c, lod) in want {
        let lod_key = (c.0 * 4 + lod as i32, c.1);
        let _ = lod_key;
        if g.patches.contains_key(&c) {
            continue;
        }
        let centre = Vec2::new((c.0 as f32 + 0.5) * PATCH, (c.1 as f32 + 0.5) * PATCH);
        let (la, lo) = local.ll(centre);
        let Some(h) = terrain.height_at(la, lo) else { continue };
        let e = Geodetic::new(la, lo, h).to_ecef();
        let rel = (e - ge).as_vec3();
        // Turn each patch a quarter turn at random so the blades don't repeat.
        let turn = Quat::from_rotation_y((hash(c.0 as u32, c.1 as u32) * 4.0).floor() * std::f32::consts::FRAC_PI_2);
        let mat = if lod == 3 { g.tufts.clone() } else { g.blades.clone() };
        let ent = commands
            .spawn((
                Mesh3d(g.meshes[lod].clone()),
                MeshMaterial3d(mat),
                Transform { translation: rel, rotation: gq32 * turn, scale: Vec3::ONE },
                bevy::light::NotShadowCaster,
                bevy::camera::visibility::NoFrustumCulling,
                ChildOf(root),
            ))
            .id();
        g.patches.insert(c, ent);
    }
}

/// The map's position in render space and the wind, each frame.
fn update_params(
    g: Res<Grass>,
    origin: Res<WorldOrigin>,
    wind: Res<super::conditions::Conditions>,
    mut mats: ResMut<Assets<GrassMaterial>>,
) {
    let Some((_, _, e)) = g.centre else { return };
    let centre = origin.local(e);
    let (la, lo) = {
        let gg = Geodetic::from_ecef(e);
        (gg.lat, gg.lon)
    };
    let frame = EnuFrame::at(Geodetic::new(la, lo, 0.0));
    let to_ecef: DMat3 = frame.ecef_to_local.transpose();
    let east_ecef = to_ecef * DVec3::X;
    let south_ecef = to_ecef * DVec3::Z;
    let east = origin.frame.dir_to_local(east_ecef).as_vec3();
    let south = origin.frame.dir_to_local(south_ecef).as_vec3();
    let strength = (wind.wind_speed / 18.0).clamp(0.02, 1.0);
    let a = wind.wind_from.to_radians();
    let dir = Vec2::new(-a.sin(), -a.cos());
    for (h, tuft) in [(&g.blades, 0.0), (&g.tufts, 1.0)] {
        if let Some(mut m) = mats.get_mut(h) {
            m.extension.params = GrassParams {
                centre: centre.extend(TEXEL),
                east: east.extend(MAP_N as f32),
                south: south.extend(0.0),
                wind: Vec4::new(dir.x, dir.y, strength, tuft),
            };
        }
    }
}

/// Cover sample for tests and the debug overlay.
pub fn cover_at(nat: &Nature, lat: f64, lon: f64) -> Option<Cover> {
    let (id, _, _) = TileId::containing(lat, lon, super::cover::CELL_ZOOM);
    let c = nat.cell(id)?;
    let (u, v) = tile_uv(id, lat, lon);
    let k = (v * GRASS_N as f64) as usize * GRASS_N + (u * GRASS_N as f64) as usize;
    c.cover.get(k).map(|&x| Cover::from_u8(x))
}
