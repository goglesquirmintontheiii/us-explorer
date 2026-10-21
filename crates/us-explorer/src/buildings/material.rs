//! The building material: standard PBR plus `buildings.wgsl` for facades and roofs.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub type BuildingMaterial = ExtendedMaterial<StandardMaterial, BuildingExt>;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct BuildingParams {
    pub night: f32,
    /// 1 for buildings showing their real interior: windows are see-through.
    pub cutout: f32,
    pub wet: f32,
    pub time: f32,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct BuildingExt {
    #[uniform(100)]
    pub params: BuildingParams,
}

impl MaterialExtension for BuildingExt {
    fn fragment_shader() -> ShaderRef {
        "embedded://us_explorer/buildings/buildings.wgsl".into()
    }
}

pub struct BuildingMaterialPlugin;

impl Plugin for BuildingMaterialPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "buildings.wgsl");
        app.add_plugins(MaterialPlugin::<BuildingMaterial>::default());
    }
}

pub fn new_material(cutout: bool) -> BuildingMaterial {
    BuildingMaterial {
        base: StandardMaterial { perceptual_roughness: 0.85, reflectance: 0.4, cull_mode: None, double_sided: true, ..default() },
        extension: BuildingExt { params: BuildingParams { cutout: cutout as u32 as f32, ..default() } },
    }
}
