//! The player's body: a jointed figure built from primitive shapes, animated
//! procedurally (idle, walk, run, jump, fall, hover, fly, swim), with outfits.
//!
//! Every limb hangs from a joint entity, so animation is just joint rotations, and the
//! whole figure is a few hundred triangles drawn with a handful of shared materials.

use crate::globe::{GlobeSet, WorldOrigin};
use crate::player::{Player, Pose};
use bevy::camera::visibility::RenderLayers;
use bevy::math::DQuat;
use bevy::prelude::*;
use usx_core::geo::EnuFrame;
use usx_core::settings::{CameraView, Character, HairStyle, Outfit};

#[derive(Component)]
pub struct BodyRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Joint {
    Hips,
    Spine,
    Neck,
    ShoulderL,
    ShoulderR,
    ElbowL,
    ElbowR,
    HipL,
    HipR,
    KneeL,
    KneeR,
}

/// Parts hidden in first person (so the camera doesn't sit inside the head).
#[derive(Component)]
pub struct HeadPart;

/// Outfit pieces that only show with some outfits.
#[derive(Component)]
pub struct OutfitPart(pub &'static [Outfit]);

/// Character look pieces: one hair style's parts, or glasses.
#[derive(Component, Clone, Copy, PartialEq)]
pub enum Look {
    Hair(HairStyle),
    Glasses,
}

impl Look {
    fn shown(self, outfit: Outfit, ch: &Character) -> bool {
        match self {
            // Hats and hoods cover the hair, except what hangs below them.
            Look::Hair(st) => {
                st == ch.hair_style && (matches!(outfit, Outfit::Casual) || matches!(st, HairStyle::Long | HairStyle::Ponytail))
            }
            Look::Glasses => ch.glasses,
        }
    }
}

#[derive(Resource)]
struct BodyMaterials {
    shirt: Handle<StandardMaterial>,
    pants: Handle<StandardMaterial>,
    shoes: Handle<StandardMaterial>,
    hat: Handle<StandardMaterial>,
    skin: Handle<StandardMaterial>,
    hair: Handle<StandardMaterial>,
    outfit: Outfit,
    character: Character,
}

pub struct BodyPlugin;

impl Plugin for BodyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_body)
            .add_systems(Update, (animate, apply_outfit))
            .add_systems(PostUpdate, place_body.after(GlobeSet).before(TransformSystems::Propagate));
    }
}

fn rgb(c: [u8; 3]) -> Color {
    Color::srgb_u8(c[0], c[1], c[2])
}

fn outfit_colors(o: Outfit, ch: &Character) -> (Color, Color, Color, Color) {
    // shirt, pants, shoes, hat
    match o {
        Outfit::Casual => (rgb(ch.shirt), rgb(ch.pants), rgb(ch.shoes), Color::srgb(0.6, 0.15, 0.12)),
        Outfit::Rain => {
            (Color::srgb(0.95, 0.72, 0.1), Color::srgb(0.16, 0.18, 0.22), Color::srgb(0.12, 0.12, 0.12), Color::srgb(0.95, 0.72, 0.1))
        }
        Outfit::Winter => {
            (Color::srgb(0.55, 0.08, 0.1), Color::srgb(0.14, 0.15, 0.2), Color::srgb(0.32, 0.22, 0.14), Color::srgb(0.85, 0.85, 0.88))
        }
        Outfit::Hiker => {
            (Color::srgb(0.32, 0.42, 0.26), Color::srgb(0.48, 0.42, 0.32), Color::srgb(0.36, 0.25, 0.16), Color::srgb(0.7, 0.62, 0.45))
        }
    }
}

fn spawn_body(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    settings: Res<crate::GameSettings>,
) {
    let o = settings.0.outfit;
    let ch = settings.0.character.clone();
    let (c_shirt, c_pants, c_shoes, c_hat) = outfit_colors(o, &ch);
    let mat = |m: &mut Assets<StandardMaterial>, c: Color, rough: f32| {
        m.add(StandardMaterial { base_color: c, perceptual_roughness: rough, ..default() })
    };
    let skin = mat(&mut mats, rgb(ch.skin), 0.6);
    let hair = mat(&mut mats, rgb(ch.hair), 0.8);
    let lens = mat(&mut mats, Color::srgb(0.05, 0.05, 0.06), 0.2);
    let shirt = mat(&mut mats, c_shirt, 0.85);
    let pants = mat(&mut mats, c_pants, 0.9);
    let shoes = mat(&mut mats, c_shoes, 0.7);
    let hat = mat(&mut mats, c_hat, 0.8);
    commands.insert_resource(BodyMaterials {
        shirt: shirt.clone(),
        pants: pants.clone(),
        shoes: shoes.clone(),
        hat: hat.clone(),
        skin: skin.clone(),
        hair: hair.clone(),
        outfit: o,
        character: ch.clone(),
    });
    let frame = meshes.add(Cuboid::new(0.07, 0.035, 0.012));
    let bridge = meshes.add(Cuboid::new(0.03, 0.01, 0.01));
    let long_back = meshes.add(Cuboid::new(0.2, 0.26, 0.05));

    let cap = |m: &mut Assets<Mesh>, r: f32, len: f32| m.add(Capsule3d::new(r, len).mesh().rings(2).latitudes(6).longitudes(10));
    let torso = cap(&mut meshes, 0.17, 0.32);
    let pelvis = cap(&mut meshes, 0.16, 0.08);
    let upper_arm = cap(&mut meshes, 0.055, 0.22);
    let forearm = cap(&mut meshes, 0.045, 0.2);
    let thigh = cap(&mut meshes, 0.075, 0.3);
    let shin = cap(&mut meshes, 0.06, 0.32);
    let head = meshes.add(Sphere::new(0.115).mesh().uv(12, 8));
    let hand = meshes.add(Sphere::new(0.045).mesh().uv(8, 5));
    let foot = meshes.add(Cuboid::new(0.1, 0.07, 0.25));
    let neck = cap(&mut meshes, 0.05, 0.05);
    let tail = cap(&mut meshes, 0.035, 0.16);
    let beanie = meshes.add(Sphere::new(0.122).mesh().uv(12, 6));
    let brim = meshes.add(Cylinder::new(0.15, 0.015).mesh().resolution(14));
    let hood = meshes.add(Sphere::new(0.135).mesh().uv(12, 8));
    let pack = meshes.add(Cuboid::new(0.28, 0.38, 0.14));

    let part = |c: &mut ChildSpawnerCommands, mesh: &Handle<Mesh>, mat: &Handle<StandardMaterial>, t: Transform| {
        c.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), t));
    };

    commands.spawn((BodyRoot, Transform::default(), Visibility::Visible)).with_children(|root| {
        root.spawn((Joint::Hips, Transform::from_xyz(0.0, 0.97, 0.0), Visibility::Inherited)).with_children(|hips| {
            part(
                hips,
                &pelvis,
                &pants,
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(1.0, 1.0, 0.8)),
            );
            for (side, joint_hip, joint_knee) in [(-1.0f32, Joint::HipL, Joint::KneeL), (1.0, Joint::HipR, Joint::KneeR)] {
                hips.spawn((joint_hip, Transform::from_xyz(0.1 * side, -0.02, 0.0), Visibility::Inherited)).with_children(|hip| {
                    part(hip, &thigh, &pants, Transform::from_xyz(0.0, -0.22, 0.0));
                    hip.spawn((joint_knee, Transform::from_xyz(0.0, -0.44, 0.0), Visibility::Inherited)).with_children(|knee| {
                        part(knee, &shin, &pants, Transform::from_xyz(0.0, -0.23, 0.0));
                        part(knee, &foot, &shoes, Transform::from_xyz(0.0, -0.47, -0.05));
                    });
                });
            }
            hips.spawn((Joint::Spine, Transform::from_xyz(0.0, 0.08, 0.0), Visibility::Inherited)).with_children(|spine| {
                part(spine, &torso, &shirt, Transform::from_xyz(0.0, 0.28, 0.0).with_scale(Vec3::new(1.15, 1.0, 0.75)));
                spine.spawn((
                    Mesh3d(pack.clone()),
                    MeshMaterial3d(hat.clone()),
                    Transform::from_xyz(0.0, 0.32, 0.17),
                    OutfitPart(&[Outfit::Hiker]),
                ));
                spine.spawn((Joint::Neck, Transform::from_xyz(0.0, 0.58, 0.0), Visibility::Inherited)).with_children(|n| {
                    n.spawn((Mesh3d(neck.clone()), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, 0.03, 0.0), HeadPart));
                    n.spawn((Mesh3d(head.clone()), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, 0.15, 0.0), HeadPart));
                    // Hair styles (one shows) and glasses.
                    let h = |n: &mut ChildSpawnerCommands, m: &Handle<Mesh>, t: Transform, st: HairStyle| {
                        n.spawn((Mesh3d(m.clone()), MeshMaterial3d(hair.clone()), t, HeadPart, Look::Hair(st)));
                    };
                    h(n, &head, Transform::from_xyz(0.0, 0.17, 0.015).with_scale(Vec3::new(1.04, 0.92, 1.04)), HairStyle::Short);
                    h(n, &head, Transform::from_xyz(0.0, 0.158, 0.008).with_scale(Vec3::new(1.015, 0.93, 1.02)), HairStyle::Buzz);
                    h(n, &head, Transform::from_xyz(0.0, 0.18, 0.015).with_scale(Vec3::new(1.16, 1.0, 1.14)), HairStyle::Curly);
                    h(n, &head, Transform::from_xyz(0.0, 0.17, 0.015).with_scale(Vec3::new(1.05, 0.95, 1.06)), HairStyle::Long);
                    h(n, &long_back, Transform::from_xyz(0.0, 0.07, 0.075), HairStyle::Long);
                    h(n, &head, Transform::from_xyz(0.0, 0.17, 0.015).with_scale(Vec3::new(1.04, 0.93, 1.04)), HairStyle::Ponytail);
                    h(n, &tail, Transform::from_xyz(0.0, 0.1, 0.13).with_rotation(Quat::from_rotation_x(0.35)), HairStyle::Ponytail);
                    for x in [-0.045f32, 0.045] {
                        n.spawn((
                            Mesh3d(frame.clone()),
                            MeshMaterial3d(lens.clone()),
                            Transform::from_xyz(x, 0.16, -0.108),
                            HeadPart,
                            Look::Glasses,
                        ));
                    }
                    n.spawn((
                        Mesh3d(bridge.clone()),
                        MeshMaterial3d(lens.clone()),
                        Transform::from_xyz(0.0, 0.165, -0.11),
                        HeadPart,
                        Look::Glasses,
                    ));
                    n.spawn((
                        Mesh3d(beanie.clone()),
                        MeshMaterial3d(hat.clone()),
                        Transform::from_xyz(0.0, 0.2, 0.0).with_scale(Vec3::new(1.0, 0.75, 1.0)),
                        HeadPart,
                        OutfitPart(&[Outfit::Winter]),
                    ));
                    n.spawn((
                        Mesh3d(hood.clone()),
                        MeshMaterial3d(hat.clone()),
                        Transform::from_xyz(0.0, 0.16, 0.025),
                        HeadPart,
                        OutfitPart(&[Outfit::Rain]),
                    ));
                    n.spawn((
                        Mesh3d(beanie.clone()),
                        MeshMaterial3d(hat.clone()),
                        Transform::from_xyz(0.0, 0.21, 0.0).with_scale(Vec3::new(1.0, 0.6, 1.0)),
                        HeadPart,
                        OutfitPart(&[Outfit::Hiker]),
                    ));
                    n.spawn((
                        Mesh3d(brim.clone()),
                        MeshMaterial3d(hat.clone()),
                        Transform::from_xyz(0.0, 0.22, 0.0),
                        HeadPart,
                        OutfitPart(&[Outfit::Hiker]),
                    ));
                });
                for (side, js, je) in [(-1.0f32, Joint::ShoulderL, Joint::ElbowL), (1.0, Joint::ShoulderR, Joint::ElbowR)] {
                    spine.spawn((js, Transform::from_xyz(0.23 * side, 0.5, 0.0), Visibility::Inherited)).with_children(|sh| {
                        part(sh, &upper_arm, &shirt, Transform::from_xyz(0.0, -0.15, 0.0));
                        sh.spawn((je, Transform::from_xyz(0.0, -0.3, 0.0), Visibility::Inherited)).with_children(|el| {
                            part(el, &forearm, &skin, Transform::from_xyz(0.0, -0.14, 0.0));
                            part(el, &hand, &skin, Transform::from_xyz(0.0, -0.3, 0.0));
                        });
                    });
                }
            });
        });
    });
}

/// Smoothly blended joint angles, so poses ease into each other.
#[derive(Default)]
struct Angles {
    v: [Vec3; 11],
    hips_drop: f32,
    lean: f32,
}

fn idx(j: Joint) -> usize {
    j as usize
}

fn animate(time: Res<Time>, p: Res<Player>, mut joints: Query<(&Joint, &mut Transform)>, mut state: Local<Angles>) {
    let t = time.elapsed_secs();
    let ph = p.gait;
    let (s, c) = (ph.sin(), ph.cos());
    let mut target = [Vec3::ZERO; 11];
    let mut drop = 0.0;
    let mut lean = 0.0;
    let set = |a: &mut [Vec3; 11], j: Joint, v: Vec3| a[idx(j)] = v;
    match p.pose {
        Pose::Idle => {
            let b = (t * 1.6).sin() * 0.02;
            set(&mut target, Joint::Spine, Vec3::new(b, 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(0.05, 0.0, -0.06 - b));
            set(&mut target, Joint::ShoulderR, Vec3::new(0.05, 0.0, 0.06 + b));
            set(&mut target, Joint::ElbowL, Vec3::new(0.15, 0.0, 0.0));
            set(&mut target, Joint::ElbowR, Vec3::new(0.15, 0.0, 0.0));
        }
        Pose::Walk | Pose::Run => {
            let run = p.pose == Pose::Run;
            let (leg, arm, knee) = if run { (0.75, 0.85, 1.4) } else { (0.42, 0.35, 0.65) };
            set(&mut target, Joint::HipL, Vec3::new(s * leg, 0.0, 0.0));
            set(&mut target, Joint::HipR, Vec3::new(-s * leg, 0.0, 0.0));
            // Knees bend while the leg swings forward.
            set(&mut target, Joint::KneeL, Vec3::new(-(knee * (c.max(0.0) + 0.15)), 0.0, 0.0));
            set(&mut target, Joint::KneeR, Vec3::new(-(knee * ((-c).max(0.0) + 0.15)), 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(-s * arm, 0.0, -0.08));
            set(&mut target, Joint::ShoulderR, Vec3::new(s * arm, 0.0, 0.08));
            let elbow = if run { 1.3 } else { 0.3 };
            set(&mut target, Joint::ElbowL, Vec3::new(elbow, 0.0, 0.0));
            set(&mut target, Joint::ElbowR, Vec3::new(elbow, 0.0, 0.0));
            set(&mut target, Joint::Spine, Vec3::new(if run { 0.18 } else { 0.04 }, s * 0.06, 0.0));
            drop = (2.0 * ph).cos().abs() * if run { 0.05 } else { 0.025 };
        }
        Pose::Jump | Pose::Fall => {
            let up = p.pose == Pose::Jump;
            set(&mut target, Joint::HipL, Vec3::new(if up { 0.7 } else { 0.3 }, 0.0, 0.0));
            set(&mut target, Joint::HipR, Vec3::new(if up { 0.2 } else { 0.4 }, 0.0, 0.0));
            set(&mut target, Joint::KneeL, Vec3::new(-1.1, 0.0, 0.0));
            set(&mut target, Joint::KneeR, Vec3::new(-0.6, 0.0, 0.0));
            let arms = if up { 2.2 } else { 1.2 };
            set(&mut target, Joint::ShoulderL, Vec3::new(arms, 0.0, -0.3));
            set(&mut target, Joint::ShoulderR, Vec3::new(arms, 0.0, 0.3));
        }
        Pose::Hover => {
            let b = (t * 1.3).sin();
            set(&mut target, Joint::HipL, Vec3::new(0.15 + b * 0.05, 0.0, -0.05));
            set(&mut target, Joint::HipR, Vec3::new(0.1 - b * 0.05, 0.0, 0.05));
            set(&mut target, Joint::KneeL, Vec3::new(-0.35, 0.0, 0.0));
            set(&mut target, Joint::KneeR, Vec3::new(-0.25, 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(0.1, 0.0, -0.35 - b * 0.05));
            set(&mut target, Joint::ShoulderR, Vec3::new(0.1, 0.0, 0.35 + b * 0.05));
            drop = -b * 0.04;
        }
        Pose::Fly => {
            // Lean into the direction of travel, legs trailing, arms back.
            lean = 1.15;
            set(&mut target, Joint::HipL, Vec3::new(-0.1, 0.0, -0.04));
            set(&mut target, Joint::HipR, Vec3::new(-0.1, 0.0, 0.04));
            set(&mut target, Joint::KneeL, Vec3::new(-0.15, 0.0, 0.0));
            set(&mut target, Joint::KneeR, Vec3::new(-0.2, 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(0.3, 0.0, -0.25));
            set(&mut target, Joint::ShoulderR, Vec3::new(0.3, 0.0, 0.25));
            set(&mut target, Joint::Neck, Vec3::new(0.9, 0.0, 0.0));
        }
        Pose::Sit => {
            // Thighs forward, shins down, hands on the lap.
            drop = 0.45;
            set(&mut target, Joint::HipL, Vec3::new(1.5, 0.0, -0.05));
            set(&mut target, Joint::HipR, Vec3::new(1.5, 0.0, 0.05));
            set(&mut target, Joint::KneeL, Vec3::new(-1.5, 0.0, 0.0));
            set(&mut target, Joint::KneeR, Vec3::new(-1.5, 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(0.45, 0.0, -0.08));
            set(&mut target, Joint::ShoulderR, Vec3::new(0.45, 0.0, 0.08));
            set(&mut target, Joint::ElbowL, Vec3::new(0.9, 0.0, 0.0));
            set(&mut target, Joint::ElbowR, Vec3::new(0.9, 0.0, 0.0));
            set(&mut target, Joint::Spine, Vec3::new(-0.05, 0.0, 0.0));
        }
        Pose::Lie => {
            // On the back: hips tipped back so the body lies flat, legs straight.
            lean = -1.5;
            drop = 0.82;
            let b = (t * 1.1).sin() * 0.01;
            set(&mut target, Joint::Spine, Vec3::new(b, 0.0, 0.0));
            set(&mut target, Joint::ShoulderL, Vec3::new(0.05, 0.0, -0.12));
            set(&mut target, Joint::ShoulderR, Vec3::new(0.05, 0.0, 0.12));
            set(&mut target, Joint::Neck, Vec3::new(-0.2, 0.0, 0.0));
        }
        Pose::Swim => {
            lean = 1.4;
            // Raise the hips so the body lies along the surface while the feet hang below.
            drop = -0.25;
            let st = (t * 3.0).sin();
            set(&mut target, Joint::ShoulderL, Vec3::new(2.6 + st * 1.2, 0.0, -0.2));
            set(&mut target, Joint::ShoulderR, Vec3::new(2.6 - st * 1.2, 0.0, 0.2));
            set(&mut target, Joint::HipL, Vec3::new(st * 0.35, 0.0, 0.0));
            set(&mut target, Joint::HipR, Vec3::new(-st * 0.35, 0.0, 0.0));
            set(&mut target, Joint::Neck, Vec3::new(1.0, 0.0, 0.0));
        }
    }
    let k = (time.delta_secs() * 12.0).min(1.0);
    for i in 0..11 {
        state.v[i] = state.v[i].lerp(target[i], k);
    }
    state.hips_drop += (drop - state.hips_drop) * k;
    state.lean += (lean - state.lean) * (time.delta_secs() * 4.0).min(1.0);
    for (j, mut tr) in &mut joints {
        let a = state.v[idx(*j)];
        let mut r = Quat::from_euler(EulerRot::XYZ, a.x, a.y, a.z);
        if *j == Joint::Hips {
            r = Quat::from_rotation_x(-state.lean) * r;
            tr.translation.y = 0.97 - state.hips_drop;
        }
        tr.rotation = r;
    }
}

fn place_body(
    p: Res<Player>,
    origin: Res<WorldOrigin>,
    mut root: Query<(&mut Transform, &mut Visibility), With<BodyRoot>>,
    mut heads: Query<&mut Visibility, (With<HeadPart>, Without<BodyRoot>, Without<OutfitPart>, Without<Look>)>,
    mut outfit_heads: Query<(&mut Visibility, &OutfitPart), (With<HeadPart>, Without<BodyRoot>, Without<Look>)>,
    mut looks: Query<(&mut Visibility, &Look), (Without<BodyRoot>, Without<OutfitPart>)>,
    settings: Res<crate::GameSettings>,
    head_ents: Query<Entity, With<HeadPart>>,
    mut commands: Commands,
    mut last_first: Local<Option<bool>>,
) {
    let first = p.view == CameraView::FirstPerson && p.showcase.is_none();
    let frame = EnuFrame::at(p.geo());
    let m = origin.frame.ecef_to_local * frame.ecef_to_local.transpose();
    let rot = (DQuat::from_mat3(&m) * DQuat::from_rotation_y(-p.body_heading.to_radians())).as_quat();
    let ch = &settings.0.character;
    for (mut t, mut v) in &mut root {
        t.translation = origin.local(p.feet);
        t.rotation = rot;
        t.scale = Vec3::new(ch.build.clamp(0.8, 1.25), ch.height.clamp(0.85, 1.15), ch.build.clamp(0.8, 1.25).sqrt());
        *v = Visibility::Visible;
    }
    for (mut v, look) in &mut looks {
        let want = if look.shown(settings.0.outfit, ch) { Visibility::Inherited } else { Visibility::Hidden };
        if *v != want {
            *v = want;
        }
    }
    // In first person the head stays out of your own view (it's on a layer only the
    // mirror camera draws), so mirrors still show it.
    let head_vis = Visibility::Inherited;
    for mut v in &mut heads {
        if *v != head_vis {
            *v = head_vis;
        }
    }
    for (mut v, part) in &mut outfit_heads {
        let want = if part.0.contains(&settings.0.outfit) { Visibility::Inherited } else { Visibility::Hidden };
        if *v != want {
            *v = want;
        }
    }
    if *last_first != Some(first) {
        *last_first = Some(first);
        let layer = if first { RenderLayers::layer(1) } else { RenderLayers::layer(0) };
        for e in &head_ents {
            commands.entity(e).insert(layer.clone());
        }
    }
}

fn apply_outfit(
    settings: Res<crate::GameSettings>,
    mut bm: Option<ResMut<BodyMaterials>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut parts: Query<(&mut Visibility, &OutfitPart), Without<HeadPart>>,
) {
    let Some(bm) = bm.as_mut() else { return };
    let o = settings.0.outfit;
    for (mut v, part) in &mut parts {
        let want = if part.0.contains(&o) { Visibility::Inherited } else { Visibility::Hidden };
        if *v != want {
            *v = want;
        }
    }
    let ch = &settings.0.character;
    if bm.outfit == o && bm.character == *ch {
        return;
    }
    bm.outfit = o;
    bm.character = ch.clone();
    let (a, b, c, d) = outfit_colors(o, ch);
    for (h, col) in [(&bm.shirt, a), (&bm.pants, b), (&bm.shoes, c), (&bm.hat, d), (&bm.skin, rgb(ch.skin)), (&bm.hair, rgb(ch.hair))] {
        if let Some(mut m) = mats.get_mut(h) {
            m.base_color = col;
        }
    }
}
