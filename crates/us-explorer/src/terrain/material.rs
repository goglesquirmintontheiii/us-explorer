//! The terrain material: Bevy's standard PBR material plus our fragment shader
//! (`terrain.wgsl`) for close-up ground detail and rock on steep slopes.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExt>;

/// Sea state for the ocean shading (shared by every terrain tile).
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType, Reflect)]
pub struct TerrainParams {
    /// Swell: direction of travel (east, south), height (m), period (s).
    pub swell: Vec4,
    /// Wind: direction of travel (east, south), speed (m/s), wind-sea height (m).
    pub wind: Vec4,
}

/// The current sea state, kept by the conditions system.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct Waves(pub TerrainParams);

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct TerrainExt {
    #[uniform(100)]
    pub params: TerrainParams,
}

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
