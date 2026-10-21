//! Buildings from OpenStreetMap: every footprint becomes a 3D building with a roof,
//! textured walls and windows; the ones near you get real doors and interiors.
//!
//! Pipeline per zoom-15 tile: Overpass (buildings layer) -> build (footprints, parts,
//! roofs, street sides, roof colours from imagery) -> per zoom-16 chunk: drape (set
//! each building level on the ground on screen) -> mesh. Buildings close to the player
//! are drawn separately in detail (real door openings, see-through windows once their
//! interior is loaded) and hidden from their chunk mesh.

pub mod build;
pub mod drape;
pub mod exterior;
pub mod furniture;
pub mod geom;
pub mod interact;
pub mod interior;
pub mod landmark;
pub mod material;
pub mod model;
pub mod plan;
pub mod textures;

use crate::globe::{GeoAnchor, Viewer};
use crate::terrain::Terrain;
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use build::Built;
use material::BuildingMaterial;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use usx_core::geo::Geodetic;
use usx_core::net::WorkPool;
use usx_core::osm::{self, Layer, OsmTile};
use usx_core::tile::TileId;

/// Above this height over the ground, buildings aren't loaded.
const MAX_AGL_M: f64 = 3_000.0;
const UNLOAD_RADIUS_M: f64 = 5_000.0;
/// Buildings within this distance are drawn in detail with real doors.
pub const DETAIL_RADIUS_M: f64 = 45.0;

/// (load within, draw within) by graphics preset.
fn ranges(p: usx_core::settings::Preset) -> (f64, f64) {
    use usx_core::settings::Preset;
    match p {
        Preset::Low => (1_200.0, 1_300.0),
        Preset::Medium => (1_800.0, 2_000.0),
        Preset::High => (2_400.0, 2_700.0),
        Preset::Ultra => (3_200.0, 3_600.0),
    }
}

enum Job {
    Build(TileId, OsmTile, Option<OsmTile>, usx_core::sources::ImagerySource),
    Drape(Arc<Built>, TileId, usize, crate::terrain::HeightSnap, u64, Vec<u32>),
}

enum Out {
    Built(TileId, Box<Built>),
    Draped(TileId, usize, u64, Option<drape::Draped>),
}

enum TileState {
    Fetching,
    Building,
    Ready(Arc<Built>),
    Failed { retry_at: f64 },
}

#[derive(Default)]
struct ChunkState {
    root: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    sig: u64,
    pending: Option<u64>,
    dist: f64,
    /// Buildings drawn separately in detail (left out of this chunk's mesh).
    hidden: Vec<u32>,
}

struct BTile {
    state: TileState,
    chunks: Vec<ChunkState>,
    last_wanted: f64,
    /// Ground to keep under each placed building (sent to the terrain).
    pads: HashMap<u32, crate::terrain::conform::Pad>,
}

/// A building's place in the world after draping.
#[derive(Clone, Copy, Debug)]
pub struct PlacedRef {
    pub tile: TileId,
    pub b: u32,
    pub base: f64,
    pub low: f64,
    pub origin: DVec3,
}

#[derive(Resource)]
pub struct Buildings {
    tiles: HashMap<TileId, BTile>,
    fetch: WorkPool<TileId, (TileId, Result<OsmTile, String>)>,
    work: WorkPool<Job, Out>,
    pub material: Handle<BuildingMaterial>,
    pub enabled: bool,
    cursor: usize,
    /// Every draped building by (tile, index).
    pub placed: HashMap<(TileId, u32), PlacedRef>,
    /// Buildings drawn in detail now.
    pub detail: HashMap<(TileId, u32), Detail>,
    pub last_error: Option<String>,
    /// Landmark model files by building id, and the models placed now.
    landmark_files: HashMap<i64, String>,
    landmarks: HashMap<(TileId, u32), Entity>,
}

/// A building drawn on its own, close up.
pub struct Detail {
    pub entity: Entity,
    pub material: Handle<BuildingMaterial>,
    pub built: Arc<Built>,
    pub placed: PlacedRef,
    pub interior: Option<Entity>,
    pub doors: Vec<Entity>,
    /// Height above the ground floor the interior's floors were built around, and
    /// whether it's only some of the floors (tall buildings).
    pub interior_y: f32,
    pub interior_partial: bool,
}

impl Buildings {
    pub fn built(&self, tile: TileId) -> Option<Arc<Built>> {
        match &self.tiles.get(&tile)?.state {
            TileState::Ready(b) => Some(b.clone()),
            _ => None,
        }
    }
}

#[derive(Resource, Default, Clone, Copy)]
pub struct BuildingStats {
    pub tiles_ready: usize,
    pub tiles_loading: usize,
    pub chunks_drawn: usize,
    pub buildings: usize,
    pub detailed: usize,
    pub interiors: usize,
}

#[derive(Resource)]
struct Holder(Option<(WorkPool<TileId, (TileId, Result<OsmTile, String>)>, WorkPool<Job, Out>)>);

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        let fetch = WorkPool::new("bld-osm", 2, |_, id: TileId| {
            let r = osm::load_layer(id, Layer::Buildings, crate::terrain::load::cache()).map_err(|e| e.to_string());
            (id, r)
        });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8) / 2;
        let work = WorkPool::new("buildings", threads.max(1), |_, job: Job| match job {
            Job::Build(id, osm, roads, img) => Out::Built(id, Box::new(build::build(id, &osm, roads.as_ref(), Some(img)))),
            Job::Drape(b, id, ci, snap, sig, hidden) => Out::Draped(id, ci, sig, drape::drape(&b, ci, &snap, &hidden)),
        });
        app.add_plugins(material::BuildingMaterialPlugin)
            .init_resource::<BuildingStats>()
            .insert_resource(Holder(Some((fetch, work))))
            .add_systems(Startup, (setup, textures::setup))
            .add_systems(
                Update,
                (want_tiles, receive, place_landmarks, schedule_drapes, manage_detail, update_material, stats)
                    .chain()
                    .after(crate::terrain::TerrainSet),
            )
            .add_systems(Update, landmark::fit);
        interact::plugin(app);
        interior::plugin(app);
    }
}

fn setup(
    mut commands: Commands,
    mut holder: ResMut<Holder>,
    mut mats: ResMut<Assets<BuildingMaterial>>,
    settings: Res<crate::GameSettings>,
) {
    let Some((fetch, work)) = holder.0.take() else { return };
    let material = mats.add(material::new_material(false));
    commands.insert_resource(Buildings {
        tiles: HashMap::new(),
        fetch,
        work,
        material,
        enabled: settings.0.buildings,
        cursor: 0,
        placed: HashMap::new(),
        detail: HashMap::new(),
        last_error: None,
        landmark_files: landmark::scan(),
        landmarks: HashMap::new(),
    });
}

fn key(id: TileId) -> u64 {
    ((id.x as u64) << 32) | id.y as u64 | 0x4000_0000_0000_0000
}

fn tile_distance(viewer: DVec3, id: TileId) -> f64 {
    let (w, s, e, n) = id.bounds();
    let g = Geodetic::from_ecef(viewer);
    let lat = g.lat.clamp(s, n);
    let lon = g.lon.clamp(w, e);
    (Geodetic::new(lat, lon, g.h).to_ecef() - viewer).length()
}

fn want_tiles(
    settings: Res<crate::GameSettings>,
    mut bl: Option<ResMut<Buildings>>,
    viewer: Res<Viewer>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let Some(bl) = bl.as_deref_mut() else { return };
    bl.enabled = settings.0.buildings;
    let (load_radius, _) = ranges(settings.0.preset);
    let now = time.elapsed_secs_f64();
    let g = viewer.geo();
    let low = viewer.agl().is_some_and(|a| a < MAX_AGL_M);
    if bl.enabled && low {
        let (c, _, _) = TileId::containing(g.lat, g.lon, osm::ZOOM);
        let span = (load_radius / c.width_m()).ceil() as i64 + 1;
        let mut want: Vec<(f64, TileId)> = Vec::new();
        for dy in -span..=span {
            for dx in -span..=span {
                let (x, y) = (c.x as i64 + dx, c.y as i64 + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let id = TileId::new(osm::ZOOM, x as u32, y as u32);
                let d = tile_distance(viewer.ecef, id);
                if d <= load_radius {
                    want.push((d, id));
                }
            }
        }
        want.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (d, id) in want {
            let t = bl.tiles.entry(id).or_insert_with(|| BTile {
                state: TileState::Failed { retry_at: 0.0 },
                chunks: Vec::new(),
                last_wanted: now,
                pads: HashMap::new(),
            });
            t.last_wanted = now;
            if let TileState::Failed { retry_at } = t.state
                && now >= retry_at
            {
                t.state = TileState::Fetching;
                bl.fetch.submit(key(id), d as f32, id);
            } else if matches!(t.state, TileState::Fetching) {
                bl.fetch.reprioritize(key(id), d as f32);
            }
        }
    }
    let far: Vec<TileId> = bl
        .tiles
        .iter()
        .filter(|(id, t)| tile_distance(viewer.ecef, **id) > UNLOAD_RADIUS_M || !bl.enabled || now - t.last_wanted > 120.0)
        .map(|(id, _)| *id)
        .collect();
    for id in far {
        if let Some(t) = bl.tiles.remove(&id) {
            bl.fetch.cancel(key(id));
            for c in t.chunks {
                if let Some(r) = c.root {
                    commands.entity(r).despawn();
                }
            }
            bl.placed.retain(|k, _| k.0 != id);
        }
    }
}

fn receive(mut bl: Option<ResMut<Buildings>>, time: Res<Time>, settings: Res<crate::GameSettings>, mut apply: Apply) {
    let Some(bl) = bl.as_deref_mut() else { return };
    let now = time.elapsed_secs_f64();
    while let Some((id, r)) = bl.fetch.try_recv() {
        let Some(t) = bl.tiles.get_mut(&id) else { continue };
        match r {
            Ok(osm) => {
                t.state = TileState::Building;
                // The road layer (if it's here yet) says which sides face the street.
                let roads = osm::cached_layer(id, Layer::Roads, crate::terrain::load::cache());
                bl.work.submit(key(id) ^ 0x8000_0000_0000_0000, 0.0, Job::Build(id, osm, roads, settings.0.imagery));
            }
            Err(e) => {
                warn!("buildings {}: {e}", id.path());
                bl.last_error = Some(e);
                t.state = TileState::Failed { retry_at: now + 30.0 };
            }
        }
    }
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < 0.006 {
        let Some(out) = bl.work.try_recv() else { break };
        match out {
            Out::Built(id, b) => {
                let Some(t) = bl.tiles.get_mut(&id) else { continue };
                info!("buildings {}: {} buildings, {} vertices", id.path(), b.buildings.len(), b.vertices);
                t.chunks = (0..b.chunks.len()).map(|_| ChunkState::default()).collect();
                t.state = TileState::Ready(Arc::from(b));
            }
            Out::Draped(id, ci, sig, d) => {
                let Some(t) = bl.tiles.get_mut(&id) else { continue };
                let TileState::Ready(built) = &t.state else { continue };
                let built = built.clone();
                let Some(c) = t.chunks.get_mut(ci) else { continue };
                c.pending = None;
                let Some(d) = d else { continue };
                c.sig = sig;
                let mut pads_changed = false;
                for p in &d.placed {
                    bl.placed.insert((id, p.b), PlacedRef { tile: id, b: p.b, base: p.base, low: p.low, origin: p.origin });
                    if let Some(pad) = pad_of(&built, p.b, p.base)
                        && t.pads.get(&p.b) != Some(&pad)
                    {
                        t.pads.insert(p.b, pad);
                        pads_changed = true;
                    }
                }
                if pads_changed {
                    let mut keys: Vec<&u32> = t.pads.keys().collect();
                    keys.sort();
                    let sig = keys.iter().fold(0xcbf2_9ce4_8422_2325u64, |a, k| {
                        (a ^ (**k as u64) ^ ((t.pads[*k].top * 20.0).round() as i64 as u64) << 24).wrapping_mul(0x100_0000_01b3)
                    });
                    crate::terrain::conform::set_pads(id, t.pads.values().cloned().collect(), sig);
                }
                let Some(c) = t.chunks.get_mut(ci) else { continue };
                apply.apply(c, d, bl.material.clone());
            }
        }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct Apply<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    anchors: Query<'w, 's, &'static mut GeoAnchor>,
}

impl Apply<'_, '_> {
    fn apply(&mut self, c: &mut ChunkState, d: drape::Draped, material: Handle<BuildingMaterial>) {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, d.uv0);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, d.uv1);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, d.color);
        mesh.insert_indices(Indices::U32(d.indices));
        let h = self.meshes.add(mesh);
        match (&c.mesh, c.root) {
            (Some(old), Some(root)) => {
                self.meshes.remove(old.id());
                self.commands.entity(root).insert(Mesh3d(h.clone()));
                if let Ok(mut a) = self.anchors.get_mut(root) {
                    a.ecef = d.center;
                    a.placed_generation = 0;
                }
            }
            _ => {
                let root = self
                    .commands
                    .spawn((
                        Mesh3d(h.clone()),
                        MeshMaterial3d(material),
                        GeoAnchor::new(d.center),
                        Transform::default(),
                        Visibility::Visible,
                    ))
                    .id();
                c.root = Some(root);
            }
        }
        c.mesh = Some(h);
    }
}

fn hidden_sig(h: &[u32]) -> u64 {
    h.iter().fold(0x9e37_79b9_7f4a_7c15u64, |a, &b| (a ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn schedule_drapes(
    settings: Res<crate::GameSettings>,
    mut bl: Option<ResMut<Buildings>>,
    terrain: Res<Terrain>,
    viewer: Res<Viewer>,
    mut commands: Commands,
) {
    let Some(bl) = bl.as_deref_mut() else { return };
    let (_, draw) = ranges(settings.0.preset);
    let mut list: Vec<(TileId, usize)> = Vec::new();
    for (id, t) in bl.tiles.iter() {
        if matches!(t.state, TileState::Ready(_)) {
            for i in 0..t.chunks.len() {
                list.push((*id, i));
            }
        }
    }
    if list.is_empty() {
        return;
    }
    list.sort();
    let budget = 48.min(list.len());
    let mut submitted = 0;
    for k in 0..budget {
        let (id, ci) = list[(bl.cursor + k) % list.len()];
        let Some(t) = bl.tiles.get_mut(&id) else { continue };
        let TileState::Ready(b) = &t.state else { continue };
        let b = b.clone();
        let chunk = &b.chunks[ci];
        let c = &mut t.chunks[ci];
        let d = tile_distance(viewer.ecef, chunk.id);
        c.dist = d;
        if d > draw + 300.0 {
            if let Some(r) = c.root.take() {
                commands.entity(r).despawn();
                c.mesh = None;
                c.sig = 0;
            }
            continue;
        }
        if d > draw || c.pending.is_some() || submitted >= 6 || chunk.geom.v.is_empty() {
            continue;
        }
        let tsig = terrain.signature(chunk.influence);
        if tsig == 0xcbf2_9ce4_8422_2325 {
            continue;
        }
        let sig = tsig ^ hidden_sig(&c.hidden);
        if sig == c.sig {
            continue;
        }
        let snap = terrain.snapshot(chunk.influence);
        if snap.is_empty() {
            continue;
        }
        c.pending = Some(sig);
        let hidden = c.hidden.clone();
        // Chunks with a building switching to/from detail go first.
        let pri = if c.root.is_some() && hidden_sig(&hidden) != 0x9e37_79b9_7f4a_7c15 { 0.0 } else { d as f32 };
        bl.work.submit(key(id) ^ ((ci as u64 + 1) << 48), pri, Job::Drape(b, id, ci, snap, sig, hidden));
        submitted += 1;
    }
    bl.cursor = (bl.cursor + budget) % list.len().max(1);
}

/// Which chunk of its tile a building is drawn in.
fn chunk_of(b: &Built, bi: u32) -> Option<usize> {
    b.chunks.iter().position(|c| c.ranges.iter().any(|r| r.b == bi))
}

/// Switch buildings near the player to detailed drawing (and back).
#[allow(clippy::too_many_arguments)]
fn manage_detail(
    mut bl: Option<ResMut<Buildings>>,
    viewer: Res<Viewer>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<BuildingMaterial>>,
    mut walk: ResMut<crate::walkable::Walkables>,
    mut timer: Local<f32>,
    time: Res<Time>,
    inside: Res<interior::Inside>,
    imats: Option<Res<textures::InteriorMats>>,
) {
    let (Some(bl), Some(imats)) = (bl.as_deref_mut(), imats) else { return };
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.25;
    let me = viewer.ecef;
    // Wanted: placed buildings within the detail radius (by distance to the footprint).
    let mut want: Vec<(f64, (TileId, u32))> = Vec::new();
    for (k, p) in &bl.placed {
        let d0 = (p.origin - me).length();
        if d0 > DETAIL_RADIUS_M + 250.0 {
            continue;
        }
        let Some(b) = bl.built(k.0) else { continue };
        let bld = &b.buildings[k.1 as usize];
        let d = footprint_distance(&b, bld, p, me);
        if bl.landmarks.contains_key(k) {
            continue;
        }
        let keep = if bl.detail.contains_key(k) { DETAIL_RADIUS_M + 15.0 } else { DETAIL_RADIUS_M };
        if d < keep || inside.0 == Some(*k) {
            want.push((d, *k));
        }
    }
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    want.truncate(24);
    let want_set: HashSet<(TileId, u32)> = want.iter().map(|w| w.1).collect();
    // Drop details no longer wanted, and rebuild ones whose building has been set at a
    // new height since (finer terrain arrived): they come back below.
    let drop: Vec<(TileId, u32)> = bl
        .detail
        .iter()
        .filter(|(k, d)| {
            !want_set.contains(k)
                || bl.placed.get(k).is_some_and(|p| (p.base - d.placed.base).abs() > 0.05 || (p.low - d.placed.low).abs() > 0.3)
        })
        .map(|(k, _)| *k)
        .collect();
    for k in drop {
        if let Some(d) = bl.detail.remove(&k) {
            commands.entity(d.entity).despawn();
            for e in d.doors {
                commands.entity(e).despawn();
            }
            if let Some(i) = d.interior {
                commands.entity(i).despawn();
            }
            mats.remove(d.material.id());
            walk.remove_solid(solid_key(k));
            if let Some(t) = bl.tiles.get_mut(&k.0)
                && let TileState::Ready(b) = &t.state
                && let Some(ci) = chunk_of(b, k.1)
            {
                t.chunks[ci].hidden.retain(|&h| h != k.1);
            }
        }
    }
    // Add new ones (a few per tick).
    let mut added = 0;
    for (_, k) in want {
        if bl.detail.contains_key(&k) || added >= 3 {
            continue;
        }
        let Some(p) = bl.placed.get(&k).copied() else { continue };
        let Some(b) = bl.built(k.0) else { continue };
        let bld = &b.buildings[k.1 as usize];
        let g = exterior::shell(
            bld,
            &exterior::Options { detailed: true, roof_col: b.roof_col[k.1 as usize], street_edges: b.street[k.1 as usize].clone() },
        );
        let mesh = detail_mesh(&b, k.1, &p, &g);
        let mat = mats.add(material::new_material(false));
        let e = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(mat.clone()),
                GeoAnchor::new(p.origin),
                Transform::default(),
                Visibility::Visible,
            ))
            .id();
        let doors = interact::spawn_exterior_doors(&mut commands, &mut meshes, &imats, &b, k, &p);
        walk.add_solid(solid_key(k), interior::exterior_solid(&b, k.1, &p));
        bl.detail.insert(
            k,
            Detail {
                entity: e,
                material: mat,
                built: b.clone(),
                placed: p,
                interior: None,
                doors,
                interior_y: 0.0,
                interior_partial: false,
            },
        );
        if let Some(t) = bl.tiles.get_mut(&k.0)
            && let Some(ci) = chunk_of(&b, k.1)
            && !t.chunks[ci].hidden.contains(&k.1)
        {
            t.chunks[ci].hidden.push(k.1);
        }
        added += 1;
    }
}

/// The ground a building needs kept below its floor: its footprint a little inside the
/// walls, down to the ground floor (or the basement floor).
fn pad_of(b: &Built, bi: u32, base: f64) -> Option<crate::terrain::conform::Pad> {
    let bld = &b.buildings[bi as usize];
    if bld.parts.iter().all(|p| p.min_h > 0.5) || matches!(bld.kind, model::Kind::Canopy | model::Kind::Carport) {
        return None;
    }
    let inner = geom::inset(&bld.outline, 0.35);
    if inner.len() < 3 || geom::area(&inner) < 4.0 {
        return None;
    }
    let ring: Vec<(f64, f64)> = inner.iter().map(|p| b.local.ll(*p)).collect();
    let (mut s, mut w, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for &(la, lo) in &ring {
        s = s.min(la);
        n = n.max(la);
        w = w.min(lo);
        e = e.max(lo);
    }
    // Only humps under the floor: basements needn't be dug (the terrain is invisible
    // from below), and a deep dig shows as a crater around the walls.
    let depth = 0.15;
    Some(crate::terrain::conform::Pad { ring, bbox: (s, w, n, e), top: ((base - depth) * 20.0).round() as f32 / 20.0 })
}

/// Put landmark models in place of their buildings (and take them away again when the
/// file goes or the tile unloads). The folder is looked at again every 10 seconds.
#[allow(clippy::too_many_arguments)]
fn place_landmarks(
    mut bl: Option<ResMut<Buildings>>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    time: Res<Time>,
    mut timer: Local<(f32, f32)>,
    mut walk: ResMut<crate::walkable::Walkables>,
) {
    let Some(bl) = bl.as_deref_mut() else { return };
    timer.0 -= time.delta_secs();
    timer.1 -= time.delta_secs();
    if timer.0 > 0.0 {
        return;
    }
    timer.0 = 1.0;
    if timer.1 <= 0.0 {
        timer.1 = 10.0;
        bl.landmark_files = landmark::scan();
    }
    let mut want: Vec<((TileId, u32), String)> = Vec::new();
    if !bl.landmark_files.is_empty() {
        for (id, t) in &bl.tiles {
            let TileState::Ready(b) = &t.state else { continue };
            for (bi, bld) in b.buildings.iter().enumerate() {
                if let Some(f) = bl.landmark_files.get(&bld.id)
                    && bl.placed.contains_key(&(*id, bi as u32))
                {
                    want.push(((*id, bi as u32), f.clone()));
                }
            }
        }
    }
    let gone: Vec<(TileId, u32)> = bl.landmarks.keys().filter(|k| !want.iter().any(|w| w.0 == **k)).copied().collect();
    for k in gone {
        if let Some(e) = bl.landmarks.remove(&k) {
            commands.entity(e).despawn();
        }
        walk.remove_solid(solid_key(k) ^ 0x10);
        if let Some(t) = bl.tiles.get_mut(&k.0)
            && let TileState::Ready(b) = &t.state
            && let Some(ci) = chunk_of(b, k.1)
        {
            t.chunks[ci].hidden.retain(|&h| h != k.1);
        }
    }
    for (k, file) in want {
        if bl.landmarks.contains_key(&k) {
            continue;
        }
        let (Some(p), Some(b)) = (bl.placed.get(&k).copied(), bl.built(k.0)) else { continue };
        // A detailed version gives way to the model.
        if let Some(d) = bl.detail.remove(&k) {
            commands.entity(d.entity).despawn();
            for e in d.doors {
                commands.entity(e).despawn();
            }
            if let Some(i) = d.interior {
                commands.entity(i).despawn();
            }
            walk.remove_solid(solid_key(k));
        }
        let bld = &b.buildings[k.1 as usize];
        let rot = interior::local_to_ecef(&b, k.1, &p);
        let root = commands.spawn((GeoAnchor::new(p.origin), Transform::default(), Visibility::Visible)).id();
        let frame = commands.spawn((Transform::from_rotation(rot), Visibility::Inherited, ChildOf(root))).id();
        let handle = assets.load(bevy::gltf::GltfAssetLabel::Scene(0).from_asset(format!("landmarks://{file}")));
        commands.spawn((
            bevy::world_serialization::WorldAssetRoot(handle),
            landmark::Landmark { height: bld.height, fitted: false, tries: 0 },
            Transform::default(),
            Visibility::Inherited,
            ChildOf(frame),
        ));
        info!("landmark {} ({}) replaces building {}", file, bld.name.as_deref().unwrap_or("unnamed"), bld.id);
        // Walls to bump into (the model's doors aren't known: no way in).
        walk.add_solid(solid_key(k) ^ 0x10, interior::exterior_solid(&b, k.1, &p));
        bl.landmarks.insert(k, root);
        if let Some(t) = bl.tiles.get_mut(&k.0)
            && let Some(ci) = chunk_of(&b, k.1)
            && !t.chunks[ci].hidden.contains(&k.1)
        {
            t.chunks[ci].hidden.push(k.1);
        }
    }
}

pub fn solid_key(k: (TileId, u32)) -> u64 {
    0x7000_0000_0000_0000 ^ ((k.0.x as u64) << 40) ^ ((k.0.y as u64) << 20) ^ k.1 as u64
}

/// Distance from a point to a building's walls (0 inside).
pub fn footprint_distance(b: &Built, bld: &model::Building, p: &PlacedRef, ecef: DVec3) -> f64 {
    let g = Geodetic::from_ecef(ecef);
    let q = b.local.xz(g.lat, g.lon);
    if geom::contains(&bld.outline, q) {
        return 0.0;
    }
    let horiz = geom::dist_to_ring(&bld.outline, q) as f64;
    let dh = (g.h - p.base).max(0.0) - bld.height as f64;
    horiz.hypot(dh.max(0.0))
}

/// The building's shell as its own mesh, relative to its origin (ECEF axes).
pub fn detail_mesh(b: &Built, bi: u32, p: &PlacedRef, g: &exterior::BGeom) -> Mesh {
    let bld = &b.buildings[bi as usize];
    let cen = geom::centroid(&bld.outline);
    let (la, lo) = b.local.ll(cen);
    let to_ecef = usx_core::geo::EnuFrame::at(Geodetic::new(la, lo, p.base)).ecef_to_local.transpose();
    let mut pos = Vec::with_capacity(g.v.len());
    let mut nrm = Vec::with_capacity(g.v.len());
    for v in &g.v {
        let mut y = v.p.y as f64;
        if v.ground {
            y = (p.low - 0.3 - p.base).min(y);
        }
        let l = DVec3::new((v.p.x - cen.x) as f64, y, (v.p.z - cen.y) as f64);
        pos.push((to_ecef * l).as_vec3().to_array());
        nrm.push((to_ecef * v.n.as_dvec3()).as_vec3().to_array());
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, g.v.iter().map(|v| v.uv).collect::<Vec<_>>());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, g.v.iter().map(|v| v.uv2).collect::<Vec<_>>());
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, g.v.iter().map(|v| v.col).collect::<Vec<_>>());
    mesh.insert_indices(Indices::U32(g.i.clone()));
    mesh
}

fn update_material(
    bl: Option<Res<Buildings>>,
    day: Res<crate::sun::DayNight>,
    mut mats: ResMut<Assets<BuildingMaterial>>,
    mut last: Local<f32>,
) {
    let Some(bl) = bl else { return };
    if (*last - day.dark).abs() < 0.005 {
        return;
    }
    *last = day.dark;
    if let Some(mut m) = mats.get_mut(&bl.material) {
        m.extension.params.night = day.dark;
    }
    for d in bl.detail.values() {
        if let Some(mut m) = mats.get_mut(&d.material) {
            m.extension.params.night = day.dark;
        }
    }
}

fn stats(bl: Option<Res<Buildings>>, mut s: ResMut<BuildingStats>) {
    let Some(bl) = bl else { return };
    let mut o = BuildingStats::default();
    for t in bl.tiles.values() {
        match &t.state {
            TileState::Ready(b) => {
                o.tiles_ready += 1;
                o.buildings += b.buildings.len();
                o.chunks_drawn += t.chunks.iter().filter(|c| c.root.is_some()).count();
            }
            _ => o.tiles_loading += 1,
        }
    }
    o.detailed = bl.detail.len();
    o.interiors = bl.detail.values().filter(|d| d.interior.is_some()).count();
    *s = o;
}

/// Whether streaming has caught up nearby (for screenshots).
pub fn settled(bl: Option<&Buildings>) -> bool {
    let Some(b) = bl else { return true };
    !b.enabled || b.tiles.values().all(|t| !matches!(t.state, TileState::Fetching | TileState::Building))
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Real Overpass answers (buildings layer) from the `fixtures-buildings` branch:
    /// USX_BLD_FIXTURES=dir cargo test --release bld_fixtures -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bld_fixtures() {
        let dir = std::env::var("USX_BLD_FIXTURES").expect("USX_BLD_FIXTURES");
        let only = std::env::var("USX_ONLY").ok();
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
                continue;
            }
            let parts: Vec<u32> = name.split('-').map(|x| x.parse().unwrap()).collect();
            let tile = TileId { z: parts[0] as u8, x: parts[1], y: parts[2] };
            let json = std::fs::read(&path).unwrap();
            let osm = osm::parse_layer(&json, Layer::Buildings).unwrap();
            let t0 = std::time::Instant::now();
            let b = build::build(tile, &osm, None, None);
            let t_build = t0.elapsed();
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for bl in &b.buildings {
                *kinds.entry(format!("{:?}", bl.kind)).or_default() += 1;
            }
            for c in &b.chunks {
                for v in &c.geom.v {
                    assert!(v.p.is_finite() && v.n.is_finite(), "non-finite shell vertex");
                }
                for &i in &c.geom.i {
                    assert!((i as usize) < c.geom.v.len());
                }
            }
            println!(
                "{name}: {} buildings, {} parts, {} chunks, {} vertices, built in {:.0} ms\n  kinds {kinds:?}",
                b.buildings.len(),
                b.buildings.iter().map(|x| x.parts.len()).sum::<usize>(),
                b.chunks.len(),
                b.vertices,
                t_build.as_secs_f64() * 1000.0
            );
            // Detailed shells.
            let mut bad = 0;
            for (bi, bl) in b.buildings.iter().enumerate() {
                let g = exterior::shell(
                    bl,
                    &exterior::Options { detailed: true, roof_col: b.roof_col[bi], street_edges: b.street[bi].clone() },
                );
                let ok = g.v.iter().all(|v| v.p.is_finite()) && g.i.iter().all(|&i| (i as usize) < g.v.len()) && !g.i.is_empty();
                if !ok {
                    bad += 1;
                }
            }
            println!("  detailed shells: {bad} bad");
            // Interiors for every building.
            let (mut ok, mut none, mut tris, mut worst) = (0usize, 0usize, 0usize, (0.0f64, String::new()));
            let mut total = 0.0;
            let mut rooms: BTreeMap<String, usize> = BTreeMap::new();
            let only_b: Option<i64> = std::env::var("USX_BID").ok().and_then(|v| v.parse().ok());
            for (bi, bl) in b.buildings.iter().enumerate() {
                if only_b.is_some_and(|x| x != bl.id) {
                    continue;
                }
                let cen = geom::centroid(&bl.outline);
                let (la, lo) = b.local.ll(cen);
                let p = PlacedRef { tile, b: bi as u32, base: 100.0, low: 99.0, origin: Geodetic::new(la, lo, 100.0).to_ecef() };
                let t = std::time::Instant::now();
                let r = interior::realise(&b, bi as u32, &p, 1.0);
                let dt = t.elapsed().as_secs_f64() * 1000.0;
                total += dt;
                if dt > worst.0 {
                    worst = (dt, format!("{:?} {:.0} m² {} levels id {}", bl.kind, bl.area, bl.levels, bl.id));
                }
                let _ = interior::exterior_solid(&b, bi as u32, &p);
                match r {
                    Some(d) => {
                        ok += 1;
                        if only_b.is_some() {
                            for sp in &d.specs {
                                if let furniture::Spec::Mirror { centre, yaw, .. } = sp {
                                    let (mla, mlo) = b.local.ll(cen + Vec2::new(centre.x, centre.z));
                                    println!(
                                        "    mirror at {mla:.7},{mlo:.7} y {:.2} facing yaw {:.2} (heading {:.0})",
                                        centre.y,
                                        yaw,
                                        (-yaw.to_degrees()).rem_euclid(360.0)
                                    );
                                }
                            }
                        }
                        for mb in d.kit.parts.values() {
                            tris += mb.idx.len() / 3;
                            for q in &mb.pos {
                                assert!(q.iter().all(|x| x.is_finite()), "non-finite interior vertex in {}", bl.id);
                            }
                            for &i in &mb.idx {
                                assert!((i as usize) < mb.pos.len());
                            }
                        }
                        for f in &d.floors {
                            assert!(f.h.is_finite());
                        }
                    }
                    None => {
                        none += 1;
                        if none <= 6 {
                            println!(
                                "    no interior: {:?} {:.0} m² {} parts {} levels {} pts min_h {:.1} id {}",
                                bl.kind,
                                bl.area,
                                bl.parts.len(),
                                bl.levels,
                                bl.outline.len(),
                                bl.parts.iter().map(|q| q.min_h).fold(f32::MAX, f32::min),
                                bl.id
                            );
                        }
                    }
                }
                if let Some(pl) = plan::plan(bl, &exterior::doors(bl, &b.street[bi]), &[]) {
                    for f in &pl.floors {
                        for r in &f.rooms {
                            *rooms.entry(format!("{:?}", r.kind)).or_default() += 1;
                        }
                    }
                }
            }
            println!(
                "  interiors: {ok} built, {none} without, {} k triangles, {:.1} ms total, worst {:.1} ms ({})\n  rooms {rooms:?}",
                tris / 1000,
                total,
                worst.0,
                worst.1
            );
        }
    }
}
