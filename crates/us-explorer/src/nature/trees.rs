//! Tree assets: materials (bark, leaves, impostor cards), species prototypes generated
//! on worker threads (four growth stages, three shapes, close and far detail), and
//! the impostor atlas that holds a side view of each prototype.

use super::species::{Bark, Season, Sp, def};
use super::sprites::{self, Canvas};
use super::treegen::{self, Buf, Detail, Stage};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use usx_core::net::WorkPool;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct VegParams {
    pub wind: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct VegExt {
    #[uniform(100)]
    pub params: VegParams,
}

impl MaterialExtension for VegExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://us_explorer/nature/tree.wgsl".into()
    }
}

pub type VegMaterial = ExtendedMaterial<StandardMaterial, VegExt>;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct BillboardParams {
    pub cuts: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct BillboardExt {
    #[uniform(100)]
    pub params: BillboardParams,
}

impl MaterialExtension for BillboardExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://us_explorer/nature/billboard.wgsl".into()
    }
}

pub type BillboardMaterial = ExtendedMaterial<StandardMaterial, BillboardExt>;

/// One prototype tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProtoKey {
    pub sp: Sp,
    pub variant: u8,
    pub stage: Stage,
}

/// A prototype ready to draw.
#[derive(Clone)]
pub struct Proto {
    /// Close-up meshes (bark, leaves); leaves may be missing (bare, cactus).
    pub full: (Handle<Mesh>, Option<Handle<Mesh>>),
    pub low: (Handle<Mesh>, Option<Handle<Mesh>>),
    pub bark: Bark,
    /// Impostor: atlas rectangle (uv min, uv max) and the card's size in metres
    /// (width, height) at scale 1.
    pub rect: (Vec2, Vec2),
    pub card: Vec2,
    pub height: f32,
}

/// Impostor info the drape workers need.
pub type SpriteInfo = Arc<HashMap<ProtoKey, ((Vec2, Vec2), Vec2)>>;

const VARIANTS: u8 = 3;
/// Impostor atlas: 16 x 16 cells of 128 px.
const IMP_CELL: usize = 128;
const IMP_GRID: usize = 16;
const IMP_SIZE: usize = IMP_CELL * IMP_GRID;

struct ProtoJob {
    sp: Sp,
    season: Season,
    full: bool,
}

struct ProtoMade {
    key: ProtoKey,
    full: Option<(Mesh, Option<Mesh>)>,
    low: (Mesh, Option<Mesh>),
    sprite: Canvas,
    extent: f32,
    height: f32,
}

struct ProtoOut {
    sp: Sp,
    season: Season,
    made: Vec<ProtoMade>,
}

#[derive(Resource)]
pub struct TreeAssets {
    pub leaves: Handle<VegMaterial>,
    pub bark: HashMap<Bark, Handle<VegMaterial>>,
    pub billboard: Handle<BillboardMaterial>,
    impostors: Handle<Image>,
    /// CPU copy of the impostor atlas, one buffer per mip level.
    atlas_levels: Vec<Vec<u8>>,
    free_cells: Vec<u32>,
    cells_of: HashMap<Sp, Vec<u32>>,
    pub protos: HashMap<ProtoKey, Proto>,
    pending: HashSet<Sp>,
    pool: WorkPool<ProtoJob, ProtoOut>,
    /// The season the prototypes were grown for.
    pub season: Season,
    /// Bumped whenever prototypes are added or replaced.
    pub generation: u64,
    pub sprite_info: SpriteInfo,
    /// Whether close-up meshes are wanted (3D trees on).
    pub full_detail: bool,
}

/// The leaf atlas is painted once; prototype workers share it (and its mip levels,
/// for drawing impostors).
fn leaf_canvas() -> &'static Canvas {
    static C: OnceLock<Canvas> = OnceLock::new();
    C.get_or_init(|| sprites::leaf_atlas(256))
}

fn leaf_levels() -> &'static [Canvas] {
    static L: OnceLock<Vec<Canvas>> = OnceLock::new();
    L.get_or_init(|| sprites::pyramid_cells(leaf_canvas(), 0.5, 256))
}

fn to_mesh(b: Buf, tangents: bool) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    let tan = tangents.then(|| crate::buildings::furniture::tangents(&b.pos, &b.nrm, &b.uv, &b.idx));
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, b.pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, b.nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, b.uv_b);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.col);
    if let Some(t) = tan {
        m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, t);
    }
    m.insert_indices(Indices::U32(b.idx));
    m
}

fn make(job: ProtoJob) -> ProtoOut {
    let leaf = leaf_levels();
    let mut made = Vec::new();
    for variant in 0..VARIANTS {
        for stage in Stage::ALL {
            let key = ProtoKey { sp: job.sp, variant, stage };
            let full = treegen::generate(job.sp, variant as u32, stage, Detail::Full, &job.season);
            let low = treegen::generate(job.sp, variant as u32, stage, Detail::Low, &job.season);
            let (sprite, extent) = sprites::impostor(&full, leaf, IMP_CELL);
            let pair = |t: treegen::TreeMesh| {
                let leaves = (!t.leaves.idx.is_empty()).then(|| to_mesh(t.leaves, false));
                (to_mesh(t.wood, true), leaves)
            };
            let height = full.height;
            made.push(ProtoMade { key, full: job.full.then(|| pair(full)), low: pair(low), sprite, extent, height });
        }
    }
    ProtoOut { sp: job.sp, season: job.season, made }
}

fn image_from(data: Vec<u8>, w: u32, h: u32, levels: u32, srgb: bool, repeat: bool) -> Image {
    let mut img = Image::new_uninit(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm },
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    let mode = if repeat { ImageAddressMode::Repeat } else { ImageAddressMode::ClampToEdge };
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode,
        address_mode_v: mode,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: if repeat { 4 } else { 1 },
        ..default()
    });
    img
}

/// Mip chain of plain RGBA bytes (box filter), for bark and normal maps.
fn plain_mips(base: Vec<u8>, size: usize) -> (Vec<u8>, u32) {
    let mut out = base.clone();
    let mut cur = base;
    let mut s = size;
    let mut levels = 1;
    while s > 1 {
        let n = s / 2;
        let mut next = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                for c in 0..4 {
                    let a = |xx: usize, yy: usize| cur[(yy * s + xx) * 4 + c] as u32;
                    next[(y * n + x) * 4 + c] = ((a(2 * x, 2 * y) + a(2 * x + 1, 2 * y) + a(2 * x, 2 * y + 1) + a(2 * x + 1, 2 * y + 1) + 2) / 4) as u8;
                }
            }
        }
        out.extend_from_slice(&next);
        cur = next;
        s = n;
        levels += 1;
    }
    (out, levels)
}

pub fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut veg: ResMut<Assets<VegMaterial>>,
    mut bbm: ResMut<Assets<BillboardMaterial>>,
    settings: Res<crate::GameSettings>,
) {
    // Leaves: the atlas is painted here at startup (a fraction of a second).
    let leaf = leaf_canvas();
    let (data, levels) = sprites::mips_cells(leaf, 0.5, 256);
    let leaf_img = images.add(image_from(data, leaf.w as u32, leaf.h as u32, levels, true, false));
    let leaves = veg.add(VegMaterial {
        base: StandardMaterial {
            base_color_texture: Some(leaf_img),
            alpha_mode: AlphaMode::Mask(0.5),
            perceptual_roughness: 0.78,
            reflectance: 0.3,
            cull_mode: None,
            double_sided: true,
            // Sunlight through the leaves.
            diffuse_transmission: 0.3,
            ..default()
        },
        extension: VegExt::default(),
    });
    let mut bark = HashMap::new();
    for b in [Bark::Furrowed, Bark::Smooth, Bark::White, Bark::Mottled, Bark::Plated, Bark::Stringy, Bark::PalmRings, Bark::PalmBoots, Bark::PalmSmooth, Bark::Cactus] {
        let size = 256;
        let (c, h) = sprites::bark(b, size);
        let (alb, lv) = plain_mips(c.to_rgba8(), size);
        let (nrm, lv2) = plain_mips(sprites::normal_map(&h, size, if matches!(b, Bark::Furrowed | Bark::Plated | Bark::PalmBoots) { 6.0 } else { 2.5 }), size);
        let alb = images.add(image_from(alb, size as u32, size as u32, lv, true, true));
        let nrm = images.add(image_from(nrm, size as u32, size as u32, lv2, false, true));
        bark.insert(
            b,
            veg.add(VegMaterial {
                base: StandardMaterial {
                    base_color_texture: Some(alb),
                    normal_map_texture: Some(nrm),
                    perceptual_roughness: if b == Bark::Cactus { 0.6 } else { 0.92 },
                    reflectance: 0.25,
                    ..default()
                },
                extension: VegExt::default(),
            }),
        );
    }
    // Impostor atlas, filled as prototypes arrive.
    let mut atlas_levels = Vec::new();
    let mut s = IMP_SIZE;
    while s >= IMP_GRID {
        atlas_levels.push(vec![0u8; s * s * 4]);
        s /= 2;
    }
    let all: Vec<u8> = atlas_levels.iter().flatten().copied().collect();
    let impostors = images.add(image_from(all, IMP_SIZE as u32, IMP_SIZE as u32, atlas_levels.len() as u32, true, false));
    let billboard = bbm.add(BillboardMaterial {
        base: StandardMaterial {
            base_color_texture: Some(impostors.clone()),
            alpha_mode: AlphaMode::Mask(0.5),
            perceptual_roughness: 0.85,
            reflectance: 0.25,
            cull_mode: None,
            double_sided: true,
            ..default()
        },
        extension: BillboardExt::default(),
    });
    let pool = WorkPool::new("tree-proto", 1, |_, j: ProtoJob| make(j));
    commands.insert_resource(TreeAssets {
        leaves,
        bark,
        billboard,
        impostors,
        atlas_levels,
        free_cells: (0..(IMP_GRID * IMP_GRID) as u32).rev().collect(),
        cells_of: HashMap::new(),
        protos: HashMap::new(),
        pending: HashSet::new(),
        pool,
        season: Season::default(),
        generation: 0,
        sprite_info: Arc::new(HashMap::new()),
        full_detail: super::three_d(settings.0.preset),
    });
}

impl TreeAssets {
    /// Ask for a species' prototypes (no-op if they're ready or on the way).
    pub fn want(&mut self, sp: Sp) {
        if self.pending.contains(&sp) || self.protos.contains_key(&ProtoKey { sp, variant: 0, stage: Stage::Mature }) {
            return;
        }
        self.pending.insert(sp);
        self.pool.submit(sp as u64, 0.0, ProtoJob { sp, season: self.season, full: self.full_detail });
    }

    pub fn ready(&self, sp: Sp) -> bool {
        self.protos.contains_key(&ProtoKey { sp, variant: 0, stage: Stage::Mature })
    }

    /// Start over with a new season (or detail setting): prototypes are regrown.
    pub fn reset(&mut self, season: Season, full_detail: bool) {
        self.season = season;
        self.full_detail = full_detail;
        self.protos.clear();
        self.pending.clear();
        for (_, cells) in self.cells_of.drain() {
            self.free_cells.extend(cells);
        }
        self.sprite_info = Arc::new(HashMap::new());
        self.generation += 1;
    }

    fn put_sprite(&mut self, cell: u32, sprite: &Canvas) {
        let (cx, cy) = ((cell as usize % IMP_GRID) * IMP_CELL, (cell as usize / IMP_GRID) * IMP_CELL);
        let (data, levels) = sprites::mips(sprite, 0.5);
        let mut off = 0;
        let mut s = IMP_CELL;
        for l in 0..levels as usize {
            if l >= self.atlas_levels.len() {
                break;
            }
            let lw = IMP_SIZE >> l;
            let (ox, oy) = (cx >> l, cy >> l);
            for y in 0..s {
                let src = &data[off + y * s * 4..off + (y + 1) * s * 4];
                let dst = ((oy + y) * lw + ox) * 4;
                self.atlas_levels[l][dst..dst + s * 4].copy_from_slice(src);
            }
            off += s * s * 4;
            s /= 2;
            if s == 0 {
                break;
            }
        }
    }
}

/// Collect finished prototypes: meshes into assets, side views into the atlas.
pub fn receive(mut ta: ResMut<TreeAssets>, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>) {
    let mut changed = false;
    while let Some(out) = ta.pool.try_recv() {
        ta.pending.remove(&out.sp);
        if out.season != ta.season {
            continue;
        }
        let d = def(out.sp);
        let mut cells = Vec::new();
        let mut info = (*ta.sprite_info).clone();
        for m in out.made {
            let Some(cell) = ta.free_cells.pop() else { break };
            cells.push(cell);
            ta.put_sprite(cell, &m.sprite);
            let (cx, cy) = (cell as usize % IMP_GRID, cell as usize / IMP_GRID);
            let pad = 0.5 / IMP_SIZE as f32;
            let rect = (
                Vec2::new(cx as f32 / IMP_GRID as f32 + pad, cy as f32 / IMP_GRID as f32 + pad),
                Vec2::new((cx + 1) as f32 / IMP_GRID as f32 - pad, (cy + 1) as f32 / IMP_GRID as f32 - pad),
            );
            let card = Vec2::splat(m.extent);
            info.insert(m.key, (rect, card));
            let add = |meshes: &mut Assets<Mesh>, p: (Mesh, Option<Mesh>)| (meshes.add(p.0), p.1.map(|l| meshes.add(l)));
            let low = add(&mut meshes, m.low);
            let full = m.full.map(|f| add(&mut meshes, f)).unwrap_or_else(|| low.clone());
            ta.protos.insert(m.key, Proto { full, low, bark: d.bark, rect, card, height: m.height });
        }
        ta.cells_of.insert(out.sp, cells);
        ta.sprite_info = Arc::new(info);
        ta.generation += 1;
        changed = true;
    }
    if changed {
        let all: Vec<u8> = ta.atlas_levels.iter().flatten().copied().collect();
        if let Some(mut img) = images.get_mut(&ta.impostors) {
            img.data = Some(all);
        }
    }
}
