//! The main camera. Where it goes each frame is decided by the player (see player.rs),
//! which writes [`Viewer`]; this module turns that into a render transform after any
//! floating-origin rebase.

use crate::globe::{Viewer, WorldOrigin, viewer_rotation_in_origin};
use bevy::camera::Exposure;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::AtmosphereEnvironmentMapLight;
use bevy::pbr::AtmosphereSettings;
use bevy::prelude::*;

#[derive(Component)]
pub struct MainCamera;

/// Set while a menu or text box has the keyboard or pointer, so the game ignores them.
#[derive(Resource, Default)]
pub struct InputBlocked {
    pub keyboard: bool,
    pub pointer: bool,
}

/// Mouse captured for looking around.
#[derive(Resource, Default)]
pub struct MouseLock(pub bool);

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputBlocked>()
            .init_resource::<MouseLock>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (apply_fov, atmosphere_for_altitude))
            .add_systems(PostUpdate, place_camera.after(crate::globe::GlobeSet).before(TransformSystems::Propagate));
    }
}

fn spawn_camera(mut commands: Commands, settings: Res<crate::GameSettings>) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        // Menus draw on this camera (never the bathroom mirror's, which renders to an
        // image and is usually off: egui picking that one left a black, dead screen).
        bevy_egui::PrimaryEguiContext,
        Projection::Perspective(PerspectiveProjection { fov: settings.0.fov_deg.to_radians(), near: 0.1, ..default() }),
        AtmosphereSettings { aerial_view_lut_max_distance: 120_000.0, ..default() },
        AtmosphereEnvironmentMapLight::default(),
        Exposure { ev100: 13.5 },
        Tonemapping::AcesFitted,
        // A soft glow around bright lights (signals, lamps), most visible at night.
        bevy::post_process::bloom::Bloom { intensity: 0.12, low_frequency_boost: 0.5, ..bevy::post_process::bloom::Bloom::NATURAL },
        Msaa::Off,
        Transform::default(),
    ));
}

fn apply_fov(settings: Res<crate::GameSettings>, mut q: Query<&mut Projection, With<MainCamera>>) {
    if !settings.is_changed() {
        return;
    }
    for mut p in &mut q {
        if let Projection::Perspective(pp) = &mut *p {
            pp.fov = settings.0.fov_deg.to_radians();
        }
    }
}

/// High up, the lookup-table atmosphere tints the ground red (its tables assume a
/// viewer near the ground): ray-march it instead above about 12 km, and stretch the
/// aerial-perspective range with height.
fn atmosphere_for_altitude(viewer: Res<Viewer>, mut q: Query<&mut AtmosphereSettings, With<MainCamera>>) {
    use bevy::pbr::AtmosphereMode;
    let h = viewer.geo().h as f32;
    for mut a in &mut q {
        let mode = match a.rendering_method {
            AtmosphereMode::LookupTexture if h > 12_000.0 => AtmosphereMode::Raymarched,
            AtmosphereMode::Raymarched if h < 10_000.0 => AtmosphereMode::LookupTexture,
            m => m,
        };
        let dist = (120_000.0f32).max(h * 8.0).min(1_200_000.0);
        let dist = (dist / 20_000.0).round() * 20_000.0;
        if (a.rendering_method as u32) != (mode as u32) {
            a.rendering_method = mode;
        }
        if (a.aerial_view_lut_max_distance - dist).abs() > 1.0 {
            a.aerial_view_lut_max_distance = dist;
        }
    }
}

fn place_camera(viewer: Res<Viewer>, origin: Res<WorldOrigin>, mut q: Query<&mut Transform, With<MainCamera>>) {
    for mut t in &mut q {
        t.translation = origin.local(viewer.ecef);
        t.rotation = viewer_rotation_in_origin(&viewer, &origin);
    }
}
