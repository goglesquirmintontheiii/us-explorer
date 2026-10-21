//! One tile of buildings, built on a worker thread: footprints and parts from the map,
//! which sides face the street, roof colours sampled from the aerial imagery, and the
//! shells split into chunks for drawing.

use super::exterior::{self, BGeom, Options};
use super::geom;
use super::model::{self, Building, Kind};
use crate::roads::build::Local;
use bevy::math::Vec2;
use std::collections::HashMap;
use usx_core::osm::OsmTile;
use usx_core::sources::ImagerySource;
use usx_core::tile::TileId;

/// Buildings are drawn in zoom-16 chunks (about 600 m).
pub const CHUNK_ZOOM: u8 = 16;

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub b: u32,
    pub v0: u32,
    pub v1: u32,
    pub i0: u32,
    pub i1: u32,
}

pub struct BChunk {
    pub id: TileId,
    pub geom: BGeom,
    pub ranges: Vec<Range>,
    /// (s, w, n, e) of the ground the chunk depends on.
    pub influence: (f64, f64, f64, f64),
}

pub struct Built {
    pub tile: TileId,
    pub local: Local,
    pub buildings: Vec<Building>,
    /// Street-facing outline edges per building.
    pub street: Vec<Vec<usize>>,
    /// Roof colour per building (tagged, from imagery, or a palette pick).
    pub roof_col: Vec<[f32; 3]>,
    /// Floor height above the highest ground under the footprint.
    pub floor_lift: Vec<f32>,
    pub chunks: Vec<BChunk>,
    pub vertices: usize,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Average roof colour inside a footprint from zoom-18 imagery (cached tiles only,
/// plus downloads through the shared cache), robust to shadows and roof clutter.
struct Imagery {
    src: ImagerySource,
    tiles: HashMap<TileId, Option<usx_core::image::RgbaImage>>,
}

impl Imagery {
    fn tile(&mut self, id: TileId) -> Option<&usx_core::image::RgbaImage> {
        let src = self.src;
        self.tiles
            .entry(id)
            .or_insert_with(|| {
                let key = format!("img/{}/{}.jpg", src.key(), id.path());
                let bytes = crate::terrain::load::fetch(&key, &src.url(id)).ok()?;
                usx_core::image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8())
            })
            .as_ref()
    }

    fn roof_colour(&mut self, local: &Local, ring: &[Vec2]) -> Option<[f32; 3]> {
        let inner = geom::inset(ring, 1.0);
        let ring = if geom::area(&inner) > 6.0 && geom::signed_area(&inner) > 0.0 { inner } else { ring.to_vec() };
        let (c, ax, half) = geom::obb(&ring);
        let mut samples: Vec<[f32; 3]> = Vec::new();
        let steps = 7;
        for i in 0..steps {
            for j in 0..steps {
                let p = c
                    + ax * half.x * (2.0 * (i as f32 + 0.5) / steps as f32 - 1.0)
                    + ax.perp() * half.y * (2.0 * (j as f32 + 0.5) / steps as f32 - 1.0);
                if !geom::contains(&ring, p) {
                    continue;
                }
                let (la, lo) = local.ll(p);
                let (t, u, v) = TileId::containing(la, lo, ROOF_ZOOM);
                let Some(img) = self.tile(t) else { continue };
                let (x, y) = ((u * img.width() as f64) as u32, (v * img.height() as f64) as u32);
                let px = img.get_pixel(x.min(img.width() - 1), y.min(img.height() - 1));
                samples.push([px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0]);
            }
        }
        if samples.len() < 3 {
            return None;
        }
        // Drop the darkest third (shadows, vents) and the brightest few (glare).
        samples.sort_by(|a, b| (a[0] + a[1] + a[2]).total_cmp(&(b[0] + b[1] + b[2])));
        let k0 = samples.len() / 3;
        let k1 = (samples.len() * 9 / 10).max(k0 + 1);
        let mut acc = [0.0f32; 3];
        for s in &samples[k0..k1] {
            for c in 0..3 {
                acc[c] += s[c];
            }
        }
        let n = (k1 - k0) as f32;
        // Imagery is darker and bluer than the roof in sunlight: lift and warm a little.
        let lin = [srgb_to_linear(acc[0] / n), srgb_to_linear(acc[1] / n), srgb_to_linear(acc[2] / n)];
        Some([(lin[0] * 1.35).min(0.9), (lin[1] * 1.3).min(0.9), (lin[2] * 1.18).min(0.9)])
    }
}

/// Palette roof colours when there's no imagery: asphalt shingle greys and browns,
/// terracotta, galvanised metal, white membrane.
fn palette_roof(b: &Building) -> [f32; 3] {
    let p = &b.parts[0];
    let pick = |opts: &[[f32; 3]]| opts[(model::hash64(b.seed ^ 0x700f) % opts.len() as u64) as usize];
    match p.roof_mat {
        model::RoofMat::Shingles => {
            pick(&[[0.18, 0.18, 0.19], [0.28, 0.26, 0.24], [0.3, 0.22, 0.17], [0.22, 0.24, 0.26], [0.36, 0.34, 0.31]])
        }
        model::RoofMat::ClayTile => pick(&[[0.55, 0.25, 0.15], [0.62, 0.36, 0.22], [0.48, 0.22, 0.14]]),
        model::RoofMat::Metal => pick(&[[0.55, 0.57, 0.58], [0.3, 0.12, 0.1], [0.18, 0.28, 0.22], [0.2, 0.22, 0.28]]),
        model::RoofMat::Slate => [0.25, 0.27, 0.3],
        model::RoofMat::Gravel => [0.5, 0.49, 0.46],
        _ => pick(&[[0.75, 0.75, 0.73], [0.55, 0.55, 0.54], [0.4, 0.4, 0.4]]),
    }
}

/// Imagery zoom roof colours are sampled at (about 1.2 m per pixel).
const ROOF_ZOOM: u8 = 17;

pub fn build(tile: TileId, osm: &OsmTile, roads: Option<&OsmTile>, imagery: Option<ImagerySource>) -> Built {
    let (cla, clo) = tile.lat_lon_at(0.5, 0.5);
    let local = Local::new(cla, clo);
    let (tw, ts, te, tn) = tile.bounds();
    let in_tile = |p: Vec2| {
        let (la, lo) = local.ll(p);
        la >= ts && la < tn && lo >= tw && lo < te
    };
    let buildings = model::buildings(osm, &local, &in_tile);
    // Street segments (vehicle roads) in a grid, for street-facing sides.
    let mut grid: HashMap<(i32, i32), Vec<(Vec2, Vec2)>> = HashMap::new();
    const G: f32 = 40.0;
    if let Some(r) = roads {
        for w in &r.ways {
            let Some(h) = w.tags.get("highway") else { continue };
            if matches!(h, "footway" | "path" | "cycleway" | "steps" | "bridleway" | "track" | "construction" | "proposed") {
                continue;
            }
            let pts: Vec<Vec2> = w.pts.iter().map(|&(a, b)| local.xz(a, b)).collect();
            for s in pts.windows(2) {
                let (lo, hi) = (s[0].min(s[1]), s[0].max(s[1]));
                for gx in (lo.x / G).floor() as i32..=(hi.x / G).floor() as i32 {
                    for gz in (lo.y / G).floor() as i32..=(hi.y / G).floor() as i32 {
                        grid.entry((gx, gz)).or_default().push((s[0], s[1]));
                    }
                }
            }
        }
    }
    let road_dist = |p: Vec2| -> f32 {
        let (cx, cz) = ((p.x / G).floor() as i32, (p.y / G).floor() as i32);
        let mut d = f32::MAX;
        for gx in cx - 1..=cx + 1 {
            for gz in cz - 1..=cz + 1 {
                for &(a, b) in grid.get(&(gx, gz)).into_iter().flatten() {
                    d = d.min(geom::seg_dist(a, b, p));
                }
            }
        }
        d
    };
    let mut street = Vec::with_capacity(buildings.len());
    for b in &buildings {
        let r = &b.outline;
        let n = r.len();
        // Distance to the nearest road straight out from each edge.
        let mut fronts: Vec<(usize, f32)> = Vec::new();
        for e in 0..n {
            let (a, c) = (r[e], r[(e + 1) % n]);
            let l = a.distance(c);
            if l < 2.0 {
                continue;
            }
            let out = -((c - a) / l).perp();
            let mid = (a + c) * 0.5;
            let mut best = f32::MAX;
            for k in 1..=10 {
                let p = mid + out * (k as f32 * 4.0);
                if road_dist(p) < 3.0 {
                    best = k as f32 * 4.0;
                    break;
                }
            }
            fronts.push((e, best));
        }
        let min = fronts.iter().map(|f| f.1).fold(f32::MAX, f32::min);
        street.push(if min < f32::MAX { fronts.iter().filter(|f| f.1 <= min + 6.0).map(|f| f.0).collect() } else { Vec::new() });
    }
    // Roof colours: tag, else imagery, else palette.
    let mut img = imagery.map(|src| Imagery { src, tiles: HashMap::new() });
    let roof_col: Vec<[f32; 3]> = buildings
        .iter()
        .map(|b| {
            b.parts[0]
                .roof_col
                .or_else(|| if b.area > 12.0 { img.as_mut().and_then(|i| i.roof_colour(&local, &b.outline)) } else { None })
                .unwrap_or_else(|| palette_roof(b))
        })
        .collect();
    let floor_lift: Vec<f32> = buildings
        .iter()
        .map(|b| match b.kind {
            Kind::House | Kind::Terrace => 0.45 + 0.3 * model::hash01(b.seed ^ 0xf1),
            Kind::Apartments | Kind::Church | Kind::Civic | Kind::School => 0.35,
            Kind::Garage | Kind::Carport | Kind::Canopy | Kind::ParkingGarage => 0.05,
            _ => 0.15,
        })
        .collect();
    // Shells into chunks.
    let mut chunks: HashMap<TileId, BChunk> = HashMap::new();
    for (bi, b) in buildings.iter().enumerate() {
        let c = geom::centroid(&b.outline);
        let (la, lo) = local.ll(c);
        let cid = TileId::containing(la, lo, CHUNK_ZOOM).0;
        let g = exterior::shell(b, &Options { detailed: false, roof_col: roof_col[bi], street_edges: street[bi].clone() });
        let ch = chunks.entry(cid).or_insert_with(|| {
            let (w, s, e, n) = cid.bounds();
            BChunk { id: cid, geom: BGeom::default(), ranges: Vec::new(), influence: (s, w, n, e) }
        });
        let v0 = ch.geom.v.len() as u32;
        let i0 = ch.geom.i.len() as u32;
        ch.geom.v.extend_from_slice(&g.v);
        ch.geom.i.extend(g.i.iter().map(|i| i + v0));
        ch.ranges.push(Range { b: bi as u32, v0, v1: ch.geom.v.len() as u32, i0, i1: ch.geom.i.len() as u32 });
        // Grow the influence box by the footprint (buildings can straddle chunks).
        for &p in &b.outline {
            let (pla, plo) = local.ll(p);
            ch.influence = (ch.influence.0.min(pla), ch.influence.1.min(plo), ch.influence.2.max(pla), ch.influence.3.max(plo));
        }
    }
    let chunks: Vec<BChunk> = chunks.into_values().collect();
    let vertices = chunks.iter().map(|c| c.geom.v.len()).sum();
    Built { tile, local, buildings, street, roof_col, floor_lift, chunks, vertices }
}
