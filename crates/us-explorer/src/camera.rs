//! Free-flying camera used until the player character arrives in Phase 2.
//!
//! Hold the right mouse button (or press Tab to lock the cursor) to look around;
//! WASD to move, Space/Ctrl (or E/Q) to rise and sink, Shift to go fast, mouse wheel
//! to change speed.

use crate::globe::{Viewer, WorldOrigin, viewer_rotation_in_origin};
use crate::terrain::TerrainStats;
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::light::AtmosphereEnvironmentMapLight;
use bevy::math::DVec3;
use bevy::pbr::AtmosphereSettings;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

#[derive(Component)]
pub struct MainCamera;

/// Fly speed in m/s, changed with the mouse wheel.
#[derive(Resource)]
pub struct FlySpeed(pub f64);

/// Set while a menu or text box has the keyboard, so movement keys are ignored.
#[derive(Resource, Default)]
pub struct InputBlocked {
    pub keyboard: bool,
    pub pointer: bool,
}

#[derive(Resource, Default)]
pub struct MouseLock(pub bool);

pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FlySpeed(60.0))
            .init_resource::<InputBlocked>()
            .init_resource::<MouseLock>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (toggle_lock, fly).chain())
            .add_systems(
                PostUpdate,
                place_camera
                    .after(crate::globe::GlobeSet)
                    .before(TransformSystems::Propagate),
            );
    }
}

fn spawn_camera(mut commands: Commands, settings: Res<crate::GameSettings>) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: settings.0.fov_deg.to_radians(),
            near: 0.2,
            ..default()
        }),
        AtmosphereSettings {
            aerial_view_lut_max_distance: 120_000.0,
            ..default()
        },
        AtmosphereEnvironmentMapLight::default(),
        Exposure { ev100: 13.5 },
        Tonemapping::AcesFitted,
        Msaa::Off,
        Transform::default(),
    ));
}

fn toggle_lock(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    blocked: Res<InputBlocked>,
    mut lock: ResMut<MouseLock>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if !blocked.keyboard && keys.just_pressed(KeyCode::Tab) {
        lock.0 = !lock.0;
    }
    if keys.just_pressed(KeyCode::Escape) {
        lock.0 = false;
    }
    let want = lock.0 || (mouse.pressed(MouseButton::Right) && !blocked.pointer);
    for mut c in &mut cursor {
        let mode = if want {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if c.grab_mode != mode {
            c.grab_mode = mode;
            c.visible = !want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    blocked: Res<InputBlocked>,
    lock: Res<MouseLock>,
    settings: Res<crate::GameSettings>,
    stats: Res<TerrainStats>,
    mut speed: ResMut<FlySpeed>,
    mut viewer: ResMut<Viewer>,
) {
    let dt = time.delta_secs_f64().min(0.1);
    let looking = lock.0 || (mouse.pressed(MouseButton::Right) && !blocked.pointer);
    if looking {
        let s = 0.08 * settings.0.mouse_sensitivity as f64;
        viewer.heading = (viewer.heading + motion.delta.x as f64 * s).rem_euclid(360.0);
        viewer.pitch = (viewer.pitch - motion.delta.y as f64 * s).clamp(-89.0, 89.0);
    }
    if !blocked.pointer && scroll.delta.y != 0.0 {
        speed.0 = (speed.0 * 1.25f64.powf(scroll.delta.y as f64)).clamp(1.5, 280_000.0);
    }
    let mut v = DVec3::ZERO;
    if !blocked.keyboard {
        let (fwd, right, up) = viewer.basis_ecef();
        let k = |c: KeyCode| keys.pressed(c) as i32 as f64;
        v += fwd * (k(KeyCode::KeyW) - k(KeyCode::KeyS));
        v += right * (k(KeyCode::KeyD) - k(KeyCode::KeyA));
        v += up
            * (k(KeyCode::Space) + k(KeyCode::KeyE) - k(KeyCode::ControlLeft) - k(KeyCode::KeyQ));
        if v.length_squared() > 0.0 {
            v = v.normalize();
        }
        let mut sp = speed.0
            * if keys.pressed(KeyCode::ShiftLeft) {
                6.0
            } else {
                1.0
            };
        // Faster high up, so crossing the country doesn't take all day.
        if let Some(agl) = viewer.agl() {
            sp *= (agl / 400.0).clamp(1.0, 60.0);
        }
        if settings.0.cap_speed_while_loading && stats.pending > 40 {
            sp = sp.min(250.0);
        }
        v *= sp;
    }
    viewer.velocity = v;
    viewer.ecef += v * dt;
    // Never go underground.
    if let Some(g) = viewer.ground {
        let mut geo = viewer.geo();
        if geo.h < g + 1.7 {
            geo.h = g + 1.7;
            viewer.ecef = geo.to_ecef();
        }
    }
}

fn place_camera(
    viewer: Res<Viewer>,
    origin: Res<WorldOrigin>,
    mut q: Query<&mut Transform, With<MainCamera>>,
) {
    for mut t in &mut q {
        t.translation = origin.local(viewer.ecef);
        t.rotation = viewer_rotation_in_origin(&viewer, &origin);
    }
}
