//! Streaming terrain: a Web Mercator quadtree of tiles with real elevation and
//! imagery, refined by on-screen texel size and loaded nearest-first.
//!
//! Each frame the selection walks the quadtree from the root. A tile splits when its
//! imagery texels would look too big on screen, but only once all four children are
//! loaded, so there are never holes: the parent stays visible until they arrive.

pub mod conform;
pub mod load;
pub mod material;
pub mod region;

use crate::camera::MainCamera;
use crate::globe::{GeoAnchor, Viewer};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use load::{HeightGrid, LoadJob, Loaded};
use material::{TerrainExt, TerrainMaterial};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use usx_core::geo::{EARTH_MEAN_RADIUS, Geodetic};
use usx_core::net::WorkPool;
use usx_core::sources::ImagerySource;
use usx_core::tile::TileId;

/// Tiles at or below this zoom are never unloaded (they are the fallback everywhere).
const KEEP_ZOOM: u8 = 4;

enum State {
    Loading { since: f64 },
    Ready { entity: Entity, image: Option<Handle<Image>>, mesh: Handle<Mesh>, material: Handle<TerrainMaterial>, gpu_bytes: usize },
    Failed { retry_at: f64 },
}

struct Tile {
    state: State,
    heights: Option<Arc<HeightGrid>>,
    center: DVec3,
    radius: f64,
    max_h: f64,
    last_wanted: f64,
    visible: bool,
    elev_src: &'static str,
    img_zoom: u8,
    /// Built from fallback data; reload after this time.
    refresh_at: Option<f64>,
    refreshing: bool,
    /// Heights were levelled under this area's roads.
    conformed: bool,
    /// Building pads the heights were dug for.
    pad_sig: u64,
    /// Bumped every time the tile's mesh is replaced.
    generation: u64,
}

#[derive(Resource)]
pub struct Terrain {
    tiles: HashMap<TileId, Tile>,
    pool: WorkPool<LoadJob, Loaded>,
    /// Multiplier on the target texel size; rises when streaming or rendering falls behind.
    pub adapt: f32,
    imagery: ImagerySource,
    lidar: bool,
    pub max_zoom: u8,
    /// GPU supports BC texture compression.
    pub bc1: bool,
    pub extra_detail_zoom: u8,
    generation: u64,
    pub error_count: u32,
    pub last_error: Option<String>,
}

/// Height grids of the tiles on screen over an area, shareable with worker threads.
pub struct HeightSnap {
    tiles: HashMap<TileId, (Arc<HeightGrid>, bool)>,
    zooms: Vec<u8>,
}

impl HeightSnap {
    /// Visible ground height, and whether it was already levelled under roads.
    pub fn ground(&self, lat: f64, lon: f64) -> Option<(f64, bool)> {
        for &z in &self.zooms {
            let (id, u, v) = TileId::containing(lat, lon, z);
            if let Some((g, c)) = self.tiles.get(&id) {
                return Some((g.sample(u, v), *c));
            }
        }
        None
    }
    /// The ground before it was levelled under roads (and dug out under lakes).
    pub fn natural(&self, lat: f64, lon: f64) -> Option<f64> {
        for &z in &self.zooms {
            let (id, u, v) = TileId::containing(lat, lon, z);
            if let Some((g, _)) = self.tiles.get(&id) {
                return Some(g.sample_natural(u, v));
            }
        }
        None
    }
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Numbers for the debug overlay and the loading indicator.
#[derive(Resource, Default)]
pub struct TerrainStats {
    pub visible: usize,
    pub loaded: usize,
    pub pending: usize,
    pub gpu_bytes: usize,
    pub deepest_visible: u8,
    pub elevation_source: &'static str,
    pub imagery_zoom_here: u8,
    pub imagery_source_zoom: u8,
    /// Seconds since the last teleport until streaming went idle.
    pub settle: Option<f32>,
    pub idle_frames: u32,
}

/// Terrain streaming systems; roads drape after these.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainSet;

/// Sent when the viewer jumps somewhere new.
#[derive(Message)]
pub struct Teleported;

#[derive(Resource)]
struct TeleportClock(Instant, bool);

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        let s = app.world().resource::<crate::GameSettings>().0.clone();
        let _ = load::CACHE.set(usx_core::cache::DiskCache::open(s.cache_dir(), s.cache_bytes()));
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(4, 12) * 2;
        let pool = WorkPool::new("tile", threads, |_, job| load::load(job));
        app.insert_resource(Terrain {
            tiles: HashMap::new(),
            pool,
            adapt: 1.0,
            imagery: s.imagery,
            lidar: s.lidar_elevation,
            max_zoom: max_zoom_for(&s),
            bc1: false,
            extra_detail_zoom: 0,
            generation: 0,
            error_count: 0,
            last_error: None,
        })
        .init_resource::<TerrainStats>()
        .add_message::<Teleported>()
        .insert_resource(TeleportClock(Instant::now(), true))
        .add_plugins((region::RegionPlugin, material::TerrainMaterialPlugin))
        .add_systems(Startup, detect_compression)
        .add_systems(Update, (on_teleport, receive, select).chain().in_set(TerrainSet));
    }
}

fn max_zoom_for(s: &usx_core::settings::Settings) -> u8 {
    let cap = match s.preset {
        usx_core::settings::Preset::Low => 17,
        usx_core::settings::Preset::Medium => 18,
        usx_core::settings::Preset::High => 19,
        usx_core::settings::Preset::Ultra => 19,
    };
    cap.min(s.imagery.max_zoom().max(17))
}

impl Terrain {
    /// Ground height (m above sea level) from the most detailed loaded tile.
    pub fn height_at(&self, lat: f64, lon: f64) -> Option<f64> {
        self.height_and_zoom(lat, lon).map(|h| h.0)
    }

    /// Visible ground height (sea surface over the sea) plus the zoom of the tile it came from.
    pub fn height_and_zoom(&self, lat: f64, lon: f64) -> Option<(f64, u8)> {
        self.lookup(lat, lon, |h, u, v| h.sample(u, v))
    }

    /// The ground before roads levelled it and lakes were dug out (lake surfaces).
    pub fn natural_at(&self, lat: f64, lon: f64) -> Option<f64> {
        self.lookup(lat, lon, |h, u, v| h.sample_natural(u, v)).map(|x| x.0)
    }

    /// Ground you stand on: the sea floor under water.
    pub fn floor_and_zoom(&self, lat: f64, lon: f64) -> Option<(f64, u8)> {
        self.lookup(lat, lon, |h, u, v| h.sample_floor(u, v))
    }

    fn lookup(&self, lat: f64, lon: f64, f: impl Fn(&load::HeightGrid, f64, f64) -> f64) -> Option<(f64, u8)> {
        for z in (0..=self.max_zoom).rev() {
            let (id, u, v) = TileId::containing(lat, lon, z);
            if let Some(t) = self.tiles.get(&id)
                && let Some(h) = &t.heights
            {
                return Some((f(h, u, v), z));
            }
        }
        None
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Drop every tile (after changing the imagery source or detail settings).
    pub fn reset(&mut self, commands: &mut Commands, settings: &usx_core::settings::Settings) {
        for (id, t) in self.tiles.drain() {
            if let State::Ready { entity, .. } = t.state {
                commands.entity(entity).despawn();
            }
            self.pool.cancel(key(id));
        }
        self.imagery = settings.imagery;
        self.lidar = settings.lidar_elevation;
        self.max_zoom = max_zoom_for(settings);
    }

    fn request(&mut self, id: TileId, priority: f32, now: f64) {
        let t = self.tiles.entry(id).or_insert_with(|| {
            let (c, r) = rough_bounds(id);
            Tile {
                state: State::Failed { retry_at: 0.0 },
                heights: None,
                center: c,
                radius: r,
                max_h: 4500.0,
                last_wanted: now,
                visible: false,
                elev_src: "",
                img_zoom: 0,
                refresh_at: None,
                refreshing: false,
                conformed: false,
                pad_sig: 0,
                generation: 0,
            }
        });
        t.last_wanted = now;
        match t.state {
            State::Loading { .. } => {
                self.pool.reprioritize(key(id), priority);
            }
            State::Failed { retry_at } if now >= retry_at => {
                t.state = State::Loading { since: now };
                let job =
                    LoadJob { id, imagery: self.imagery, lidar: self.lidar, max_imagery_zoom: self.imagery.max_zoom(), bc1: self.bc1 };
                self.pool.submit(key(id), priority, job);
            }
            _ => {}
        }
    }

    /// Tiles on screen covering a box (s, w, n, e), plus coarse ones as a fallback,
    /// for draping roads onto exactly the ground being drawn.
    pub fn snapshot(&self, bbox: (f64, f64, f64, f64)) -> HeightSnap {
        let mut tiles = HashMap::new();
        self.visit(bbox, |id, t| {
            if let Some(h) = &t.heights
                && (t.visible || id.z <= 13)
            {
                tiles.insert(id, (h.clone(), t.conformed && id.z >= conform::CONFORM_ZOOM));
            }
        });
        let mut zooms: Vec<u8> = tiles.keys().map(|k: &TileId| k.z).collect();
        zooms.sort_unstable_by(|a, b| b.cmp(a));
        zooms.dedup();
        HeightSnap { tiles, zooms }
    }

    /// Changes whenever the ground drawn inside a box changes.
    pub fn signature(&self, bbox: (f64, f64, f64, f64)) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        self.visit(bbox, |id, t| {
            if t.visible && t.heights.is_some() {
                h ^= key(id).wrapping_mul(0x100_0000_01b3) ^ t.generation.rotate_left(17);
                h = h.wrapping_mul(0x100_0000_01b3);
            }
        });
        h
    }

    fn visit(&self, (s, w, n, e): (f64, f64, f64, f64), mut f: impl FnMut(TileId, &Tile)) {
        for z in 0..=self.max_zoom {
            let (a, _, _) = TileId::containing(n, w, z);
            let (b, _, _) = TileId::containing(s, e, z);
            let count = (b.x as u64 - a.x as u64 + 1) * (b.y as u64 - a.y as u64 + 1);
            if count > 4096 {
                break;
            }
            for y in a.y..=b.y {
                for x in a.x..=b.x {
                    let id = TileId::new(z, x, y);
                    if let Some(t) = self.tiles.get(&id)
                        && matches!(t.state, State::Ready { .. })
                    {
                        f(id, t);
                    }
                }
            }
        }
    }

    fn ready(&self, id: TileId) -> bool {
        matches!(self.tiles.get(&id).map(|t| &t.state), Some(State::Ready { .. }))
    }
}

fn key(id: TileId) -> u64 {
    ((id.z as u64) << 58) | ((id.x as u64) << 29) | id.y as u64
}

/// Bounding sphere before a tile's real heights are known.
fn rough_bounds(id: TileId) -> (DVec3, f64) {
    let (lat, lon) = id.lat_lon_at(0.5, 0.5);
    let c = Geodetic::new(lat, lon, 1500.0).to_ecef();
    let mut r: f64 = 0.0;
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.0), (0.5, 1.0), (0.0, 0.5), (1.0, 0.5)] {
        let (la, lo) = id.lat_lon_at(u, v);
        for h in [-100.0, 4500.0] {
            r = r.max((Geodetic::new(la, lo, h).to_ecef() - c).length());
        }
    }
    (c, r)
}

fn detect_compression(device: Option<Res<bevy::render::renderer::RenderDevice>>, mut terrain: ResMut<Terrain>) {
    terrain.bc1 = device.is_some_and(|d| d.features().contains(bevy::render::render_resource::WgpuFeatures::TEXTURE_COMPRESSION_BC));
    info!("terrain textures: {}", if terrain.bc1 { "BC1 compressed" } else { "uncompressed RGBA" });
}

fn on_teleport(mut ev: MessageReader<Teleported>, mut clock: ResMut<TeleportClock>, mut stats: ResMut<TerrainStats>) {
    if ev.read().last().is_some() {
        clock.0 = Instant::now();
        clock.1 = true;
        stats.settle = None;
    }
}

/// Turn finished loads into meshes and textures, a few per frame to avoid hitches.
#[allow(clippy::too_many_arguments)]
fn receive(
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    let start = Instant::now();
    let mut n = 0;
    while n < 24 && start.elapsed().as_secs_f32() < 0.004 {
        let Some(Loaded { id, result }) = terrain.pool.try_recv() else {
            break;
        };
        n += 1;
        let Some(tile) = terrain.tiles.get(&id) else {
            continue;
        };
        if !matches!(tile.state, State::Loading { .. }) && !tile.refreshing {
            continue;
        }
        let refreshing = tile.refreshing;
        match result {
            Err(e) => {
                terrain.error_count += 1;
                terrain.last_error = Some(e);
                let t = terrain.tiles.get_mut(&id).unwrap();
                if refreshing {
                    t.refreshing = false;
                    t.refresh_at = Some(now + 30.0);
                } else {
                    t.state = State::Failed { retry_at: now + 8.0 };
                }
            }
            Ok(data) => {
                let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
                let gpu_mesh = data.mesh.positions.len() * 48 + data.mesh.indices.len() * 2;
                mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.mesh.positions);
                mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.mesh.normals);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.mesh.uvs);
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, data.mesh.detail_uvs);
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, data.mesh.water);
                mesh.insert_indices(Indices::U16(data.mesh.indices));
                let mesh = meshes.add(mesh);
                let (image, gpu_img) = match data.image {
                    Some(img) => {
                        let bytes = img.data.len();
                        let mut im = Image::new_uninit(
                            Extent3d { width: load::TEX as u32, height: load::TEX as u32, depth_or_array_layers: 1 },
                            TextureDimension::D2,
                            if img.bc1 { TextureFormat::Bc1RgbaUnormSrgb } else { TextureFormat::Rgba8UnormSrgb },
                            RenderAssetUsages::RENDER_WORLD,
                        );
                        im.data = Some(img.data);
                        im.texture_descriptor.mip_level_count = img.levels;
                        im.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                            address_mode_u: ImageAddressMode::ClampToEdge,
                            address_mode_v: ImageAddressMode::ClampToEdge,
                            mag_filter: ImageFilterMode::Linear,
                            min_filter: ImageFilterMode::Linear,
                            mipmap_filter: ImageFilterMode::Linear,
                            anisotropy_clamp: 8,
                            ..default()
                        });
                        (Some(images.add(im)), bytes)
                    }
                    None => (None, 0),
                };
                let material = materials.add(TerrainMaterial {
                    base: StandardMaterial {
                        base_color: if image.is_some() { Color::WHITE } else { Color::srgb(0.42, 0.45, 0.38) },
                        base_color_texture: image.clone(),
                        perceptual_roughness: 0.92,
                        reflectance: 0.25,
                        cull_mode: None,
                        ..default()
                    },
                    extension: TerrainExt {},
                });
                let mut ec = commands.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    GeoAnchor::new(data.center),
                    Transform::default(),
                    Visibility::Hidden,
                ));
                // Only tiles near enough to be inside the shadow distance cast shadows.
                if id.z < 11 {
                    ec.insert(bevy::light::NotShadowCaster);
                }
                let entity = ec.id();
                terrain.generation += 1;
                let generation = terrain.generation;
                let t = terrain.tiles.get_mut(&id).unwrap();
                if let State::Ready { entity: old, .. } = t.state {
                    commands.entity(old).despawn();
                }
                t.conformed = data.conformed;
                t.pad_sig = data.pad_sig;
                t.generation = generation;
                t.refreshing = false;
                t.refresh_at = data.degraded.then_some(now + 30.0);
                t.center = data.center;
                t.radius = data.radius;
                t.max_h = data.heights.max as f64;
                t.heights = Some(data.heights);
                t.elev_src = data.elevation_source;
                t.img_zoom = data.imagery_zoom;
                t.state = State::Ready { entity, image, mesh, material, gpu_bytes: gpu_mesh + gpu_img };
            }
        }
    }
}

struct View {
    cam: DVec3,
    fwd: DVec3,
    cam_h: f64,
    proj: f64,
    half_fov: f64,
}

impl View {
    /// On-screen size of one imagery texel of this tile, in pixels.
    fn texel_px(&self, id: TileId, t: &Tile) -> f64 {
        let d = ((self.cam - t.center).length() - t.radius).max(1.0);
        (id.width_m() / load::TEX as f64) * self.proj / d
    }
    fn over_horizon(&self, t: &Tile) -> bool {
        let dh = (self.cam_h.max(10.0) * (2.0 * EARTH_MEAN_RADIUS + self.cam_h.max(10.0))).sqrt();
        let th = t.max_h.max(0.0) + 100.0;
        let dt = (th * (2.0 * EARTH_MEAN_RADIUS + th)).sqrt();
        (self.cam - t.center).length() - t.radius > dh + dt
    }
    fn in_view(&self, t: &Tile) -> bool {
        let v = t.center - self.cam;
        let d = v.length();
        if d <= t.radius {
            return true;
        }
        let ang = (v / d).dot(self.fwd).clamp(-1.0, 1.0).acos();
        ang < self.half_fov + (t.radius / d).asin()
    }
}

#[allow(clippy::too_many_arguments)]
fn select(
    mut terrain: ResMut<Terrain>,
    mut stats: ResMut<TerrainStats>,
    viewer: Res<Viewer>,
    settings: Res<crate::GameSettings>,
    windows: Query<&Window>,
    cams: Query<&Projection, With<MainCamera>>,
    mut vis: Query<&mut Visibility>,
    mut commands: Commands,
    time: Res<Time>,
    mut clock: ResMut<TeleportClock>,
    args: Res<crate::Args>,
) {
    let now = time.elapsed_secs_f64();
    let preset = settings.0.preset;
    let height = windows.iter().next().map(|w| w.resolution.physical_height()).unwrap_or(900) as f64;
    let fov = match cams.iter().next() {
        Some(Projection::Perspective(p)) => p.fov as f64,
        _ => 70f64.to_radians(),
    };
    let aspect = windows.iter().next().map(|w| w.width() / w.height().max(1.0)).unwrap_or(1.7) as f64;
    let (fwd, _, _) = viewer.basis_ecef();
    let view = View {
        cam: viewer.ecef,
        fwd,
        cam_h: viewer.geo().h - viewer.ground.unwrap_or(0.0).min(viewer.geo().h),
        proj: height / (2.0 * (fov / 2.0).tan()),
        half_fov: ((fov / 2.0).tan() * (1.0 + aspect * aspect).sqrt()).atan(),
    };

    // Adapt detail to how well streaming and rendering keep up.
    let pending = terrain.pool.waiting() + terrain.pool.busy() + terrain.pool.finished();
    let frame_ms = time.delta_secs() * 1000.0;
    let budget_ms = if settings.0.vsync { 34.0 } else { 20.0 };
    let frame_slow = args.shots.is_empty() && frame_ms > budget_ms * 1.5;
    let behind = pending > 120 || frame_slow || stats.visible > preset.max_tiles();
    let fine = pending < 24 && (frame_ms < budget_ms || !args.shots.is_empty()) && stats.visible < preset.max_tiles() * 3 / 4;
    terrain.adapt = if behind {
        (terrain.adapt * 1.015).min(3.0)
    } else if fine {
        (terrain.adapt * 0.995).max(1.0)
    } else {
        terrain.adapt
    };
    let target = preset.texel_pixels() as f64 * terrain.adapt as f64;
    let max_zoom = terrain.max_zoom;

    let mut visible: Vec<TileId> = Vec::new();
    let mut stack = vec![TileId::ROOT];
    let mut requests: Vec<(TileId, f32)> = Vec::new();
    while let Some(id) = stack.pop() {
        let Some(t) = terrain.tiles.get(&id) else {
            requests.push((id, -10.0 + id.z as f32));
            continue;
        };
        if id.z > 2 && view.over_horizon(t) {
            continue;
        }
        let in_view = view.in_view(t);
        let px = view.texel_px(id, t) * if in_view { 1.0 } else { 0.3 };
        let dist = ((view.cam - t.center).length() - t.radius).max(0.0);
        let near = dist < preset.detail_radius_m();
        let want_split = id.z < max_zoom && (px > target || (id.z < 9 && near));
        if want_split {
            let kids = id.children();
            let all_ready = kids.iter().all(|k| terrain.ready(*k));
            for k in kids {
                let pr = (dist / id.width_m().max(1.0)) as f32 + if in_view { 0.0 } else { 6.0 } + k.z as f32 * 0.05;
                requests.push((k, pr));
            }
            if all_ready {
                stack.extend(kids);
                continue;
            }
        }
        if terrain.ready(id) {
            visible.push(id);
        }
    }
    // Prefetch along the direction of travel at lower priority.
    let speed = viewer.velocity.length();
    if speed > 8.0 {
        let ahead = viewer.ecef + viewer.velocity * 3.0;
        let g = Geodetic::from_ecef(ahead);
        for z in (10..=max_zoom).step_by(2) {
            let (id, _, _) = TileId::containing(g.lat, g.lon, z);
            requests.push((id, 8.0 + z as f32 * 0.1));
        }
    }
    for (id, p) in requests {
        terrain.request(id, p, now);
    }
    let mut stale: Vec<TileId> = terrain
        .tiles
        .iter()
        .filter(|(_, t)| !t.refreshing && t.refresh_at.is_some_and(|r| now > r) && now - t.last_wanted < 2.0)
        .map(|(id, _)| *id)
        .collect();
    // Detailed tiles built before their roads were known: rebuild them levelled.
    let reconform: Vec<TileId> = terrain
        .tiles
        .iter()
        .filter(|(id, t)| {
            id.z >= conform::CONFORM_ZOOM
                && !t.refreshing
                && matches!(t.state, State::Ready { .. })
                && now - t.last_wanted < 2.0
                && (!t.conformed && crate::roads::corridors_for(**id).is_some() || conform::pads_sig(**id) != t.pad_sig)
        })
        .map(|(id, _)| *id)
        .take(24)
        .collect();
    stale.extend(reconform);
    for id in stale {
        let job =
            LoadJob { id, imagery: terrain.imagery, lidar: terrain.lidar, max_imagery_zoom: terrain.imagery.max_zoom(), bc1: terrain.bc1 };
        terrain.pool.submit(key(id), 20.0, job);
        if let Some(t) = terrain.tiles.get_mut(&id) {
            t.refreshing = true;
        }
    }

    // Show exactly the selected tiles.
    let mut gpu = 0;
    let mut deepest = 0;
    for t in terrain.tiles.values_mut() {
        t.visible = false;
    }
    for id in &visible {
        if let Some(t) = terrain.tiles.get_mut(id) {
            t.visible = true;
            t.last_wanted = now;
            deepest = deepest.max(id.z);
        }
        // Keep ancestors warm so zooming out is instant.
        let mut p = id.parent();
        while let Some(pid) = p {
            if let Some(pt) = terrain.tiles.get_mut(&pid) {
                if pt.last_wanted >= now {
                    break;
                }
                pt.last_wanted = now;
            }
            p = pid.parent();
        }
    }
    let mut evict = Vec::new();
    let mut ready_count = 0;
    let loaded_budget = preset.max_tiles() * 2;
    let total_ready = terrain.tiles.values().filter(|t| matches!(t.state, State::Ready { .. })).count();
    for (id, t) in terrain.tiles.iter() {
        match &t.state {
            State::Ready { entity, gpu_bytes, .. } => {
                ready_count += 1;
                gpu += gpu_bytes;
                if let Ok(mut v) = vis.get_mut(*entity) {
                    let want = if t.visible { Visibility::Visible } else { Visibility::Hidden };
                    if *v != want {
                        *v = want;
                    }
                }
                let idle = now - t.last_wanted;
                if id.z > KEEP_ZOOM && !t.visible && (idle > 60.0 || (idle > 3.0 && total_ready > loaded_budget)) {
                    evict.push(*id);
                }
            }
            State::Loading { since } => {
                if now - t.last_wanted > 2.0 && now - since > 0.5 {
                    evict.push(*id);
                }
            }
            State::Failed { .. } => {
                if now - t.last_wanted > 30.0 {
                    evict.push(*id);
                }
            }
        }
    }
    for id in evict {
        if let Some(t) = terrain.tiles.remove(&id) {
            match t.state {
                State::Ready { entity, .. } => commands.entity(entity).despawn(),
                State::Loading { .. } => {
                    terrain.pool.cancel(key(id));
                }
                State::Failed { .. } => {}
            }
        }
    }

    stats.visible = visible.len();
    stats.loaded = ready_count;
    stats.pending = pending;
    stats.gpu_bytes = gpu;
    stats.deepest_visible = deepest;
    let g = viewer.geo();
    let here = (0..=max_zoom).rev().map(|z| TileId::containing(g.lat, g.lon, z).0).find(|id| terrain.ready(*id));
    stats.imagery_zoom_here = here.map(|id| id.z).unwrap_or(0);
    stats.imagery_source_zoom = here.and_then(|id| terrain.tiles.get(&id)).map(|t| t.img_zoom).unwrap_or(0);
    stats.elevation_source = here.and_then(|id| terrain.tiles.get(&id)).map(|t| t.elev_src).unwrap_or("");
    if pending == 0 {
        stats.idle_frames += 1;
    } else {
        stats.idle_frames = 0;
    }
    if clock.1 && stats.idle_frames > 30 && deepest > 8 {
        stats.settle = Some(clock.0.elapsed().as_secs_f32());
        clock.1 = false;
    }
}
