//! Roads, bridges, tunnels, railways and roadside objects from OpenStreetMap.
//!
//! Pipeline per zoom-14 tile (about 2 km): fetch (Overpass, disk-cached) -> build
//! (graph, junctions, profiles, flat geometry, props, sign art) -> per ~600 m chunk:
//! drape onto the terrain on screen -> mesh. Chunks re-drape whenever the terrain under
//! them changes, and the terrain itself is levelled under the roads (`terrain::conform`).

pub mod build;
pub mod classify;
pub mod drape;
pub mod material;
pub mod parking;
pub mod props;
pub mod signs;

use crate::globe::{GeoAnchor, GlobeSet, Viewer};
use crate::terrain::Terrain;
use crate::walkable::{Post, Walkables};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use build::{Built, Corridor};
use material::{RoadExt, RoadMaterial, RoadParams};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock, RwLock};
use usx_core::geo::Geodetic;
use usx_core::net::WorkPool;
use usx_core::osm::{self, OsmTile};
use usx_core::tile::TileId;

const UNLOAD_RADIUS_M: f64 = 4_500.0;

/// (load roads within, draw road meshes within, roadside objects within) in metres, by
/// graphics preset. Imagery shows roads further out.
fn ranges(p: usx_core::settings::Preset) -> (f64, f64, f64) {
    use usx_core::settings::Preset;
    match p {
        Preset::Low => (1_300.0, 1_400.0, 320.0),
        Preset::Medium => (1_800.0, 2_000.0, 480.0),
        Preset::High => (2_200.0, 2_600.0, 650.0),
        Preset::Ultra => (2_600.0, 3_200.0, 900.0),
    }
}
/// Above this height over the ground, roads aren't loaded.
const MAX_AGL_M: f64 = 1_500.0;

// ------------------------------------------------------------------ corridors

/// What a road tile asks of the terrain: corridors to level and lakes to dig out.
pub type ConformData = (Vec<Corridor>, Vec<build::Lake>);
type CorridorMap = (HashMap<TileId, Arc<ConformData>>, VecDeque<TileId>);

fn corridors() -> &'static RwLock<CorridorMap> {
    static C: OnceLock<RwLock<CorridorMap>> = OnceLock::new();
    C.get_or_init(|| RwLock::new((HashMap::new(), VecDeque::new())))
}

/// Road corridors and lakes covering a terrain tile, once that area's roads are known.
/// Includes the neighbouring road tiles' corridors that reach into it (roads just
/// across a tile border are wide enough to matter).
pub fn corridors_for(id: TileId) -> Option<Arc<ConformData>> {
    if id.z < osm::ZOOM {
        return None;
    }
    let m = corridors().read().ok()?;
    let own = id.ancestor(osm::ZOOM);
    let base = m.0.get(&own)?.clone();
    let (w, s, e, n) = id.bounds();
    let pad = 0.0006;
    let hits = |b: (f64, f64, f64, f64)| !(b.0 > n + pad || b.2 < s - pad || b.1 > e + pad || b.3 < w - pad);
    let mut out: Option<ConformData> = None;
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let (x, y) = (own.x as i64 + dx, own.y as i64 + dy);
            if x < 0 || y < 0 {
                continue;
            }
            let Some(other) = m.0.get(&TileId::new(own.z, x as u32, y as u32)) else { continue };
            for c in other.0.iter().filter(|c| hits(c.bbox)) {
                out.get_or_insert_with(|| (base.0.clone(), base.1.clone())).0.push(c.clone());
            }
            for l in other.1.iter().filter(|l| hits(l.bbox)) {
                out.get_or_insert_with(|| (base.0.clone(), base.1.clone())).1.push(l.clone());
            }
        }
    }
    Some(out.map(Arc::new).unwrap_or(base))
}

fn set_corridors(id: TileId, c: Arc<ConformData>) {
    let Ok(mut m) = corridors().write() else { return };
    if m.0.insert(id, c).is_none() {
        m.1.push_back(id);
    }
    while m.1.len() > 400 {
        if let Some(old) = m.1.pop_front() {
            m.0.remove(&old);
        }
    }
}

// ------------------------------------------------------------------ state

/// A built tile plus its sign art, shared with drape jobs.
pub struct Prepared {
    pub built: Built,
    /// Per chunk: sign atlas, and per prop its face rectangles in it.
    pub atlases: Vec<Option<signs::Atlas>>,
    pub faces: Vec<Vec<Vec<([f32; 4], f32, f32)>>>,
}

enum Job {
    Build(TileId, OsmTile),
    Drape(Arc<Prepared>, TileId, usize, crate::terrain::HeightSnap, u64),
}

enum Out {
    Built(TileId, Box<Prepared>),
    Draped(TileId, usize, u64, Option<drape::Draped>),
}

#[derive(Default)]
struct ChunkState {
    root: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    center: DVec3,
    /// Terrain signature of the current drape, and of one in flight.
    sig: u64,
    pending: Option<u64>,
    /// Prop placements from the last drape (relative to `center`).
    placements: Vec<(usize, Vec3, Quat)>,
    props: Vec<Entity>,
    atlas_mat: Option<Handle<StandardMaterial>>,
    /// Distance from the viewer at the last check.
    dist: f64,
}

enum TileState {
    Fetching,
    Building,
    Ready(Arc<Prepared>),
    Failed { retry_at: f64 },
}

struct RoadTile {
    state: TileState,
    /// Within a kilometre of the viewer (screenshots wait for these).
    near: bool,
    chunks: Vec<ChunkState>,
    last_wanted: f64,
}

#[derive(Resource)]
pub struct Roads {
    tiles: HashMap<TileId, RoadTile>,
    fetch: WorkPool<TileId, (TileId, Result<OsmTile, String>)>,
    work: WorkPool<Job, Out>,
    material: Handle<RoadMaterial>,
    pub enabled: bool,
    pub last_error: Option<String>,
    /// Round-robin cursor over chunks for drape checks.
    cursor: usize,
}

/// Lakes and rivers of the loaded road tiles, for swimming.
#[derive(Resource, Default)]
pub struct InlandWater {
    tiles: HashMap<TileId, Vec<usx_core::osm::Area>>,
}

fn in_ring(r: &[(f64, f64)], lat: f64, lon: f64) -> bool {
    let mut c = false;
    let n = r.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a.0 > lat) != (b.0 > lat) && lon < (b.1 - a.1) * (lat - a.0) / (b.0 - a.0) + a.1 {
            c = !c;
        }
        j = i;
    }
    c
}

impl InlandWater {
    /// Whether a point is on a lake, reservoir or river area.
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        let (id, _, _) = TileId::containing(lat, lon, osm::ZOOM);
        self.tiles.get(&id).is_some_and(|areas| {
            areas.iter().any(|a| a.outer.iter().any(|r| in_ring(r, lat, lon)) && !a.inner.iter().any(|r| in_ring(r, lat, lon)))
        })
    }
}

/// Numbers for the debug overlay.
#[derive(Resource, Default, Clone, Copy)]
pub struct RoadStats {
    pub tiles_ready: usize,
    pub tiles_loading: usize,
    pub chunks_drawn: usize,
    pub props: usize,
    pub jobs: usize,
}

pub struct RoadsPlugin;

impl Plugin for RoadsPlugin {
    fn build(&self, app: &mut App) {
        let fetch = WorkPool::new("osm", 3, |_, id: TileId| {
            let r = osm::load(id, crate::terrain::load::cache()).map_err(|e| e.to_string());
            (id, r)
        });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 6) / 2;
        let work = WorkPool::new("roads", threads.max(1), |_, job: Job| match job {
            Job::Build(id, osm) => Out::Built(id, Box::new(prepare(id, &osm))),
            Job::Drape(p, id, chunk, snap, sig) => Out::Draped(id, chunk, sig, drape::drape(&p.built, chunk, &snap)),
        });
        app.add_plugins(material::RoadMaterialPlugin)
            .init_resource::<RoadStats>()
            .init_resource::<props::Controllers>()
            .init_resource::<InlandWater>()
            .insert_resource(RoadsHolder(Some((fetch, work))))
            .add_systems(Startup, (setup, props::setup, props::setup_lamp_lights))
            .add_systems(
                Update,
                (want_tiles, receive, schedule_drapes, manage_props, update_material).chain().after(crate::terrain::TerrainSet),
            )
            .add_systems(Update, (props::run_signals, props::run_lamps))
            .add_systems(PostUpdate, keep_anchors_fresh.before(GlobeSet));
    }
}

/// Pools are created in `build` but the material needs `Assets`, so the resource is
/// finished in `setup`.
#[derive(Resource)]
struct RoadsHolder(Option<(WorkPool<TileId, (TileId, Result<OsmTile, String>)>, WorkPool<Job, Out>)>);

fn setup(
    mut commands: Commands,
    mut holder: ResMut<RoadsHolder>,
    mut mats: ResMut<Assets<RoadMaterial>>,
    settings: Res<crate::GameSettings>,
) {
    let Some((fetch, work)) = holder.0.take() else { return };
    let material = mats.add(RoadMaterial {
        base: StandardMaterial {
            perceptual_roughness: 0.9,
            reflectance: 0.3,
            cull_mode: None,
            double_sided: true,
            // Win depth ties with the terrain under it, by a margin that grows with
            // distance (coarse terrain far away is only roughly levelled).
            depth_bias: 30_000.0,
            ..default()
        },
        extension: RoadExt { params: RoadParams::default() },
    });
    commands.insert_resource(Roads {
        tiles: HashMap::new(),
        fetch,
        work,
        material,
        enabled: settings.0.roads,
        last_error: None,
        cursor: 0,
    });
}

/// Build a tile and paint its signs (worker thread).
fn prepare(id: TileId, osm: &OsmTile) -> Prepared {
    // Elevation tiles that failed (offline) aren't asked for again during this build.
    let failed = std::cell::RefCell::new(std::collections::HashSet::new());
    let dem = |lat: f64, lon: f64| {
        let (t, _, _) = TileId::containing(lat, lon, usx_core::sources::TERRARIUM_MAX_ZOOM);
        if failed.borrow().contains(&t) {
            return None;
        }
        let h = crate::terrain::load::dem_height(lat, lon);
        if h.is_none() {
            failed.borrow_mut().insert(t);
        }
        h
    };
    let built = build::build(id, osm, &dem);
    let mut atlases = Vec::with_capacity(built.chunks.len());
    let mut faces = Vec::with_capacity(built.chunks.len());
    for c in &built.chunks {
        let mut all = Vec::new();
        let mut owner = Vec::with_capacity(c.props.len());
        for p in &c.props {
            let f = props::custom_faces(&p.kind);
            owner.push((all.len(), f.len()));
            all.extend(f);
        }
        let atlas = signs::pack(all);
        let rects: Vec<Vec<([f32; 4], f32, f32)>> =
            owner.iter().map(|&(start, n)| atlas.as_ref().map(|a| a.rects[start..start + n].to_vec()).unwrap_or_default()).collect();
        atlases.push(atlas);
        faces.push(rects);
    }
    Prepared { built, atlases, faces }
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
    mut roads: Option<ResMut<Roads>>,
    viewer: Res<Viewer>,
    time: Res<Time>,
    mut commands: Commands,
    mut walk: ResMut<Walkables>,
    mut controllers: ResMut<props::Controllers>,
    mut water: ResMut<InlandWater>,
) {
    let Some(roads) = roads.as_deref_mut() else { return };
    roads.enabled = settings.0.roads;
    let (load_radius, _, _) = ranges(settings.0.preset);
    let now = time.elapsed_secs_f64();
    let g = viewer.geo();
    let low = viewer.agl().is_some_and(|a| a < MAX_AGL_M);
    if roads.enabled && low {
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
            let t = roads.tiles.entry(id).or_insert_with(|| RoadTile {
                state: TileState::Failed { retry_at: 0.0 },
                near: false,
                chunks: Vec::new(),
                last_wanted: now,
            });
            t.last_wanted = now;
            t.near = d < 1_000.0;
            if let TileState::Failed { retry_at } = t.state
                && now >= retry_at
            {
                t.state = TileState::Fetching;
                roads.fetch.submit(id_key(id), d as f32, id);
            } else if matches!(t.state, TileState::Fetching) {
                roads.fetch.reprioritize(id_key(id), d as f32);
            }
        }
    }
    // Unload far tiles.
    let far: Vec<TileId> = roads
        .tiles
        .iter()
        .filter(|(id, t)| tile_distance(viewer.ecef, **id) > UNLOAD_RADIUS_M || (!roads.enabled) || now - t.last_wanted > 120.0)
        .map(|(id, _)| *id)
        .collect();
    for id in far {
        water.tiles.remove(&id);
        if let Some(t) = roads.tiles.remove(&id) {
            roads.fetch.cancel(id_key(id));
            if let TileState::Ready(p) = &t.state {
                for (i, c) in t.chunks.iter().enumerate() {
                    if let Some(r) = c.root {
                        commands.entity(r).despawn();
                    }
                    walk.remove_owner(drape::chunk_owner(&p.built, i));
                    walk.remove_posts(drape::chunk_owner(&p.built, i));
                }
                for ctl in &p.built.controllers {
                    controllers.0.remove(&ctl.id);
                }
            }
        }
    }
}

fn id_key(id: TileId) -> u64 {
    ((id.x as u64) << 32) | id.y as u64
}

fn receive(
    mut roads: Option<ResMut<Roads>>,
    time: Res<Time>,
    mut controllers: ResMut<props::Controllers>,
    mut water: ResMut<InlandWater>,
    mut parts: DrapeApply,
) {
    let Some(roads) = roads.as_deref_mut() else { return };
    let now = time.elapsed_secs_f64();
    while let Some((id, r)) = roads.fetch.try_recv() {
        let Some(t) = roads.tiles.get_mut(&id) else { continue };
        match r {
            Ok(osm) => {
                t.state = TileState::Building;
                roads.work.submit(id_key(id) ^ 0x8000_0000_0000_0000, 0.0, Job::Build(id, osm));
            }
            Err(e) => {
                warn!("roads {}: {e}", id.path());
                roads.last_error = Some(e);
                t.state = TileState::Failed { retry_at: now + 20.0 };
            }
        }
    }
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < 0.006 {
        let Some(out) = roads.work.try_recv() else { break };
        match out {
            Out::Built(id, p) => {
                let Some(t) = roads.tiles.get_mut(&id) else { continue };
                info!(
                    "roads {}: {} ways, {} junctions, {} bridges, {} tunnels, {} props, {} vertices",
                    id.path(),
                    p.built.stats.ways,
                    p.built.stats.junctions,
                    p.built.stats.bridges,
                    p.built.stats.tunnels,
                    p.built.stats.props,
                    p.built.stats.vertices
                );
                set_corridors(id, Arc::new((p.built.corridors.clone(), p.built.lakes.clone())));
                if !p.built.water.is_empty() {
                    water.tiles.insert(id, p.built.water.clone());
                }
                for c in &p.built.controllers {
                    controllers.0.insert(c.id, (c.groups, c.cycle_offset));
                }
                t.chunks = (0..p.built.chunks.len()).map(|_| ChunkState::default()).collect();
                t.state = TileState::Ready(Arc::from(p));
            }
            Out::Draped(id, ci, sig, d) => {
                let Some(t) = roads.tiles.get_mut(&id) else { continue };
                let TileState::Ready(p) = &t.state else { continue };
                let p = p.clone();
                let Some(c) = t.chunks.get_mut(ci) else { continue };
                c.pending = None;
                let Some(d) = d else { continue };
                c.sig = sig;
                parts.apply(&p, ci, c, d, roads.material.clone());
            }
        }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct DrapeApply<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    walk: ResMut<'w, Walkables>,
    anchors: Query<'w, 's, &'static mut GeoAnchor>,
    transforms: Query<'w, 's, &'static mut Transform, With<props::RoadProp>>,
}

impl DrapeApply<'_, '_> {
    fn apply(&mut self, p: &Prepared, ci: usize, c: &mut ChunkState, d: drape::Draped, material: Handle<RoadMaterial>) {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, d.uv0);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, d.uv1);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, d.color);
        mesh.insert_indices(Indices::U32(d.indices));
        match (&c.mesh, c.root) {
            (Some(_), Some(root)) => {
                // A fresh mesh each time; the old one is freed when its last handle
                // goes (removing it here raced the renderer).
                let h = self.meshes.add(mesh);
                self.commands.entity(root).insert(Mesh3d(h.clone()));
                c.mesh = Some(h);
                if let Ok(mut a) = self.anchors.get_mut(root) {
                    a.ecef = d.center;
                    a.placed_generation = 0;
                }
            }
            _ => {
                let h = self.meshes.add(mesh);
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
                c.mesh = Some(h);
                c.root = Some(root);
            }
        }
        c.center = d.center;
        // Props move with the new drape.
        c.placements = d.props;
        if !c.props.is_empty() {
            for (k, &(_, t, r)) in c.placements.iter().enumerate() {
                if let Some(&e) = c.props.get(k)
                    && let Ok(mut tr) = self.transforms.get_mut(e)
                {
                    tr.translation = t;
                    tr.rotation = r;
                }
            }
        }
        let owner = drape::chunk_owner(&p.built, ci);
        self.walk.remove_owner(owner);
        for s in d.strips {
            self.walk.add(s);
        }
    }
}

fn schedule_drapes(
    settings: Res<crate::GameSettings>,
    mut roads: Option<ResMut<Roads>>,
    terrain: Res<Terrain>,
    viewer: Res<Viewer>,
    mut commands: Commands,
    mut walk: ResMut<Walkables>,
) {
    let Some(roads) = roads.as_deref_mut() else { return };
    let (_, drape_radius, _) = ranges(settings.0.preset);
    // Check a slice of chunks each frame; submit at most a few drapes.
    let mut list: Vec<(TileId, usize)> = Vec::new();
    for (id, t) in roads.tiles.iter() {
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
        let (id, ci) = list[(roads.cursor + k) % list.len()];
        let Some(t) = roads.tiles.get_mut(&id) else { continue };
        let TileState::Ready(p) = &t.state else { continue };
        let p = p.clone();
        let chunk = &p.built.chunks[ci];
        let c = &mut t.chunks[ci];
        let d = tile_distance(viewer.ecef, chunk.id);
        c.dist = d;
        if d > drape_radius + 300.0 {
            // Out of range: drop the mesh (props go with it).
            if let Some(r) = c.root.take() {
                commands.entity(r).despawn();
                c.mesh = None;
                c.props.clear();
                c.sig = 0;
                walk.remove_owner(drape::chunk_owner(&p.built, ci));
                walk.remove_posts(drape::chunk_owner(&p.built, ci));
            }
            continue;
        }
        if d > drape_radius || c.pending.is_some() || submitted >= 6 || chunk.geom.v.is_empty() && chunk.props.is_empty() {
            continue;
        }
        let sig = terrain.signature(chunk.influence);
        if sig == c.sig || sig == 0xcbf2_9ce4_8422_2325 {
            continue;
        }
        let snap = terrain.snapshot(chunk.influence);
        if snap.is_empty() {
            continue;
        }
        c.pending = Some(sig);
        roads.work.submit(id_key(id) ^ ((ci as u64 + 1) << 56), d as f32, Job::Drape(p.clone(), id, ci, snap, sig));
        submitted += 1;
    }
    roads.cursor = (roads.cursor + budget) % list.len().max(1);
}

#[allow(clippy::too_many_arguments)]
fn manage_props(
    settings: Res<crate::GameSettings>,
    mut roads: Option<ResMut<Roads>>,
    mut commands: Commands,
    mut assets: Option<ResMut<props::PropAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut walk: ResMut<Walkables>,
    mut stats: ResMut<RoadStats>,
) {
    let (Some(roads), Some(assets)) = (roads.as_deref_mut(), assets.as_deref_mut()) else { return };
    let (_, _, prop_radius) = ranges(settings.0.preset);
    let mut spawned_chunks = 0;
    let (mut ready, mut loading, mut drawn, mut nprops) = (0, 0, 0, 0);
    for (_, t) in roads.tiles.iter_mut() {
        let TileState::Ready(p) = &t.state else {
            loading += 1;
            continue;
        };
        ready += 1;
        let p = p.clone();
        for (ci, c) in t.chunks.iter_mut().enumerate() {
            if c.root.is_some() {
                drawn += 1;
            }
            nprops += c.props.len();
            let Some(root) = c.root else { continue };
            let want = c.dist < prop_radius;
            let drop = c.dist > prop_radius + 150.0;
            if want && c.props.is_empty() && !c.placements.is_empty() && spawned_chunks < 2 {
                spawned_chunks += 1;
                if c.atlas_mat.is_none()
                    && let Some(a) = &p.atlases[ci]
                {
                    let img = images.add(props::atlas_image(a));
                    c.atlas_mat = Some(mats.add(props::sign_material(img)));
                }
                let chunk = &p.built.chunks[ci];
                let owner = drape::chunk_owner(&p.built, ci);
                for &(pi, tr, rot) in &c.placements {
                    let prop = &chunk.props[pi];
                    let t = Transform::from_translation(tr).with_rotation(rot);
                    let e = props::spawn(
                        &mut commands,
                        assets,
                        &mut meshes,
                        root,
                        &prop.kind,
                        prop.id,
                        t,
                        &p.faces[ci][pi],
                        c.atlas_mat.as_ref(),
                    );
                    c.props.push(e);
                    // Collision posts.
                    for (off, r, h) in props::solids(&prop.kind) {
                        let world = c.center + (tr + rot * Vec3::new(off.x, 0.0, off.y)).as_dvec3();
                        let g = Geodetic::from_ecef(world);
                        walk.add_post(Post { lat: g.lat, lon: g.lon, radius: r as f64, h0: g.h - 0.2, h1: g.h + h as f64 }, owner);
                    }
                }
            } else if drop && !c.props.is_empty() {
                for e in c.props.drain(..) {
                    commands.entity(e).despawn();
                }
                walk.remove_posts(drape::chunk_owner(&p.built, ci));
            }
        }
    }
    *stats = RoadStats {
        tiles_ready: ready,
        tiles_loading: loading,
        chunks_drawn: drawn,
        props: nprops,
        jobs: roads.work.waiting() + roads.work.busy(),
    };
}

fn update_material(
    roads: Option<Res<Roads>>,
    day: Res<crate::sun::DayNight>,
    mut mats: ResMut<Assets<RoadMaterial>>,
    time: Res<Time>,
    mut last: Local<f32>,
) {
    let Some(roads) = roads else { return };
    // Water ripples move with time; a few updates a second would look steppy.
    let t = (time.elapsed_secs_f64() % 2000.0) as f32;
    let _ = &mut last;
    if let Some(mut m) = mats.get_mut(&roads.material) {
        m.extension.params.night = day.dark;
        m.extension.params.time = t;
    }
}

/// Props are children of the chunk root, so they follow its anchor automatically.
fn keep_anchors_fresh() {}

/// Whether streaming has caught up (for screenshots).
pub fn settled(roads: Option<&Roads>) -> bool {
    let Some(r) = roads else { return true };
    if !r.enabled {
        return true;
    }
    r.tiles
        .values()
        .filter(|t| t.near)
        .all(|t| !matches!(t.state, TileState::Fetching | TileState::Building) && t.chunks.iter().all(|c| c.pending.is_none()))
}
