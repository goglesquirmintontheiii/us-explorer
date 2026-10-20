//! Tile loading on worker threads: fetch (disk cache first), decode, build the mesh
//! and the mip-mapped imagery texture. Nothing here touches the ECS.

use bevy::math::DVec3;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use usx_core::cache::{DiskCache, MISSING};
use usx_core::geo::Geodetic;
use usx_core::image;
use usx_core::net::{self, NetError};
use usx_core::sources::{self, ImagerySource, TERRARIUM_MAX_ZOOM};
use usx_core::tile::TileId;

/// Vertices per tile edge. 33 gives 32x32 quads: 4.7 m spacing on a zoom-18 tile.
pub const GRID: usize = 33;
pub const TEX: usize = 256;
/// Repeat period of the procedural ground detail, metres (must match terrain.wgsl).
pub const DETAIL_PERIOD: f64 = 4096.0;

/// Terrain heights for one tile on a GRID x GRID lattice (row 0 = north edge),
/// metres above sea level.
pub struct HeightGrid {
    pub h: Vec<f32>,
    pub min: f32,
    pub max: f32,
}

impl HeightGrid {
    /// Height at tile-local (u, v), interpolated on the same triangles the mesh uses
    /// so the ground you stand on is exactly the ground you see.
    pub fn sample(&self, u: f64, v: f64) -> f64 {
        let n = (GRID - 1) as f64;
        let (fx, fy) = ((u * n).clamp(0.0, n - 1e-9), (v * n).clamp(0.0, n - 1e-9));
        let (ix, iy) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - ix as f64, fy - iy as f64);
        let at = |x: usize, y: usize| self.h[y * GRID + x] as f64;
        let (a, b, c, d) = (
            at(ix, iy),
            at(ix + 1, iy),
            at(ix, iy + 1),
            at(ix + 1, iy + 1),
        );
        // Quads are split along the a-d diagonal.
        if tx >= ty {
            a + (b - a) * tx + (d - b) * ty
        } else {
            a + (d - c) * tx + (c - a) * ty
        }
    }
}

pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Web Mercator metres, wrapped to [`DETAIL_PERIOD`], for the detail shader.
    pub detail_uvs: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
}

pub struct ImageData {
    /// All mip levels, largest first: BC1 blocks when `bc1`, else RGBA8 (both sRGB).
    pub data: Vec<u8>,
    pub levels: u32,
    pub bc1: bool,
}

pub struct LoadJob {
    pub id: TileId,
    pub imagery: ImagerySource,
    pub lidar: bool,
    pub max_imagery_zoom: u8,
    /// Compress imagery to BC1 (8x less video memory) when the GPU supports it.
    pub bc1: bool,
}

pub struct Loaded {
    pub id: TileId,
    pub result: Result<TileData, String>,
}

pub struct TileData {
    pub center: DVec3,
    pub radius: f64,
    pub heights: Arc<HeightGrid>,
    pub mesh: MeshData,
    pub image: Option<ImageData>,
    /// Zoom the imagery really came from (lower when the source had no tile).
    pub imagery_zoom: u8,
    pub elevation_source: &'static str,
    /// Built without real data (offline); the tile asks again later.
    pub degraded: bool,
}

pub fn cache() -> &'static DiskCache {
    CACHE.get().expect("cache opened at startup")
}
pub static CACHE: OnceLock<DiskCache> = OnceLock::new();

/// Fetch through the disk cache. Missing resources are remembered too.
pub fn fetch(key: &str, url: &str) -> Result<Vec<u8>, NetError> {
    let c = cache();
    if let Some(b) = c.get(key) {
        return if b == MISSING {
            Err(NetError::NotFound)
        } else {
            Ok(b)
        };
    }
    match net::get(url) {
        Ok(b) => {
            c.put(key, &b);
            Ok(b)
        }
        Err(NetError::NotFound) => {
            c.put(key, MISSING);
            Err(NetError::NotFound)
        }
        Err(e) => Err(e),
    }
}

// ------------------------------------------------------------------ elevation

/// Small in-memory LRU of decoded terrarium tiles, because deep zoom tiles all
/// sample the same zoom-15 parent.
fn dem_lru() -> &'static Mutex<(HashMap<TileId, Arc<Vec<f32>>>, VecDeque<TileId>)> {
    static L: OnceLock<Mutex<(HashMap<TileId, Arc<Vec<f32>>>, VecDeque<TileId>)>> = OnceLock::new();
    L.get_or_init(|| Mutex::new((HashMap::new(), VecDeque::new())))
}

fn terrarium(id: TileId) -> Result<Arc<Vec<f32>>, String> {
    if let Some(d) = dem_lru().lock().unwrap().0.get(&id).cloned() {
        return Ok(d);
    }
    let bytes = fetch(
        &format!("dem/terrarium/{}.png", id.path()),
        &sources::terrarium_url(id),
    )
    .map_err(|e| format!("elevation {}: {e}", id.path()))?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| format!("elevation decode: {e}"))?
        .to_rgb8();
    if img.width() as usize != TEX || img.height() as usize != TEX {
        return Err("elevation tile has an unexpected size".into());
    }
    let h: Vec<f32> = img
        .pixels()
        .map(|p| p[0] as f32 * 256.0 + p[1] as f32 + p[2] as f32 / 256.0 - 32768.0)
        .collect();
    let h = Arc::new(h);
    let mut l = dem_lru().lock().unwrap();
    l.0.insert(id, h.clone());
    l.1.push_back(id);
    while l.1.len() > 96 {
        if let Some(old) = l.1.pop_front() {
            l.0.remove(&old);
        }
    }
    Ok(h)
}

fn bilinear(px: &[f32], w: usize, x: f64, y: f64) -> f32 {
    let x = (x - 0.5).clamp(0.0, (w - 1) as f64);
    let y = (y - 0.5).clamp(0.0, (w - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(w - 1));
    let (tx, ty) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let a = px[y0 * w + x0] * (1.0 - tx) + px[y0 * w + x1] * tx;
    let b = px[y1 * w + x0] * (1.0 - tx) + px[y1 * w + x1] * tx;
    a * (1.0 - ty) + b * ty
}

/// Heights for a tile: lidar from USGS 3DEP when enabled and deep enough, otherwise
/// sampled from the terrarium pyramid (walking up to a parent when a tile is missing).
fn heights(job: &LoadJob) -> Result<(HeightGrid, &'static str), String> {
    let id = job.id;
    if job.lidar && id.z > TERRARIUM_MAX_ZOOM && in_us(id) {
        if let Some(g) = lidar_heights(id) {
            return Ok((g, "USGS 3DEP lidar"));
        }
    }
    let mut ez = id.z.min(TERRARIUM_MAX_ZOOM);
    let dem = loop {
        match terrarium(id.ancestor(ez)) {
            Ok(d) => break d,
            Err(e) if ez == 0 => return Err(e),
            Err(e) if !e.contains("not found") => return Err(e),
            Err(_) => ez -= 1,
        }
    };
    let (u0, v0, s) = id.rect_in_ancestor(ez);
    let mut h = Vec::with_capacity(GRID * GRID);
    for j in 0..GRID {
        for i in 0..GRID {
            let u = u0 + s * i as f64 / (GRID - 1) as f64;
            let v = v0 + s * j as f64 / (GRID - 1) as f64;
            // Ocean depths are kept out of the visible surface until water exists (Phase 5).
            h.push(bilinear(&dem, TEX, u * TEX as f64, v * TEX as f64).max(0.0));
        }
    }
    Ok((grid_from(h), "terrain tiles (3DEP 10 m / SRTM)"))
}

fn grid_from(h: Vec<f32>) -> HeightGrid {
    let (mut min, mut max) = (f32::MAX, f32::MIN);
    for &v in &h {
        min = min.min(v);
        max = max.max(v);
    }
    HeightGrid { h, min, max }
}

/// Rough box around the US states and territories with 3DEP coverage.
fn in_us(id: TileId) -> bool {
    let (w, s, e, n) = id.bounds();
    let boxes = [
        (-125.0, 24.0, -66.0, 49.5),
        (-170.0, 51.0, -129.0, 71.5),
        (-161.0, 18.5, -154.5, 22.5),
        (-68.0, 17.5, -64.5, 18.6),
    ];
    boxes
        .iter()
        .any(|&(bw, bs, be, bn)| e > bw && w < be && n > bs && s < bn)
}

/// Exact-size lattice from the 3DEP dynamic service, as a 32-bit float TIFF.
fn lidar_heights(id: TileId) -> Option<HeightGrid> {
    let bytes = fetch(
        &format!("dem/3dep/{}.tif", id.path()),
        &sources::usgs_3dep_url(id, GRID as u32),
    )
    .ok()?;
    let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).ok()?;
    let (w, hgt) = dec.dimensions().ok()?;
    if w as usize != GRID || hgt as usize != GRID {
        return None;
    }
    let tiff::decoder::DecodingResult::F32(v) = dec.read_image().ok()? else {
        return None;
    };
    // Any hole (no lidar, edge of coverage) makes us fall back to the full-coverage DEM.
    if v.len() != GRID * GRID || v.iter().any(|x| !x.is_finite() || *x < -500.0) {
        return None;
    }
    Some(grid_from(v.into_iter().map(|x| x.max(0.0)).collect()))
}

// ------------------------------------------------------------------- imagery

fn decode_rgba(bytes: &[u8]) -> Option<image::RgbaImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    (img.width() as usize == TEX && img.height() as usize == TEX).then_some(img)
}

/// Esri answers missing deep tiles with a flat grey "no data" image instead of a 404.
fn is_placeholder(img: &image::RgbaImage, bytes: usize) -> bool {
    if bytes > 6000 {
        return false;
    }
    let (mut sum, mut sq, n) = (0f64, 0f64, (TEX * TEX) as f64);
    for p in img.pixels() {
        let l = (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        sum += l;
        sq += l * l;
    }
    let mean = sum / n;
    (sq / n - mean * mean).sqrt() < 6.0
}

/// Imagery for a tile; when the source has none at this zoom, crop and enlarge the
/// nearest ancestor that exists (up to 5 levels up).
fn imagery(job: &LoadJob) -> Option<(image::RgbaImage, u8)> {
    let id = job.id;
    let top = id.z.min(job.max_imagery_zoom);
    for z in (top.saturating_sub(5)..=top).rev() {
        let a = id.ancestor(z);
        let key = format!("img/{}/{}.jpg", job.imagery.key(), a.path());
        let Ok(bytes) = fetch(&key, &job.imagery.url(a)) else {
            continue;
        };
        let Some(img) = decode_rgba(&bytes) else {
            continue;
        };
        if job.imagery == ImagerySource::Esri && is_placeholder(&img, bytes.len()) {
            continue;
        }
        if z == id.z {
            return Some((img, z));
        }
        let (u0, v0, s) = id.rect_in_ancestor(z);
        let (x, y, w) = (
            (u0 * TEX as f64) as u32,
            (v0 * TEX as f64) as u32,
            ((s * TEX as f64) as u32).max(1),
        );
        let crop = image::imageops::crop_imm(&img, x, y, w, w).to_image();
        return Some((
            image::imageops::resize(
                &crop,
                TEX as u32,
                TEX as u32,
                image::imageops::FilterType::Triangle,
            ),
            z,
        ));
    }
    None
}

/// Box-filtered mip chain in one buffer, optionally BC1-compressed (levels down to 4x4).
fn mip_chain(img: image::RgbaImage, bc1: bool) -> ImageData {
    let mut levels = vec![img];
    while levels.last().unwrap().width() > if bc1 { 4 } else { 1 } {
        let cur = levels.last().unwrap();
        let (w, h) = (cur.width() / 2, cur.height() / 2);
        let mut next = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = cur.get_pixel(x * 2 + dx, y * 2 + dy);
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                }
                next.put_pixel(x, y, image::Rgba(acc.map(|v| ((v + 2) / 4) as u8)));
            }
        }
        levels.push(next);
    }
    let n = levels.len() as u32;
    if !bc1 {
        let data = levels
            .iter()
            .flat_map(|l| l.as_raw().iter().copied())
            .collect();
        return ImageData {
            data,
            levels: n,
            bc1,
        };
    }
    let fmt = texpresso::Format::Bc1;
    let params = texpresso::Params {
        algorithm: texpresso::Algorithm::RangeFit,
        ..Default::default()
    };
    let total: usize = levels
        .iter()
        .map(|l| fmt.compressed_size(l.width() as usize, l.height() as usize))
        .sum();
    let mut data = vec![0u8; total];
    let mut off = 0;
    for l in &levels {
        let sz = fmt.compressed_size(l.width() as usize, l.height() as usize);
        fmt.compress(
            l.as_raw(),
            l.width() as usize,
            l.height() as usize,
            params,
            &mut data[off..off + sz],
        );
        off += sz;
    }
    ImageData {
        data,
        levels: n,
        bc1,
    }
}

// ---------------------------------------------------------------------- mesh

fn build_mesh(id: TileId, g: &HeightGrid) -> (MeshData, DVec3, f64) {
    let (clat, clon) = id.lat_lon_at(0.5, 0.5);
    let center = Geodetic::new(clat, clon, (g.min as f64 + g.max as f64) * 0.5).to_ecef();
    let n = GRID - 1;
    let mut ecef = Vec::with_capacity(GRID * GRID);
    let mut ups = Vec::with_capacity(GRID * GRID);
    for j in 0..GRID {
        for i in 0..GRID {
            let (lat, lon) = id.lat_lon_at(i as f64 / n as f64, j as f64 / n as f64);
            let geo = Geodetic::new(lat, lon, g.h[j * GRID + i] as f64);
            ecef.push(geo.to_ecef());
            ups.push(geo.up());
        }
    }
    // Detail coordinates: Mercator metres, shifted by a whole number of periods so the
    // values stay small; the shader's noise repeats on that period, hiding the shift.
    let merc = |lat: f64, lon: f64| {
        let r = usx_core::geo::WGS84_A;
        let lat = lat.clamp(-85.0, 85.0).to_radians();
        (
            r * lon.to_radians(),
            -r * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln(),
        )
    };
    let (cx, cy) = merc(clat, clon);
    let (sx, sy) = (
        (cx / DETAIL_PERIOD).floor() * DETAIL_PERIOD,
        (cy / DETAIL_PERIOD).floor() * DETAIL_PERIOD,
    );
    let mut detail_uvs = Vec::with_capacity(GRID * GRID + 4 * GRID);
    let mut radius: f64 = 0.0;
    let mut positions = Vec::with_capacity(GRID * GRID + 4 * GRID);
    let mut normals = Vec::with_capacity(GRID * GRID + 4 * GRID);
    let mut uvs = Vec::with_capacity(GRID * GRID + 4 * GRID);
    for j in 0..GRID {
        for i in 0..GRID {
            let k = j * GRID + i;
            let p = ecef[k];
            radius = radius.max((p - center).length());
            let (l, r) = (
                ecef[j * GRID + i.saturating_sub(1)],
                ecef[j * GRID + (i + 1).min(n)],
            );
            let (t, b) = (
                ecef[j.saturating_sub(1) * GRID + i],
                ecef[(j + 1).min(n) * GRID + i],
            );
            // East x south points down; south x east points up.
            let mut nrm = (b - t).cross(r - l).normalize_or_zero();
            if nrm.dot(ups[k]) < 0.0 {
                nrm = -nrm;
            }
            if nrm == DVec3::ZERO {
                nrm = ups[k];
            }
            positions.push((p - center).as_vec3().to_array());
            normals.push(nrm.as_vec3().to_array());
            uvs.push([i as f32 / n as f32, j as f32 / n as f32]);
            detail_uvs.push(if id.z >= 12 {
                let (lat, lon) = id.lat_lon_at(i as f64 / n as f64, j as f64 / n as f64);
                let (x, y) = merc(lat, lon);
                [(x - sx) as f32, (y - sy) as f32]
            } else {
                [0.0, 0.0]
            });
        }
    }
    let mut indices: Vec<u16> = Vec::with_capacity(n * n * 6 + 4 * n * 6);
    for j in 0..n {
        for i in 0..n {
            let a = (j * GRID + i) as u16;
            let (b, c, d) = (a + 1, a + GRID as u16, a + GRID as u16 + 1);
            // Counter-clockwise seen from above, split along a-d to match HeightGrid::sample.
            indices.extend_from_slice(&[a, c, d, a, d, b]);
        }
    }
    // Skirts hang below every edge to hide cracks between tiles of different zooms.
    let depth = (id.width_m() * 0.03).clamp(4.0, 2_000.0);
    let edges: [Vec<usize>; 4] = [
        (0..GRID).map(|i| i).collect(),                  // north, west->east
        (0..GRID).map(|j| j * GRID + n).collect(),       // east, north->south
        (0..GRID).rev().map(|i| n * GRID + i).collect(), // south, east->west
        (0..GRID).rev().map(|j| j * GRID).collect(),     // west, south->north
    ];
    for edge in edges {
        let base = positions.len() as u16;
        for &k in &edge {
            let p = ecef[k] - ups[k] * depth;
            positions.push((p - center).as_vec3().to_array());
            normals.push(normals[k]);
            uvs.push(uvs[k]);
            detail_uvs.push(detail_uvs[k]);
        }
        for w in 0..n {
            let (top0, top1) = (edge[w] as u16, edge[w + 1] as u16);
            let (bot0, bot1) = (base + w as u16, base + w as u16 + 1);
            indices.extend_from_slice(&[top0, bot0, bot1, top0, bot1, top1]);
        }
    }
    (
        MeshData {
            positions,
            normals,
            uvs,
            detail_uvs,
            indices,
        },
        center,
        radius + depth,
    )
}

pub fn load(job: LoadJob) -> Loaded {
    let id = job.id;
    let result = (|| {
        // The coarse globe must always exist, even offline: fall back to sea level.
        let (g, elevation_source, mut degraded) = match heights(&job) {
            Ok((g, s)) => (g, s, false),
            Err(_) if id.z <= 4 => (grid_from(vec![0.0; GRID * GRID]), "none (offline)", true),
            Err(e) => return Err(e),
        };
        let (mesh, center, radius) = build_mesh(id, &g);
        let img = imagery(&job);
        degraded |= img.is_none();
        let imagery_zoom = img.as_ref().map(|i| i.1).unwrap_or(0);
        Ok(TileData {
            center,
            radius,
            heights: Arc::new(g),
            mesh,
            image: img.map(|(i, _)| mip_chain(i, job.bc1)),
            imagery_zoom,
            elevation_source,
            degraded,
        })
    })();
    Loaded { id, result }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_matches_triangles() {
        let mut h = vec![0.0; GRID * GRID];
        h[GRID + 1] = 10.0; // vertex (1,1)
        let g = grid_from(h);
        let n = (GRID - 1) as f64;
        assert!((g.sample(1.0 / n, 1.0 / n) - 10.0).abs() < 1e-6);
        assert!((g.sample(0.5 / n, 0.5 / n) - 5.0).abs() < 1e-6); // on the a-d diagonal
        assert!(g.sample(0.9 / n, 0.1 / n).abs() < 2.0); // lower-right triangle of quad (0,0)
    }

    #[test]
    fn bc1_chain_sizes() {
        let img =
            image::RgbaImage::from_fn(256, 256, |x, y| image::Rgba([x as u8, y as u8, 90, 255]));
        let t = std::time::Instant::now();
        let d = mip_chain(img.clone(), true);
        eprintln!("bc1 mip chain: {:?}", t.elapsed());
        assert_eq!(d.levels, 7);
        assert_eq!(
            d.data.len(),
            (256 * 256 + 128 * 128 + 64 * 64 + 32 * 32 + 16 * 16 + 64 + 16) / 2
        );
        assert_eq!(mip_chain(img, false).levels, 9);
    }

    #[test]
    fn mesh_normals_point_up() {
        let id = TileId::containing(39.74, -104.99, 14).0;
        let g = grid_from(vec![1600.0; GRID * GRID]);
        let (m, center, _) = build_mesh(id, &g);
        let up = Geodetic::from_ecef(center).up();
        let nrm = m.normals[GRID * 16 + 16];
        let d = DVec3::new(nrm[0] as f64, nrm[1] as f64, nrm[2] as f64).dot(up);
        assert!(d > 0.999, "{d}");
        assert_eq!(m.indices.len(), 32 * 32 * 6 + 4 * 32 * 6);
    }
}
