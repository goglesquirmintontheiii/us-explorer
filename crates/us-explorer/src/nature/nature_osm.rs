//! The nature layer of OpenStreetMap for one zoom-15 tile, sorted into what the game
//! uses: land cover areas, mapped trees and tree rows, coastline, rivers and streams,
//! fences and walls, power lines, wind turbines, buoys and beacons, piers, lifeguard
//! towers and street furniture.

use super::species::{self, Sp};
use usx_core::osm::{OsmTile, Tags};
use usx_core::tile::TileId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverClass {
    Forest,
    Grass,
    Park,
    Farm,
    Scrub,
    Sand,
    Wetland,
    Rock,
    Golf,
    Dune,
}

pub struct CoverArea {
    pub class: CoverClass,
    pub outer: Vec<Vec<(f64, f64)>>,
    pub inner: Vec<Vec<(f64, f64)>>,
    /// (s, w, n, e)
    pub bbox: (f64, f64, f64, f64),
    /// Beach (for sand areas): wet edge, foam and lifeguard towers nearby.
    pub beach: bool,
}

pub struct MappedTree {
    pub lat: f64,
    pub lon: f64,
    pub species: Option<Sp>,
    pub height: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarrierKind {
    ChainLink,
    WoodPicket,
    WoodPrivacy,
    Metal,
    Wall,
    StoneWall,
    Hedge,
    Retaining,
    Jersey,
}

pub struct Barrier {
    pub kind: BarrierKind,
    pub pts: Vec<(f64, f64)>,
    pub height: f32,
    pub id: i64,
}

pub struct PowerLine {
    pub pts: Vec<(f64, f64)>,
    pub nodes: Vec<i64>,
    pub kv: f32,
    pub cables: u32,
    pub id: i64,
}

pub struct Turbine {
    pub lat: f64,
    pub lon: f64,
    pub hub: f32,
    pub rotor: f32,
    pub id: i64,
}

/// A buoy or beacon (seamark).
#[derive(Clone, Debug)]
pub struct Seamark {
    pub lat: f64,
    pub lon: f64,
    pub id: i64,
    /// buoy_lateral, buoy_cardinal, buoy_special_purpose, buoy_safe_water,
    /// buoy_isolated_danger, mooring, beacon_lateral, beacon_cardinal, light_float...
    pub kind: String,
    /// Body colours top to bottom.
    pub colours: Vec<[f32; 3]>,
    /// can, conical, pillar, spar, sphere, barrel, stake, pile, tower...
    pub shape: String,
    /// Light: colour and flash period (s), if lit.
    pub light: Option<([f32; 3], f32, String)>,
    pub topmark: Option<String>,
    pub name: Option<String>,
    pub beacon: bool,
}

pub struct Pier {
    pub pts: Vec<(f64, f64)>,
    pub area: bool,
    pub width: f32,
    pub kind: String,
    pub id: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FurnitureKind {
    WasteBasket,
    BikeRack,
    DrinkingWater,
    Phone,
    Vending,
    Shelter,
    Fountain,
    Grill,
    Recycling,
    PicnicTable,
    Firepit,
    Flagpole,
    WaterTower,
    Mast,
    Lighthouse,
    Windmill,
    Lifeguard,
}

pub struct Furniture {
    pub kind: FurnitureKind,
    pub lat: f64,
    pub lon: f64,
    pub id: i64,
    pub height: Option<f32>,
}

#[derive(Default)]
pub struct NatureData {
    pub tile: Option<TileId>,
    pub cover: Vec<CoverArea>,
    pub trees: Vec<MappedTree>,
    pub tree_rows: Vec<Vec<(f64, f64)>>,
    pub coast: Vec<Vec<(f64, f64)>>,
    /// Rivers, streams, canals and ditches: (kind, line, width m).
    pub waterways: Vec<(String, Vec<(f64, f64)>, f32, i64)>,
    pub barriers: Vec<Barrier>,
    pub power: Vec<PowerLine>,
    pub towers: Vec<(i64, f64, f64, String)>,
    pub turbines: Vec<Turbine>,
    pub seamarks: Vec<Seamark>,
    pub piers: Vec<Pier>,
    pub furniture: Vec<Furniture>,
}

impl NatureData {
    /// Whether a mapped coastline passes within `m` metres.
    pub fn coast_near(&self, lat: f64, lon: f64, m: f64) -> bool {
        let dlat = m / 111_000.0;
        let dlon = m / (111_000.0 * lat.to_radians().cos().max(0.1));
        self.coast.iter().flatten().any(|&(a, b)| (a - lat).abs() < dlat && (b - lon).abs() < dlon)
    }
}

fn bbox(rings: &[Vec<(f64, f64)>]) -> (f64, f64, f64, f64) {
    let mut bb = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
    for &(la, lo) in rings.iter().flatten() {
        bb = (bb.0.min(la), bb.1.min(lo), bb.2.max(la), bb.3.max(lo));
    }
    bb
}

fn cover_class(t: &Tags) -> Option<(CoverClass, bool)> {
    if let Some(n) = t.get("natural") {
        return Some(match n {
            "wood" => (CoverClass::Forest, false),
            "scrub" | "heath" => (CoverClass::Scrub, false),
            "grassland" => (CoverClass::Grass, false),
            "dune" => (CoverClass::Dune, false),
            "sand" | "shingle" => (CoverClass::Sand, false),
            "beach" => (CoverClass::Sand, true),
            "wetland" => (CoverClass::Wetland, false),
            "bare_rock" | "scree" => (CoverClass::Rock, false),
            _ => return None,
        });
    }
    if let Some(l) = t.get("landuse") {
        return Some(match l {
            "forest" => (CoverClass::Forest, false),
            "grass" | "meadow" | "village_green" | "recreation_ground" | "cemetery" | "flowerbed" => (CoverClass::Grass, false),
            "farmland" | "orchard" | "vineyard" | "allotments" | "plant_nursery" => (CoverClass::Farm, false),
            _ => return None,
        });
    }
    if let Some(g) = t.get("golf") {
        return Some(match g {
            "bunker" => (CoverClass::Sand, false),
            _ => (CoverClass::Golf, false),
        });
    }
    if let Some(l) = t.get("leisure") {
        return Some(match l {
            "golf_course" => (CoverClass::Golf, false),
            "park" | "garden" | "pitch" | "playground" | "dog_park" => (CoverClass::Park, false),
            "nature_reserve" => return None,
            _ => return None,
        });
    }
    None
}

fn barrier_kind(t: &Tags) -> Option<(BarrierKind, f32)> {
    let b = t.get("barrier")?;
    let h = t.num("height").map(|h| h as f32);
    let mat = t.get("material").or(t.get("fence_type")).unwrap_or("");
    Some(match b {
        "fence" => {
            let k = match mat {
                "chain_link" | "chain" | "wire" | "metal_bars" | "mesh" => BarrierKind::ChainLink,
                "wood" | "wooden" | "panel" | "board" | "privacy" => BarrierKind::WoodPrivacy,
                "pickets" | "picket" | "split_rail" | "rail" => BarrierKind::WoodPicket,
                "metal" | "railing" | "iron" | "wrought_iron" | "steel" => BarrierKind::Metal,
                _ => BarrierKind::ChainLink,
            };
            let dh = match k {
                BarrierKind::WoodPrivacy => 1.8,
                BarrierKind::WoodPicket => 1.0,
                BarrierKind::Metal => 1.5,
                _ => 1.8,
            };
            (k, h.unwrap_or(dh))
        }
        "wall" | "city_wall" => {
            let stone = matches!(mat, "stone" | "rock" | "sandstone" | "limestone" | "granite") || t.is("wall", "dry_stone");
            (if stone { BarrierKind::StoneWall } else { BarrierKind::Wall }, h.unwrap_or(1.6))
        }
        "hedge" => (BarrierKind::Hedge, h.unwrap_or(1.5)),
        "retaining_wall" => (BarrierKind::Retaining, h.unwrap_or(1.2)),
        "jersey_barrier" => (BarrierKind::Jersey, 0.8),
        _ => return None,
    })
}

/// Seamark colours: "red", "green;white", "yellow;black;yellow"...
fn colours(v: Option<&str>) -> Vec<[f32; 3]> {
    v.unwrap_or("")
        .split(';')
        .filter_map(|c| {
            Some(match c.trim() {
                "red" => [0.75, 0.08, 0.06],
                "green" => [0.05, 0.42, 0.18],
                "yellow" => [0.95, 0.75, 0.05],
                "black" => [0.06, 0.06, 0.06],
                "white" => [0.92, 0.92, 0.9],
                "orange" => [0.95, 0.4, 0.05],
                "blue" => [0.1, 0.25, 0.7],
                "grey" | "gray" => [0.5, 0.5, 0.5],
                _ => return None,
            })
        })
        .collect()
}

/// Sort a nature-layer tile.
pub fn sort(tile: TileId, osm: &OsmTile) -> NatureData {
    let mut d = NatureData { tile: Some(tile), ..Default::default() };
    for a in &osm.areas {
        if let Some((class, beach)) = cover_class(&a.tags) {
            d.cover.push(CoverArea { class, outer: a.outer.clone(), inner: a.inner.clone(), bbox: bbox(&a.outer), beach });
        }
        if a.tags.get("man_made").is_some_and(|m| m == "pier") {
            if let Some(r) = a.outer.first() {
                d.piers.push(Pier { pts: r.clone(), area: true, width: 0.0, kind: "pier".into(), id: a.id });
            }
        }
    }
    for w in &osm.ways {
        let t = &w.tags;
        if t.is("natural", "tree_row") {
            d.tree_rows.push(w.pts.clone());
        } else if t.is("natural", "coastline") {
            d.coast.push(w.pts.clone());
        } else if let Some(k) = t.get("waterway") {
            let width = t.num("width").map(|x| x as f32).unwrap_or(match k {
                "river" => 25.0,
                "canal" => 12.0,
                "stream" => 3.0,
                _ => 1.5,
            });
            d.waterways.push((k.to_string(), w.pts.clone(), width, w.id));
        } else if let Some((kind, height)) = barrier_kind(t) {
            d.barriers.push(Barrier { kind, pts: w.pts.clone(), height, id: w.id });
        } else if t.is("power", "line") {
            let kv = t
                .get("voltage")
                .and_then(|v| v.split(';').filter_map(|x| x.trim().parse::<f32>().ok()).fold(None, |m: Option<f32>, x| Some(m.map_or(x, |m| m.max(x)))))
                .map(|v| v / 1000.0)
                .unwrap_or(115.0);
            let cables = t.get("cables").and_then(|c| c.split(';').filter_map(|x| x.trim().parse::<u32>().ok()).max()).unwrap_or(3);
            d.power.push(PowerLine { pts: w.pts.clone(), nodes: w.nodes.clone(), kv, cables, id: w.id });
        } else if let Some(m) = t.get("man_made") {
            if matches!(m, "pier" | "breakwater" | "groyne") {
                let width = t.num("width").map(|x| x as f32).unwrap_or(if m == "pier" { 4.0 } else { 6.0 });
                d.piers.push(Pier { pts: w.pts.clone(), area: false, width, kind: m.to_string(), id: w.id });
            }
        }
    }
    for n in &osm.nodes {
        let t = &n.tags;
        if t.is("natural", "tree") {
            let h = t.num("height").map(|x| x as f32).filter(|h| (1.0..120.0).contains(h));
            d.trees.push(MappedTree { lat: n.lat, lon: n.lon, species: species::from_tags(t), height: h });
        } else if t.is("power", "tower") {
            d.towers.push((n.id, n.lat, n.lon, t.get("design").or(t.get("structure")).unwrap_or("").to_string()));
        } else if t.is("power", "generator") && t.is("generator:source", "wind") {
            let hub = t.num("height").map(|x| x as f32).unwrap_or(80.0).clamp(20.0, 180.0);
            let rotor = t.num("rotor:diameter").map(|x| x as f32).unwrap_or(hub * 1.2).clamp(10.0, 200.0);
            d.turbines.push(Turbine { lat: n.lat, lon: n.lon, hub, rotor, id: n.id });
        } else if let Some(kind) = t.get("seamark:type") {
            if let Some(s) = seamark(n.id, n.lat, n.lon, kind, t) {
                d.seamarks.push(s);
            }
        } else if let Some(k) = furniture_kind(t) {
            d.furniture.push(Furniture { kind: k, lat: n.lat, lon: n.lon, id: n.id, height: t.num("height").map(|x| x as f32) });
        }
    }
    d
}

fn furniture_kind(t: &Tags) -> Option<FurnitureKind> {
    use FurnitureKind as F;
    if t.is("emergency", "lifeguard") {
        return Some(F::Lifeguard);
    }
    if let Some(a) = t.get("amenity") {
        return Some(match a {
            "waste_basket" => F::WasteBasket,
            "bicycle_parking" => F::BikeRack,
            "drinking_water" => F::DrinkingWater,
            "telephone" => F::Phone,
            "vending_machine" => F::Vending,
            "shelter" => F::Shelter,
            "fountain" => F::Fountain,
            "bbq" => F::Grill,
            "recycling" => F::Recycling,
            _ => return None,
        });
    }
    if let Some(l) = t.get("leisure") {
        return Some(match l {
            "picnic_table" => F::PicnicTable,
            "firepit" => F::Firepit,
            _ => return None,
        });
    }
    if let Some(m) = t.get("man_made") {
        return Some(match m {
            "flagpole" => F::Flagpole,
            "water_tower" => F::WaterTower,
            "mast" => F::Mast,
            "lighthouse" => F::Lighthouse,
            "windmill" => F::Windmill,
            _ => return None,
        });
    }
    None
}

fn seamark(id: i64, lat: f64, lon: f64, kind: &str, t: &Tags) -> Option<Seamark> {
    let floating = kind.starts_with("buoy_") || kind == "mooring" || kind == "light_float" || kind == "light_vessel";
    let beacon = kind.starts_with("beacon_");
    if !floating && !beacon {
        return None;
    }
    let pre = format!("seamark:{kind}");
    let get = |k: &str| t.get(&format!("{pre}:{k}"));
    let mut cols = colours(get("colour"));
    if cols.is_empty() {
        // IALA-B (the Americas): red right returning, green left; cardinal yellow and black.
        cols = match kind {
            "buoy_lateral" | "beacon_lateral" => match get("category") {
                Some("port") => vec![[0.05, 0.42, 0.18]],
                Some("starboard") => vec![[0.75, 0.08, 0.06]],
                Some("preferred_channel_port") => vec![[0.05, 0.42, 0.18], [0.75, 0.08, 0.06], [0.05, 0.42, 0.18]],
                Some("preferred_channel_starboard") => vec![[0.75, 0.08, 0.06], [0.05, 0.42, 0.18], [0.75, 0.08, 0.06]],
                _ => vec![[0.75, 0.08, 0.06]],
            },
            "buoy_cardinal" | "beacon_cardinal" => vec![[0.06, 0.06, 0.06], [0.95, 0.75, 0.05]],
            "buoy_safe_water" => vec![[0.75, 0.08, 0.06], [0.92, 0.92, 0.9], [0.75, 0.08, 0.06], [0.92, 0.92, 0.9]],
            "buoy_isolated_danger" => vec![[0.06, 0.06, 0.06], [0.75, 0.08, 0.06], [0.06, 0.06, 0.06]],
            "buoy_special_purpose" => vec![[0.95, 0.75, 0.05]],
            "mooring" => vec![[0.92, 0.92, 0.9], [0.1, 0.25, 0.7]],
            _ => vec![[0.75, 0.08, 0.06]],
        };
    }
    let shape = get("shape")
        .map(str::to_string)
        .unwrap_or_else(|| match (kind, get("category")) {
            ("buoy_lateral", Some("port")) => "can".into(),
            ("buoy_lateral", Some("starboard")) => "conical".into(),
            ("mooring", _) => "sphere".into(),
            ("buoy_safe_water", _) => "sphere".into(),
            (k, _) if k.starts_with("beacon") => "stake".into(),
            _ => "pillar".into(),
        });
    let light = t.get("seamark:light:colour").map(|c| {
        let col = colours(Some(c)).first().copied().unwrap_or([1.0, 1.0, 0.9]);
        let period = t.num("seamark:light:period").map(|p| p as f32).unwrap_or(4.0).clamp(0.5, 30.0);
        (col, period, t.get("seamark:light:character").unwrap_or("Fl").to_string())
    });
    Some(Seamark {
        lat,
        lon,
        id,
        kind: kind.to_string(),
        colours: cols,
        shape,
        light,
        topmark: t.get("seamark:topmark:shape").map(str::to_string),
        name: t.get("seamark:name").or(t.get("name")).map(str::to_string),
        beacon,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use usx_core::osm::{Node, Tags};

    #[test]
    fn sorts_buoys_and_trees() {
        let tags = |v: &[(&str, &str)]| Tags(v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect());
        let osm = OsmTile {
            nodes: vec![
                Node { id: 1, lat: 25.77, lon: -80.13, tags: tags(&[("seamark:type", "buoy_lateral"), ("seamark:buoy_lateral:category", "starboard")]) },
                Node { id: 2, lat: 25.78, lon: -80.13, tags: tags(&[("natural", "tree"), ("genus", "Sabal")]) },
                Node {
                    id: 3,
                    lat: 25.79,
                    lon: -80.13,
                    tags: tags(&[("seamark:type", "buoy_cardinal"), ("seamark:light:colour", "white"), ("seamark:light:period", "15")]),
                },
            ],
            ..Default::default()
        };
        let d = sort(TileId::new(15, 0, 0), &osm);
        assert_eq!(d.seamarks.len(), 2);
        assert_eq!(d.seamarks[0].shape, "conical");
        assert_eq!(d.seamarks[0].colours[0], [0.75, 0.08, 0.06]);
        assert_eq!(d.seamarks[1].light.as_ref().unwrap().1, 15.0);
        assert_eq!(d.trees[0].species, Some(Sp::SabalPalm));
    }
}
