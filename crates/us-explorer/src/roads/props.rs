//! Roadside objects: built from simple shared parts (so Bevy batches them) and placed
//! by the road chunks. Each object is its own entity with its parts as children, ready
//! to be knocked over piece by piece in Phase 18.
//!
//! Local axes for every object: +y up, front facing -z (toward `heading`), +x to its right.

use super::build::PropKind;
use super::signs::{self, Atlas};
use crate::sun::DayNight;
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    /// Size in millimetres.
    Box(u16, u16, u16),
    /// Radius, height in millimetres.
    Cyl(u16, u16),
    Cone(u16, u16),
    Ball(u16),
    /// A flat square facing -z, the whole texture on it (signal symbols); side in mm.
    Quad(u16),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mat {
    Galvanized,
    SignalGreen,
    SignalBody,
    Wood,
    Concrete,
    Red,
    Yellow,
    Blue,
    Black,
    Orange,
    White,
    Glass,
    SignBack,
    LampHead,
    Steel,
    /// Solar panel glass.
    Panel,
    CameraGrey,
}

/// Something a system updates on a part.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum PartTag {
    /// Signal lamp: 0 red, 1 yellow, 2 green.
    SignalLamp(u8),
    /// Pedestrian signal: 0 hand, 1 walking person.
    PedLamp(u8),
}

/// A signal head (or pedestrian head), driven by its junction's cycle.
#[derive(Component, Clone, Copy, Debug)]
pub struct SignalHead {
    pub junction: i64,
    pub group: u8,
    pub ped: bool,
    pub state: u8,
}

/// Where a street lamp's light comes from (for the nearest few, real lights at night).
#[derive(Component)]
pub struct LampSource;

/// A placed roadside object.
#[derive(Component, Clone, Debug)]
pub struct RoadProp {
    pub id: u64,
}

struct Part {
    shape: Shape,
    mat: Mat,
    t: Transform,
    tag: Option<PartTag>,
}

fn mm(m: f32) -> u16 {
    (m * 1000.0).round().clamp(1.0, 65_000.0) as u16
}

fn bx(w: f32, h: f32, d: f32, mat: Mat, at: Vec3) -> Part {
    Part { shape: Shape::Box(mm(w), mm(h), mm(d)), mat, t: Transform::from_translation(at), tag: None }
}

fn cyl(r: f32, h: f32, mat: Mat, at: Vec3) -> Part {
    Part { shape: Shape::Cyl(mm(r), mm(h)), mat, t: Transform::from_translation(at), tag: None }
}

impl Part {
    /// Turn a cylinder to point along z (a lens or disc facing forward).
    fn rot_x(mut self) -> Self {
        self.t.rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        self
    }
}

/// Signal lamp colours (red, amber, green), saturated so they read as colours.
pub const SIGNAL_COLOURS: [Color; 3] = [Color::srgb(1.0, 0.04, 0.02), Color::srgb(1.0, 0.42, 0.0), Color::srgb(0.0, 0.9, 0.42)];
/// Brightness of each signal colour (nits, daytime units). Green is dimmer: the eye
/// (and the tone mapper) sees it as much brighter, and at the same level it washes out.
const SIGNAL_NITS: [f32; 3] = [9_000.0, 8_000.0, 3_500.0];
/// Pedestrian symbols: Portland orange hand, lunar white walking person.
pub const PED_COLOURS: [Color; 2] = [Color::srgb(1.0, 0.38, 0.02), Color::srgb(0.92, 0.95, 1.0)];

/// Standard sign faces painted once at startup.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StdSign {
    Stop,
    AllWay,
    Yield,
    Speed(u32),
    Bus,
    /// Crossbuck blades: "RAILROAD" and "CROSSING".
    Rail(u8),
    /// Reserved parking with the accessibility symbol.
    Accessible,
}

#[derive(Resource)]
pub struct PropAssets {
    meshes: HashMap<Shape, Handle<Mesh>>,
    mats: HashMap<Mat, Handle<StandardMaterial>>,
    std_faces: HashMap<StdSign, (Handle<Mesh>, f32, f32)>,
    std_mat: Handle<StandardMaterial>,
    pub lamp_on: [Handle<StandardMaterial>; 3],
    pub lamp_off: [Handle<StandardMaterial>; 3],
    pub ped_on: [Handle<StandardMaterial>; 2],
    pub ped_off: [Handle<StandardMaterial>; 2],
    cars: HashMap<(u8, u8), Handle<Mesh>>,
    car_mat: Handle<StandardMaterial>,
}

fn color_of(m: Mat) -> (Color, f32, f32) {
    // (colour, roughness, metallic)
    match m {
        Mat::Galvanized => (Color::srgb(0.62, 0.64, 0.66), 0.45, 0.8),
        Mat::SignalGreen => (Color::srgb(0.13, 0.2, 0.15), 0.6, 0.3),
        Mat::SignalBody => (Color::srgb(0.12, 0.12, 0.1), 0.6, 0.2),
        Mat::Wood => (Color::srgb(0.32, 0.24, 0.17), 0.95, 0.0),
        Mat::Concrete => (Color::srgb(0.6, 0.59, 0.56), 0.85, 0.0),
        Mat::Red => (Color::srgb(0.7, 0.07, 0.06), 0.5, 0.1),
        Mat::Yellow => (Color::srgb(0.9, 0.72, 0.05), 0.5, 0.1),
        Mat::Blue => (Color::srgb(0.05, 0.18, 0.5), 0.45, 0.2),
        Mat::Black => (Color::srgb(0.04, 0.04, 0.045), 0.55, 0.1),
        Mat::Orange => (Color::srgb(0.95, 0.35, 0.03), 0.55, 0.0),
        Mat::White => (Color::srgb(0.9, 0.9, 0.88), 0.6, 0.0),
        Mat::Glass => (Color::srgba(0.7, 0.8, 0.85, 0.25), 0.05, 0.0),
        Mat::SignBack => (Color::srgb(0.55, 0.56, 0.57), 0.5, 0.7),
        Mat::LampHead => (Color::srgb(0.3, 0.3, 0.32), 0.5, 0.5),
        Mat::Steel => (Color::srgb(0.5, 0.5, 0.52), 0.35, 0.9),
        Mat::Panel => (Color::srgb(0.05, 0.07, 0.12), 0.12, 0.3),
        Mat::CameraGrey => (Color::srgb(0.16, 0.16, 0.17), 0.5, 0.2),
    }
}

/// Quad facing -z (front) or +z (back) with an atlas rectangle, readable from that side.
pub fn face_mesh(w: f32, h: f32, r: [f32; 4], front: bool) -> Mesh {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let (z, n, xl, xr) = if front { (-0.012, -1.0, hw, -hw) } else { (0.012, 1.0, -hw, hw) };
    // Corners as seen by the reader: bottom-left, bottom-right, top-right, top-left.
    let pos = vec![[xl, -hh, z], [xr, -hh, z], [xr, hh, z], [xl, hh, z]];
    let uv = vec![[r[0], r[3]], [r[2], r[3]], [r[2], r[1]], [r[0], r[1]]];
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, n]; 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U16(vec![0, 1, 2, 0, 2, 3]));
    m
}

pub fn atlas_image(a: &Atlas) -> Image {
    let mut img = Image::new(
        Extent3d { width: a.canvas.w, height: a.canvas.h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        a.canvas.px.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        anisotropy_clamp: 4,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

pub fn sign_material(image: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color_texture: Some(image),
        alpha_mode: AlphaMode::Mask(0.5),
        perceptual_roughness: 0.55,
        reflectance: 0.4,
        cull_mode: None,
        double_sided: false,
        ..default()
    }
}

pub fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut m = HashMap::new();
    for mat in [
        Mat::Galvanized,
        Mat::SignalGreen,
        Mat::SignalBody,
        Mat::Wood,
        Mat::Concrete,
        Mat::Red,
        Mat::Yellow,
        Mat::Blue,
        Mat::Black,
        Mat::Orange,
        Mat::White,
        Mat::Glass,
        Mat::SignBack,
        Mat::LampHead,
        Mat::Steel,
        Mat::Panel,
        Mat::CameraGrey,
    ] {
        let (c, rough, metal) = color_of(mat);
        let mut sm = StandardMaterial { base_color: c, perceptual_roughness: rough, metallic: metal, ..default() };
        if mat == Mat::Glass {
            sm.alpha_mode = AlphaMode::Blend;
        }
        if mat == Mat::Panel {
            // Solar cells: a 6 x 10 grid of dark blue cells with silver busbars.
            let (w, h) = (96u32, 64u32);
            let mut px = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    let (cx, cy) = (x % 16, y % 11);
                    let gap = cx == 0 || cy == 0;
                    let bus = !gap && (cx == 5 || cx == 11);
                    let c: [u8; 4] = if gap {
                        [200, 205, 210, 255]
                    } else if bus {
                        [120, 125, 135, 255]
                    } else {
                        [18, 28, 58, 255]
                    };
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 4].copy_from_slice(&c);
                }
            }
            let img = Image::new(
                Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                TextureDimension::D2,
                px,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            sm.base_color = Color::WHITE;
            sm.base_color_texture = Some(images.add(img));
        }
        m.insert(mat, mats.add(sm));
    }
    let lamp = |mats: &mut Assets<StandardMaterial>, c: Color, on: bool| {
        mats.add(StandardMaterial {
            base_color: if on { c } else { c.darker(0.6) },
            emissive: if on { c.to_linear() * 0.0 } else { LinearRgba::BLACK },
            perceptual_roughness: 0.3,
            ..default()
        })
    };
    let lamp_on = SIGNAL_COLOURS.map(|c| lamp(&mut mats, c, true));
    let lamp_off = SIGNAL_COLOURS.map(|c| lamp(&mut mats, c.with_luminance(0.035), false));
    // Pedestrian heads: the symbol is the texture, lit through the emissive channel.
    let sym = |mats: &mut Assets<StandardMaterial>, images: &mut Assets<Image>, walk: bool, c: Color, on: bool| {
        let img = images.add(atlas_image(&Atlas { canvas: signs::ped_symbol(walk), rects: Vec::new() }));
        mats.add(StandardMaterial {
            base_color: if on { c } else { c.with_luminance(0.03) },
            base_color_texture: Some(img.clone()),
            emissive_texture: Some(img),
            emissive: LinearRgba::BLACK,
            perceptual_roughness: 0.4,
            ..default()
        })
    };
    let ped_on = [sym(&mut mats, &mut images, false, PED_COLOURS[0], true), sym(&mut mats, &mut images, true, PED_COLOURS[1], true)];
    let ped_off = [sym(&mut mats, &mut images, false, PED_COLOURS[0], false), sym(&mut mats, &mut images, true, PED_COLOURS[1], false)];

    // Standard signs in one shared atlas.
    let mut order: Vec<StdSign> =
        vec![StdSign::Stop, StdSign::AllWay, StdSign::Yield, StdSign::Bus, StdSign::Rail(0), StdSign::Rail(1), StdSign::Accessible];
    order.extend((5..=85).step_by(5).map(StdSign::Speed));
    let faces: Vec<signs::Face> = order
        .iter()
        .map(|s| match s {
            StdSign::Stop => signs::stop(),
            StdSign::AllWay => signs::all_way(),
            StdSign::Yield => signs::yield_sign(),
            StdSign::Bus => signs::bus_stop(),
            StdSign::Speed(n) => signs::speed_limit(*n),
            StdSign::Rail(k) => signs::crossbuck_blade(if *k == 0 { "RAILROAD" } else { "CROSSING" }),
            StdSign::Accessible => signs::accessible_parking(),
        })
        .collect();
    let atlas = signs::pack(faces).expect("standard signs fit");
    let img = images.add(atlas_image(&atlas));
    let std_mat = mats.add(sign_material(img));
    let mut std_faces = HashMap::new();
    for (s, (r, w, h)) in order.iter().zip(atlas.rects.iter()) {
        std_faces.insert(*s, (meshes.add(face_mesh(*w, *h, *r, true)), *w, *h));
    }
    // Car paint: glossy, with a clear coat; colours come from the meshes.
    let car_mat = mats.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.3,
        reflectance: 0.6,
        clearcoat: 0.7,
        clearcoat_perceptual_roughness: 0.08,
        ..default()
    });
    commands.insert_resource(PropAssets {
        meshes: HashMap::new(),
        mats: m,
        std_faces,
        std_mat,
        lamp_on,
        lamp_off,
        ped_on,
        ped_off,
        cars: HashMap::new(),
        car_mat,
    });
}

impl PropAssets {
    fn mesh(&mut self, meshes: &mut Assets<Mesh>, s: Shape) -> Handle<Mesh> {
        self.meshes
            .entry(s)
            .or_insert_with(|| {
                let f = |v: u16| v as f32 / 1000.0;
                match s {
                    Shape::Box(w, h, d) => meshes.add(Cuboid::new(f(w), f(h), f(d))),
                    Shape::Cyl(r, h) => meshes.add(Cylinder::new(f(r), f(h)).mesh().resolution(10)),
                    Shape::Cone(r, h) => meshes.add(Cone { radius: f(r), height: f(h) }.mesh().resolution(12)),
                    Shape::Ball(r) => meshes.add(Sphere::new(f(r)).mesh().uv(10, 6)),
                    Shape::Quad(a) => meshes.add(face_mesh(f(a), f(a), [0.0, 0.0, 1.0, 1.0], true)),
                }
            })
            .clone()
    }
}

/// Faces a prop needs from its chunk's sign atlas.
pub fn custom_faces(kind: &PropKind) -> Vec<signs::Face> {
    match kind {
        PropKind::StreetSign { names } => vec![signs::street_blade(&names[0]), signs::street_blade(&names[1])],
        PropKind::Shield { text } => vec![signs::shield(text)],
        PropKind::ExitSign { exit, text } | PropKind::Gantry { exit, text, .. } | PropKind::OverpassSign { exit, text } => {
            vec![signs::exit_panel(exit, text)]
        }
        PropKind::MileMarker { text } => vec![signs::mile_marker(text)],
        _ => Vec::new(),
    }
}

/// Collision for the player: (radius, height), or several posts for long things.
pub fn solids(kind: &PropKind) -> Vec<(Vec2, f32, f32)> {
    let one = |r: f32, h: f32| vec![(Vec2::ZERO, r, h)];
    match kind {
        PropKind::Signal { .. } | PropKind::Lamp { .. } | PropKind::UtilityPole | PropKind::Alpr => one(0.16, 6.0),
        PropKind::StreetSign { .. }
        | PropKind::Stop { .. }
        | PropKind::Yield
        | PropKind::Shield { .. }
        | PropKind::SpeedLimit { .. }
        | PropKind::PedSignal { .. } => one(0.06, 2.5),
        PropKind::Hydrant => one(0.16, 0.7),
        PropKind::Mailbox => one(0.3, 1.1),
        PropKind::Bollard => one(0.12, 1.0),
        PropKind::Cone => one(0.18, 0.7),
        PropKind::Barrel => one(0.32, 1.0),
        PropKind::Bench => vec![(Vec2::new(-0.5, 0.0), 0.3, 0.8), (Vec2::new(0.5, 0.0), 0.3, 0.8)],
        PropKind::ExitSign { .. } => vec![(Vec2::new(-1.6, 0.0), 0.15, 5.0), (Vec2::new(1.6, 0.0), 0.15, 5.0)],
        PropKind::Guardrail { len } => {
            let n = (len / 1.0).ceil() as i32;
            (0..=n).map(|k| (Vec2::new(0.0, -len * 0.5 + k as f32 * len / n as f32), 0.2, 0.8)).collect()
        }
        PropKind::BusStop { .. } => one(0.06, 2.5),
        PropKind::MileMarker { .. } => one(0.05, 1.5),
        PropKind::Crossbuck { .. } => one(0.1, 3.6),
        PropKind::Gantry { span, .. } => vec![(Vec2::new(-span * 0.5, 0.0), 0.3, 8.0), (Vec2::new(span * 0.5, 0.0), 0.3, 8.0)],
        PropKind::OverpassSign { .. } => Vec::new(),
        PropKind::ParkingSign => one(0.05, 2.3),
        PropKind::Car { body, .. } => {
            let h = super::cars::length(*body) * 0.5 - 0.9;
            vec![(Vec2::new(0.0, -h), 0.95, 1.5), (Vec2::ZERO, 0.95, 1.5), (Vec2::new(0.0, h), 0.95, 1.5)]
        }
    }
}

fn parts(kind: &PropKind) -> Vec<Part> {
    use Mat::*;
    let mut p = Vec::new();
    match kind {
        PropKind::Signal { mast, .. } => {
            let mast = *mast;
            p.push(cyl(0.13, 6.6, SignalGreen, Vec3::new(0.0, 3.3, 0.0)));
            p.push(bx(mast, 0.16, 0.16, SignalGreen, Vec3::new(mast * 0.5, 6.1, 0.0)));
            let heads: Vec<f32> = if mast > 6.0 { vec![mast * 0.5, mast - 0.4] } else { vec![mast - 0.3] };
            for x in heads {
                p.push(bx(0.36, 1.05, 0.3, SignalBody, Vec3::new(x, 5.45, 0.0)));
                // Backplate with a yellow border, like many US cities.
                p.push(bx(0.6, 1.3, 0.02, Yellow, Vec3::new(x, 5.45, 0.16)));
                p.push(bx(0.56, 1.26, 0.025, Black, Vec3::new(x, 5.45, 0.15)));
                for (i, y) in [5.78f32, 5.45, 5.12].into_iter().enumerate() {
                    p.push(Part {
                        shape: Shape::Cyl(110, 40),
                        mat: Black,
                        t: Transform::from_xyz(x, y, -0.16).with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                        tag: Some(PartTag::SignalLamp(i as u8)),
                    });
                }
            }
            // A second head on the pole for traffic stopped close.
            p.push(bx(0.3, 0.9, 0.26, SignalBody, Vec3::new(0.0, 3.0, -0.22)));
            for (i, y) in [3.28f32, 3.0, 2.72].into_iter().enumerate() {
                p.push(Part {
                    shape: Shape::Cyl(95, 40),
                    mat: Black,
                    t: Transform::from_xyz(0.0, y, -0.37).with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                    tag: Some(PartTag::SignalLamp(i as u8)),
                });
            }
        }
        PropKind::PedSignal { .. } => {
            p.push(cyl(0.06, 3.0, SignalGreen, Vec3::new(0.0, 1.5, 0.0)));
            // Faces across the street (+x): a housing with the hand above the person.
            p.push(bx(0.24, 0.72, 0.38, SignalBody, Vec3::new(0.14, 2.62, 0.0)));
            p.push(bx(0.2, 0.03, 0.38, SignalBody, Vec3::new(0.3, 2.99, 0.0)));
            for (i, y) in [2.8f32, 2.44].into_iter().enumerate() {
                p.push(Part {
                    shape: Shape::Quad(320),
                    mat: Black,
                    t: Transform::from_xyz(0.265, y, 0.0).with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)),
                    tag: Some(PartTag::PedLamp(i as u8)),
                });
            }
        }
        PropKind::Lamp { arm } => {
            let arm = *arm;
            p.push(cyl(0.1, 9.0, Galvanized, Vec3::new(0.0, 4.5, 0.0)));
            p.push(bx(0.08, 0.08, arm, Galvanized, Vec3::new(0.0, 8.9, -arm * 0.5)));
            p.push(bx(0.34, 0.12, 0.62, LampHead, Vec3::new(0.0, 8.86, -arm)));
        }
        PropKind::StreetSign { .. } => {
            // The pole stops under the lower blade; a cap bracket holds both blades.
            p.push(cyl(0.035, 2.86, Galvanized, Vec3::new(0.0, 1.43, 0.0)));
            p.push(cyl(0.045, 0.06, Galvanized, Vec3::new(0.0, 2.87, 0.0)));
        }
        PropKind::Stop { .. } | PropKind::Yield | PropKind::Shield { .. } | PropKind::SpeedLimit { .. } => {
            p.push(cyl(0.03, 2.7, Galvanized, Vec3::new(0.0, 1.35, 0.03)));
        }
        PropKind::ExitSign { .. } => {
            for x in [-1.6f32, 1.6] {
                p.push(cyl(0.12, 5.0, Galvanized, Vec3::new(x, 2.5, 0.08)));
            }
            p.push(bx(4.3, 2.3, 0.05, SignBack, Vec3::new(0.0, 4.0, 0.035)));
        }
        PropKind::Guardrail { len } => {
            let len = *len;
            let n = (len / 2.0).round().max(1.0) as i32;
            for k in 0..n {
                let z = -len * 0.5 + (k as f32 + 0.5) * len / n as f32;
                p.push(bx(0.15, 0.75, 0.15, Steel, Vec3::new(0.0, 0.375, z)));
            }
            p.push(bx(0.06, 0.32, len, Galvanized, Vec3::new(-0.1, 0.6, 0.0)));
        }
        PropKind::Cone => {
            p.push(Part { shape: Shape::Cone(170, 700), mat: Orange, t: Transform::from_xyz(0.0, 0.37, 0.0), tag: None });
            p.push(bx(0.38, 0.03, 0.38, Black, Vec3::new(0.0, 0.015, 0.0)));
            p.push(cyl(0.1, 0.08, White, Vec3::new(0.0, 0.47, 0.0)));
        }
        PropKind::Barrel => {
            p.push(cyl(0.3, 1.0, Orange, Vec3::new(0.0, 0.5, 0.0)));
            for y in [0.35f32, 0.7] {
                p.push(cyl(0.305, 0.12, White, Vec3::new(0.0, y, 0.0)));
            }
        }
        PropKind::Alpr => {
            // Flock Safety style unit: a slim black pole with a base plate and cap; a solar
            // panel in an aluminium frame on a tilt bracket at the top; the camera (a
            // small dark housing with a sun hood, lens and infrared ring) on a short arm
            // aimed down at the lane; a cable loop between them.
            use std::f32::consts::FRAC_PI_2;
            let rx = |a: f32| Quat::from_rotation_x(a);
            p.push(cyl(0.18, 0.02, Black, Vec3::new(0.0, 0.01, 0.0)));
            p.push(cyl(0.055, 2.4, Black, Vec3::new(0.0, 1.2, 0.0)));
            p.push(cyl(0.045, 2.2, Black, Vec3::new(0.0, 3.5, 0.0)));
            p.push(cyl(0.05, 0.04, Black, Vec3::new(0.0, 4.62, 0.0)));
            // Panel: tilt bracket, frame, cells (textured), facing up and slightly back.
            p.push(bx(0.06, 0.24, 0.06, Black, Vec3::new(0.0, 4.72, 0.04)));
            let tilt = rx(-0.62);
            let c = Vec3::new(0.0, 4.86, 0.1);
            p.push(Part {
                shape: Shape::Box(620, 35, 470),
                mat: Galvanized,
                t: Transform::from_translation(c).with_rotation(tilt),
                tag: None,
            });
            p.push(Part {
                shape: Shape::Quad(560),
                mat: Panel,
                t: Transform::from_translation(c + tilt * Vec3::new(0.0, 0.02, 0.0))
                    .with_rotation(tilt * rx(-FRAC_PI_2))
                    .with_scale(Vec3::new(1.0, 0.76, 1.0)),
                tag: None,
            });
            // Arm toward the road and the camera.
            p.push(bx(0.04, 0.04, 0.34, Black, Vec3::new(0.0, 3.86, -0.17)));
            p.push(bx(0.06, 0.1, 0.06, Black, Vec3::new(0.0, 3.82, -0.33)));
            let cam = Vec3::new(0.0, 3.72, -0.38);
            let aim = rx(0.18);
            let at = |v: Vec3| cam + aim * v;
            p.push(Part {
                shape: Shape::Box(130, 125, 230),
                mat: CameraGrey,
                t: Transform::from_translation(at(Vec3::ZERO)).with_rotation(aim),
                tag: None,
            });
            p.push(Part {
                shape: Shape::Box(150, 14, 270),
                mat: CameraGrey,
                t: Transform::from_translation(at(Vec3::new(0.0, 0.07, -0.03))).with_rotation(aim),
                tag: None,
            });
            for x in [-0.07f32, 0.07] {
                p.push(Part {
                    shape: Shape::Box(12, 70, 120),
                    mat: CameraGrey,
                    t: Transform::from_translation(at(Vec3::new(x, 0.035, -0.12))).with_rotation(aim),
                    tag: None,
                });
            }
            p.push(Part {
                shape: Shape::Box(120, 115, 8),
                mat: Black,
                t: Transform::from_translation(at(Vec3::new(0.0, 0.0, -0.118))).with_rotation(aim),
                tag: None,
            });
            p.push(Part {
                shape: Shape::Cyl(38, 14),
                mat: Black,
                t: Transform::from_translation(at(Vec3::new(0.0, -0.005, -0.124))).with_rotation(aim * rx(FRAC_PI_2)),
                tag: None,
            });
            p.push(Part {
                shape: Shape::Cyl(24, 6),
                mat: Glass,
                t: Transform::from_translation(at(Vec3::new(0.0, -0.005, -0.133))).with_rotation(aim * rx(FRAC_PI_2)),
                tag: None,
            });
            // Cable down the arm into the pole.
            p.push(cyl(0.008, 0.25, Black, Vec3::new(0.035, 3.97, -0.05)));
        }
        PropKind::MileMarker { .. } => {
            p.push(bx(0.05, 1.5, 0.03, Galvanized, Vec3::new(0.0, 0.75, 0.02)));
        }
        PropKind::Crossbuck { lights } => {
            p.push(cyl(0.07, 3.9, White, Vec3::new(0.0, 1.95, 0.0)));
            if *lights {
                p.push(bx(1.5, 0.1, 0.1, Black, Vec3::new(0.0, 2.6, 0.0)));
                for x in [-0.6f32, 0.6] {
                    p.push(cyl(0.3, 0.02, Black, Vec3::new(x, 2.6, -0.06)).rot_x());
                    p.push(cyl(0.15, 0.06, Red, Vec3::new(x, 2.6, -0.1)).rot_x());
                    p.push(bx(0.34, 0.08, 0.2, Black, Vec3::new(x, 2.78, -0.14)));
                }
            }
        }
        PropKind::Gantry { span, .. } => {
            let span = *span;
            for x in [-span * 0.5, span * 0.5] {
                p.push(cyl(0.25, 7.9, Galvanized, Vec3::new(x, 3.95, 0.0)));
                p.push(cyl(0.45, 0.6, Concrete, Vec3::new(x, 0.1, 0.0)));
            }
            // A box truss: two chords with cross members.
            for z in [-0.35f32, 0.35] {
                for y in [6.95f32, 7.75] {
                    p.push(bx(span, 0.1, 0.1, Galvanized, Vec3::new(0.0, y, z)));
                }
            }
            let n = (span / 1.6).round().max(2.0) as i32;
            for k in 0..=n {
                let x = -span * 0.5 + k as f32 * span / n as f32;
                p.push(bx(0.06, 0.8, 0.06, Galvanized, Vec3::new(x, 7.35, -0.35)));
                p.push(bx(0.06, 0.8, 0.06, Galvanized, Vec3::new(x, 7.35, 0.35)));
            }
            p.push(bx(4.3, 2.3, 0.06, SignBack, Vec3::new(-(span * 0.5 - 3.2).max(0.0), 6.6, -0.42)));
        }
        PropKind::OverpassSign { .. } => {
            p.push(bx(4.3, 2.3, 0.06, SignBack, Vec3::new(0.0, 0.6, 0.04)));
            for x in [-1.5f32, 1.5] {
                p.push(bx(0.1, 2.0, 0.12, Steel, Vec3::new(x, 0.6, 0.1)));
            }
        }
        PropKind::UtilityPole => {
            p.push(cyl(0.15, 11.0, Wood, Vec3::new(0.0, 5.5, 0.0)));
            p.push(bx(2.4, 0.1, 0.1, Wood, Vec3::new(0.0, 10.4, 0.0)));
            for x in [-1.0f32, 0.0, 1.0] {
                p.push(cyl(0.05, 0.18, White, Vec3::new(x, 10.55, 0.0)));
            }
        }
        PropKind::Hydrant => {
            p.push(cyl(0.12, 0.55, Red, Vec3::new(0.0, 0.275, 0.0)));
            p.push(Part { shape: Shape::Ball(125), mat: Red, t: Transform::from_xyz(0.0, 0.55, 0.0), tag: None });
            p.push(Part {
                shape: Shape::Cyl(50, 340),
                mat: Red,
                t: Transform::from_xyz(0.0, 0.38, 0.0).with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
                tag: None,
            });
            p.push(cyl(0.16, 0.05, Red, Vec3::new(0.0, 0.03, 0.0)));
        }
        PropKind::Mailbox => {
            p.push(bx(0.48, 0.85, 0.5, Blue, Vec3::new(0.0, 0.43, 0.0)));
            p.push(Part {
                shape: Shape::Cyl(240, 500),
                mat: Blue,
                t: Transform::from_xyz(0.0, 0.86, 0.0).with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                tag: None,
            });
        }
        PropKind::BusStop { shelter, bench } => {
            p.push(cyl(0.035, 2.8, Galvanized, Vec3::new(0.0, 1.4, 0.0)));
            if *shelter {
                p.push(bx(3.2, 0.08, 1.6, SignBack, Vec3::new(1.8, 2.55, 0.5)));
                p.push(bx(3.2, 2.3, 0.03, Glass, Vec3::new(1.8, 1.3, 1.25)));
                for x in [0.3f32, 3.3] {
                    p.push(bx(0.06, 2.55, 0.06, Galvanized, Vec3::new(x, 1.27, 1.25)));
                    p.push(bx(0.06, 2.55, 0.06, Galvanized, Vec3::new(x, 1.27, -0.25)));
                }
            }
            if *bench || *shelter {
                p.push(bx(1.6, 0.06, 0.42, Wood, Vec3::new(1.8, 0.45, 0.95)));
                for x in [1.2f32, 2.4] {
                    p.push(bx(0.06, 0.45, 0.4, Black, Vec3::new(x, 0.22, 0.95)));
                }
            }
        }
        PropKind::Bench => {
            p.push(bx(1.6, 0.06, 0.45, Wood, Vec3::new(0.0, 0.45, 0.0)));
            p.push(bx(1.6, 0.36, 0.05, Wood, Vec3::new(0.0, 0.75, 0.22)));
            for x in [-0.65f32, 0.65] {
                p.push(bx(0.06, 0.45, 0.45, Black, Vec3::new(x, 0.22, 0.0)));
            }
        }
        PropKind::Bollard => {
            p.push(cyl(0.1, 1.0, Yellow, Vec3::new(0.0, 0.5, 0.0)));
            p.push(cyl(0.102, 0.08, Black, Vec3::new(0.0, 0.85, 0.0)));
        }
        PropKind::ParkingSign => {
            p.push(cyl(0.03, 2.2, Galvanized, Vec3::new(0.0, 1.1, 0.03)));
        }
        PropKind::Car { .. } => {}
    }
    p
}

/// Spawn one object (as a child of its chunk). `faces` are this prop's rects in the
/// chunk's atlas, with `atlas_mat` the chunk's sign material.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    assets: &mut PropAssets,
    meshes: &mut Assets<Mesh>,
    parent: Entity,
    kind: &PropKind,
    id: u64,
    t: Transform,
    faces: &[([f32; 4], f32, f32)],
    atlas_mat: Option<&Handle<StandardMaterial>>,
) -> Entity {
    let e = commands.spawn((RoadProp { id }, t, Visibility::Inherited, ChildOf(parent))).id();
    let mut children: Vec<(Handle<Mesh>, Handle<StandardMaterial>, Transform, Option<PartTag>)> = Vec::new();
    for part in parts(kind) {
        let m = assets.mesh(meshes, part.shape);
        let mat = assets.mats[&part.mat].clone();
        children.push((m, mat, part.t, part.tag));
    }
    let back = assets.mats[&Mat::SignBack].clone();
    let std = |s: StdSign, assets: &PropAssets| assets.std_faces.get(&s).cloned();
    let std_face = |s: StdSign, y: f32, children: &mut Vec<_>, assets: &mut PropAssets, meshes: &mut Assets<Mesh>| {
        if let Some((m, w, h)) = std(s, assets) {
            children.push((m, assets.std_mat.clone(), Transform::from_xyz(0.0, y, 0.0), None));
            let bm = assets.mesh(meshes, Shape::Box(mm(w * 0.98), mm(h * 0.98), 4));
            children.push((bm, back.clone(), Transform::from_xyz(0.0, y, 0.002), None));
        }
    };
    match kind {
        PropKind::Stop { all_way } => {
            std_face(StdSign::Stop, 2.3, &mut children, assets, meshes);
            if *all_way {
                std_face(StdSign::AllWay, 1.82, &mut children, assets, meshes);
            }
        }
        PropKind::Yield => std_face(StdSign::Yield, 2.2, &mut children, assets, meshes),
        PropKind::SpeedLimit { mph } => std_face(StdSign::Speed((mph / 5 * 5).clamp(5, 85)), 2.1, &mut children, assets, meshes),
        PropKind::BusStop { .. } => {
            // The sign stands across the curb, read by traffic approaching the stop
            // (the shelter faces the road).
            if let Some((m, w, h)) = std(StdSign::Bus, assets) {
                let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
                children.push((m, assets.std_mat.clone(), Transform::from_xyz(0.0, 2.4, 0.0).with_rotation(rot), None));
                let bm = assets.mesh(meshes, Shape::Box(mm(w * 0.98), mm(h * 0.98), 4));
                children.push((bm, back.clone(), Transform::from_xyz(0.002, 2.4, 0.0).with_rotation(rot), None));
            }
        }
        PropKind::ParkingSign => std_face(StdSign::Accessible, 1.95, &mut children, assets, meshes),
        PropKind::Car { body, paint } => {
            let m = assets.cars.entry((*body, *paint)).or_insert_with(|| meshes.add(super::cars::mesh(*body, *paint))).clone();
            children.push((m, assets.car_mat.clone(), Transform::IDENTITY, None));
        }
        PropKind::Crossbuck { .. } => {
            // Two blades crossed at 90 degrees, readable from the front.
            for (k, a) in [(0u8, 0.785f32), (1, -0.785)] {
                if let Some((m, w, h)) = std(StdSign::Rail(k), assets) {
                    let rot = Quat::from_rotation_z(a);
                    children.push((
                        m,
                        assets.std_mat.clone(),
                        Transform::from_xyz(0.0, 3.35, -0.08 - k as f32 * 0.01).with_rotation(rot),
                        None,
                    ));
                    let bm = assets.mesh(meshes, Shape::Box(mm(w * 0.98), mm(h * 0.98), 4));
                    children.push((bm, back.clone(), Transform::from_xyz(0.0, 3.35, -0.07 + k as f32 * 0.01).with_rotation(rot), None));
                }
            }
        }
        _ => {}
    }
    if let Some(am) = atlas_mat {
        match kind {
            PropKind::StreetSign { .. } => {
                // Two blades crossing at right angles, readable from both sides.
                for (k, &(r, w, h)) in faces.iter().enumerate().take(2) {
                    let rot = if k == 0 { Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) } else { Quat::IDENTITY };
                    let y = if k == 0 { 3.0 } else { 3.25 };
                    for front in [true, false] {
                        let m = meshes.add(face_mesh(w, h, r, front));
                        children.push((m, am.clone(), Transform::from_xyz(0.0, y, 0.0).with_rotation(rot), None));
                    }
                }
            }
            PropKind::Shield { .. } => {
                if let Some(&(r, w, h)) = faces.first() {
                    children.push((meshes.add(face_mesh(w, h, r, true)), am.clone(), Transform::from_xyz(0.0, 2.1, 0.0), None));
                    let bm = assets.mesh(meshes, Shape::Box(mm(w), mm(h), 4));
                    children.push((bm, back.clone(), Transform::from_xyz(0.0, 2.1, 0.002), None));
                }
            }
            PropKind::ExitSign { .. } => {
                if let Some(&(r, w, h)) = faces.first() {
                    children.push((meshes.add(face_mesh(w, h, r, true)), am.clone(), Transform::from_xyz(0.0, 4.0, 0.0), None));
                }
            }
            PropKind::Gantry { span, .. } => {
                if let Some(&(r, w, h)) = faces.first() {
                    let x = -(span * 0.5 - 3.2).max(0.0);
                    children.push((meshes.add(face_mesh(w, h, r, true)), am.clone(), Transform::from_xyz(x, 6.6, -0.46), None));
                }
            }
            PropKind::OverpassSign { .. } => {
                if let Some(&(r, w, h)) = faces.first() {
                    children.push((meshes.add(face_mesh(w, h, r, true)), am.clone(), Transform::from_xyz(0.0, 0.6, 0.0), None));
                }
            }
            PropKind::MileMarker { .. } => {
                if let Some(&(r, w, h)) = faces.first() {
                    children.push((meshes.add(face_mesh(w, h, r, true)), am.clone(), Transform::from_xyz(0.0, 1.2, 0.0), None));
                    let bm = assets.mesh(meshes, Shape::Box(mm(w), mm(h), 4));
                    children.push((bm, back.clone(), Transform::from_xyz(0.0, 1.2, 0.003), None));
                }
            }
            _ => {}
        }
    }
    for (m, mat, t, tag) in children {
        let mut ec = commands.spawn((Mesh3d(m), MeshMaterial3d(mat), t, ChildOf(e)));
        if let Some(tag) = tag {
            ec.insert(tag);
        }
        if matches!(kind, PropKind::Lamp { .. }) && t.translation.y > 8.5 {
            ec.insert(LampSource);
        }
    }
    match kind {
        PropKind::Signal { junction, group, .. } => {
            commands.entity(e).insert(SignalHead { junction: *junction, group: *group, ped: false, state: u8::MAX });
        }
        PropKind::PedSignal { junction, group } => {
            commands.entity(e).insert(SignalHead { junction: *junction, group: *group, ped: true, state: u8::MAX });
        }
        _ => {}
    }
    e
}

// ------------------------------------------------------------------ signals

/// Phase timing: green, yellow, all-red (seconds).
pub const GREEN: f32 = 24.0;
pub const AMBER: f32 = 3.6;
pub const ALL_RED: f32 = 1.4;

/// Signal state for one approach group at time `t`: 0 red, 1 yellow, 2 green; and for
/// pedestrians crossing that approach: 0 don't walk, 1 walk, 2 flashing hand.
pub fn signal_state(groups: u8, offset: f32, group: u8, t: f64) -> (u8, u8) {
    let slot = GREEN + AMBER + ALL_RED;
    let cycle = slot * groups.max(2) as f32;
    let pt = ((t + offset as f64).rem_euclid(cycle as f64)) as f32;
    let current = (pt / slot) as u8;
    let within = pt - current as f32 * slot;
    let veh = if current != group {
        0
    } else if within < GREEN {
        2
    } else if within < GREEN + AMBER {
        1
    } else {
        0
    };
    // People cross an approach while the traffic beside them (another group) has green.
    let ped = if current != group && within < GREEN * 0.55 {
        1
    } else if current != group && within < GREEN {
        2
    } else {
        0
    };
    (veh, ped)
}

#[derive(Resource, Default)]
pub struct Controllers(pub HashMap<i64, (u8, f32)>);

#[allow(clippy::type_complexity)]
pub fn run_signals(
    time: Res<Time>,
    controllers: Res<Controllers>,
    assets: Option<Res<PropAssets>>,
    day: Res<DayNight>,
    mut heads: Query<(&mut SignalHead, &Children)>,
    mut lamps: Query<(&PartTag, &mut MeshMaterial3d<StandardMaterial>)>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut last_dark: Local<f32>,
) {
    let Some(assets) = assets else { return };
    // Lamp brightness follows exposure so lit lamps look right day and night.
    if (*last_dark - day.dark).abs() > 0.01 {
        *last_dark = day.dark;
        let k = 2.0f32.powf(-10.0 * day.dark);
        let on = |c: Color, s: f32| c.to_linear() * (s * k);
        // About as bright as a real LED head: well above a sunlit wall, but not so far
        // above that the tone mapper bleaches the colour to white.
        for ((h, c), n) in assets.lamp_on.iter().zip(SIGNAL_COLOURS).zip(SIGNAL_NITS) {
            if let Some(mut m) = mats.get_mut(h) {
                m.emissive = on(c, n);
            }
        }
        // Pedestrian symbols are small and must read in full sun.
        for (h, c) in assets.ped_on.iter().zip(PED_COLOURS) {
            if let Some(mut m) = mats.get_mut(h) {
                m.emissive = on(c, 30_000.0);
            }
        }
    }
    let t = time.elapsed_secs_f64();
    let blink = (t * 1.0).fract() < 0.5;
    for (mut head, children) in &mut heads {
        let (groups, offset) = controllers.0.get(&head.junction).copied().unwrap_or((2, 0.0));
        let (veh, ped) = signal_state(groups, offset, head.group, t);
        let state = if head.ped { if ped == 2 && !blink { 3 } else { ped } } else { veh };
        if state == head.state {
            continue;
        }
        head.state = state;
        for c in children.iter() {
            if let Ok((tag, mut mat)) = lamps.get_mut(c) {
                let want = match *tag {
                    PartTag::SignalLamp(i) => {
                        if i as usize
                            == [0usize, 1, 2][match state {
                                0 => 0,
                                1 => 1,
                                _ => 2,
                            }]
                            && !head.ped
                        {
                            assets.lamp_on[i as usize].clone()
                        } else {
                            assets.lamp_off[i as usize].clone()
                        }
                    }
                    PartTag::PedLamp(i) => {
                        let lit = match state {
                            1 => i == 1,
                            0 | 2 => i == 0,
                            _ => false,
                        };
                        if lit { assets.ped_on[i as usize].clone() } else { assets.ped_off[i as usize].clone() }
                    }
                };
                if mat.0 != want {
                    mat.0 = want;
                }
            }
        }
    }
}

// ------------------------------------------------------------------ street lights

/// A few real point lights that follow the nearest street lamps after dark.
#[derive(Component)]
pub struct LampLight;

pub fn setup_lamp_lights(mut commands: Commands) {
    for _ in 0..24 {
        commands.spawn((
            LampLight,
            PointLight { intensity: 0.0, range: 45.0, radius: 0.2, color: Color::srgb(1.0, 0.86, 0.68), ..default() },
            Transform::default(),
            Visibility::Hidden,
        ));
    }
}

#[allow(clippy::type_complexity)]
pub fn run_lamps(
    day: Res<DayNight>,
    assets: Option<Res<PropAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    cams: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut sources: Query<(&GlobalTransform, &mut Transform), (With<LampSource>, Without<LampLight>)>,
    mut lights: Query<(&mut Transform, &mut PointLight, &mut Visibility), With<LampLight>>,
    mut timer: Local<f32>,
    time: Res<Time>,
    mut last: Local<f32>,
) {
    let Some(assets) = assets else { return };
    // On from sunset to sunrise (with a little margin).
    let on = 1.0 - ((day.elevation + 2.0) / 5.0).clamp(0.0, 1.0);
    if (*last - on).abs() > 0.01 || (on > 0.0 && (*last - day.dark).abs() > 0.02) {
        *last = on;
        if let Some(mut m) = mats.get_mut(&assets.mats[&Mat::LampHead]) {
            m.emissive = Color::srgb(1.0, 0.85, 0.62).to_linear() * (40_000.0 * on * 2.0f32.powf(-10.0 * day.dark));
        }
    }
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 0.4;
    let Some(cam) = cams.iter().next().map(|c| c.translation()) else { return };
    // Far lamp heads are smaller than a pixel and twinkle as you move; grow them with
    // distance so they stay about a pixel or two across.
    for (g, mut t) in &mut sources {
        let d = g.translation().distance(cam);
        let s = (d / 140.0).clamp(1.0, 7.0);
        if (t.scale.x - s).abs() > 0.05 * s {
            t.scale = Vec3::splat(s);
        }
    }
    let mut near: Vec<(f32, Vec3)> = if on > 0.0 {
        sources.iter().map(|(g, _)| (g.translation().distance_squared(cam), g.translation())).filter(|(d, _)| *d < 260.0 * 260.0).collect()
    } else {
        Vec::new()
    };
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut it = near.into_iter();
    for (mut t, mut l, mut v) in &mut lights {
        match it.next() {
            Some((_, p)) => {
                t.translation = p - Vec3::Y * 0.3;
                // Lumens; the exposure system takes care of day vs night.
                l.intensity = 14_000.0 * on;
                *v = Visibility::Visible;
            }
            None => {
                *v = Visibility::Hidden;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_cycle_is_exclusive() {
        // At every moment at most one group has green or yellow, and peds never walk
        // across traffic that has green.
        for k in 0..2000 {
            let t = k as f64 * 0.1;
            let s: Vec<(u8, u8)> = (0..3).map(|g| signal_state(3, 4.0, g, t)).collect();
            assert!(s.iter().filter(|x| x.0 > 0).count() <= 1, "{t}: {s:?}");
            for (g, &(veh, ped)) in s.iter().enumerate() {
                assert!(!(veh == 2 && ped == 1), "group {g} green while walk at {t}");
            }
        }
    }
}
