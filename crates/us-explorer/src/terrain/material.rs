//! The terrain material: Bevy's standard PBR material plus our fragment shader
//! (`terrain.wgsl`) for close-up ground detail and rock on steep slopes.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct TerrainExt {}

impl MaterialExtension for TerrainExt {
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/terrain/terrain.wgsl".into()
    }
}

pub struct TerrainMaterialPlugin;

impl Plugin for TerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "terrain.wgsl");
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
    }
}
