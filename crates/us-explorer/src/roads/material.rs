//! The road material: standard PBR plus `road.wgsl` for surfaces and markings.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub type RoadMaterial = ExtendedMaterial<StandardMaterial, RoadExt>;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct RoadParams {
    pub night: f32,
    pub wet: f32,
    pub time: f32,
    pub _pad: f32,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct RoadExt {
    #[uniform(100)]
    pub params: RoadParams,
}

impl MaterialExtension for RoadExt {
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/roads/road.wgsl".into()
    }
}

pub struct RoadMaterialPlugin;

impl Plugin for RoadMaterialPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "road.wgsl");
        app.add_plugins(MaterialPlugin::<RoadMaterial>::default());
    }
}
