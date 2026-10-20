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
            .add_systems(Update, apply_fov)
            .add_systems(PostUpdate, place_camera.after(crate::globe::GlobeSet).before(TransformSystems::Propagate));
    }
}

fn spawn_camera(mut commands: Commands, settings: Res<crate::GameSettings>) {
    commands.spawn((
        MainCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: settings.0.fov_deg.to_radians(), near: 0.1, ..default() }),
        AtmosphereSettings { aerial_view_lut_max_distance: 120_000.0, ..default() },
        AtmosphereEnvironmentMapLight::default(),
        Exposure { ev100: 13.5 },
        Tonemapping::AcesFitted,
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

fn place_camera(viewer: Res<Viewer>, origin: Res<WorldOrigin>, mut q: Query<&mut Transform, With<MainCamera>>) {
    for mut t in &mut q {
        t.translation = origin.local(viewer.ecef);
        t.rotation = viewer_rotation_in_origin(&viewer, &origin);
    }
}
