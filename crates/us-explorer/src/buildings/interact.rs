//! Using things in buildings: right-click doors to open and close them, sit on chairs
//! and sofas (move to get up), lie on beds, run taps, showers and baths, flush
//! toilets, open fridges, ovens, microwaves and cabinets, switch screens on, ride
//! elevators. Bathroom mirrors show a real reflection.

use super::build::Built;
use super::furniture::{FixtureKind, HingeKind, Kit, Spec, WHITE, rgb};
use super::interior::{DoorData, DoorStyle, LiftData};
use super::model::{EntranceKind, Kind};
use super::textures::{InteriorMats, M};
use super::{PlacedRef, exterior, geom};
use crate::globe::{GeoAnchor, Viewer, WorldOrigin};
use crate::walkable::Walkables;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{Exposure, RenderTarget, visibility::RenderLayers};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::math::{Affine3A, DVec3, Mat3A, Vec3A};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::shader::ShaderRef;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use usx_core::geo::Geodetic;
use usx_core::tile::TileId;

/// How far you can reach, from your eyes.
const REACH_M: f32 = 2.4;

/// Something you can right-click: an oriented box in a building's local frame.
#[derive(Component, Clone)]
pub struct Usable {
    /// Building-local frame: ECEF origin and rotation to ECEF.
    pub origin: DVec3,
    pub rot: Quat,
    pub centre: Vec3,
    pub half: Vec3,
    pub brot: Quat,
    pub what: Use,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Use {
    Hinge,
    Seat { at: Vec3, yaw: f32, lie: bool },
    Fixture,
    Screen,
    LiftButton { car: Entity, floor: usize },
    LiftCall { car: Entity, floor: usize },
}

/// A door or appliance door turning on its hinge.
#[derive(Component)]
pub struct Hinge {
    pub axis: Vec3,
    pub angle: f32,
    pub t: f32,
    pub open: bool,
    pub door_id: Option<u32>,
    pub kind: HingeKind,
    pub hinge: Vec3,
    pub centre0: Vec3,
    pub brot0: Quat,
}

/// Running water: taps, showers, baths, toilets.
#[derive(Component)]
pub struct Water {
    pub kind: FixtureKind,
    pub on: bool,
    pub level: f32,
    pub t: f32,
    pub stream: Entity,
    pub surface: Entity,
    pub spout: Vec3,
    pub basin: Vec3,
    pub fill_h: f32,
}

#[derive(Component)]
pub struct ScreenState {
    pub on: bool,
    pub mesh: Entity,
}

/// An elevator car.
#[derive(Component)]
pub struct LiftCar {
    pub id: u32,
    pub base: f64,
    pub floors: Vec<f32>,
    pub at: usize,
    pub target: usize,
    pub y: f32,
    pub door: f32,
    pub dwell: f32,
    pub centre: Vec2,
    pub fwd: Vec3,
    pub side: Vec3,
    pub width: f32,
    pub depth: f32,
    /// Car door panels, and each floor's hall door panels.
    pub car_doors: [Entity; 2],
    pub hall_doors: Vec<([Entity; 2], Vec3)>,
}

/// Moves with a lift car (its buttons).
#[derive(Component)]
pub struct RideAlong {
    pub car: Entity,
    pub rel: Vec3,
}

/// A bathroom mirror.
#[derive(Component)]
pub struct MirrorQuad {
    pub origin: DVec3,
    pub rot: Quat,
    pub centre: Vec3,
    pub normal: Vec3,
    pub w: f32,
    pub h: f32,
    pub live: bool,
}

#[derive(Component)]
struct MirrorCamera;

/// The thing under the crosshair, if it's within reach.
#[derive(Resource, Default)]
pub struct Aim(pub Option<(Entity, String)>);

/// Screen-space reflection material for mirrors.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct MirrorMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub image: Handle<Image>,
}

impl Material for MirrorMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/buildings/mirror.wgsl".into()
    }
}

#[derive(Resource)]
pub struct UseMats {
    pub lit_screen: Handle<StandardMaterial>,
    pub dark_screen: Handle<StandardMaterial>,
    pub mirror_dull: Handle<StandardMaterial>,
    pub mirror_live: Handle<MirrorMaterial>,
    pub mirror_image: Handle<Image>,
    pub unit_quad: Handle<Mesh>,
    pub unit_cyl: Handle<Mesh>,
    pub unit_cone: Handle<Mesh>,
}

pub fn plugin(app: &mut App) {
    bevy::asset::embedded_asset!(app, "mirror.wgsl");
    app.add_plugins(MaterialPlugin::<MirrorMaterial>::default())
        .init_resource::<Aim>()
        .add_systems(Startup, setup)
        .add_systems(Update, (aim, use_input, animate_hinges, animate_water, animate_lifts).chain().after(crate::ui::UiInput))
        .add_systems(PostUpdate, mirror_camera.after(crate::globe::GlobeSet).before(bevy::transform::TransformSystems::Propagate))
        .add_systems(EguiPrimaryContextPass, draw_prompt.after(crate::ui::DrawUi));
}

fn mirror_image(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    let mut image = Image::new_uninit(
        Extent3d { width: size.x.max(16), height: size.y.max(16), depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Bgra8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage |= TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    images.add(image)
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut mirror_mats: ResMut<Assets<MirrorMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let image = mirror_image(&mut images, UVec2::new(960, 540));
    let lit_screen = std_mats.add(StandardMaterial {
        base_color: Color::srgb(0.1, 0.12, 0.16),
        emissive: LinearRgba::rgb(0.9, 1.6, 3.2) * 600.0,
        perceptual_roughness: 0.2,
        ..default()
    });
    let dark_screen = std_mats.add(StandardMaterial {
        base_color: Color::srgb(0.02, 0.02, 0.025),
        perceptual_roughness: 0.15,
        reflectance: 0.6,
        ..default()
    });
    let mirror_dull =
        std_mats.add(StandardMaterial { base_color: Color::srgb(0.75, 0.78, 0.8), metallic: 1.0, perceptual_roughness: 0.08, ..default() });
    let mirror_live = mirror_mats.add(MirrorMaterial { image: image.clone() });
    let unit_quad = meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(0.5)).mesh());
    let mut k = Kit::default();
    k.cyl(M::Water, Affine3A::IDENTITY, 1.0, 1.0, 10, WHITE, false);
    let unit_cyl = meshes.add(k.into_meshes().pop().map(|m| m.1).unwrap_or_else(|| Mesh::from(Cylinder::new(1.0, 1.0))));
    let mut k = Kit::default();
    k.frustum(M::Water, Affine3A::IDENTITY, 1.0, 0.2, 1.0, 16, WHITE, false);
    let unit_cone = meshes.add(k.into_meshes().pop().map(|m| m.1).unwrap_or_else(|| Mesh::from(Cylinder::new(1.0, 1.0))));
    commands.insert_resource(UseMats {
        lit_screen,
        dark_screen,
        mirror_dull,
        mirror_live,
        mirror_image: image.clone(),
        unit_quad,
        unit_cyl,
        unit_cone,
    });
    commands.spawn((
        MirrorCamera,
        Camera3d::default(),
        Camera {
            order: -1,
            invert_culling: true,
            is_active: false,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.55, 0.6, 0.68)),
            ..default()
        },
        RenderTarget::Image(image.into()),
        Projection::Perspective(PerspectiveProjection { near: 0.05, ..default() }),
        Exposure { ev100: 9.5 },
        Tonemapping::AcesFitted,
        Msaa::Off,
        RenderLayers::from_layers(&[0, 1]),
        Transform::default(),
    ));
}

// ------------------------------------------------------------------ spawning

fn spawn_kit(commands: &mut Commands, meshes: &mut Assets<Mesh>, imats: &InteriorMats, parent: Entity, kit: Kit) {
    for (m, mesh) in kit.into_meshes() {
        let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(imats.get(m)), Transform::default(), ChildOf(parent)));
        if matches!(m, M::Glass | M::Water) {
            e.insert(bevy::light::NotShadowCaster);
        }
    }
}

/// Yaw that turns +X onto `along` (both horizontal).
fn yaw_to(along: Vec3) -> f32 {
    (-along.z).atan2(along.x)
}

/// A door leaf: from the hinge at the origin along +X, `w` wide and `h` tall, centred
/// on z = 0 (both faces finished).
fn door_leaf(k: &mut Kit, w: f32, h: f32, style: DoorStyle, col: [f32; 4], variant: u32) {
    let t = 0.045;
    let at = |x: f32, y: f32, z: f32| Affine3A::from_translation(Vec3::new(x, y, z));
    match style {
        DoorStyle::Glass => {
            let fc = rgb(0.62, 0.64, 0.66);
            let (stile, top, bottom) = (0.1, 0.1, 0.22);
            k.cube(M::Metal, at(stile * 0.5, h * 0.5, 0.0), Vec3::new(stile, h, t), fc);
            k.cube(M::Metal, at(w - stile * 0.5, h * 0.5, 0.0), Vec3::new(stile, h, t), fc);
            k.cube(M::Metal, at(w * 0.5, h - top * 0.5, 0.0), Vec3::new(w, top, t), fc);
            k.cube(M::Metal, at(w * 0.5, bottom * 0.5, 0.0), Vec3::new(w, bottom, t), fc);
            k.cube(M::Glass, at(w * 0.5, (h - top + bottom) * 0.5, 0.0), Vec3::new(w - stile * 2.0, h - top - bottom, 0.01), WHITE);
            // Push bars both sides.
            for z in [-0.05f32, 0.05] {
                k.cube(M::Chrome, at(w * 0.5, 1.0, z), Vec3::new(w - 0.3, 0.035, 0.035), WHITE);
            }
        }
        DoorStyle::Metal => {
            k.cube(M::Metal, at(w * 0.5, h * 0.5, 0.0), Vec3::new(w, h, t), col);
            for z in [-0.05f32, 0.05] {
                k.cube(M::Chrome, at(w - 0.25, 1.0, z), Vec3::new(0.3, 0.04, 0.03), WHITE);
            }
            k.cube(M::Glass, at(w * 0.5, 1.6, 0.0), Vec3::new(0.12, 0.5, t + 0.004), WHITE);
        }
        DoorStyle::Panel => {
            k.cube(M::Paint, at(w * 0.5, h * 0.5, 0.0), Vec3::new(w, h, t), col);
            let raised = |k: &mut Kit, x0: f32, x1: f32, y0: f32, y1: f32| {
                for z in [-t * 0.5 - 0.006, t * 0.5 + 0.006] {
                    k.cube(M::Paint, at((x0 + x1) * 0.5, (y0 + y1) * 0.5, z), Vec3::new(x1 - x0, y1 - y0, 0.012), col);
                }
            };
            let lite = |k: &mut Kit, x0: f32, x1: f32, y0: f32, y1: f32| {
                k.cube(M::Glass, at((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0), Vec3::new(x1 - x0, y1 - y0, t + 0.004), WHITE);
                // Muntin frame.
                for z in [-t * 0.5 - 0.005, t * 0.5 + 0.005] {
                    k.cube(M::Paint, at((x0 + x1) * 0.5, y0, z), Vec3::new(x1 - x0 + 0.04, 0.03, 0.01), col);
                    k.cube(M::Paint, at((x0 + x1) * 0.5, y1, z), Vec3::new(x1 - x0 + 0.04, 0.03, 0.01), col);
                }
            };
            let (m, mid) = (0.12, w * 0.5);
            match variant % 5 {
                // Six-panel.
                0 => {
                    for (y0, y1) in [(0.15, 0.85), (0.98, 1.68), (1.8, h - 0.12)] {
                        raised(k, m, mid - 0.05, y0, y1);
                        raised(k, mid + 0.05, w - m, y0, y1);
                    }
                }
                // Half lite.
                1 => {
                    raised(k, m, w - m, 0.15, 0.9);
                    lite(k, m, w - m, 1.05, h - 0.15);
                }
                // Craftsman: three small lites over two tall panels.
                2 => {
                    let lw = (w - 2.0 * m) / 3.0;
                    for i in 0..3 {
                        let x0 = m + i as f32 * lw;
                        lite(k, x0 + 0.02, x0 + lw - 0.02, h - 0.55, h - 0.15);
                    }
                    raised(k, m, mid - 0.04, 0.15, h - 0.7);
                    raised(k, mid + 0.04, w - m, 0.15, h - 0.7);
                    for z in [-t * 0.5 - 0.012, t * 0.5 + 0.012] {
                        k.cube(M::Paint, at(w * 0.5, h - 0.62, z), Vec3::new(w - 0.1, 0.06, 0.02), col);
                    }
                }
                // Flush with a narrow vertical lite.
                3 => {
                    lite(k, w - 0.3, w - 0.18, 0.4, h - 0.3);
                }
                // Two-panel.
                _ => {
                    raised(k, m, w - m, 0.15, h * 0.42);
                    raised(k, m, w - m, h * 0.48, h - 0.15);
                }
            }
            // Lever handles both sides and hinges.
            for z in [-1.0f32, 1.0] {
                let zz = z * (t * 0.5 + 0.03);
                k.cyl(
                    M::Chrome,
                    Affine3A::from_rotation_translation(
                        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
                        Vec3::new(w - 0.07, 0.95, zz - 0.015 * z),
                    ),
                    0.025,
                    0.03,
                    10,
                    WHITE,
                    true,
                );
                k.cube(M::Chrome, at(w - 0.13, 0.95, zz), Vec3::new(0.12, 0.02, 0.02), WHITE);
            }
            for y in [0.25, h * 0.5, h - 0.25] {
                k.cube(M::Chrome, at(0.0, y, 0.0), Vec3::new(0.015, 0.1, t + 0.01), WHITE);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_hinge(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    imats: &InteriorMats,
    parent: Entity,
    origin: DVec3,
    rot: Quat,
    kit: Kit,
    hinge: Vec3,
    angle: f32,
    kind: HingeKind,
    half: Vec3,
    centre: Vec3,
    flip_down: bool,
    door_id: Option<u32>,
    front: Vec3,
    box_rot: Option<Quat>,
) -> Entity {
    let e0 = centre - hinge;
    let (axis, sign) = if flip_down {
        let a = front.cross(Vec3::Y).normalize_or(Vec3::X);
        let s = if a.cross(e0).dot(front) > 0.0 { 1.0 } else { -1.0 };
        (a, s)
    } else {
        let v = Vec3::Y.cross(e0);
        (Vec3::Y, if v.dot(front) > 0.0 { 1.0 } else { -1.0 })
    };
    let f = Vec3::new(front.x, 0.0, front.z).normalize_or(Vec3::NEG_Z);
    let brot0 = box_rot.unwrap_or_else(|| Quat::from_rotation_y((-f.x).atan2(-f.z)));
    let e = commands
        .spawn((
            Transform::from_translation(hinge),
            Visibility::Inherited,
            ChildOf(parent),
            Hinge { axis, angle: angle * sign, t: 0.0, open: false, door_id, kind, hinge, centre0: centre, brot0 },
            Usable { origin, rot, centre, half: half + Vec3::splat(0.02), brot: brot0, what: Use::Hinge },
        ))
        .id();
    spawn_kit(commands, meshes, imats, e, kit);
    e
}

/// A frame entity in a building's local frame (x east, y up, z south at its centroid).
fn frame_root(commands: &mut Commands, origin: DVec3, rot: Quat) -> (Entity, Entity) {
    let root = commands.spawn((GeoAnchor::new(origin), Transform::default(), Visibility::Visible)).id();
    let frame = commands.spawn((Transform::from_rotation(rot), Visibility::Inherited, ChildOf(root))).id();
    (root, frame)
}

/// Exterior doors of a building drawn in detail: real doors in the shell's openings.
pub fn spawn_exterior_doors(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    imats: &InteriorMats,
    b: &Built,
    key: (TileId, u32),
    p: &PlacedRef,
) -> Vec<Entity> {
    let bld = &b.buildings[key.1 as usize];
    if bld.parts.len() != 1 {
        return Vec::new();
    }
    let doors = exterior::doors(bld, &b.street[key.1 as usize]);
    if doors.iter().all(|d| d.3 == EntranceKind::Garage) {
        return Vec::new();
    }
    let rot = super::interior::local_to_ecef(b, key.1, p);
    let (root, frame) = frame_root(commands, p.origin, rot);
    let cen = geom::centroid(&bld.outline);
    let n = bld.outline.len();
    let colour = exterior::door_colour(bld.kind, bld.seed);
    let col = [colour[0], colour[1], colour[2], 1.0];
    let shop = bld.kind.storefront() || matches!(bld.kind, Kind::Office | Kind::School | Kind::Civic | Kind::Hospital | Kind::Hotel);
    for (di, &(e, at, w, kind)) in doors.iter().enumerate() {
        if kind == EntranceKind::Garage {
            continue;
        }
        let (a, c) = (bld.outline[e] - cen, bld.outline[(e + 1) % n] - cen);
        let len = a.distance(c);
        let d = (c - a) / len.max(1e-3);
        let inward = d.perp();
        let hh = if w > 1.5 { 2.5 } else { 2.15 };
        let id = super::interior::ext_door_id(key, e, at);
        let line = |u: f32| a + d * u + inward * 0.08;
        let style = if shop && kind != EntranceKind::Service {
            DoorStyle::Glass
        } else if kind == EntranceKind::Service && !bld.kind.residential() {
            DoorStyle::Metal
        } else {
            DoorStyle::Panel
        };
        let variant = (super::model::hash64(bld.seed ^ di as u64) % 5) as u32;
        let leaves: Vec<(f32, f32, f32)> =
            if w > 1.5 { vec![(at - w * 0.5, 1.0, w * 0.5), (at + w * 0.5, -1.0, w * 0.5)] } else { vec![(at - w * 0.5, 1.0, w)] };
        // Commercial doors swing out, house doors in.
        let front2 = if style == DoorStyle::Panel { inward } else { -inward };
        for (u, dirs, lw) in leaves {
            let hp = line(u);
            let along = Vec3::new(d.x * dirs, 0.0, d.y * dirs);
            let mut leaf = Kit::default();
            door_leaf(&mut leaf, lw - 0.01, hh - 0.02, style, if style == DoorStyle::Metal { rgb(0.45, 0.47, 0.5) } else { col }, variant);
            let kit = rotate_kit(leaf, Quat::from_rotation_y(yaw_to(along)));
            let hinge = Vec3::new(hp.x, 0.0, hp.y);
            let centre = hinge + along * lw * 0.5 + Vec3::Y * hh * 0.5;
            let half = Vec3::new(lw * 0.5, hh * 0.5, 0.04);
            let front = Vec3::new(front2.x, 0.0, front2.y);
            // The Usable box is in the door's own axes (x along the leaf).
            spawn_hinge(
                commands,
                meshes,
                imats,
                frame,
                p.origin,
                rot,
                kit,
                hinge,
                1.7,
                HingeKind::ExteriorDoor,
                half,
                centre,
                false,
                Some(id),
                front,
                Some(Quat::from_rotation_y(yaw_to(along))),
            );
        }
        // A doormat and a house number for houses.
        if bld.kind.residential() && style == DoorStyle::Panel {
            let mut k = Kit::default();
            let m = a + d * at - inward * 0.45;
            let yaw = yaw_to(Vec3::new(d.x, 0.0, d.y));
            k.cube(
                M::Rubber,
                Affine3A::from_rotation_translation(Quat::from_rotation_y(yaw), Vec3::new(m.x, 0.012, m.y)),
                Vec3::new(0.8, 0.02, 0.5),
                rgb(0.25, 0.2, 0.15),
            );
            spawn_kit(commands, meshes, imats, frame, k);
        }
    }
    vec![root]
}

fn rotate_kit(kit: Kit, q: Quat) -> Kit {
    let mut out = Kit::default();
    let a = Affine3A::from_quat(q);
    for (m, mb) in kit.parts {
        let b = out.parts.entry(m).or_default();
        b.pos = mb.pos.into_iter().map(|p| a.transform_point3(Vec3::from(p)).to_array()).collect();
        b.nrm = mb.nrm.into_iter().map(|n| a.transform_vector3(Vec3::from(n)).to_array()).collect();
        b.uv = mb.uv;
        b.col = mb.col;
        b.idx = mb.idx;
    }
    out
}

/// Interactive parts of an interior: doors, appliance doors, seats, water, screens,
/// mirrors, lights and the elevator.
#[allow(clippy::too_many_arguments)]
pub fn spawn_specs(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    imats: &InteriorMats,
    um: &UseMats,
    frame: Entity,
    specs: Vec<Spec>,
    doors: &[DoorData],
    lift: Option<&LiftData>,
    origin: DVec3,
    rot: Quat,
    base: f64,
    near_y: f32,
) {
    // Room doors.
    for d in doors {
        let mut leaf = Kit::default();
        door_leaf(&mut leaf, d.width - 0.02, 2.06, d.kind, d.colour, 4 + (d.id % 2) * 1);
        let q = Quat::from_rotation_y(yaw_to(d.along));
        let kit = rotate_kit(leaf, q);
        let front = Vec3::new(d.along.z, 0.0, -d.along.x);
        let centre = d.hinge + d.along * d.width * 0.5 + Vec3::Y * 1.03;
        spawn_hinge(
            commands,
            meshes,
            imats,
            frame,
            origin,
            rot,
            kit,
            d.hinge + Vec3::Y * 0.005,
            1.6,
            HingeKind::Door,
            Vec3::new(d.width * 0.5, 1.03, 0.04),
            centre,
            false,
            Some(d.id),
            front,
            Some(q),
        );
    }
    // Lights: the ones nearest the floor you're on (a few dozen at most).
    let mut lights: Vec<(Vec3, f32)> =
        specs.iter().filter_map(|s| if let Spec::Light { at, lumens } = s { Some((*at, *lumens)) } else { None }).collect();
    lights.sort_by(|a, b| (a.0.y - near_y).abs().total_cmp(&(b.0.y - near_y).abs()));
    lights.truncate(20);
    for (at, lumens) in lights {
        commands.spawn((
            PointLight {
                intensity: lumens * 4.0,
                range: 9.0,
                radius: 0.15,
                shadow_maps_enabled: false,
                color: Color::srgb(1.0, 0.94, 0.85),
                ..default()
            },
            Transform::from_translation(at),
            ChildOf(frame),
        ));
    }
    for s in specs {
        match s {
            Spec::Hinged { kit, hinge, angle, kind, half, centre, flip_down, door_id, front } => {
                spawn_hinge(
                    commands, meshes, imats, frame, origin, rot, kit, hinge, angle, kind, half, centre, flip_down, door_id, front, None,
                );
            }
            Spec::Seat { at, yaw, half, centre, lie } => {
                commands.spawn((
                    Usable { origin, rot, centre, half, brot: Quat::from_rotation_y(yaw), what: Use::Seat { at, yaw, lie } },
                    ChildOf(frame),
                    Transform::default(),
                ));
            }
            Spec::Fixture { kind, spout, basin_centre, basin_half, fill_h, half, centre, yaw } => {
                let q = Quat::from_rotation_y(yaw);
                let water = imats.get(M::Water);
                let stream_mesh =
                    if matches!(kind, FixtureKind::Shower | FixtureKind::TubShower) { um.unit_cone.clone() } else { um.unit_cyl.clone() };
                let stream = commands
                    .spawn((
                        Mesh3d(stream_mesh),
                        MeshMaterial3d(water.clone()),
                        Transform::from_translation(spout),
                        Visibility::Hidden,
                        bevy::light::NotShadowCaster,
                        ChildOf(frame),
                    ))
                    .id();
                let surface = commands
                    .spawn((
                        Mesh3d(um.unit_quad.clone()),
                        MeshMaterial3d(water),
                        Transform::from_translation(basin_centre).with_rotation(q).with_scale(Vec3::new(
                            basin_half.x * 2.0,
                            1.0,
                            basin_half.z * 2.0,
                        )),
                        if kind == FixtureKind::Toilet { Visibility::Inherited } else { Visibility::Hidden },
                        bevy::light::NotShadowCaster,
                        ChildOf(frame),
                    ))
                    .id();
                commands.spawn((
                    Usable { origin, rot, centre, half: half + Vec3::splat(0.03), brot: q, what: Use::Fixture },
                    Water { kind, on: false, level: 0.0, t: 0.0, stream, surface, spout, basin: basin_centre, fill_h },
                    Transform::default(),
                    ChildOf(frame),
                ));
            }
            Spec::Mirror { centre, yaw, w, h } => {
                let q = Quat::from_rotation_y(yaw);
                let n = q * Vec3::NEG_Z;
                let c = centre + n * 0.003;
                commands.spawn((
                    Mesh3d(um.unit_quad.clone()),
                    MeshMaterial3d(um.mirror_dull.clone()),
                    // Plane3d faces +Y: stand it up to face n.
                    Transform::from_translation(c)
                        .with_rotation(q * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(w, 1.0, h)),
                    MirrorQuad { origin, rot, centre: c, normal: n, w, h, live: false },
                    bevy::light::NotShadowCaster,
                    ChildOf(frame),
                ));
            }
            Spec::Screen { centre, yaw, w, h } => {
                let q = Quat::from_rotation_y(yaw);
                let n = q * Vec3::NEG_Z;
                let mesh = commands
                    .spawn((
                        Mesh3d(um.unit_quad.clone()),
                        MeshMaterial3d(um.dark_screen.clone()),
                        Transform::from_translation(centre + n * 0.004)
                            .with_rotation(q * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
                            .with_scale(Vec3::new(w, 1.0, h)),
                        ChildOf(frame),
                    ))
                    .id();
                commands.spawn((
                    Usable { origin, rot, centre, half: Vec3::new(w * 0.5 + 0.02, h * 0.5 + 0.02, 0.06), brot: q, what: Use::Screen },
                    ScreenState { on: false, mesh },
                    Transform::default(),
                    ChildOf(frame),
                ));
            }
            Spec::Light { .. } | Spec::Conveyor { .. } => {}
        }
    }
    if let Some(l) = lift {
        spawn_lift(commands, meshes, imats, frame, l, origin, rot, base);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_lift(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    imats: &InteriorMats,
    frame: Entity,
    l: &LiftData,
    origin: DVec3,
    rot: Quat,
    base: f64,
) {
    if l.floors.is_empty() {
        return;
    }
    let half_depth = l.centre.distance(l.door_mid).max(0.6);
    let depth = half_depth * 2.0;
    let width = if (depth - l.size.y).abs() < (depth - l.size.x).abs() { l.size.x } else { l.size.y };
    let fwd = Vec3::new(l.door_dir.x, 0.0, l.door_dir.y).normalize_or(Vec3::Z);
    let side = Vec3::new(l.door_along.x, 0.0, l.door_along.y).normalize_or(Vec3::X);
    let pt = |s: f32, y: f32, f: f32| side * s + Vec3::Y * y + fwd * f;
    let y0 = l.floors[0];
    let car =
        commands.spawn((Transform::from_translation(Vec3::new(l.centre.x, y0, l.centre.y)), Visibility::Inherited, ChildOf(frame))).id();
    // The car: floor, walls, ceiling light, handrail.
    let (hw, hd) = (width * 0.5 - 0.04, half_depth - 0.04);
    let mut k = Kit::default();
    let yaw = yaw_to(side);
    let r = Quat::from_rotation_y(yaw);
    let bx = |k: &mut Kit, m: M, c: Vec3, size: Vec3, col: [f32; 4]| k.cube(m, Affine3A::from_rotation_translation(r, c), size, col);
    bx(&mut k, M::Marble, pt(0.0, -0.03, 0.0), Vec3::new(hw * 2.0, 0.06, hd * 2.0), WHITE);
    bx(&mut k, M::Metal, pt(0.0, 2.45, 0.0), Vec3::new(hw * 2.0, 0.04, hd * 2.0), rgb(0.8, 0.8, 0.82));
    bx(&mut k, M::LightPanel, pt(0.0, 2.42, 0.0), Vec3::new(hw * 1.2, 0.02, hd * 1.2), WHITE);
    bx(&mut k, M::Metal, pt(0.0, 1.22, -hd), Vec3::new(hw * 2.0, 2.44, 0.03), rgb(0.72, 0.73, 0.75));
    for s in [-1.0f32, 1.0] {
        bx(&mut k, M::Metal, pt(s * hw, 1.22, 0.0), Vec3::new(0.03, 2.44, hd * 2.0), rgb(0.72, 0.73, 0.75));
        bx(&mut k, M::Chrome, pt(s * (hw - 0.06), 0.9, -0.1), Vec3::new(0.03, 0.03, hd * 1.5), WHITE);
    }
    bx(&mut k, M::Chrome, pt(0.0, 0.9, -hd + 0.06), Vec3::new(hw * 1.6, 0.03, 0.03), WHITE);
    // Front returns either side of the doorway.
    for s in [-1.0f32, 1.0] {
        bx(&mut k, M::Metal, pt(s * (hw + 0.55) * 0.5, 1.22, hd), Vec3::new((hw - 0.55).max(0.05), 2.44, 0.04), rgb(0.72, 0.73, 0.75));
    }
    bx(
        &mut k,
        M::Plastic,
        pt(hw - 0.025, 1.15, hd - 0.35),
        Vec3::new(0.01, 0.06 + 0.09 * l.floors.len() as f32, 0.14),
        rgb(0.1, 0.1, 0.11),
    );
    spawn_kit(commands, meshes, imats, car, k);
    // Floor buttons.
    for (i, _) in l.floors.iter().enumerate() {
        let rel = pt(hw - 0.035, 1.15 - 0.045 * (l.floors.len() as f32 - 1.0) + 0.09 * i as f32, hd - 0.35);
        let mut b = Kit::default();
        b.cyl(
            M::LightPanel,
            Affine3A::from_rotation_translation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::IDENTITY, Vec3::ZERO),
            0.022,
            0.01,
            12,
            WHITE,
            true,
        );
        let be = commands.spawn((Transform::from_translation(rel).with_rotation(r), Visibility::Inherited, ChildOf(car))).id();
        spawn_kit(commands, meshes, imats, be, b);
        commands.spawn((
            Usable {
                origin,
                rot,
                centre: rel + Vec3::new(l.centre.x, y0, l.centre.y),
                half: Vec3::new(0.03, 0.035, 0.03),
                brot: r,
                what: Use::LiftButton { car, floor: i },
            },
            RideAlong { car, rel },
            Transform::default(),
            ChildOf(frame),
        ));
    }
    // Sliding doors: on the car, and on each floor's landing.
    let panel = |commands: &mut Commands, meshes: &mut Assets<Mesh>, parent: Entity, at: Vec3| -> Entity {
        let e = commands.spawn((Transform::from_translation(at), Visibility::Inherited, ChildOf(parent))).id();
        let mut k = Kit::default();
        k.cube(
            M::Metal,
            Affine3A::from_rotation_translation(r, Vec3::new(0.0, 1.07, 0.0)),
            Vec3::new(0.56, 2.14, 0.03),
            rgb(0.78, 0.79, 0.8),
        );
        spawn_kit(commands, meshes, imats, e, k);
        e
    };
    let car_doors = [panel(commands, meshes, car, pt(-0.275, 0.0, hd + 0.02)), panel(commands, meshes, car, pt(0.275, 0.0, hd + 0.02))];
    let mut hall_doors = Vec::new();
    let mid = Vec3::new(l.door_mid.x, 0.0, l.door_mid.y);
    for &fy in &l.floors {
        let at = mid + fwd * 0.05 + Vec3::Y * fy;
        hall_doors.push(([panel(commands, meshes, frame, at + side * -0.275), panel(commands, meshes, frame, at + side * 0.275)], at));
    }
    commands.entity(car).insert(LiftCar {
        id: l.id,
        base,
        floors: l.floors.clone(),
        at: 0,
        target: 0,
        y: y0,
        door: 0.0,
        dwell: 0.0,
        centre: l.centre,
        fwd,
        side,
        width,
        depth,
        car_doors,
        hall_doors,
    });
    for &(p, floor) in &l.calls {
        commands.spawn((
            Usable { origin, rot, centre: p, half: Vec3::new(0.06, 0.1, 0.05), brot: r, what: Use::LiftCall { car, floor } },
            Transform::default(),
            ChildOf(frame),
        ));
    }
}

// ------------------------------------------------------------------ aiming

fn ray_box(o: Vec3, d: Vec3, c: Vec3, half: Vec3, q: Quat) -> Option<f32> {
    let inv = q.inverse();
    let (o, d) = (inv * (o - c), inv * d);
    let mut t0 = f32::MIN;
    let mut t1 = f32::MAX;
    for i in 0..3 {
        let (oi, di, h) = (o[i], d[i], half[i]);
        if di.abs() < 1e-6 {
            if oi.abs() > h {
                return None;
            }
            continue;
        }
        let (a, b) = ((-h - oi) / di, (h - oi) / di);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t1 >= t0.max(0.0)).then_some(t0.max(0.0))
}

fn label(
    u: &Usable,
    hinges: &Query<&Hinge>,
    water: &Query<&Water>,
    screens: &Query<&ScreenState>,
    e: Entity,
    lifts: &Query<&LiftCar>,
) -> String {
    match u.what {
        Use::Hinge => {
            let Ok(h) = hinges.get(e) else { return String::new() };
            let verb = if h.open { "Close" } else { "Open" };
            let what = match h.kind {
                HingeKind::Door | HingeKind::ExteriorDoor => "door",
                HingeKind::Fridge => "fridge",
                HingeKind::Freezer => "freezer",
                HingeKind::Oven => "oven",
                HingeKind::Microwave => "microwave",
                HingeKind::Dishwasher => "dishwasher",
                HingeKind::Cabinet => "cabinet",
                HingeKind::Washer => "washer",
            };
            format!("{verb} {what}")
        }
        Use::Seat { lie, .. } => if lie { "Lie down" } else { "Sit" }.into(),
        Use::Fixture => {
            let Ok(w) = water.get(e) else { return String::new() };
            match (w.kind, w.on) {
                (FixtureKind::Toilet, _) => "Flush".into(),
                (FixtureKind::Shower | FixtureKind::TubShower, false) => "Turn on shower".into(),
                (FixtureKind::Shower | FixtureKind::TubShower, true) => "Turn off shower".into(),
                (FixtureKind::Tub, false) => "Run bath".into(),
                (FixtureKind::Tub, true) => "Turn off tap".into(),
                (_, false) => "Turn on tap".into(),
                (_, true) => "Turn off tap".into(),
            }
        }
        Use::Screen => if screens.get(e).is_ok_and(|s| s.on) { "Turn off screen" } else { "Turn on screen" }.into(),
        Use::LiftButton { car, floor } => {
            let ground = lifts.get(car).ok().and_then(|c| c.floors.iter().position(|y| y.abs() < 0.5)).unwrap_or(0);
            let n = floor as i32 - ground as i32;
            if n == 0 { "Floor G".into() } else { format!("Floor {n}") }
        }
        Use::LiftCall { .. } => "Call elevator".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn aim(
    viewer: Res<Viewer>,
    player: Res<crate::player::Player>,
    origin: Res<WorldOrigin>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::camera::MainCamera>>,
    lock: Res<crate::camera::MouseLock>,
    usables: Query<(Entity, &Usable)>,
    hinges: Query<&Hinge>,
    water: Query<&Water>,
    screens: Query<&ScreenState>,
    lifts: Query<&LiftCar>,
    walk: Res<Walkables>,
    mut out: ResMut<Aim>,
) {
    out.0 = None;
    if usables.is_empty() {
        return;
    }
    // The ray: through the cursor when it's free, else the middle of the screen.
    let (fwd, _, _) = viewer.basis_ecef();
    let mut o = viewer.ecef;
    let mut d = fwd;
    if !lock.0
        && let (Ok(w), Ok((cam, gt))) = (windows.single(), cams.single())
        && let Some(cp) = w.cursor_position()
        && let Ok(ray) = cam.viewport_to_world(gt, cp)
    {
        let to_ecef = origin.frame.ecef_to_local.transpose();
        o = origin.frame.origin + to_ecef * ray.origin.as_dvec3();
        d = (to_ecef * Vec3::from(ray.direction).as_dvec3()).normalize();
    }
    let eye = player.feet + (player.feet.normalize()) * crate::player::EYE_HEIGHT;
    let mut best: Option<(f32, Entity)> = None;
    for (e, u) in &usables {
        if (u.origin - eye).length() > 400.0 {
            continue;
        }
        let inv = u.rot.inverse();
        let ol = inv * (o - u.origin).as_vec3();
        let dl = inv * d.as_vec3();
        if let Some(t) = ray_box(ol, dl, u.centre, u.half, u.brot) {
            let hit = o + d * t as f64;
            if (hit - eye).length() > REACH_M as f64 + 0.3 {
                continue;
            }
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, e));
            }
        }
    }
    let Some((t, e)) = best else { return };
    // Not through a wall.
    let hit = o + d * (t as f64 - 0.08).max(0.0);
    if walk.solid_hit(Geodetic::from_ecef(o), Geodetic::from_ecef(hit)).is_some() {
        return;
    }
    let Ok((_, u)) = usables.get(e) else { return };
    let l = label(u, &hinges, &water, &screens, e, &lifts);
    if !l.is_empty() {
        out.0 = Some((e, l));
    }
}

#[allow(clippy::too_many_arguments)]
fn use_input(
    mouse: Res<ButtonInput<MouseButton>>,
    blocked: Res<crate::camera::InputBlocked>,
    menus: Res<crate::ui::Menus>,
    aim: Res<Aim>,
    usables: Query<&Usable>,
    mut hinges: Query<&mut Hinge>,
    mut water: Query<&mut Water>,
    mut screens: Query<&mut ScreenState>,
    mut lifts: Query<&mut LiftCar>,
    mut player: ResMut<crate::player::Player>,
    mut commands: Commands,
    um: Res<UseMats>,
) {
    if !mouse.just_pressed(MouseButton::Right) || blocked.pointer || menus.any_open() {
        return;
    }
    let Some((e, _)) = &aim.0 else { return };
    let Ok(u) = usables.get(*e) else { return };
    match u.what {
        Use::Hinge => {
            let Ok(h) = hinges.get(*e) else { return };
            let (open, id) = (!h.open, h.door_id);
            match id {
                // Double doors open together.
                Some(id) => {
                    for mut h in &mut hinges {
                        if h.door_id == Some(id) {
                            h.open = open;
                        }
                    }
                }
                None => {
                    if let Ok(mut h) = hinges.get_mut(*e) {
                        h.open = open;
                    }
                }
            }
        }
        Use::Seat { at, yaw, lie } => {
            let up = u.rot * Vec3::Y;
            let p = u.origin + (u.rot * at).as_dvec3();
            let drop = if lie { 0.05 } else { 0.44 };
            let fwd = u.rot * (Quat::from_rotation_y(yaw) * Vec3::NEG_Z);
            // Heading: degrees clockwise from north, in the frame at the seat.
            let f = usx_core::geo::EnuFrame::at(Geodetic::from_ecef(p));
            let fl = f.dir_to_local(fwd.as_dvec3());
            let heading = (fl.x.atan2(-fl.z).to_degrees() + 360.0) % 360.0;
            player.seat = Some(crate::player::Seat { feet: p - (up * drop).as_dvec3(), heading, lie, stand: player.feet });
            player.vel = DVec3::ZERO;
            player.mode = crate::player::Mode::Walk;
        }
        Use::Fixture => {
            if let Ok(mut w) = water.get_mut(*e) {
                if w.kind == FixtureKind::Toilet {
                    if !w.on {
                        w.on = true;
                        w.t = 0.0;
                    }
                } else {
                    w.on = !w.on;
                }
            }
        }
        Use::Screen => {
            if let Ok(mut s) = screens.get_mut(*e) {
                s.on = !s.on;
                let m = if s.on { um.lit_screen.clone() } else { um.dark_screen.clone() };
                commands.entity(s.mesh).insert(MeshMaterial3d(m));
            }
        }
        Use::LiftButton { car, floor } | Use::LiftCall { car, floor } => {
            if let Ok(mut c) = lifts.get_mut(car) {
                if c.target == floor && c.at == floor {
                    c.dwell = 0.0;
                }
                c.target = floor;
            }
        }
    }
}

fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn animate_hinges(time: Res<Time>, mut q: Query<(&mut Hinge, &mut Transform, &mut Usable)>, mut walk: ResMut<Walkables>) {
    let dt = time.delta_secs();
    for (mut h, mut tr, mut u) in &mut q {
        let target = if h.open { 1.0 } else { 0.0 };
        if (h.t - target).abs() < 1e-4 && tr.rotation == Quat::from_axis_angle(h.axis, h.angle * ease(h.t)) {
            continue;
        }
        let speed = if matches!(h.kind, HingeKind::Door | HingeKind::ExteriorDoor) { 1.6 } else { 2.5 };
        h.t = if h.t < target { (h.t + dt * speed).min(1.0) } else { (h.t - dt * speed).max(0.0) };
        let q = Quat::from_axis_angle(h.axis, h.angle * ease(h.t));
        tr.rotation = q;
        u.centre = h.hinge + q * (h.centre0 - h.hinge);
        u.brot = q * h.brot0;
        if let Some(id) = h.door_id {
            if h.open && h.t > 0.35 {
                walk.open_doors.insert(id);
            } else if !h.open {
                walk.open_doors.remove(&id);
            }
        }
    }
}

fn animate_water(time: Res<Time>, mut q: Query<&mut Water>, mut tr: Query<(&mut Transform, &mut Visibility)>) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    for mut w in &mut q {
        if !w.on && w.level <= 0.0 && w.kind != FixtureKind::Toilet {
            continue;
        }
        let (spout, basin, fill_h) = (w.spout, w.basin, w.fill_h);
        match w.kind {
            FixtureKind::Toilet => {
                if w.on {
                    w.t += dt;
                    if w.t > 5.0 {
                        w.on = false;
                        w.t = 0.0;
                    }
                }
                let t = w.t;
                let (dy, spin) = if !w.on {
                    (0.0, 0.0)
                } else if t < 2.0 {
                    (-0.07 * (t / 2.0), t * 6.0)
                } else {
                    (-0.07 * (1.0 - ((t - 2.0) / 3.0)), t * 3.0)
                };
                if let Ok((mut st, _)) = tr.get_mut(w.surface) {
                    st.translation = basin + Vec3::Y * dy;
                    st.rotation = Quat::from_rotation_y(spin) * st.rotation.normalize();
                    let _ = st.rotation;
                }
                continue;
            }
            FixtureKind::Shower | FixtureKind::TubShower => {
                w.level = if w.on { (w.level + dt * 0.5).min(0.06) } else { (w.level - dt * 0.05).max(0.0) };
            }
            FixtureKind::Tub => {
                w.level = if w.on { (w.level + dt / 40.0).min(0.9) } else { (w.level - dt / 25.0).max(0.0) };
            }
            _ => {
                w.level = if w.on { (w.level + dt / 18.0).min(0.6) } else { (w.level - dt / 6.0).max(0.0) };
            }
        }
        let surf_y = basin.y + w.level * fill_h.max(0.004) + 0.004;
        if let Ok((mut st, mut v)) = tr.get_mut(w.surface) {
            st.translation.y = surf_y;
            let want = if w.level > 0.002 { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
        if let Ok((mut st, mut v)) = tr.get_mut(w.stream) {
            let want = if w.on { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
            if w.on {
                let len = (spout.y - surf_y).max(0.05);
                let shower = matches!(w.kind, FixtureKind::Shower | FixtureKind::TubShower);
                let r = if shower { 0.24 } else { 0.0065 + 0.0015 * (now * 23.0).sin() };
                // The stream mesh rises from its base: put the base at the surface.
                st.translation = Vec3::new(spout.x, surf_y, spout.z);
                st.scale = Vec3::new(r, len, r);
                st.rotation = Quat::from_rotation_y(now * 2.0);
            }
        }
    }
}

fn animate_lifts(
    time: Res<Time>,
    mut cars: Query<(&mut LiftCar, &mut Transform), Without<RideAlong>>,
    mut panels: Query<&mut Transform, (Without<LiftCar>, Without<RideAlong>, Without<Usable>)>,
    mut riders: Query<(&RideAlong, &mut Usable)>,
    mut walk: ResMut<Walkables>,
) {
    let dt = time.delta_secs();
    for (mut c, mut tr) in &mut cars {
        let c = &mut *c;
        let ty = c.floors[c.target.min(c.floors.len() - 1)];
        let moving = (c.y - ty).abs() > 0.005;
        if moving {
            // Close up, then travel.
            c.door = (c.door - dt * 1.4).max(0.0);
            if c.door <= 0.0 {
                let dir = (ty - c.y).signum();
                let dist = (ty - c.y).abs();
                let v = (1.8f32).min(dist * 1.5 + 0.15);
                c.y += dir * (v * dt).min(dist);
                c.at = c.floors.iter().enumerate().min_by(|a, b| (a.1 - c.y).abs().total_cmp(&(b.1 - c.y).abs())).map(|x| x.0).unwrap_or(0);
                if (c.y - ty).abs() <= 0.005 {
                    c.y = ty;
                    c.at = c.target;
                    c.dwell = 0.0;
                }
            }
        } else {
            c.at = c.target;
            c.dwell += dt;
            let want = if c.dwell < 7.0 { 1.0 } else { 0.0 };
            c.door = if c.door < want { (c.door + dt * 1.4).min(1.0) } else { (c.door - dt * 1.4).max(0.0) };
        }
        tr.translation = Vec3::new(c.centre.x, c.y, c.centre.y);
        walk.lifts.insert(c.id, c.base + c.y as f64);
        let hd = c.depth * 0.5 - 0.04;
        let open = ease(c.door) * 0.52;
        for (i, pe) in c.car_doors.iter().enumerate() {
            let s = if i == 0 { -1.0 } else { 1.0 };
            if let Ok(mut pt) = panels.get_mut(*pe) {
                pt.translation = c.side * s * (0.275 + open) + c.fwd * (hd + 0.02);
            }
        }
        for (fi, (hall, at)) in c.hall_doors.iter().enumerate() {
            let o = if fi == c.at && !moving { open } else { 0.0 };
            for (i, pe) in hall.iter().enumerate() {
                let s = if i == 0 { -1.0 } else { 1.0 };
                if let Ok(mut pt) = panels.get_mut(*pe) {
                    pt.translation = *at + c.side * s * (0.275 + o);
                }
            }
            let id = c.id ^ (fi as u32 + 1);
            if fi == c.at && !moving && c.door > 0.6 {
                walk.open_doors.insert(id);
            } else {
                walk.open_doors.remove(&id);
            }
        }
    }
    for (r, mut u) in &mut riders {
        if let Ok((c, _)) = cars.get(r.car) {
            u.centre = Vec3::new(c.centre.x, c.y, c.centre.y) + r.rel;
        }
    }
}

// ------------------------------------------------------------------ mirrors

#[allow(clippy::too_many_arguments)]
fn mirror_camera(
    viewer: Res<Viewer>,
    origin: Res<WorldOrigin>,
    main: Query<(&Projection, &Exposure), (With<crate::camera::MainCamera>, Without<MirrorCamera>)>,
    mut cam: Query<(&mut Camera, &mut Transform, &mut Projection, &mut Exposure, &mut RenderTarget), With<MirrorCamera>>,
    mut mirrors: Query<(Entity, &mut MirrorQuad)>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut um: ResMut<UseMats>,
    mut images: ResMut<Assets<Image>>,
    mut mm: ResMut<Assets<MirrorMaterial>>,
    mut commands: Commands,
) {
    let Ok((mut camera, mut tr, mut proj, mut exp, mut target)) = cam.single_mut() else { return };
    let Ok((Projection::Perspective(main_p), main_e)) = main.single() else { return };
    let (fwd, _, _) = viewer.basis_ecef();
    // The nearest mirror in front of you, facing you.
    let mut best: Option<(f64, Entity)> = None;
    for (e, m) in &mirrors {
        let c = m.origin + (m.rot * m.centre).as_dvec3();
        let n = (m.rot * m.normal).as_dvec3();
        let to = viewer.ecef - c;
        let d = to.length();
        if d > 6.0 || to.dot(n) < 0.05 || fwd.dot(-to.normalize()) < 0.15 {
            continue;
        }
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, e));
        }
    }
    for (e, mut m) in &mut mirrors {
        let live = best.is_some_and(|b| b.1 == e);
        if m.live != live {
            m.live = live;
            if live {
                commands.entity(e).remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(um.mirror_live.clone()));
            } else {
                commands.entity(e).remove::<MeshMaterial3d<MirrorMaterial>>().insert(MeshMaterial3d(um.mirror_dull.clone()));
            }
        }
    }
    let Some((_, me)) = best else {
        if camera.is_active {
            camera.is_active = false;
        }
        return;
    };
    // Keep the reflection at half the window's resolution.
    if let Ok(w) = windows.single() {
        let want = UVec2::new((w.physical_width() / 2).max(16), (w.physical_height() / 2).max(16));
        let have = images.get(&um.mirror_image).map(|i| i.size()).unwrap_or(UVec2::ZERO);
        if have != want {
            let img = mirror_image(&mut images, want);
            images.remove(um.mirror_image.id());
            um.mirror_image = img.clone();
            *target = RenderTarget::Image(img.clone().into());
            if let Some(mut m) = mm.get_mut(&um.mirror_live) {
                m.image = img;
            }
        }
    }
    let Ok((_, m)) = mirrors.get(me) else { return };
    let cam_t = origin.local(viewer.ecef);
    let cam_r = crate::globe::viewer_rotation_in_origin(&viewer, &origin);
    let c = origin.local(m.origin + (m.rot * m.centre).as_dvec3());
    let n = origin.frame.dir_to_local((m.rot * m.normal).as_dvec3()).as_vec3().normalize();
    let main_t = Transform::from_translation(cam_t).with_rotation(cam_r);
    let refl = Mat4::from_translation(c) * Mat4::from_mat3a(bevy::math::reflection_matrix(n)) * Mat4::from_translation(-c);
    *tr = Transform::from_matrix(refl * main_t.to_matrix());
    let view_from_world = Mat3A::from_quat(cam_r).inverse();
    let plane_n = (view_from_world * Vec3A::from(-n)).normalize();
    let dist = n.dot(c - cam_t);
    *proj = Projection::Perspective(PerspectiveProjection { near_clip_plane: Vec3::from(plane_n).extend(dist), ..main_p.clone() });
    exp.ev100 = main_e.ev100;
    camera.is_active = true;
}

// ------------------------------------------------------------------ prompt

fn draw_prompt(
    mut contexts: EguiContexts,
    aim: Res<Aim>,
    menus: Res<crate::ui::Menus>,
    player: Res<crate::player::Player>,
    map: Res<crate::map::FullMap>,
) -> Result {
    if menus.any_open() || map.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let text = match (&aim.0, &player.seat) {
        (_, Some(_)) => Some("Move to get up".to_string()),
        (Some((_, l)), None) => Some(format!("Right-click: {l}")),
        _ => None,
    };
    if let Some(t) = text {
        egui::Area::new("use-prompt".into()).anchor(egui::Align2::CENTER_CENTER, [0.0, 46.0]).interactable(false).show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(140))
                .inner_margin(egui::Margin::symmetric(10, 5))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(t).color(egui::Color32::from_gray(235)));
                });
        });
    }
    if aim.0.is_some() {
        let c = ctx.content_rect().center();
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, "use-dot".into())).circle_filled(
            c,
            3.0,
            egui::Color32::from_white_alpha(220),
        );
    }
    Ok(())
}
