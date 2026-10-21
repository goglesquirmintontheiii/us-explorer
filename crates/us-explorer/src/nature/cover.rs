//! What grows where, cell by cell (zoom-17 tiles, about 300 m): the aerial imagery is
//! read pixel by pixel (tree canopy, lawn, dry grass, scrub, sand, bare ground), roads,
//! buildings, parking and water are masked out from the map data, OpenStreetMap land
//! cover and mapped trees are folded in, and trees are planted to fill the canopy seen
//! from above (crown sizes from the canopy itself), with species for the region and
//! setting (street, woods, riverside, beach).

use super::nature_osm::{CoverClass, NatureData};
use super::species::{self, Biome, Season, Setting, Sp, def};
use super::treegen::Stage;
use crate::roads::build::{Corridor, Lake, Local};
use bevy::math::Vec2;
use std::sync::Arc;
use usx_core::sources::ImagerySource;
use usx_core::tile::TileId;

/// Cells are zoom-17 tiles.
pub const CELL_ZOOM: u8 = 17;
/// Imagery and analysis resolution: zoom 18 (four tiles, 512 px across a cell).
const IMG_ZOOM: u8 = 18;
const N: usize = 512;
/// Ground-cover raster kept per cell for grass (256 x 256, about 1.2 m).
pub const GRASS_N: usize = 256;

/// Ground cover per pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Cover {
    /// Paved, built on, or unknown: nothing grows.
    #[default]
    None = 0,
    Water = 1,
    Canopy = 2,
    Grass = 3,
    DryGrass = 4,
    Shrub = 5,
    Sand = 6,
    Crop = 7,
    Wetland = 8,
    Rock = 9,
    /// Sand held by dune grass (sea oats, beach grass) behind a beach.
    Dune = 10,
}

impl Cover {
    pub fn from_u8(v: u8) -> Cover {
        match v {
            1 => Cover::Water,
            2 => Cover::Canopy,
            3 => Cover::Grass,
            4 => Cover::DryGrass,
            5 => Cover::Shrub,
            6 => Cover::Sand,
            7 => Cover::Crop,
            8 => Cover::Wetland,
            9 => Cover::Rock,
            10 => Cover::Dune,
            _ => Cover::None,
        }
    }
    /// Grass grows here (and how dense, 0..1).
    pub fn grass(self) -> f32 {
        match self {
            Cover::Grass => 1.0,
            Cover::DryGrass => 0.75,
            Cover::Canopy => 0.45,
            Cover::Shrub => 0.4,
            Cover::Crop => 1.0,
            Cover::Wetland => 0.9,
            Cover::Dune => 0.4,
            _ => 0.0,
        }
    }
}

/// One tree (or shrub) to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TreeInst {
    /// Position in the cell's local frame (metres, x east, z south).
    pub p: Vec2,
    pub sp: Sp,
    pub stage: Stage,
    pub variant: u8,
    /// Uniform scale on the prototype.
    pub scale: f32,
    pub yaw: f32,
}

/// Everything about one cell's vegetation.
pub struct CellData {
    pub id: TileId,
    pub local: Local,
    pub biome: Biome,
    pub season: Season,
    pub trees: Vec<TreeInst>,
    /// Ground cover (Cover as u8) and its colour (sRGB bytes), GRASS_N x GRASS_N, row 0
    /// at the north edge.
    pub cover: Vec<u8>,
    pub colour: Vec<[u8; 3]>,
    /// Whether imagery was available (else placement came from map data alone).
    pub imagery: bool,
}

impl CellData {
    /// Cover and colour at a lat/lon inside this cell.
    pub fn sample(&self, lat: f64, lon: f64) -> Option<(Cover, [u8; 3])> {
        let (u, v) = tile_uv(self.id, lat, lon);
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let k = (v * GRASS_N as f64) as usize * GRASS_N + (u * GRASS_N as f64) as usize;
        Some((Cover::from_u8(self.cover[k]), self.colour[k]))
    }
}

/// Tile-local (u, v) of a lat/lon (not clamped).
pub fn tile_uv(id: TileId, lat: f64, lon: f64) -> (f64, f64) {
    let n = (1u64 << id.z) as f64;
    let latr = lat.to_radians();
    let fx = (lon + 180.0) / 360.0 * n;
    let fy = (1.0 - (latr.tan() + 1.0 / latr.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (fx - id.x as f64, fy - id.y as f64)
}

/// Inputs for building a cell (gathered on the main thread, built on a worker).
pub struct CellInput {
    pub id: TileId,
    pub imagery: ImagerySource,
    pub season: Season,
    pub corridors: Vec<Corridor>,
    /// Lakes and rivers (dug out) and parking lots (levelled) from the road layer.
    pub areas: Vec<Lake>,
    pub buildings: Vec<crate::buildings::Footprint>,
    pub nature: Option<Arc<NatureData>>,
}

fn decode(bytes: &[u8]) -> Option<usx_core::image::RgbaImage> {
    usx_core::image::load_from_memory(bytes).ok().map(|i| i.to_rgba8())
}

/// The cell's imagery at zoom 18 stitched to 512 x 512 (or zoom 17 enlarged).
fn imagery(id: TileId, src: ImagerySource) -> Option<usx_core::image::RgbaImage> {
    let mut out = usx_core::image::RgbaImage::new(N as u32, N as u32);
    let mut ok = true;
    for dy in 0..2u32 {
        for dx in 0..2u32 {
            let t = TileId::new(IMG_ZOOM, id.x * 2 + dx, id.y * 2 + dy);
            let key = format!("img/{}/{}.jpg", src.key(), t.path());
            match crate::terrain::load::fetch(&key, &src.url(t)).ok().and_then(|b| decode(&b)) {
                Some(img) if img.width() == 256 => {
                    usx_core::image::imageops::overlay(&mut out, &img, (dx * 256) as i64, (dy * 256) as i64);
                }
                _ => ok = false,
            }
        }
    }
    if ok {
        return Some(out);
    }
    let key = format!("img/{}/{}.jpg", src.key(), id.path());
    let img = crate::terrain::load::fetch(&key, &src.url(id)).ok().and_then(|b| decode(&b))?;
    Some(usx_core::image::imageops::resize(&img, N as u32, N as u32, usx_core::image::imageops::FilterType::Triangle))
}

/// Mask values.
const M_FREE: u8 = 0;
const M_ROAD: u8 = 1;
const M_BUILDING: u8 = 2;
const M_WATER: u8 = 3;
const M_LOT: u8 = 4;

struct Raster {
    id: TileId,
    /// Pixel -> lat/lon and back (pixel centres).
    n: usize,
}

impl Raster {
    fn ll(&self, x: f32, y: f32) -> (f64, f64) {
        self.id.lat_lon_at(x as f64 / self.n as f64, y as f64 / self.n as f64)
    }
    fn px(&self, lat: f64, lon: f64) -> Vec2 {
        let (u, v) = tile_uv(self.id, lat, lon);
        Vec2::new((u * self.n as f64) as f32, (v * self.n as f64) as f32)
    }
}

/// Fill a polygon (pixel coordinates) into a mask with scanlines.
fn fill_poly(mask: &mut [u8], n: usize, rings: &[Vec<Vec2>], val: u8, only_free: bool) {
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for p in rings.iter().flatten() {
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    let ya = y0.floor().max(0.0) as i64;
    let yb = (y1.ceil() as i64).min(n as i64 - 1);
    let mut xs: Vec<f32> = Vec::new();
    for y in ya..=yb {
        let yc = y as f32 + 0.5;
        xs.clear();
        for r in rings {
            let m = r.len();
            for i in 0..m {
                let (a, b) = (r[i], r[(i + 1) % m]);
                if (a.y > yc) != (b.y > yc) {
                    xs.push(a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.chunks_exact(2) {
            let xa = (pair[0] - 0.5).ceil().max(0.0) as i64;
            let xb = ((pair[1] - 0.5).floor() as i64).min(n as i64 - 1);
            for x in xa..=xb {
                let k = y as usize * n + x as usize;
                if !only_free || mask[k] == M_FREE {
                    mask[k] = val;
                }
            }
        }
    }
}

/// Thick polyline (pixels) into a mask.
fn fill_line(mask: &mut [u8], n: usize, pts: &[Vec2], half_px: f32, val: u8) {
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let lo = a.min(b) - Vec2::splat(half_px + 1.0);
        let hi = a.max(b) + Vec2::splat(half_px + 1.0);
        if hi.x < 0.0 || hi.y < 0.0 || lo.x > n as f32 || lo.y > n as f32 {
            continue;
        }
        let ab = b - a;
        let l2 = ab.length_squared().max(1e-6);
        for y in (lo.y.max(0.0) as usize)..(hi.y.min(n as f32) as usize) {
            for x in (lo.x.max(0.0) as usize)..(hi.x.min(n as f32) as usize) {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                if p.distance(a + ab * t) <= half_px {
                    let k = y * n + x;
                    if mask[k] == M_FREE || val == M_ROAD && mask[k] == M_LOT {
                        mask[k] = val;
                    }
                }
            }
        }
    }
}

fn srgb(c: u8) -> f32 {
    c as f32 / 255.0
}

/// Classify one imagery pixel (with its 3x3 luminance spread as texture).
fn classify(r: f32, g: f32, b: f32, spread: f32, biome: Biome) -> Cover {
    let lum = (r + g + b) / 3.0;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let chroma = mx - mn;
    let exg = 2.0 * g - r - b;
    let arid = matches!(biome, Biome::Sonoran | Biome::Mojave | Biome::Chihuahuan | Biome::GreatBasin);
    // Water: dark, blue or blue-green, smooth.
    if b > r + 0.02 && b >= g - 0.02 && lum < 0.3 && spread < 0.05 {
        return Cover::Water;
    }
    // Green vegetation.
    if exg > 0.035 && g >= r * 0.95 || (g > r && g > b && chroma > 0.05) {
        // Tree canopy: darker and textured (crowns and their shadows); lawns are
        // lighter and smooth.
        let dark = lum < 0.3;
        let rough = spread > 0.045;
        if dark && rough || lum < 0.2 {
            return Cover::Canopy;
        }
        if rough && lum < 0.36 {
            return if arid { Cover::Shrub } else { Cover::Canopy };
        }
        return Cover::Grass;
    }
    // Very dark and textured: canopy in shadow (or deep shadows of anything).
    if lum < 0.12 && spread > 0.04 {
        return Cover::Canopy;
    }
    // Tan, brown: dry grass, bare soil, sand.
    let warm = r > b + 0.04 && chroma > 0.06;
    if warm {
        if lum > 0.55 {
            return Cover::Sand;
        }
        if exg > -0.04 && lum > 0.22 {
            return if arid && spread > 0.05 { Cover::Shrub } else { Cover::DryGrass };
        }
        return if arid { Cover::Rock } else { Cover::DryGrass };
    }
    if lum > 0.62 && chroma < 0.12 && r > b {
        return Cover::Sand;
    }
    Cover::None
}

/// Rough crown radius of a mature tree of a species.
fn crown_r(sp: Sp) -> f32 {
    let d = def(sp);
    (d.height * d.width * 0.5).clamp(0.6, 14.0)
}

/// Build a cell (worker thread).
pub fn build(input: CellInput) -> CellData {
    let id = input.id;
    let (cla, clo) = id.lat_lon_at(0.5, 0.5);
    let local = Local::new(cla, clo);
    let elev = crate::terrain::load::dem_height(cla, clo).unwrap_or(0.0);
    let biome = species::biome_at(cla, clo, elev);
    let r = Raster { id, n: N };
    let mpp = (id.width_m() / N as f64) as f32;
    // ---- masks from map data
    let mut mask = vec![M_FREE; N * N];
    let (w, s, e, nn) = id.bounds();
    let hits = |bb: (f64, f64, f64, f64)| !(bb.0 > nn || bb.2 < s || bb.1 > e || bb.3 < w);
    for a in input.areas.iter().filter(|a| !a.level) {
        if !hits(a.bbox) {
            continue;
        }
        let mut rings: Vec<Vec<Vec2>> = a.outer.iter().map(|ring| ring.iter().map(|&(la, lo)| r.px(la, lo)).collect()).collect();
        rings.extend(a.inner.iter().map(|ring| ring.iter().map(|&(la, lo)| r.px(la, lo)).collect::<Vec<_>>()));
        fill_poly(&mut mask, N, &rings, M_WATER, false);
    }
    for l in input.areas.iter().filter(|l| l.level && hits(l.bbox)) {
        let mut rings: Vec<Vec<Vec2>> = l.outer.iter().map(|ring| ring.iter().map(|&(la, lo)| r.px(la, lo)).collect()).collect();
        rings.extend(l.inner.iter().map(|ring| ring.iter().map(|&(la, lo)| r.px(la, lo)).collect::<Vec<_>>()));
        fill_poly(&mut mask, N, &rings, M_LOT, true);
    }
    for c in input.corridors.iter().filter(|c| hits(c.bbox)) {
        let pts: Vec<Vec2> = c.pts.iter().map(|&(la, lo)| r.px(la, lo)).collect();
        fill_line(&mut mask, N, &pts, (c.half + 0.3) / mpp, M_ROAD);
    }
    for f in input.buildings.iter().filter(|f| hits(f.bbox)) {
        let ring: Vec<Vec2> = f.ring.iter().map(|&(la, lo)| r.px(la, lo)).collect();
        fill_poly(&mut mask, N, &[ring], M_BUILDING, false);
    }
    // ---- imagery
    let img = imagery(id, input.imagery);
    let mut cover = vec![Cover::None; N * N];
    let mut colour = vec![[90u8, 110, 60]; N * N];
    if let Some(img) = &img {
        let lum = |x: usize, y: usize| {
            let p = img.get_pixel(x as u32, y as u32);
            (p[0] as f32 + p[1] as f32 + p[2] as f32) / (3.0 * 255.0)
        };
        for y in 0..N {
            for x in 0..N {
                let k = y * N + x;
                let p = img.get_pixel(x as u32, y as u32);
                colour[k] = [p[0], p[1], p[2]];
                if mask[k] != M_FREE {
                    continue;
                }
                // Texture: spread of luminance around the pixel.
                let (mut lo, mut hi) = (1.0f32, 0.0f32);
                for dy in [-1i64, 1] {
                    for dx in [-1i64, 1] {
                        let (xx, yy) = ((x as i64 + dx).clamp(0, N as i64 - 1) as usize, (y as i64 + dy).clamp(0, N as i64 - 1) as usize);
                        let l = lum(xx, yy);
                        lo = lo.min(l);
                        hi = hi.max(l);
                    }
                }
                cover[k] = classify(srgb(p[0]), srgb(p[1]), srgb(p[2]), hi - lo, biome);
            }
        }
    }
    // ---- OpenStreetMap land cover
    if let Some(nat) = &input.nature {
        for c in nat.cover.iter().filter(|c| hits(c.bbox)) {
            let rings: Vec<Vec<Vec2>> = c.outer.iter().chain(c.inner.iter()).map(|ring| ring.iter().map(|&(la, lo)| r.px(la, lo)).collect()).collect();
            let mut m = vec![0u8; N * N];
            fill_poly(&mut m, N, &rings, 1, false);
            for k in 0..N * N {
                if m[k] == 0 || mask[k] != M_FREE {
                    continue;
                }
                let have = cover[k];
                cover[k] = match c.class {
                    // Woods: canopy unless imagery clearly shows open ground.
                    CoverClass::Forest => {
                        if img.is_none() || matches!(have, Cover::Canopy | Cover::None | Cover::Shrub) { Cover::Canopy } else { have }
                    }
                    CoverClass::Grass | CoverClass::Park => {
                        if matches!(have, Cover::Canopy) { have } else if img.is_some() && have == Cover::None { Cover::None } else { Cover::Grass }
                    }
                    CoverClass::Farm => {
                        if matches!(have, Cover::Canopy) { have } else { Cover::Crop }
                    }
                    CoverClass::Scrub => {
                        if matches!(have, Cover::Canopy | Cover::Shrub) { Cover::Shrub } else { Cover::DryGrass }
                    }
                    CoverClass::Sand => Cover::Sand,
                    CoverClass::Dune => Cover::Dune,
                    CoverClass::Wetland => {
                        if have == Cover::Canopy { have } else { Cover::Wetland }
                    }
                    CoverClass::Rock => Cover::Rock,
                    CoverClass::Golf => {
                        if have == Cover::Canopy || have == Cover::Sand { have } else { Cover::Grass }
                    }
                };
            }
        }
    }
    for k in 0..N * N {
        if mask[k] == M_WATER {
            cover[k] = Cover::Water;
        }
    }
    // Dunes: sand on the coast well back from the water grows dune grass.
    if input.nature.as_ref().is_some_and(|n| n.coast_near(cla, clo, 600.0)) {
        let wet = distance_to(&cover.iter().map(|c| *c as u8).collect::<Vec<u8>>(), N, |c| c == Cover::Water as u8, 60.0 / mpp);
        for k in 0..N * N {
            if cover[k] == Cover::Sand && wet[k] >= 42.0 / mpp && mask[k] == M_FREE {
                cover[k] = Cover::Dune;
            }
        }
    }
    // ---- setting for species: built-up, beside water, on the coast, or wild
    let built = mask.iter().filter(|&&m| m == M_ROAD || m == M_BUILDING || m == M_LOT).count() as f32 / (N * N) as f32;
    let urban = built > 0.12;
    let water_dist = distance_to(&mask, N, |m| m == M_WATER, 40.0 / mpp);
    let sand_dist = distance_to(&cover.iter().map(|c| *c as u8).collect::<Vec<u8>>(), N, |c| c == Cover::Sand as u8, 120.0 / mpp);
    let coastline_near = input.nature.as_ref().is_some_and(|n| n.coast_near(cla, clo, 400.0));
    // ---- trees
    let mut trees: Vec<TreeInst> = Vec::new();
    let mut rng = Rng(id.x as u64 * 0x9E37_79B9 ^ (id.y as u64) << 20);
    // Mapped trees first (they're exact).
    let mut taken: Vec<(Vec2, f32)> = Vec::new();
    let setting_at = |px: usize, py: usize| -> Setting {
        let k = py * N + px;
        if (coastline_near || sand_dist[k] < 60.0 / mpp) && (sand_dist[k] < 80.0 / mpp) {
            Setting::Coastal
        } else if water_dist[k] < 30.0 / mpp {
            Setting::Riparian
        } else if urban {
            Setting::Urban
        } else {
            Setting::Wild
        }
    };
    if let Some(nat) = &input.nature {
        for t in &nat.trees {
            let p = r.px(t.lat, t.lon);
            if p.x < 0.0 || p.y < 0.0 || p.x >= N as f32 || p.y >= N as f32 {
                continue;
            }
            let (px, py) = (p.x as usize, p.y as usize);
            let set = setting_at(px, py);
            let sp = t.species.unwrap_or_else(|| species::pick(species::mix(biome, set), rng.f()));
            let d = def(sp);
            let h = t.height.unwrap_or(d.height * rng.r(0.5, 1.1));
            let (stage, scale) = stage_for(&d, h);
            let pl = local.xz(t.lat, t.lon);
            trees.push(TreeInst { p: pl, sp, stage, variant: (rng.f() * 3.0) as u8, scale, yaw: rng.f() * 6.283 });
            taken.push((p, (d.width * h * 0.5 / mpp).max(1.0)));
        }
    }
    // Canopy: plant crowns to fill it (Poisson disc, crown size from how far the
    // canopy extends round each spot, capped by the species).
    let canopy: Vec<u8> = cover.iter().map(|c| (*c == Cover::Canopy) as u8).collect();
    let dt = distance_to(&canopy, N, |c| c == 0, 30.0 / mpp);
    let shrubby: Vec<u8> = cover.iter().map(|c| (*c == Cover::Shrub) as u8).collect();
    let grid = 3usize; // candidate spacing, pixels
    let mut cands: Vec<(f32, usize, usize)> = Vec::new();
    for y in (0..N).step_by(grid) {
        for x in (0..N).step_by(grid) {
            let (jx, jy) = ((x + (rng.f() * grid as f32) as usize).min(N - 1), (y + (rng.f() * grid as f32) as usize).min(N - 1));
            let k = jy * N + jx;
            if canopy[k] == 1 && mask[k] == M_FREE {
                // Bigger crowns first.
                cands.push((dt[k] + rng.f(), jx, jy));
            } else if shrubby[k] == 1 && mask[k] == M_FREE && rng.f() < 0.35 {
                cands.push((-1.0 - rng.f(), jx, jy));
            }
        }
    }
    cands.sort_by(|a, b| b.0.total_cmp(&a.0));
    // Spatial hash of placed crowns.
    let bucket = 16usize;
    let nb = N / bucket + 1;
    let mut buckets: Vec<Vec<(Vec2, f32)>> = vec![Vec::new(); nb * nb];
    for &(p, rad) in &taken {
        let (bx, by) = ((p.x as usize / bucket).min(nb - 1), (p.y as usize / bucket).min(nb - 1));
        buckets[by * nb + bx].push((p, rad));
    }
    let max_trees = 6000;
    for (score, x, y) in cands {
        if trees.len() >= max_trees {
            break;
        }
        let shrub = score < 0.0;
        let set = setting_at(x, y);
        let list = if shrub { shrub_mix(biome) } else { species::mix(biome, set) };
        let sp = species::pick(list, rng.f());
        let d = def(sp);
        let cap = crown_r(sp) * rng.r(0.75, 1.25) / mpp;
        let k = y * N + x;
        let rad_px = if shrub { (d.height * d.width * 0.5 / mpp).max(1.0) } else { dt[k].clamp(1.2 / mpp, cap.max(1.3 / mpp)) };
        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
        // Room for it? Crowns may overlap a little.
        let (bx, by) = ((x / bucket).min(nb - 1), (y / bucket).min(nb - 1));
        let reach = ((rad_px + 14.0 / mpp) / bucket as f32).ceil() as i64;
        let mut ok = true;
        'scan: for yy in (by as i64 - reach).max(0)..=(by as i64 + reach).min(nb as i64 - 1) {
            for xx in (bx as i64 - reach).max(0)..=(bx as i64 + reach).min(nb as i64 - 1) {
                for &(q, qr) in &buckets[yy as usize * nb + xx as usize] {
                    if p.distance(q) < (rad_px + qr) * 0.72 {
                        ok = false;
                        break 'scan;
                    }
                }
            }
        }
        if !ok {
            continue;
        }
        buckets[by * nb + bx].push((p, rad_px));
        // Height from the crown: a crown of radius R on a species w wide per metre of
        // height stands about 2R/w tall.
        let crown_m = rad_px * mpp;
        let h = if shrub { d.height * rng.r(0.7, 1.2) } else { (2.0 * crown_m / d.width.max(0.15)).clamp(d.height * 0.15, d.height * 1.35) };
        let (stage, scale) = stage_for(&d, h);
        let (la, lo) = r.ll(p.x, p.y);
        trees.push(TreeInst { p: local.xz(la, lo), sp, stage, variant: (rng.f() * 3.0) as u8, scale, yaw: rng.f() * 6.283 });
    }
    // Tree rows mapped as lines.
    if let Some(nat) = &input.nature {
        for row in &nat.tree_rows {
            let pts: Vec<Vec2> = row.iter().map(|&(la, lo)| local.xz(la, lo)).collect();
            for w2 in pts.windows(2) {
                let len = w2[0].distance(w2[1]);
                let n = (len / 8.0).floor().max(1.0) as usize;
                for i in 0..n {
                    let pl = w2[0].lerp(w2[1], (i as f32 + 0.5) / n as f32);
                    let (la, lo) = local.ll(pl);
                    let p = r.px(la, lo);
                    if p.x < 0.0 || p.y < 0.0 || p.x >= N as f32 || p.y >= N as f32 {
                        continue;
                    }
                    let sp = species::pick(species::mix(biome, Setting::Urban), rng.f());
                    let d = def(sp);
                    let h = d.height * rng.r(0.6, 1.05);
                    let (stage, scale) = stage_for(&d, h);
                    trees.push(TreeInst { p: pl, sp, stage, variant: (rng.f() * 3.0) as u8, scale, yaw: rng.f() * 6.283 });
                }
            }
        }
    }
    // ---- ground cover raster for grass (2x2 pixels per texel)
    let mut gcover = vec![0u8; GRASS_N * GRASS_N];
    let mut gcol = vec![[0u8; 3]; GRASS_N * GRASS_N];
    for y in 0..GRASS_N {
        for x in 0..GRASS_N {
            let mut counts = [0u8; 10];
            let mut acc = [0u32; 3];
            let mut n = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let k = (y * 2 + dy) * N + x * 2 + dx;
                    let c = if mask[k] != M_FREE && mask[k] != M_WATER { Cover::None } else { cover[k] };
                    counts[c as usize] += 1;
                    if c.grass() > 0.0 {
                        for i in 0..3 {
                            acc[i] += colour[k][i] as u32;
                        }
                        n += 1;
                    }
                }
            }
            let best = (0..10).max_by_key(|&i| counts[i]).unwrap_or(0);
            gcover[y * GRASS_N + x] = best as u8;
            gcol[y * GRASS_N + x] = if n > 0 { [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8] } else { [0, 0, 0] };
        }
    }
    CellData { id, local, biome, season: input.season, trees, cover: gcover, colour: gcol, imagery: img.is_some() }
}

fn shrub_mix(b: Biome) -> &'static [(Sp, f32)] {
    match b {
        Biome::Sonoran | Biome::Chihuahuan | Biome::Mojave => &[(Sp::Creosote, 3.0), (Sp::Shrub, 1.0)],
        Biome::GreatBasin | Biome::InlandNorthwest => &[(Sp::Sagebrush, 3.0), (Sp::Shrub, 0.5)],
        Biome::SouthFlorida | Biome::GulfCoast => &[(Sp::SawPalmetto, 2.0), (Sp::Shrub, 2.0)],
        _ => &[(Sp::Shrub, 1.0)],
    }
}

/// Growth stage and scale for a tree `h` metres tall.
fn stage_for(d: &species::Def, h: f32) -> (Stage, f32) {
    let rel = h / d.height.max(0.5);
    let stage = if rel < 0.34 {
        Stage::Sapling
    } else if rel < 0.72 {
        Stage::Young
    } else if rel < 1.12 {
        Stage::Mature
    } else {
        Stage::Old
    };
    (stage, (rel / stage.scale()).clamp(0.7, 1.35))
}

/// Chamfer distance (pixels) to the nearest pixel where `is_src` holds, capped.
fn distance_to(v: &[u8], n: usize, is_src: impl Fn(u8) -> bool, cap: f32) -> Vec<f32> {
    let mut d: Vec<f32> = v.iter().map(|&x| if is_src(x) { 0.0 } else { cap }).collect();
    let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..n {
        for x in 0..n {
            let k = y * n + x;
            let mut m = d[k];
            if x > 0 {
                m = m.min(d[k - 1] + a);
            }
            if y > 0 {
                m = m.min(d[k - n] + a);
                if x > 0 {
                    m = m.min(d[k - n - 1] + b);
                }
                if x + 1 < n {
                    m = m.min(d[k - n + 1] + b);
                }
            }
            d[k] = m;
        }
    }
    for y in (0..n).rev() {
        for x in (0..n).rev() {
            let k = y * n + x;
            let mut m = d[k];
            if x + 1 < n {
                m = m.min(d[k + 1] + a);
            }
            if y + 1 < n {
                m = m.min(d[k + n] + a);
                if x + 1 < n {
                    m = m.min(d[k + n + 1] + b);
                }
                if x > 0 {
                    m = m.min(d[k + n - 1] + b);
                }
            }
            d[k] = m;
        }
    }
    d
}

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn r(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_classes() {
        let b = Biome::Northeast;
        // Lawn: light, green, smooth.
        assert_eq!(classify(0.35, 0.48, 0.25, 0.02, b), Cover::Grass);
        // Canopy: dark green, textured.
        assert_eq!(classify(0.12, 0.2, 0.1, 0.08, b), Cover::Canopy);
        // Asphalt.
        assert_eq!(classify(0.35, 0.35, 0.36, 0.02, b), Cover::None);
        // Beach sand.
        assert_eq!(classify(0.85, 0.78, 0.65, 0.02, b), Cover::Sand);
        // Dry grass.
        assert_eq!(classify(0.55, 0.48, 0.32, 0.03, Biome::California), Cover::DryGrass);
        // Water.
        assert_eq!(classify(0.05, 0.12, 0.18, 0.01, b), Cover::Water);
    }

    #[test]
    fn distance_transform() {
        let n = 16;
        let mut v = vec![1u8; n * n];
        v[8 * n + 8] = 0;
        let d = distance_to(&v, n, |x| x == 0, 100.0);
        assert_eq!(d[8 * n + 8], 0.0);
        assert!((d[8 * n + 12] - 4.0).abs() < 0.01);
    }
}
