//! The night sky: real stars (Yale Bright Star Catalogue), the five naked-eye planets
//! and the moon where they really are, the moon's phase and its light, the Milky Way
//! washed out by the light pollution where you are (NASA's VIIRS Black Marble), cloud
//! cover hiding it all, and the aurora from NOAA's forecast.

pub mod astro;

use crate::globe::{GlobeSet, Viewer, WorldOrigin};
use crate::nature::conditions::Conditions;
use crate::sun::SimClock;
use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, SunDisk};
use bevy::math::DVec3;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use usx_core::geo::Geodetic;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect, PartialEq)]
pub struct SkyParams {
    pub eq_from_world: Mat4,
    pub moon: Vec4,
    pub sun: Vec4,
    pub night: Vec4,
    pub aurora: Vec4,
    pub aurora2: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
}

impl Material for SkyMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://us_explorer/sky/sky.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/sky/sky.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        d: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        d.primitive.cull_mode = None;
        if let Some(ds) = d.depth_stencil.as_mut() {
            ds.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect, PartialEq)]
pub struct StarParams {
    pub p: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct StarMaterial {
    #[uniform(0)]
    pub params: StarParams,
}

impl Material for StarMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://us_explorer/sky/stars.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/sky/stars.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        d: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        d.primitive.cull_mode = None;
        if let Some(ds) = d.depth_stencil.as_mut() {
            ds.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// Things in the sky that turn with the stars.
#[derive(Component)]
struct Celestial;
#[derive(Component)]
struct Dome;
#[derive(Component)]
struct PlanetPoints;
/// The moon's light.
#[derive(Component)]
pub struct Moonlight;

#[derive(Resource)]
struct SkyHandles {
    sky: Handle<SkyMaterial>,
    stars: Handle<StarMaterial>,
    planets: Handle<Mesh>,
}

/// Artificial light at night where you are, 0 (wilderness) .. 1 (downtown).
#[derive(Resource, Default)]
pub struct LightPollution {
    pub level: f32,
    pub known: bool,
    rx: Option<crossbeam_channel::Receiver<Option<f32>>>,
    at: Option<(f64, f64)>,
}

/// What the overlay shows: moon phase and local time.
#[derive(Resource, Default, Clone)]
pub struct SkyInfo {
    pub moon_lit: f32,
    pub moon_waxing: bool,
    pub moon_elevation: f32,
}

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "sky.wgsl");
        bevy::asset::embedded_asset!(app, "stars.wgsl");
        app.add_plugins((MaterialPlugin::<SkyMaterial>::default(), MaterialPlugin::<StarMaterial>::default()))
            .init_resource::<LightPollution>()
            .init_resource::<SkyInfo>()
            .add_systems(Startup, spawn)
            .add_systems(Update, light_pollution)
            .add_systems(PostUpdate, update.after(GlobeSet).before(TransformSystems::Propagate));
    }
}

const STARS: &[u8] = include_bytes!("stars.bin");

/// Star colour from surface temperature (K), roughly.
fn star_colour(k: f32) -> [f32; 3] {
    let t = ((k - 3000.0) / 9000.0).clamp(0.0, 1.0);
    let warm = [1.0, 0.72, 0.48];
    let mid = [1.0, 0.96, 0.9];
    let cool = [0.72, 0.8, 1.0];
    let (a, b, f) = if t < 0.33 { (warm, mid, t / 0.33) } else { (mid, cool, (t - 0.33) / 0.67) };
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f)
}

/// Points as camera-facing quads: direction, colour, magnitude, twinkle seed.
fn points_mesh(points: &[(Vec3, [f32; 3], f32)], usage: RenderAssetUsages) -> Mesh {
    let n = points.len();
    let (mut pos, mut uv, mut info, mut col, mut idx) =
        (Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 6));
    for (i, &(d, c, mag)) in points.iter().enumerate() {
        let seed = (i as f32 * 0.618_034).fract();
        for corner in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
            pos.push(d.to_array());
            uv.push(corner);
            info.push([mag, seed]);
            col.push([c[0], c[1], c[2], 1.0]);
        }
        let b = (i * 4) as u32;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, usage)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, info)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
        .with_inserted_indices(Indices::U32(idx))
}

fn catalogue() -> Vec<(Vec3, [f32; 3], f32)> {
    STARS
        .chunks_exact(6)
        .map(|c| {
            let ra = u16::from_le_bytes([c[0], c[1]]) as f32 / 65535.0 * std::f32::consts::TAU;
            let dec = i16::from_le_bytes([c[2], c[3]]) as f32 / 32767.0 * std::f32::consts::FRAC_PI_2;
            let mag = c[4] as f32 / 25.0 - 2.0;
            let k = c[5] as f32 * 100.0;
            (Vec3::new(dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()), star_colour(k), mag)
        })
        .collect()
}

fn planet_points(unix: f64) -> Vec<(Vec3, [f32; 3], f32)> {
    astro::planets(unix).iter().zip(astro::PLANETS.iter()).map(|(&(d, mag), p)| (d.as_vec3(), p.2, mag)).collect()
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut skies: ResMut<Assets<SkyMaterial>>,
    mut star_mats: ResMut<Assets<StarMaterial>>,
    clock: Res<SimClock>,
) {
    let sky = skies.add(SkyMaterial::default());
    let stars = star_mats.add(StarMaterial::default());
    let dome = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());
    let star_mesh = meshes.add(points_mesh(&catalogue(), RenderAssetUsages::RENDER_WORLD));
    let planets = meshes.add(points_mesh(&planet_points(clock.time()), RenderAssetUsages::default()));
    let common = (Transform::default(), NotShadowCaster, bevy::camera::visibility::NoFrustumCulling);
    commands.spawn((Dome, Mesh3d(dome), MeshMaterial3d(sky.clone()), common.clone()));
    commands.spawn((Celestial, Mesh3d(star_mesh), MeshMaterial3d(stars.clone()), common.clone()));
    commands.spawn((Celestial, PlanetPoints, Mesh3d(planets.clone()), MeshMaterial3d(stars.clone()), common));
    commands.spawn((
        Moonlight,
        DirectionalLight { illuminance: 0.0, color: Color::srgb(0.78, 0.84, 1.0), shadow_maps_enabled: false, ..default() },
        SunDisk::OFF,
        Transform::default(),
    ));
    commands.insert_resource(SkyHandles { sky, stars, planets });
}

#[allow(clippy::too_many_arguments)]
fn update(
    clock: Res<SimClock>,
    origin: Res<WorldOrigin>,
    cond: Res<Conditions>,
    lp: Res<LightPollution>,
    handles: Option<Res<SkyHandles>>,
    mut skies: ResMut<Assets<SkyMaterial>>,
    mut star_mats: ResMut<Assets<StarMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut celestial: Query<&mut Transform, (With<Celestial>, Without<Moonlight>)>,
    mut moon_light: Query<(&mut DirectionalLight, &mut Transform), With<Moonlight>>,
    mut info: ResMut<SkyInfo>,
    mut planets_at: Local<f64>,
) {
    let Some(h) = handles else { return };
    let unix = clock.time();
    let frame = &origin.frame;
    // Equatorial -> world rotation.
    let cols = [DVec3::X, DVec3::Y, DVec3::Z].map(|v| frame.dir_to_local(astro::eq_to_ecef(v, unix)).as_vec3());
    let world_from_eq = Mat3::from_cols(cols[0], cols[1], cols[2]);
    let rot = Quat::from_mat3(&world_from_eq).normalize();
    for mut t in &mut celestial {
        if t.rotation.angle_between(rot) > 1e-6 {
            t.rotation = rot;
        }
    }
    if (unix - *planets_at).abs() > 600.0 {
        *planets_at = unix;
        if let Some(mut m) = meshes.get_mut(&h.planets) {
            *m = points_mesh(&planet_points(unix), RenderAssetUsages::default());
        }
    }
    // The moon from where you stand (it's close enough for parallax to matter).
    let obs = frame.to_ecef(DVec3::ZERO);
    let moon_ecef = astro::eq_to_ecef(astro::moon_eq(unix), unix) * 6_378_137.0;
    let moon = frame.dir_to_local((moon_ecef - obs).normalize()).as_vec3();
    let sun = frame.dir_to_local(crate::sun::sun_dir_ecef(unix)).as_vec3();
    let (lit, waxing) = astro::moon_phase(unix);
    let lit = lit as f32;
    let sun_el = sun.y.asin().to_degrees();
    let dark = ((-sun_el - 3.0) / 12.0).clamp(0.0, 1.0);
    let moon_el = moon.y.asin().to_degrees();
    *info = SkyInfo { moon_lit: lit, moon_waxing: waxing, moon_elevation: moon_el };

    // Aurora towards the geomagnetic north pole.
    let pole = Geodetic::new(80.7, -72.7, 0.0).to_ecef();
    let mut mn = frame.dir_to_local(pole - obs).as_vec3();
    mn.y = 0.0;
    let mn = mn.normalize_or(Vec3::NEG_Z);
    let here = cond.aurora_here;
    let north = cond.aurora_north;
    let strength = ((here.max(north * 0.7) - 4.0) / 35.0).clamp(0.0, 1.0);
    let overhead = ((here - 8.0) / 40.0).clamp(0.0, 1.0);
    let base = 0.05 + 0.25 * (north / 60.0).clamp(0.0, 1.0) + 0.5 * overhead;

    let light_pollution = if lp.known { lp.level } else { 0.35 };
    let params = SkyParams {
        eq_from_world: Mat4::from_mat3(world_from_eq.transpose()),
        moon: moon.extend(0.0045 * 1.5),
        sun: sun.extend(2500.0),
        night: Vec4::new(dark, light_pollution, cond.cloud_cover, 0.0),
        aurora: mn.extend(strength),
        aurora2: Vec4::new(base, overhead, 0.0, 0.0),
    };
    if let Some(m) = skies.get(&h.sky)
        && m.params != params
        && let Some(mut m) = skies.get_mut(&h.sky)
    {
        m.params = params;
    }
    // Faintest star you can see: ~6.3 under a dark sky, ~3 in a city centre.
    let limit = 6.4 - 3.6 * light_pollution.powf(0.7);
    let sp = StarParams { p: Vec4::new(dark, limit, 1.0 - 0.95 * cond.cloud_cover, 1.0) };
    if let Some(m) = star_mats.get(&h.stars)
        && m.params != sp
        && let Some(mut m) = star_mats.get_mut(&h.stars)
    {
        m.params = sp;
    }
    // Moonlight: a full moon high in a clear sky is enough to walk by.
    for (mut l, mut t) in &mut moon_light {
        let up = ((moon_el + 1.0) / 12.0).clamp(0.0, 1.0);
        let illum = 3.0 * lit.powf(1.5) * up * dark * (1.0 - 0.75 * cond.cloud_cover);
        if (l.illuminance - illum).abs() > 0.01 {
            l.illuminance = illum;
        }
        let want = Transform::IDENTITY.looking_to(-moon, Vec3::Y);
        if t.rotation.angle_between(want.rotation) > 1e-4 {
            *t = want;
        }
    }
}

/// Fetch how lit up the area is at night (VIIRS Black Marble), when you move far.
fn light_pollution(mut lp: ResMut<LightPollution>, viewer: Res<Viewer>) {
    if let Some(rx) = &lp.rx {
        if let Ok(v) = rx.try_recv() {
            lp.rx = None;
            if let Some(v) = v {
                lp.level = v;
                lp.known = true;
            }
        }
        return;
    }
    let g = viewer.geo();
    if lp.at.is_some_and(|(la, lo)| (la - g.lat).abs() < 0.05 && (lo - g.lon).abs() < 0.05) {
        return;
    }
    lp.at = Some((g.lat, g.lon));
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (lat, lon) = (g.lat, g.lon);
    std::thread::spawn(move || {
        let _ = tx.send(black_marble(lat, lon));
    });
    lp.rx = Some(rx);
}

/// Sky glow from city lights within ~25 km, nearer ones counting more.
fn black_marble(lat: f64, lon: f64) -> Option<f32> {
    use usx_core::tile::TileId;
    const Z: u8 = 8;
    let (t, u, v) = TileId::containing(lat, lon, Z);
    // Sample a 3x3 block of tiles so the circle never falls off an edge.
    let mut tiles = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let n = 1i64 << Z;
            let (x, y) = ((t.x as i64 + dx).rem_euclid(n), t.y as i64 + dy);
            if !(0..n).contains(&y) {
                tiles.push(None);
                continue;
            }
            let id = TileId { z: Z, x: x as u32, y: y as u32 };
            let url = format!(
                "https://gibs.earthdata.nasa.gov/wmts/epsg3857/best/VIIRS_Black_Marble/default/2016-01-01/GoogleMapsCompatible_Level8/{}/{}/{}.png",
                id.z, id.y, id.x
            );
            let img = crate::terrain::load::fetch(&format!("lights/{}.png", id.path()), &url)
                .ok()
                .and_then(|b| usx_core::image::load_from_memory(&b).ok())
                .map(|i| i.to_rgb8());
            tiles.push(img);
        }
    }
    if tiles[4].is_none() {
        return None;
    }
    // A pixel is ~ 610 m * cos(lat) at zoom 8.
    let px_m = 156_543.0 / 256.0 * lat.to_radians().cos();
    let r = (25_000.0 / px_m).ceil() as i64;
    let (cx, cy) = ((u * 256.0) as i64 + 256, (v * 256.0) as i64 + 256);
    let (mut sum, mut wsum) = (0.0f64, 0.0f64);
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            let d = (((x - cx) * (x - cx) + (y - cy) * (y - cy)) as f64).sqrt() * px_m;
            if d > 25_000.0 {
                continue;
            }
            let (tx, ty) = ((x / 256) as usize, (y / 256) as usize);
            let Some(Some(img)) = tiles.get(ty * 3 + tx) else { continue };
            let p = img.get_pixel((x % 256) as u32, (y % 256) as u32);
            let lum = (p[0] as f64 * 0.3 + p[1] as f64 * 0.5 + p[2] as f64 * 0.2) / 255.0;
            // Dark land in the product is ~0.08: count only light above it.
            let lit = ((lum - 0.1) / 0.9).max(0.0);
            let w = 1.0 / (1.0 + (d / 3_000.0).powi(2));
            sum += lit * w;
            wsum += w;
        }
    }
    (wsum > 0.0).then(|| ((sum / wsum) * 2.2).clamp(0.0, 1.0) as f32)
}
