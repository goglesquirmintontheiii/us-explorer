//! Interior and furniture materials with textures painted at startup (no image files
//! to ship): wood floors, ceramic and subway tile, carpet, concrete, marble, wood
//! grain, fabric, brushed metal, acoustic ceiling tile, linoleum and packaging.
//! Floors and tile get normal maps so planks and grout catch the light.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

/// Interior surface and furniture materials. Texture coordinates are in metres
/// divided by each texture's size (see `size`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum M {
    // Surfaces (tinted by vertex colour where noted).
    Paint,
    Ceiling,
    CeilingTile,
    WoodFloor,
    TileFloor,
    Carpet,
    Concrete,
    Marble,
    Linoleum,
    SubwayTile,
    Brick,
    // Furniture.
    Wood,
    DarkWood,
    Fabric,
    Leather,
    Metal,
    Chrome,
    Plastic,
    WhitePlastic,
    Ceramic,
    Glass,
    Mirror,
    Screen,
    Rubber,
    Cardboard,
    Packaging,
    Water,
    Plant,
    Insulation,
    Copper,
    LightPanel,
}

impl M {
    /// Metres covered by one repeat of the texture.
    pub fn size(self) -> f32 {
        match self {
            M::WoodFloor => 2.0,
            M::TileFloor => 1.2,
            M::SubwayTile => 0.6,
            M::Carpet => 1.0,
            M::Concrete => 4.0,
            M::Marble => 2.0,
            M::Linoleum => 1.2,
            M::CeilingTile => 1.2,
            M::Brick => 1.0,
            M::Wood | M::DarkWood => 1.0,
            M::Fabric | M::Leather => 0.5,
            M::Metal => 0.5,
            M::Packaging => 1.0,
            M::Insulation => 1.2,
            _ => 1.0,
        }
    }
}

#[derive(Resource)]
pub struct InteriorMats {
    pub mats: HashMap<M, Handle<StandardMaterial>>,
}

impl InteriorMats {
    pub fn get(&self, m: M) -> Handle<StandardMaterial> {
        self.mats.get(&m).cloned().unwrap_or_default()
    }
}

fn h2(x: u32, y: u32, s: u32) -> f32 {
    let mut h = x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_add(0x9e37_79b9).wrapping_mul(0x85eb_ca6b) ^ s.wrapping_mul(0xc2b2_ae35);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}

/// Value noise on a wrapping lattice of `period` cells.
fn noise(x: f32, y: f32, period: u32, s: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let w = |a: f32| (a as i64).rem_euclid(period as i64) as u32;
    let (x0, y0, x1, y1) = (w(xi), w(yi), w(xi + 1.0), w(yi + 1.0));
    let a = h2(x0, y0, s) + (h2(x1, y0, s) - h2(x0, y0, s)) * ux;
    let b = h2(x0, y1, s) + (h2(x1, y1, s) - h2(x0, y1, s)) * ux;
    a + (b - a) * uy
}

fn fbm(x: f32, y: f32, period: u32, s: u32) -> f32 {
    noise(x, y, period, s) * 0.5 + noise(x * 2.0, y * 2.0, period * 2, s + 1) * 0.3 + noise(x * 4.0, y * 4.0, period * 4, s + 2) * 0.2
}

struct Tex {
    n: u32,
    rgb: Vec<[f32; 3]>,
    height: Vec<f32>,
}

impl Tex {
    fn new(n: u32, f: impl Fn(f32, f32) -> ([f32; 3], f32)) -> Self {
        let mut rgb = Vec::with_capacity((n * n) as usize);
        let mut height = Vec::with_capacity((n * n) as usize);
        for y in 0..n {
            for x in 0..n {
                let (c, h) = f(x as f32 / n as f32, y as f32 / n as f32);
                rgb.push(c);
                height.push(h);
            }
        }
        Self { n, rgb, height }
    }

    fn albedo(&self) -> Image {
        let px: Vec<u8> = self
            .rgb
            .iter()
            .flat_map(|c| {
                let s = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
                [s(c[0]), s(c[1]), s(c[2]), 255]
            })
            .collect();
        image(self.n, px, TextureFormat::Rgba8UnormSrgb)
    }

    fn normal(&self, strength: f32) -> Image {
        let n = self.n as i64;
        let at = |x: i64, y: i64| self.height[(y.rem_euclid(n) * n + x.rem_euclid(n)) as usize];
        let mut px = Vec::with_capacity((n * n * 4) as usize);
        for y in 0..n {
            for x in 0..n {
                let dx = (at(x + 1, y) - at(x - 1, y)) * strength;
                let dy = (at(x, y + 1) - at(x, y - 1)) * strength;
                let v = Vec3::new(-dx, -dy, 1.0).normalize();
                px.extend_from_slice(&[
                    ((v.x * 0.5 + 0.5) * 255.0) as u8,
                    ((v.y * 0.5 + 0.5) * 255.0) as u8,
                    ((v.z * 0.5 + 0.5) * 255.0) as u8,
                    255,
                ]);
            }
        }
        image(self.n, px, TextureFormat::Rgba8Unorm)
    }
}

fn image(n: u32, px: Vec<u8>, fmt: TextureFormat) -> Image {
    let mut img = Image::new(
        Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        fmt,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn mul(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

/// Wood grain: long stretched noise with growth rings.
fn grain(x: f32, y: f32, s: u32) -> f32 {
    let w = fbm(x * 2.0, y * 24.0, 16, s);
    ((x * 9.0 + w * 3.0) * std::f32::consts::TAU).sin() * 0.5 + 0.5
}

pub fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let mut out = HashMap::new();
    let mut add = |m: M,
                   tex: Option<&Tex>,
                   normal: Option<f32>,
                   sm: StandardMaterial,
                   images: &mut Assets<Image>,
                   mats: &mut Assets<StandardMaterial>| {
        let mut sm = sm;
        // Interiors are seen from both sides (walls, doors, furniture boxes).
        sm.cull_mode = None;
        sm.double_sided = true;
        if let Some(t) = tex {
            sm.base_color_texture = Some(images.add(t.albedo()));
            if let Some(s) = normal {
                sm.normal_map_texture = Some(images.add(t.normal(s)));
            }
        }
        out.insert(m, mats.add(sm));
    };
    let std =
        |c: Color, rough: f32, metal: f32| StandardMaterial { base_color: c, perceptual_roughness: rough, metallic: metal, ..default() };

    // Oak plank floor: 0.18 m boards (in a 2 m repeat: 11 boards), staggered joints.
    let wood_floor = Tex::new(512, |u, v| {
        let boards = 11.0;
        let row = (u * boards).floor();
        let fu = (u * boards).fract();
        let off = h2(row as u32, 0, 3);
        let seg = ((v + off) * 2.0).floor();
        let fv = ((v + off) * 2.0).fract();
        let tone = 0.75 + 0.35 * h2(row as u32, seg as u32, 5);
        let g = grain(fu * 0.3 + row * 0.13, v + off, row as u32);
        let base = lerp3([0.42, 0.27, 0.15], [0.62, 0.43, 0.26], g * 0.6 + 0.2);
        let gap = fu < 0.025 || fu > 0.975 || fv < 0.006;
        (if gap { [0.12, 0.08, 0.05] } else { mul(base, tone) }, if gap { 0.0 } else { 1.0 })
    });
    add(M::WoodFloor, Some(&wood_floor), Some(2.0), std(Color::WHITE, 0.45, 0.0), &mut images, &mut mats);
    // Ceramic floor tile: 0.3 m (4 per repeat) with grout.
    let tile = Tex::new(512, |u, v| {
        let (fu, fv) = ((u * 4.0).fract(), (v * 4.0).fract());
        let cell = ((u * 4.0).floor() as u32, (v * 4.0).floor() as u32);
        let grout = fu < 0.015 || fu > 0.985 || fv < 0.015 || fv > 0.985;
        let t = 0.9 + 0.08 * h2(cell.0, cell.1, 9) + 0.06 * fbm(u * 8.0, v * 8.0, 8, 4);
        (if grout { [0.55, 0.53, 0.5] } else { mul([0.86, 0.84, 0.8], t) }, if grout { 0.0 } else { 1.0 })
    });
    add(M::TileFloor, Some(&tile), Some(3.0), std(Color::WHITE, 0.25, 0.0), &mut images, &mut mats);
    // Subway tile for kitchen backsplashes and bathroom walls: 0.075 x 0.15 m.
    let subway = Tex::new(512, |u, v| {
        let row = (v * 8.0).floor();
        let fu = (u * 4.0 + 0.5 * (row % 2.0)).fract();
        let fv = (v * 8.0).fract();
        let grout = fu < 0.02 || fu > 0.98 || fv < 0.04 || fv > 0.96;
        let edge = (fu.min(1.0 - fu) * 6.0).min(fv.min(1.0 - fv) * 3.0).min(1.0);
        (if grout { [0.7, 0.7, 0.68] } else { mul([0.95, 0.95, 0.94], 0.92 + 0.08 * edge) }, if grout { 0.0 } else { edge })
    });
    add(M::SubwayTile, Some(&subway), Some(3.0), std(Color::WHITE, 0.12, 0.0), &mut images, &mut mats);
    let carpet = Tex::new(256, |u, v| {
        let n = fbm(u * 64.0, v * 64.0, 64, 11) * 0.6 + h2((u * 256.0) as u32, (v * 256.0) as u32, 12) * 0.4;
        (mul([0.8, 0.8, 0.8], 0.75 + 0.3 * n), n)
    });
    add(M::Carpet, Some(&carpet), Some(1.5), std(Color::WHITE, 0.98, 0.0), &mut images, &mut mats);
    let concrete = Tex::new(256, |u, v| {
        let n = fbm(u * 8.0, v * 8.0, 8, 21) * 0.7 + h2((u * 256.0) as u32, (v * 256.0) as u32, 22) * 0.3;
        (mul([0.6, 0.59, 0.57], 0.82 + 0.25 * n), n * 0.3)
    });
    add(M::Concrete, Some(&concrete), Some(1.0), std(Color::WHITE, 0.9, 0.0), &mut images, &mut mats);
    let marble = Tex::new(512, |u, v| {
        let w = fbm(u * 4.0, v * 4.0, 4, 31);
        let vein = ((u * 6.0 + v * 2.0 + w * 6.0) * std::f32::consts::PI).sin().abs().powf(0.15);
        let (fu, fv) = ((u * 2.0).fract(), (v * 2.0).fract());
        let grout = fu < 0.004 || fv < 0.004;
        (if grout { [0.6, 0.6, 0.58] } else { lerp3([0.55, 0.55, 0.56], [0.94, 0.93, 0.91], vein) }, if grout { 0.0 } else { 1.0 })
    });
    add(M::Marble, Some(&marble), Some(1.0), std(Color::WHITE, 0.08, 0.0), &mut images, &mut mats);
    let lino = Tex::new(256, |u, v| {
        let cell = ((u * 4.0).floor() as u32, (v * 4.0).floor() as u32);
        let alt = (cell.0 + cell.1) % 2 == 0;
        let n = fbm(u * 32.0, v * 32.0, 32, 41);
        (mul(if alt { [0.82, 0.81, 0.78] } else { [0.72, 0.72, 0.7] }, 0.9 + 0.12 * n), 0.0)
    });
    add(M::Linoleum, Some(&lino), None, std(Color::WHITE, 0.3, 0.0), &mut images, &mut mats);
    let ceiling_tile = Tex::new(256, |u, v| {
        let (fu, fv) = ((u * 2.0).fract(), (v * 2.0).fract());
        let grid = fu < 0.02 || fu > 0.98 || fv < 0.02 || fv > 0.98;
        let pits = h2((u * 256.0) as u32, (v * 256.0) as u32, 51);
        (if grid { [0.85, 0.85, 0.84] } else { mul([0.9, 0.9, 0.88], 0.9 + 0.1 * pits) }, if grid { 0.0 } else { 1.0 })
    });
    add(M::CeilingTile, Some(&ceiling_tile), Some(1.0), std(Color::WHITE, 0.95, 0.0), &mut images, &mut mats);
    let brick = Tex::new(256, |u, v| {
        let row = (v * 13.3).floor();
        let fu = (u * 5.0 + 0.5 * (row % 2.0)).fract();
        let fv = (v * 13.3).fract();
        let mortar = fu < 0.04 || fv < 0.12;
        let c = ((u * 5.0 + 0.5 * (row % 2.0)).floor() as u32, row as u32);
        (if mortar { [0.68, 0.66, 0.62] } else { mul([0.56, 0.28, 0.2], 0.8 + 0.35 * h2(c.0, c.1, 61)) }, if mortar { 0.0 } else { 1.0 })
    });
    add(M::Brick, Some(&brick), Some(2.0), std(Color::WHITE, 0.9, 0.0), &mut images, &mut mats);
    let wood = Tex::new(256, |u, v| {
        let g = grain(u, v, 71);
        (lerp3([0.45, 0.3, 0.17], [0.66, 0.48, 0.3], g * 0.7 + 0.15), g)
    });
    add(M::Wood, Some(&wood), Some(0.5), std(Color::WHITE, 0.5, 0.0), &mut images, &mut mats);
    let dark_wood = Tex::new(256, |u, v| {
        let g = grain(u, v, 73);
        (lerp3([0.16, 0.09, 0.05], [0.3, 0.18, 0.1], g * 0.7 + 0.15), g)
    });
    add(M::DarkWood, Some(&dark_wood), Some(0.5), std(Color::WHITE, 0.4, 0.0), &mut images, &mut mats);
    let fabric = Tex::new(256, |u, v| {
        let weave = (((u * 128.0) as u32 + (v * 128.0) as u32) % 2) as f32;
        let n = fbm(u * 16.0, v * 16.0, 16, 81);
        (mul([0.85, 0.85, 0.85], 0.85 + 0.08 * weave + 0.1 * n), weave * 0.5 + n * 0.5)
    });
    add(M::Fabric, Some(&fabric), Some(1.0), std(Color::WHITE, 0.95, 0.0), &mut images, &mut mats);
    let leather = Tex::new(256, |u, v| {
        let n = fbm(u * 24.0, v * 24.0, 24, 91);
        (mul([0.85, 0.85, 0.85], 0.88 + 0.15 * n), n)
    });
    add(M::Leather, Some(&leather), Some(1.0), std(Color::WHITE, 0.55, 0.0), &mut images, &mut mats);
    let brushed = Tex::new(256, |u, v| {
        let n = noise(u * 2.0, v * 256.0, 2, 101) * 0.6 + noise(u * 4.0, v * 512.0, 4, 102) * 0.4;
        (mul([0.75, 0.76, 0.77], 0.9 + 0.12 * n), n * 0.2)
    });
    add(M::Metal, Some(&brushed), None, std(Color::WHITE, 0.35, 0.85), &mut images, &mut mats);
    let insulation = Tex::new(256, |u, v| {
        let n = fbm(u * 12.0, v * 12.0, 12, 111);
        let batt = ((u * 3.0).fract() < 0.03) as u32 as f32;
        (mul([0.93, 0.7, 0.55], (0.8 + 0.25 * n) * (1.0 - 0.4 * batt)), n)
    });
    add(M::Insulation, Some(&insulation), Some(2.0), std(Color::WHITE, 1.0, 0.0), &mut images, &mut mats);
    // Packaging: bands of colour (no text or logos), so shelves look stocked.
    let pack = Tex::new(256, |u, v| {
        let col = (u * 16.0).floor() as u32;
        let row = (v * 8.0).floor() as u32;
        let palette = [
            [0.75, 0.15, 0.12],
            [0.95, 0.75, 0.2],
            [0.15, 0.35, 0.7],
            [0.2, 0.55, 0.3],
            [0.92, 0.92, 0.88],
            [0.55, 0.25, 0.55],
            [0.95, 0.5, 0.15],
            [0.1, 0.1, 0.12],
        ];
        let a = palette[(h2(col, row, 121) * 8.0) as usize % 8];
        let b = palette[(h2(col, row, 122) * 8.0) as usize % 8];
        let fv = (v * 8.0).fract();
        let fu = (u * 16.0).fract();
        let band = fv > 0.35 && fv < 0.6;
        let edge = fu < 0.04 || fu > 0.96;
        (
            if edge {
                [0.05, 0.05, 0.05]
            } else if band {
                b
            } else {
                a
            },
            if edge { 0.0 } else { 1.0 },
        )
    });
    add(M::Packaging, Some(&pack), Some(0.5), std(Color::WHITE, 0.6, 0.0), &mut images, &mut mats);
    let cardboard = Tex::new(128, |u, v| {
        let n = fbm(u * 8.0, v * 8.0, 8, 131);
        let tape = (v - 0.5).abs() < 0.04;
        (if tape { [0.75, 0.68, 0.5] } else { mul([0.62, 0.47, 0.3], 0.88 + 0.15 * n) }, 0.0)
    });
    add(M::Cardboard, Some(&cardboard), None, std(Color::WHITE, 0.95, 0.0), &mut images, &mut mats);

    add(M::Paint, None, None, std(Color::WHITE, 0.92, 0.0), &mut images, &mut mats);
    add(M::Ceiling, None, None, std(Color::srgb(0.93, 0.93, 0.92), 0.95, 0.0), &mut images, &mut mats);
    add(M::Chrome, None, None, std(Color::srgb(0.9, 0.9, 0.92), 0.08, 1.0), &mut images, &mut mats);
    add(M::Copper, None, None, std(Color::srgb(0.8, 0.45, 0.3), 0.35, 1.0), &mut images, &mut mats);
    add(M::Plastic, None, None, std(Color::srgb(0.12, 0.12, 0.13), 0.45, 0.0), &mut images, &mut mats);
    add(M::WhitePlastic, None, None, std(Color::srgb(0.92, 0.92, 0.9), 0.35, 0.0), &mut images, &mut mats);
    add(M::Ceramic, None, None, std(Color::srgb(0.96, 0.96, 0.95), 0.08, 0.0), &mut images, &mut mats);
    add(M::Rubber, None, None, std(Color::srgb(0.06, 0.06, 0.06), 0.9, 0.0), &mut images, &mut mats);
    add(M::Plant, None, None, std(Color::srgb(0.12, 0.35, 0.12), 0.8, 0.0), &mut images, &mut mats);
    add(
        M::Screen,
        None,
        None,
        StandardMaterial {
            base_color: Color::srgb(0.02, 0.02, 0.025),
            perceptual_roughness: 0.05,
            emissive: LinearRgba::rgb(4.0, 6.0, 9.0),
            ..default()
        },
        &mut images,
        &mut mats,
    );
    add(
        M::LightPanel,
        None,
        None,
        StandardMaterial { base_color: Color::WHITE, emissive: LinearRgba::rgb(600.0, 580.0, 540.0), ..default() },
        &mut images,
        &mut mats,
    );
    add(
        M::Glass,
        None,
        None,
        StandardMaterial {
            base_color: Color::srgba(0.75, 0.85, 0.88, 0.22),
            perceptual_roughness: 0.03,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        &mut images,
        &mut mats,
    );
    add(
        M::Water,
        None,
        None,
        StandardMaterial {
            base_color: Color::srgba(0.75, 0.86, 0.95, 0.55),
            perceptual_roughness: 0.02,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        &mut images,
        &mut mats,
    );
    add(M::Mirror, None, None, std(Color::srgb(0.8, 0.82, 0.84), 0.02, 1.0), &mut images, &mut mats);
    commands.insert_resource(InteriorMats { mats: out });
}
