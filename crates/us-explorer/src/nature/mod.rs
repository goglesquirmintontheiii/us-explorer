//! Vegetation, water and the details between roads and buildings.
//!
//! Streaming works in two sizes: the nature layer of OpenStreetMap comes in zoom-15
//! tiles (like roads and buildings); vegetation is worked out in zoom-17 cells (about
//! 300 m) from the aerial imagery and the map, then draped on the ground being drawn.
//! Close trees are full 3D models (two levels of detail), further ones flat impostor
//! cards; below the default graphics preset every tree is a card.

pub mod conditions;
pub mod cover;
pub mod details;
pub mod features;
pub mod grass;
pub mod ndvi;
mod ndvi_palette;
pub mod nature_osm;
pub mod species;
pub mod sprites;
pub mod treegen;
pub mod trees;

use crate::globe::{GeoAnchor, GlobeSet, Viewer};
use crate::terrain::{HeightSnap, Terrain};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilityRange;
use bevy::math::{DMat3, DQuat, DVec3};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;
use cover::{CellData, CellInput};
use nature_osm::NatureData;
use species::Sp;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use trees::{BillboardMaterial, ProtoKey, SpriteInfo, TreeAssets, VegMaterial};
use usx_core::geo::{EnuFrame, Geodetic};
use usx_core::net::WorkPool;
use usx_core::osm::{self, Layer, OsmTile};
use usx_core::settings::Preset;
use usx_core::tile::TileId;

/// Full 3D trees and grass blades from the default preset up; flat cards below it.
pub fn three_d(p: Preset) -> bool {
    matches!(p, Preset::High | Preset::Ultra)
}

/// Distances (metres) by preset.
#[derive(Clone, Copy, Debug)]
pub struct Ranges {
    /// Impostor cards out to here.
    pub cards: f32,
    /// Close-up 3D trees within this.
    pub full: f32,
    /// Simpler 3D trees out to here (cards beyond).
    pub low: f32,
    /// Grass around you.
    pub grass: f32,
}

pub fn ranges(p: Preset) -> Ranges {
    match p {
        Preset::Low => Ranges { cards: 650.0, full: 0.0, low: 0.0, grass: 18.0 },
        Preset::Medium => Ranges { cards: 1000.0, full: 0.0, low: 0.0, grass: 26.0 },
        Preset::High => Ranges { cards: 1500.0, full: 55.0, low: 170.0, grass: 40.0 },
        Preset::Ultra => Ranges { cards: 2200.0, full: 80.0, low: 260.0, grass: 60.0 },
    }
}

/// Above this height over the ground vegetation isn't loaded.
const MAX_AGL_M: f64 = 2_500.0;

enum NatState {
    Fetching,
    Sorting,
    Ready(Arc<NatureData>),
    Failed { retry_at: f64 },
}

struct NatTile {
    state: NatState,
    last_wanted: f64,
}

enum CellState {
    Waiting { since: f64 },
    Building,
    Ready(Arc<CellData>),
}

struct Cell {
    state: CellState,
    root: Option<Entity>,
    /// 3D tree entities (when close enough).
    trees: Vec<Entity>,
    billboard: Option<(Entity, Handle<Mesh>)>,
    /// Terrain signature and prototype generation of the current drape.
    sig: u64,
    pgen: u64,
    pending: bool,
    with_3d: bool,
    dist: f64,
    last_wanted: f64,
    species: HashSet<Sp>,
}

struct DrapedCell {
    center: DVec3,
    /// Per tree: translation, rotation (ECEF axes, relative to `center`), scale.
    trees: Vec<(usize, Vec3, Quat, f32)>,
    bb: Option<Mesh>,
}

enum Job {
    Sort(TileId, OsmTile),
    Cell(Box<CellInput>),
    Drape(Arc<CellData>, HeightSnap, u64, SpriteInfo, u64),
}

enum Out {
    Sorted(TileId, NatureData),
    Cell(TileId, Arc<CellData>),
    Draped(TileId, u64, u64, Option<DrapedCell>),
}

/// Numbers for the debug overlay.
#[derive(Resource, Default, Clone, Copy)]
pub struct NatureStats {
    pub tiles: usize,
    pub cells: usize,
    pub cells_building: usize,
    pub trees: usize,
    pub trees_3d: usize,
    pub species: usize,
}

#[derive(Resource)]
pub struct Nature {
    tiles: HashMap<TileId, NatTile>,
    fetch: WorkPool<TileId, (TileId, Result<OsmTile, String>)>,
    work: WorkPool<Job, Out>,
    cells: HashMap<TileId, Cell>,
    pub enabled: bool,
    timer: f32,
    /// Season of the prototypes in use (refreshed every few game hours), and where.
    season_check: f64,
    season_at: Option<(f64, f64)>,
    /// Satellite greenness where you are (lat, lon it was for), and a fetch underway.
    pub greenness: Option<(f64, f64, Option<ndvi::Greenness>)>,
    green_rx: Option<crossbeam_channel::Receiver<(f64, f64, Option<ndvi::Greenness>)>>,
}

impl Nature {
    /// The sorted nature layer for a zoom-15 tile, if loaded.
    pub fn tile(&self, id: TileId) -> Option<Arc<NatureData>> {
        match &self.tiles.get(&id)?.state {
            NatState::Ready(d) => Some(d.clone()),
            _ => None,
        }
    }
    /// Loaded vegetation cells (for grass).
    pub fn cell(&self, id: TileId) -> Option<&Arc<CellData>> {
        match &self.cells.get(&id)?.state {
            CellState::Ready(d) => Some(d),
            _ => None,
        }
    }
    /// Every loaded nature tile.
    pub fn ready_tiles(&self) -> impl Iterator<Item = (&TileId, &Arc<NatureData>)> {
        self.tiles.iter().filter_map(|(k, t)| if let NatState::Ready(d) = &t.state { Some((k, d)) } else { None })
    }
}

pub struct NaturePlugin;

impl Plugin for NaturePlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "tree.wgsl");
        bevy::asset::embedded_asset!(app, "billboard.wgsl");
        let fetch = WorkPool::new("nature-osm", 2, |_, id: TileId| {
            let r = osm::load_layer(id, Layer::Nature, crate::terrain::load::cache()).map_err(|e| e.to_string());
            (id, r)
        });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8) / 2;
        let work = WorkPool::new("nature", threads.max(1), |_, job: Job| match job {
            Job::Sort(id, osm) => Out::Sorted(id, nature_osm::sort(id, &osm)),
            Job::Cell(input) => {
                let id = input.id;
                Out::Cell(id, Arc::new(cover::build(*input)))
            }
            Job::Drape(cell, snap, sig, info, pgen) => Out::Draped(cell.id, sig, pgen, drape(&cell, &snap, &info)),
        });
        let enabled = app.world().resource::<crate::GameSettings>().0.vegetation;
        app.add_plugins((MaterialPlugin::<VegMaterial>::default(), MaterialPlugin::<BillboardMaterial>::default()))
            .init_resource::<NatureStats>()
            .insert_resource(Nature {
                tiles: HashMap::new(),
                fetch,
                work,
                cells: HashMap::new(),
                enabled,
                timer: 0.0,
                season_check: -1e18,
                season_at: None,
                greenness: None,
                green_rx: None,
            })
            .add_plugins((conditions::ConditionsPlugin, grass::GrassPlugin, details::DetailsPlugin))
            .add_systems(Startup, trees::setup)
            .add_systems(
                Update,
                (season, want, receive, trees::receive, build_cells, schedule_drapes, update_materials)
                    .chain()
                    .after(crate::terrain::TerrainSet)
                    .before(GlobeSet),
            );
    }
}

/// The season where you are: prototypes are regrown when it changes noticeably.
#[allow(clippy::too_many_arguments)]
fn season(
    mut nat: ResMut<Nature>,
    mut ta: Option<ResMut<TreeAssets>>,
    clock: Res<crate::sun::SimClock>,
    viewer: Res<Viewer>,
    settings: Res<crate::GameSettings>,
    args: Res<crate::Args>,
    mut commands: Commands,
) {
    let Some(ta) = ta.as_deref_mut() else { return };
    let t = clock.time();
    let full = three_d(settings.0.preset);
    let g = viewer.geo();
    // Satellite greenness: fetched once per area in the background.
    if let Some(rx) = &nat.green_rx
        && let Ok(r) = rx.try_recv()
    {
        info!("satellite greenness: {:?}", r.2);
        nat.greenness = Some(r);
        nat.green_rx = None;
        nat.season_check = -1e18;
    }
    let far = nat.greenness.is_none_or(|(la, lo, _)| (la - g.lat).abs() > 1.0 || (lo - g.lon).abs() > 1.0);
    if far && nat.green_rx.is_none() && args.shots.is_empty() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (la, lo) = (g.lat, g.lon);
        std::thread::spawn(move || {
            let _ = tx.send((la, lo, ndvi::fetch(la, lo, t)));
        });
        nat.green_rx = Some(rx);
    }
    // Every few game hours, or after a long trip.
    let moved = nat.season_at.is_none_or(|(la, lo)| (la - g.lat).abs() > 0.5 || (lo - g.lon).abs() > 0.5);
    if (t - nat.season_check).abs() < 3.0 * 3600.0 && full == ta.full_detail && !moved {
        return;
    }
    nat.season_check = t;
    nat.season_at = Some((g.lat, g.lon));
    let elev = crate::terrain::load::dem_height(g.lat, g.lon).unwrap_or(g.h.max(0.0));
    let biome = species::biome_at(g.lat, g.lon, elev);
    let mut s = species::season(biome, g.lat, elev, species::day_of_year(t));
    // Where the satellite sees the seasons change, it leads.
    if let Some((_, _, Some(gr))) = nat.greenness
        && let Some(l) = gr.leafiness()
        && !matches!(biome, species::Biome::Tropical | species::Biome::SouthFlorida)
    {
        s.leaves = s.leaves * 0.35 + l * 0.65;
        s.dormant = s.dormant.max((1.0 - l) * 0.8) * if l > 0.8 { 0.3 } else { 1.0 };
    }
    // Round so small changes don't regrow every tree.
    for v in [&mut s.leaves, &mut s.fall, &mut s.spring, &mut s.dry, &mut s.dormant] {
        *v = (*v * 10.0).round() / 10.0;
    }
    if s != ta.season || full != ta.full_detail {
        info!("vegetation season: {s:?} ({biome:?}), 3D trees {full}");
        ta.reset(s, full);
        // Everything is redrawn with the new prototypes.
        for c in nat.cells.values_mut() {
            clear_cell(&mut commands, c);
            c.pgen = 0;
            c.sig = 0;
        }
    }
}

fn clear_cell(commands: &mut Commands, c: &mut Cell) {
    if let Some(r) = c.root.take() {
        commands.entity(r).despawn();
    }
    c.trees.clear();
    c.billboard = None;
    c.with_3d = false;
}

pub(crate) fn tile_distance(viewer: DVec3, id: TileId) -> f64 {
    let (w, s, e, n) = id.bounds();
    let g = Geodetic::from_ecef(viewer);
    let lat = g.lat.clamp(s, n);
    let lon = g.lon.clamp(w, e);
    (Geodetic::new(lat, lon, g.h).to_ecef() - viewer).length()
}

/// Which tiles and cells are wanted; unload the rest.
#[allow(clippy::too_many_arguments)]
fn want(
    settings: Res<crate::GameSettings>,
    mut nat: ResMut<Nature>,
    viewer: Res<Viewer>,
    time: Res<Time>,
    terrain: Res<Terrain>,
    mut commands: Commands,
    mut stats: ResMut<NatureStats>,
) {
    nat.enabled = settings.0.vegetation;
    nat.timer -= time.delta_secs();
    if nat.timer > 0.0 {
        return;
    }
    nat.timer = 0.4;
    let now = time.elapsed_secs_f64();
    let r = ranges(settings.0.preset);
    let g = viewer.geo();
    let ground = terrain.height_at(g.lat, g.lon).unwrap_or(0.0);
    let high = g.h - ground > MAX_AGL_M;
    if nat.enabled && !high {
        let radius = r.cards as f64 + 200.0;
        let dlat = radius / 111_000.0;
        let dlon = radius / (111_000.0 * g.lat.to_radians().cos().max(0.05));
        for id in usx_core::tile::tiles_in_box(g.lat - dlat, g.lon - dlon, g.lat + dlat, g.lon + dlon, osm::ZOOM) {
            let t = nat.tiles.entry(id).or_insert(NatTile { state: NatState::Fetching, last_wanted: now });
            t.last_wanted = now;
            match t.state {
                NatState::Fetching => {
                    let d = tile_distance(viewer.ecef, id);
                    let f = &nat.fetch;
                    f.submit(id_key(id), d as f32, id);
                }
                NatState::Failed { retry_at } if now > retry_at => {
                    nat.tiles.get_mut(&id).unwrap().state = NatState::Fetching;
                }
                _ => {}
            }
        }
        for id in usx_core::tile::tiles_in_box(g.lat - dlat, g.lon - dlon, g.lat + dlat, g.lon + dlon, cover::CELL_ZOOM) {
            let d = tile_distance(viewer.ecef, id);
            if d > r.cards as f64 {
                continue;
            }
            let c = nat.cells.entry(id).or_insert_with(|| Cell {
                state: CellState::Waiting { since: now },
                root: None,
                trees: Vec::new(),
                billboard: None,
                sig: 0,
                pgen: 0,
                pending: false,
                with_3d: false,
                dist: d,
                last_wanted: now,
                species: HashSet::new(),
            });
            c.last_wanted = now;
            c.dist = d;
        }
    }
    // Unload what hasn't been wanted for a while.
    let stale: Vec<TileId> = nat.cells.iter().filter(|(_, c)| now - c.last_wanted > 6.0).map(|(k, _)| *k).collect();
    for k in stale {
        if let Some(mut c) = nat.cells.remove(&k) {
            clear_cell(&mut commands, &mut c);
        }
    }
    let stale: Vec<TileId> = nat.tiles.iter().filter(|(_, t)| now - t.last_wanted > 20.0).map(|(k, _)| *k).collect();
    for k in stale {
        nat.tiles.remove(&k);
        nat.fetch.cancel(id_key(k));
    }
    stats.tiles = nat.tiles.values().filter(|t| matches!(t.state, NatState::Ready(_))).count();
    stats.cells = nat.cells.values().filter(|c| matches!(c.state, CellState::Ready(_))).count();
    stats.cells_building = nat.cells.values().filter(|c| !matches!(c.state, CellState::Ready(_))).count();
    stats.trees = nat.cells.values().map(|c| if let CellState::Ready(d) = &c.state { d.trees.len() } else { 0 }).sum();
    stats.trees_3d = nat.cells.values().filter(|c| c.with_3d).map(|c| c.trees.len()).sum();
}

fn id_key(id: TileId) -> u64 {
    ((id.z as u64) << 56) | ((id.x as u64) << 28) | id.y as u64
}

#[allow(clippy::too_many_arguments)]
fn receive(
    mut nat: ResMut<Nature>,
    mut ta: Option<ResMut<TreeAssets>>,
    time: Res<Time>,
    settings: Res<crate::GameSettings>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut anchors: Query<&mut GeoAnchor>,
) {
    let now = time.elapsed_secs_f64();
    while let Some((id, r)) = nat.fetch.try_recv() {
        let Some(t) = nat.tiles.get_mut(&id) else { continue };
        match r {
            Ok(osm) => {
                t.state = NatState::Sorting;
                nat.work.submit(id_key(id) ^ 0x1, 0.0, Job::Sort(id, osm));
            }
            Err(e) => {
                warn!("nature {}: {e}", id.path());
                t.state = NatState::Failed { retry_at: now + 30.0 };
            }
        }
    }
    let Some(ta) = ta.as_deref_mut() else { return };
    let r = ranges(settings.0.preset);
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < 0.006 {
        let Some(out) = nat.work.try_recv() else { break };
        match out {
            Out::Sorted(id, d) => {
                if let Some(t) = nat.tiles.get_mut(&id) {
                    info!(
                        "nature {}: {} areas, {} trees, {} rows, {} barriers, {} power lines, {} turbines, {} seamarks",
                        id.path(),
                        d.cover.len(),
                        d.trees.len(),
                        d.tree_rows.len(),
                        d.barriers.len(),
                        d.power.len(),
                        d.turbines.len(),
                        d.seamarks.len()
                    );
                    t.state = NatState::Ready(Arc::new(d));
                }
            }
            Out::Cell(id, d) => {
                if let Some(c) = nat.cells.get_mut(&id) {
                    c.species = d.trees.iter().map(|t| t.sp).collect();
                    for &sp in &c.species {
                        ta.want(sp);
                    }
                    c.state = CellState::Ready(d);
                    c.sig = 0;
                    c.pgen = 0;
                }
            }
            Out::Draped(id, sig, pgen, d) => {
                let Some(c) = nat.cells.get_mut(&id) else { continue };
                c.pending = false;
                let (Some(d), CellState::Ready(data)) = (d, &c.state) else { continue };
                if pgen != ta.generation {
                    continue;
                }
                let data = data.clone();
                c.sig = sig;
                c.pgen = pgen;
                apply(&mut commands, &mut meshes, &mut anchors, ta, c, &data, d, r);
            }
        }
    }
}

/// Put a draped cell on screen: the cards mesh, and 3D trees when close.
#[allow(clippy::too_many_arguments)]
fn apply(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    anchors: &mut Query<&mut GeoAnchor>,
    ta: &TreeAssets,
    c: &mut Cell,
    data: &CellData,
    d: DrapedCell,
    r: Ranges,
) {
    let root = match c.root {
        Some(root) => {
            if let Ok(mut a) = anchors.get_mut(root) {
                a.ecef = d.center;
                a.placed_generation = 0;
            }
            root
        }
        None => {
            let root = commands.spawn((GeoAnchor::new(d.center), Transform::default(), Visibility::Visible)).id();
            c.root = Some(root);
            root
        }
    };
    // Cards.
    if let Some((e, _)) = c.billboard.take() {
        commands.entity(e).despawn();
    }
    if let Some(m) = d.bb {
        let h = meshes.add(m);
        let e = commands
            .spawn((
                Mesh3d(h.clone()),
                MeshMaterial3d(ta.billboard.clone()),
                Transform::default(),
                bevy::light::NotShadowCaster,
                bevy::camera::visibility::NoFrustumCulling,
                ChildOf(root),
            ))
            .id();
        c.billboard = Some((e, h));
    }
    // 3D trees (re-spawned on every drape: simplest, and drapes are rare).
    for e in c.trees.drain(..) {
        commands.entity(e).despawn();
    }
    c.with_3d = false;
    if r.low <= 0.0 || c.dist > (r.low + 200.0) as f64 {
        return;
    }
    c.with_3d = true;
    let fade = 8.0;
    for &(i, t, q, s) in &d.trees {
        let tree = &data.trees[i];
        let key = ProtoKey { sp: tree.sp, variant: tree.variant, stage: tree.stage };
        let Some(p) = ta.protos.get(&key) else { continue };
        let Some(bark) = ta.bark.get(&p.bark) else { continue };
        let tr = Transform { translation: t, rotation: q, scale: Vec3::splat(s) };
        let full_r = VisibilityRange { start_margin: 0.0..0.0, end_margin: (r.full - fade)..(r.full + fade), use_aabb: false };
        let low_r = VisibilityRange { start_margin: (r.full - fade)..(r.full + fade), end_margin: (r.low - fade)..(r.low + fade), use_aabb: false };
        let mut spawn = |mesh: &Handle<Mesh>, mat: Handle<VegMaterial>, range: VisibilityRange, shadow: bool| {
            let mut e = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat), tr, range, ChildOf(root)));
            if !shadow {
                e.insert(bevy::light::NotShadowCaster);
            }
            c.trees.push(e.id());
        };
        spawn(&p.full.0, bark.clone(), full_r.clone(), true);
        if let Some(l) = &p.full.1 {
            spawn(l, ta.leaves.clone(), full_r.clone(), true);
        }
        spawn(&p.low.0, bark.clone(), low_r.clone(), false);
        if let Some(l) = &p.low.1 {
            spawn(l, ta.leaves.clone(), low_r, false);
        }
    }
}

/// Build cells once what they depend on has loaded (or after a wait).
fn build_cells(mut nat: ResMut<Nature>, time: Res<Time>, settings: Res<crate::GameSettings>, ta: Option<Res<TreeAssets>>) {
    let Some(ta) = ta else { return };
    let now = time.elapsed_secs_f64();
    let mut waiting: Vec<(f64, TileId)> =
        nat.cells.iter().filter_map(|(k, c)| if let CellState::Waiting { .. } = c.state { Some((c.dist, *k)) } else { None }).collect();
    waiting.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut submitted = 0;
    let in_flight = nat.cells.values().filter(|c| matches!(c.state, CellState::Building)).count();
    for (d, id) in waiting {
        if submitted >= 3 || in_flight + submitted >= 6 {
            break;
        }
        let parent = id.ancestor(osm::ZOOM);
        let since = match nat.cells[&id].state {
            CellState::Waiting { since } => since,
            _ => continue,
        };
        let waited = now - since;
        let nature = match nat.tiles.get(&parent).map(|t| &t.state) {
            Some(NatState::Ready(d)) => Some(d.clone()),
            Some(NatState::Failed { .. }) => None,
            _ if waited > 20.0 => None,
            _ => continue,
        };
        let roads = crate::roads::corridors_for(parent);
        let buildings = crate::buildings::footprints_for(parent);
        let roads_needed = settings.0.roads;
        let buildings_needed = settings.0.buildings;
        if (roads_needed && roads.is_none() || buildings_needed && buildings.is_none()) && waited < 10.0 {
            continue;
        }
        let (corridors, areas) = roads.map(|r| (r.0.clone(), r.1.clone())).unwrap_or_default();
        let input = CellInput {
            id,
            imagery: settings.0.imagery,
            season: ta.season,
            corridors,
            areas,
            buildings: buildings.map(|b| (*b).clone()).unwrap_or_default(),
            nature,
        };
        nat.work.submit(id_key(id), d as f32 + 1000.0, Job::Cell(Box::new(input)));
        nat.cells.get_mut(&id).unwrap().state = CellState::Building;
        submitted += 1;
    }
}

/// (Re)drape cells whose ground changed or whose tree prototypes arrived.
fn schedule_drapes(mut nat: ResMut<Nature>, terrain: Res<Terrain>, ta: Option<Res<TreeAssets>>, settings: Res<crate::GameSettings>) {
    let Some(ta) = ta else { return };
    let r = ranges(settings.0.preset);
    let mut list: Vec<(f64, TileId)> = nat
        .cells
        .iter()
        .filter(|(_, c)| matches!(c.state, CellState::Ready(_)) && !c.pending)
        .map(|(k, c)| (c.dist, *k))
        .collect();
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut submitted = 0;
    for (d, id) in list {
        if submitted >= 4 {
            break;
        }
        let c = nat.cells.get(&id).unwrap();
        let CellState::Ready(data) = &c.state else { continue };
        // Wait for this cell's species (they come quickly).
        if !c.species.iter().all(|&sp| ta.ready(sp)) {
            continue;
        }
        let (w, s, e, n) = id.bounds();
        let bbox = (s, w, n, e);
        let sig = terrain.signature(bbox);
        // 3D trees appear and go as you move between distance bands.
        let want_3d = r.low > 0.0 && d <= (r.low + 200.0) as f64;
        let band_changed = want_3d != c.with_3d && c.root.is_some();
        if sig == c.sig && c.pgen == ta.generation && !band_changed || sig == 0xcbf2_9ce4_8422_2325 {
            continue;
        }
        let snap = terrain.snapshot(bbox);
        if snap.is_empty() {
            continue;
        }
        let data = data.clone();
        let pgen = ta.generation;
        nat.work.submit(id_key(id) ^ 0x2, d as f32, Job::Drape(data, snap, sig, ta.sprite_info.clone(), pgen));
        nat.cells.get_mut(&id).unwrap().pending = true;
        submitted += 1;
    }
}

/// Place a cell's trees on the ground (worker thread).
fn drape(cell: &CellData, snap: &HeightSnap, info: &SpriteInfo) -> Option<DrapedCell> {
    let (cla, clo) = cell.local.ll(Vec2::ZERO);
    let base_h = snap.ground(cla, clo).map(|x| x.0).unwrap_or(0.0);
    let center = Geodetic::new(cla, clo, base_h).to_ecef();
    let frame = EnuFrame::at(Geodetic::new(cla, clo, base_h));
    // Local (x east, y up, -z north) -> ECEF axes for the whole cell (300 m: the
    // curvature is a few millimetres).
    let to_ecef: DMat3 = frame.ecef_to_local.transpose();
    let q0 = DQuat::from_mat3(&to_ecef);
    let mut trees = Vec::with_capacity(cell.trees.len());
    let (mut pos, mut uv, mut uvb, mut col, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, t) in cell.trees.iter().enumerate() {
        let (la, lo) = cell.local.ll(t.p);
        let Some((h, _)) = snap.ground(la, lo) else { continue };
        let e = Geodetic::new(la, lo, h - 0.05).to_ecef();
        let rel = (e - center).as_vec3();
        let q = (q0 * DQuat::from_rotation_y(t.yaw as f64)).as_quat();
        trees.push((i, rel, q, t.scale));
        // The card.
        let key = ProtoKey { sp: t.sp, variant: t.variant, stage: t.stage };
        let Some(&((u0, u1), card)) = info.get(&key) else { continue };
        let w = card.x * t.scale;
        let hgt = card.y * t.scale;
        let b = pos.len() as u32;
        let p = rel.to_array();
        for (cx, cy, u, v) in [(-0.5f32, 0.0f32, u0.x, u1.y), (0.5, 0.0, u1.x, u1.y), (0.5, 1.0, u1.x, u0.y), (-0.5, 1.0, u0.x, u0.y)] {
            pos.push(p);
            uv.push([u, v]);
            uvb.push([cx * w, cy * hgt]);
            col.push([1.0f32, 1.0, 1.0, 1.0]);
        }
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let bb = (!idx.is_empty()).then(|| {
        let n = pos.len();
        let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
        let up = (center / center.length()).as_vec3().to_array();
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![up; n]);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvb);
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
        m.insert_indices(Indices::U32(idx));
        m
    });
    Some(DrapedCell { center, trees, bb })
}

/// Wind on the leaves; card distances follow the preset.
fn update_materials(
    ta: Option<Res<TreeAssets>>,
    wind: Res<conditions::Conditions>,
    settings: Res<crate::GameSettings>,
    mut veg: ResMut<Assets<VegMaterial>>,
    mut bbm: ResMut<Assets<BillboardMaterial>>,
    mut last: Local<Option<(Vec4, Vec4)>>,
) {
    let Some(ta) = ta else { return };
    let r = ranges(settings.0.preset);
    let strength = (wind.wind_speed / 18.0).clamp(0.02, 1.0);
    let a = wind.wind_from.to_radians();
    // Wind blows *from* `wind_from`: towards the opposite direction (east, north).
    let dir = Vec2::new(-a.sin(), -a.cos());
    let w = Vec4::new(dir.x, dir.y, strength, 0.0);
    let near = if r.low > 0.0 { r.low } else { 0.0 };
    let cuts = Vec4::new(near, r.cards, strength, 0.0);
    if last.is_some_and(|(lw, lc)| (lw - w).length() < 0.01 && (lc - cuts).length() < 0.5) {
        return;
    }
    *last = Some((w, cuts));
    for h in std::iter::once(&ta.leaves).chain(ta.bark.values()) {
        if let Some(mut m) = veg.get_mut(h) {
            m.extension.params.wind = w;
        }
    }
    if let Some(mut m) = bbm.get_mut(&ta.billboard) {
        m.extension.params.cuts = cuts;
    }
}

/// Whether vegetation near you has finished loading (for screenshots).
pub fn settled(nat: Option<&Nature>, viewer: &Viewer) -> bool {
    let Some(n) = nat else { return true };
    if !n.enabled {
        return true;
    }
    let _ = viewer;
    n.cells.values().filter(|c| c.dist < 400.0).all(|c| match &c.state {
        CellState::Ready(d) => d.trees.is_empty() || !c.pending && c.root.is_some(),
        _ => false,
    })
}
