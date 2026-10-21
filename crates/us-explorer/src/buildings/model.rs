//! From OpenStreetMap tags to buildings: what each building is (house, office, store,
//! restaurant...), how tall, its parts, roof shape, materials and colours, and where
//! its doors are.

use super::geom;
use crate::roads::build::Local;
use bevy::math::Vec2;
use usx_core::osm::{OsmTile, Tags};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    House,
    Terrace,
    Apartments,
    Office,
    Retail,
    Supermarket,
    FastFood,
    Cafe,
    Restaurant,
    Bar,
    Industrial,
    Warehouse,
    Garage,
    Carport,
    Shed,
    School,
    Church,
    Hospital,
    Hotel,
    Civic,
    ParkingGarage,
    /// Roof on posts with open sides (petrol station canopies, shelters).
    Canopy,
    Other,
}

impl Kind {
    pub fn residential(self) -> bool {
        matches!(self, Kind::House | Kind::Terrace | Kind::Apartments)
    }
    pub fn food(self) -> bool {
        matches!(self, Kind::FastFood | Kind::Cafe | Kind::Restaurant | Kind::Bar)
    }
    /// Shops and restaurants get big ground-floor windows.
    pub fn storefront(self) -> bool {
        matches!(self, Kind::Retail | Kind::Supermarket | Kind::FastFood | Kind::Cafe | Kind::Restaurant | Kind::Bar)
    }
    pub fn floor_height(self) -> f32 {
        match self {
            Kind::House | Kind::Terrace => 2.9,
            Kind::Apartments | Kind::Hotel => 3.0,
            Kind::Office | Kind::Hospital | Kind::School | Kind::Civic => 3.8,
            Kind::Retail | Kind::Supermarket => 4.5,
            Kind::Industrial | Kind::Warehouse => 7.0,
            Kind::Garage | Kind::Carport | Kind::Shed => 2.8,
            Kind::Church => 8.0,
            Kind::ParkingGarage => 3.1,
            _ => 3.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Roof {
    Flat,
    Gabled,
    Hipped,
    Pyramidal,
    Skillion,
    Gambrel,
    Mansard,
    Dome,
    Onion,
    Cone,
    Round,
    HalfHipped,
}

impl Roof {
    fn from_tag(v: &str) -> Option<Roof> {
        Some(match v {
            "flat" => Roof::Flat,
            "gabled" | "saltbox" => Roof::Gabled,
            "hipped" | "side_hipped" => Roof::Hipped,
            "half-hipped" => Roof::HalfHipped,
            "pyramidal" => Roof::Pyramidal,
            "skillion" | "lean_to" => Roof::Skillion,
            "gambrel" => Roof::Gambrel,
            "mansard" => Roof::Mansard,
            "dome" => Roof::Dome,
            "onion" => Roof::Onion,
            "cone" => Roof::Cone,
            "round" => Roof::Round,
            _ => return None,
        })
    }
}

/// Wall finishes the facade shader knows (also in buildings.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Wall {
    Brick = 0,
    Siding = 1,
    Stucco = 2,
    Concrete = 3,
    Glass = 4,
    Stone = 5,
    WoodShingle = 6,
    Metal = 7,
    Plaster = 8,
}

/// Roof coverings (also in buildings.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RoofMat {
    Shingles = 0,
    ClayTile = 1,
    Metal = 2,
    Membrane = 3,
    Gravel = 4,
    Slate = 5,
    Copper = 6,
    Glass = 7,
}

/// Window arrangements (also in buildings.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Windows {
    /// Houses: double-hung windows with frames, spread out.
    House = 0,
    /// Apartments, hotels: regular punched windows.
    Punched = 1,
    /// Offices: ribbon windows across most of each floor.
    Ribbon = 2,
    /// All glass between mullions.
    Curtain = 3,
    /// Warehouses and factories: high small windows.
    Clerestory = 4,
    /// Tall arched windows (churches, civic buildings).
    Tall = 5,
    None = 6,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub outer: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
    /// Bottom and top of the walls, metres above the building's base.
    pub min_h: f32,
    pub wall_top: f32,
    pub roof: Roof,
    pub roof_h: f32,
    /// Ridge direction for gabled roofs (unit, x/z), when tagged.
    pub ridge: Option<Vec2>,
    pub wall: Wall,
    pub wall_col: [f32; 3],
    pub roof_mat: RoofMat,
    /// Tagged or measured from imagery later; None means "pick from the palette".
    pub roof_col: Option<[f32; 3]>,
    pub windows: Windows,
    pub levels: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntranceKind {
    Main,
    Service,
    Garage,
}

#[derive(Clone, Copy, Debug)]
pub struct Entrance {
    /// Which outline edge, and how far along it (metres).
    pub edge: usize,
    pub along: f32,
    pub kind: EntranceKind,
}

#[derive(Clone, Debug)]
pub struct IndoorArea {
    pub ring: Vec<Vec2>,
    pub level: i32,
    pub corridor: bool,
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Building {
    pub id: i64,
    pub kind: Kind,
    pub name: Option<String>,
    /// Main outline (counter-clockwise), and courtyards.
    pub outline: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
    pub parts: Vec<Part>,
    pub levels: u32,
    pub basement: bool,
    pub height: f32,
    pub entrances: Vec<Entrance>,
    pub indoor: Vec<IndoorArea>,
    pub seed: u64,
    /// Cuisine for restaurants (fine dining vs casual...), shop type for stores.
    pub detail: Option<String>,
    pub area: f32,
    /// Famous enough to have a wikidata entry (for landmark models).
    pub wikidata: Option<String>,
}

pub fn hash64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

pub fn hash01(x: u64) -> f32 {
    (hash64(x) >> 40) as f32 / (1u64 << 24) as f32
}

/// Parse a colour tag: #rrggbb, #rgb or a common name. Linear-ish 0..1 sRGB values.
pub fn colour(v: &str) -> Option<[f32; 3]> {
    let c = v.trim().to_ascii_lowercase();
    if let Some(h) = c.strip_prefix('#') {
        let p = |s: &str| u8::from_str_radix(s, 16).ok().map(|x| x as f32 / 255.0);
        return match h.len() {
            6 => Some([p(&h[0..2])?, p(&h[2..4])?, p(&h[4..6])?]),
            3 => Some([p(&h[0..1].repeat(2))?, p(&h[1..2].repeat(2))?, p(&h[2..3].repeat(2))?]),
            _ => None,
        };
    }
    Some(match c.as_str() {
        "white" => [0.92, 0.92, 0.9],
        "black" => [0.08, 0.08, 0.08],
        "grey" | "gray" => [0.55, 0.55, 0.55],
        "darkgrey" | "darkgray" | "dark_grey" => [0.33, 0.33, 0.34],
        "lightgrey" | "lightgray" | "light_grey" | "silver" => [0.75, 0.75, 0.75],
        "red" => [0.6, 0.15, 0.12],
        "darkred" | "maroon" => [0.4, 0.1, 0.08],
        "brown" => [0.42, 0.28, 0.18],
        "tan" | "beige" => [0.82, 0.74, 0.6],
        "cream" | "ivory" => [0.93, 0.9, 0.8],
        "yellow" => [0.88, 0.78, 0.35],
        "orange" => [0.85, 0.48, 0.2],
        "green" => [0.3, 0.45, 0.3],
        "darkgreen" => [0.16, 0.28, 0.18],
        "blue" => [0.25, 0.38, 0.6],
        "lightblue" => [0.6, 0.72, 0.85],
        "terracotta" => [0.7, 0.36, 0.24],
        "sandstone" => [0.82, 0.7, 0.52],
        _ => return None,
    })
}

/// Broad US building traditions, for default materials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Northeast,
    Southeast,
    Midwest,
    Mountain,
    Southwest,
    Pacific,
}

pub fn region(lat: f64, lon: f64) -> Region {
    if lon < -116.0 && lat > 37.0 || lon < -120.0 {
        Region::Pacific
    } else if lon < -103.0 && lat < 37.5 {
        Region::Southwest
    } else if lon < -102.0 {
        Region::Mountain
    } else if lat < 36.5 && lon > -100.0 {
        Region::Southeast
    } else if lon > -82.0 && lat > 38.5 {
        Region::Northeast
    } else {
        Region::Midwest
    }
}

fn levels_of(t: &Tags) -> Option<f32> {
    t.num("building:levels").or(t.num("levels")).map(|x| x as f32)
}

/// What the building is, from its own tags and the shops and amenities inside it.
fn classify(t: &Tags, pois: &[&Tags], area: f32) -> (Kind, Option<String>) {
    let b = t.get("building").or(t.get("building:part")).unwrap_or("yes");
    // Shops and restaurants inside only decide what a low, generic building is: a cafe
    // on the ground floor doesn't make a 30-storey tower a restaurant.
    let low = levels_of(t).is_none_or(|l| l <= 3.0) && t.num("height").is_none_or(|h| h <= 14.0);
    let generic = matches!(b, "yes" | "commercial" | "retail" | "kiosk" | "civic" | "public");
    let own = [t];
    let from: &[&Tags] = if low && generic { pois } else { &[] };
    for p in from.iter().chain(own.iter()) {
        if let Some(a) = p.get("amenity") {
            match a {
                "fast_food" => return (Kind::FastFood, p.get("cuisine").map(str::to_string)),
                "cafe" | "ice_cream" => return (Kind::Cafe, p.get("cuisine").map(str::to_string)),
                "restaurant" | "food_court" => return (Kind::Restaurant, p.get("cuisine").map(str::to_string)),
                "bar" | "pub" | "nightclub" | "biergarten" => return (Kind::Bar, None),
                "school" | "college" | "university" | "kindergarten" | "library" => return (Kind::School, None),
                "hospital" | "clinic" | "doctors" | "dentist" => return (Kind::Hospital, None),
                "place_of_worship" => return (Kind::Church, p.get("religion").map(str::to_string)),
                "townhall" | "courthouse" | "police" | "fire_station" | "post_office" | "community_centre" => {
                    return (Kind::Civic, Some(a.to_string()));
                }
                "parking" if b != "yes" || t.get("parking").is_some_and(|v| v == "multi-storey") => return (Kind::ParkingGarage, None),
                "fuel" => return (Kind::Canopy, Some("fuel".into())),
                "bank" => return (Kind::Office, Some("bank".into())),
                _ => {}
            }
        }
        if let Some(s) = p.get("shop") {
            return match s {
                "supermarket" | "grocery" | "greengrocer" | "wholesale" | "department_store" => (Kind::Supermarket, Some(s.to_string())),
                _ => (Kind::Retail, Some(s.to_string())),
            };
        }
        if p.get("office").is_some() {
            return (Kind::Office, None);
        }
        if p.get("tourism").is_some_and(|v| matches!(v, "hotel" | "motel" | "hostel" | "guest_house")) {
            return (Kind::Hotel, None);
        }
        if p.get("craft").is_some() {
            return (Kind::Industrial, None);
        }
        if p.get("healthcare").is_some() {
            return (Kind::Hospital, None);
        }
    }
    let k = match b {
        "house" | "detached" | "semidetached_house" | "bungalow" | "cabin" | "farm" | "residential" | "static_caravan" | "dormitory"
            if area < 450.0 =>
        {
            Kind::House
        }
        "terrace" | "townhouse" => Kind::Terrace,
        "apartments" | "residential" | "dormitory" => Kind::Apartments,
        "office" | "commercial" | "government" => {
            if b == "commercial" && area < 600.0 {
                Kind::Retail
            } else {
                Kind::Office
            }
        }
        "retail" | "kiosk" => Kind::Retail,
        "supermarket" => Kind::Supermarket,
        "industrial" | "manufacture" | "factory" => Kind::Industrial,
        "warehouse" | "storage_tank" | "hangar" | "barn" | "farm_auxiliary" | "greenhouse" => Kind::Warehouse,
        "garage" | "garages" => Kind::Garage,
        "carport" => Kind::Carport,
        "shed" | "hut" | "outbuilding" => Kind::Shed,
        "school" | "university" | "college" | "kindergarten" => Kind::School,
        "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" | "religious" | "shrine" => Kind::Church,
        "hospital" => Kind::Hospital,
        "hotel" => Kind::Hotel,
        "civic" | "public" | "fire_station" | "train_station" | "transportation" | "museum" => Kind::Civic,
        "parking" => Kind::ParkingGarage,
        "roof" | "canopy" => Kind::Canopy,
        "stadium" | "sports_hall" | "sports_centre" => Kind::Civic,
        // Untagged ("yes"): a small footprint is almost always a house or a garage.
        _ => {
            if area < 45.0 {
                Kind::Shed
            } else if area < 330.0 {
                Kind::House
            } else if area < 1500.0 {
                Kind::Retail
            } else {
                Kind::Warehouse
            }
        }
    };
    (k, None)
}

/// Default number of floors by kind and size.
fn default_levels(k: Kind, area: f32, seed: u64) -> u32 {
    let r = hash01(seed ^ 0x1e7);
    match k {
        Kind::House => {
            if area > 140.0 && r < 0.65 || r < 0.35 {
                2
            } else {
                1
            }
        }
        Kind::Terrace => 2 + (r < 0.4) as u32,
        Kind::Apartments => 3 + (r * 3.0) as u32,
        Kind::Office => (2.0 + area.sqrt() / 12.0 * (0.5 + r)).clamp(2.0, 12.0) as u32,
        Kind::Hotel => 3 + (r * 4.0) as u32,
        Kind::Hospital => 3 + (r * 3.0) as u32,
        Kind::School | Kind::Civic => 1 + (r < 0.5) as u32,
        Kind::ParkingGarage => 3 + (r * 3.0) as u32,
        Kind::Retail | Kind::FastFood | Kind::Cafe | Kind::Restaurant | Kind::Bar => 1 + (r < 0.15 && area > 200.0) as u32,
        _ => 1,
    }
}

/// Walls, roofs and windows for a kind of building in a region.
fn materials(k: Kind, reg: Region, levels: u32, seed: u64) -> (Wall, [f32; 3], RoofMat, Windows) {
    let r = hash01(seed ^ 0x3a1);
    let pick = |opts: &[[f32; 3]], s: u64| opts[(hash64(seed ^ s) % opts.len() as u64) as usize];
    let brick = [[0.55, 0.27, 0.2], [0.62, 0.34, 0.25], [0.48, 0.24, 0.18], [0.7, 0.55, 0.42], [0.58, 0.4, 0.32]];
    let siding = [
        [0.92, 0.91, 0.87],
        [0.78, 0.8, 0.78],
        [0.62, 0.68, 0.72],
        [0.85, 0.8, 0.66],
        [0.55, 0.62, 0.55],
        [0.45, 0.5, 0.58],
        [0.88, 0.84, 0.75],
    ];
    let stucco = [[0.9, 0.85, 0.74], [0.86, 0.78, 0.64], [0.93, 0.91, 0.86], [0.82, 0.68, 0.54], [0.88, 0.82, 0.7]];
    let concrete = [[0.72, 0.71, 0.68], [0.8, 0.79, 0.75], [0.62, 0.61, 0.59]];
    let glass = [[0.32, 0.42, 0.48], [0.24, 0.3, 0.34], [0.38, 0.5, 0.52]];
    let metal = [[0.75, 0.76, 0.76], [0.6, 0.62, 0.64], [0.35, 0.45, 0.55], [0.82, 0.8, 0.72]];
    match k {
        Kind::House | Kind::Terrace | Kind::Garage | Kind::Shed => {
            let (w, roof) = match reg {
                Region::Southwest => {
                    if r < 0.8 {
                        (Wall::Stucco, if r < 0.5 { RoofMat::ClayTile } else { RoofMat::Shingles })
                    } else {
                        (Wall::Brick, RoofMat::Shingles)
                    }
                }
                Region::Pacific => {
                    if r < 0.45 {
                        (Wall::Stucco, if r < 0.2 { RoofMat::ClayTile } else { RoofMat::Shingles })
                    } else if r < 0.8 {
                        (Wall::Siding, RoofMat::Shingles)
                    } else {
                        (Wall::WoodShingle, RoofMat::Shingles)
                    }
                }
                Region::Southeast => {
                    if r < 0.5 {
                        (Wall::Brick, RoofMat::Shingles)
                    } else if r < 0.9 {
                        (Wall::Siding, if r < 0.8 { RoofMat::Shingles } else { RoofMat::Metal })
                    } else {
                        (Wall::Stucco, RoofMat::Shingles)
                    }
                }
                Region::Mountain => {
                    if r < 0.45 {
                        (Wall::Siding, RoofMat::Shingles)
                    } else if r < 0.7 {
                        (Wall::Brick, RoofMat::Shingles)
                    } else if r < 0.85 {
                        (Wall::Stucco, RoofMat::Shingles)
                    } else {
                        (Wall::Stone, RoofMat::Metal)
                    }
                }
                _ => {
                    if r < 0.55 {
                        (Wall::Siding, RoofMat::Shingles)
                    } else if r < 0.88 {
                        (Wall::Brick, if r < 0.84 { RoofMat::Shingles } else { RoofMat::Slate })
                    } else {
                        (Wall::WoodShingle, RoofMat::Shingles)
                    }
                }
            };
            let col = match w {
                Wall::Brick => pick(&brick, 7),
                Wall::Stucco => pick(&stucco, 7),
                Wall::Stone => [0.62, 0.58, 0.52],
                Wall::WoodShingle => [0.5, 0.4, 0.3],
                _ => pick(&siding, 7),
            };
            let win = if matches!(k, Kind::Shed) || matches!(k, Kind::Garage) && r < 0.6 { Windows::None } else { Windows::House };
            (w, col, roof, win)
        }
        Kind::Apartments | Kind::Hotel => {
            let w = match reg {
                Region::Southwest | Region::Pacific if r < 0.6 => Wall::Stucco,
                _ if r < 0.5 => Wall::Brick,
                _ if r < 0.75 => Wall::Siding,
                _ => Wall::Concrete,
            };
            let col = match w {
                Wall::Brick => pick(&brick, 9),
                Wall::Stucco => pick(&stucco, 9),
                Wall::Siding => pick(&siding, 9),
                _ => pick(&concrete, 9),
            };
            (w, col, if levels <= 4 && r > 0.6 { RoofMat::Shingles } else { RoofMat::Membrane }, Windows::Punched)
        }
        Kind::Office | Kind::Hospital => {
            if levels >= 6 && r < 0.65 || r < 0.25 {
                (Wall::Glass, pick(&glass, 11), RoofMat::Gravel, Windows::Curtain)
            } else if r < 0.75 {
                (Wall::Concrete, pick(&concrete, 11), RoofMat::Membrane, Windows::Ribbon)
            } else {
                (Wall::Brick, pick(&brick, 11), RoofMat::Membrane, Windows::Punched)
            }
        }
        Kind::Retail | Kind::Supermarket | Kind::FastFood | Kind::Cafe | Kind::Restaurant | Kind::Bar => {
            let w = match reg {
                Region::Southwest | Region::Pacific | Region::Southeast if r < 0.5 => Wall::Stucco,
                _ if r < 0.45 => Wall::Brick,
                _ if r < 0.8 => Wall::Concrete,
                _ => Wall::Stone,
            };
            let col = match w {
                Wall::Brick => pick(&brick, 13),
                Wall::Stucco => pick(&stucco, 13),
                Wall::Stone => [0.7, 0.66, 0.58],
                _ => pick(&concrete, 13),
            };
            (w, col, RoofMat::Membrane, Windows::Punched)
        }
        Kind::Industrial | Kind::Warehouse => {
            if r < 0.6 {
                (Wall::Metal, pick(&metal, 15), RoofMat::Metal, Windows::Clerestory)
            } else {
                (Wall::Concrete, pick(&concrete, 15), RoofMat::Membrane, Windows::Clerestory)
            }
        }
        Kind::Church => (if r < 0.6 { Wall::Brick } else { Wall::Stone }, pick(&brick, 17), RoofMat::Slate, Windows::Tall),
        Kind::School | Kind::Civic => {
            if r < 0.6 {
                (Wall::Brick, pick(&brick, 19), RoofMat::Membrane, Windows::Ribbon)
            } else {
                (Wall::Stone, [0.78, 0.74, 0.66], RoofMat::Membrane, Windows::Tall)
            }
        }
        Kind::ParkingGarage => (Wall::Concrete, pick(&concrete, 21), RoofMat::Membrane, Windows::Ribbon),
        Kind::Carport | Kind::Canopy => (Wall::Metal, [0.85, 0.86, 0.86], RoofMat::Metal, Windows::None),
        Kind::Other => (Wall::Concrete, pick(&concrete, 23), RoofMat::Membrane, Windows::Punched),
    }
}

fn wall_from_tag(v: &str) -> Option<Wall> {
    Some(match v {
        "brick" => Wall::Brick,
        "wood" | "timber_framing" | "vinyl" | "plastic" => Wall::Siding,
        "plaster" | "stucco" => Wall::Stucco,
        "concrete" | "cement_block" | "reinforced_concrete" => Wall::Concrete,
        "glass" | "mirror" => Wall::Glass,
        "stone" | "sandstone" | "limestone" | "granite" | "marble" => Wall::Stone,
        "metal" | "steel" | "aluminium" | "aluminum" => Wall::Metal,
        _ => return None,
    })
}

fn roof_mat_from_tag(v: &str) -> Option<RoofMat> {
    Some(match v {
        "roof_tiles" | "tile" | "tiles" => RoofMat::ClayTile,
        "metal" | "copper" if v == "metal" => RoofMat::Metal,
        "copper" => RoofMat::Copper,
        "slate" => RoofMat::Slate,
        "concrete" | "tar_paper" | "asphalt" | "eternit" => RoofMat::Membrane,
        "gravel" => RoofMat::Gravel,
        "glass" => RoofMat::Glass,
        "asphalt_shingles" | "shingles" | "wood" => RoofMat::Shingles,
        _ => return None,
    })
}

/// One building or part from its tags.
#[allow(clippy::too_many_arguments)]
fn make_part(outer: Vec<Vec2>, holes: Vec<Vec<Vec2>>, t: &Tags, kind: Kind, reg: Region, seed: u64, building_levels: Option<f32>) -> Part {
    let area = geom::area(&outer);
    let fh = kind.floor_height();
    let levels_tag = levels_of(t).or(building_levels);
    let levels = levels_tag.map(|l| l.round().max(1.0) as u32).unwrap_or_else(|| default_levels(kind, area, seed));
    let roof_tag = t.get("roof:shape").and_then(Roof::from_tag);
    // Houses without a roof tag: pitched, the way most American houses are built.
    let roof = roof_tag.unwrap_or_else(|| match kind {
        Kind::House | Kind::Terrace | Kind::Garage | Kind::Shed => {
            let r = hash01(seed ^ 0x77);
            if reg == Region::Southwest && r < 0.35 {
                Roof::Flat
            } else if r < 0.55 {
                Roof::Gabled
            } else {
                Roof::Hipped
            }
        }
        Kind::Church => Roof::Gabled,
        Kind::Apartments if levels <= 3 && hash01(seed ^ 0x78) < 0.5 => Roof::Hipped,
        _ => Roof::Flat,
    });
    let (_, _, half) = geom::obb(&outer);
    let short = half.y * 2.0;
    let roof_h = t.num("roof:height").map(|v| v as f32).unwrap_or_else(|| match roof {
        Roof::Flat => 0.0,
        Roof::Gabled | Roof::Hipped | Roof::HalfHipped | Roof::Gambrel => {
            (short * 0.5 * if kind == Kind::Church { 0.9 } else { 0.5 }).clamp(1.0, 9.0)
        }
        Roof::Pyramidal | Roof::Cone => (short * 0.45).clamp(1.5, 20.0),
        Roof::Skillion => (short * 0.18).clamp(0.6, 3.0),
        Roof::Mansard => 2.8,
        Roof::Dome | Roof::Onion | Roof::Round => (short * 0.5).clamp(1.0, 40.0),
    });
    let min_h = t
        .num("min_height")
        .map(|v| v as f32)
        .unwrap_or_else(|| t.num("building:min_level").or(t.num("min_level")).unwrap_or(0.0) as f32 * fh);
    let total = t.num("height").map(|v| v as f32);
    let wall_top = match total {
        Some(h) => (h - roof_h).max(min_h + 1.0),
        None => {
            let ground = if kind.storefront() { 4.6 } else { fh };
            (ground + fh * (levels.saturating_sub(1)) as f32 + if roof == Roof::Flat && !kind.residential() { 0.8 } else { 0.0 })
                .max(min_h + 1.0)
        }
    };
    let (mut wall, mut wall_col, mut roof_mat, mut windows) = materials(kind, reg, levels, seed);
    if let Some(w) = t.get("building:material").and_then(wall_from_tag) {
        wall = w;
        if w == Wall::Glass {
            windows = Windows::Curtain;
        }
    }
    if let Some(c) = t.get("building:colour").and_then(colour) {
        wall_col = c;
    }
    if let Some(m) = t.get("roof:material").and_then(roof_mat_from_tag) {
        roof_mat = m;
    }
    if roof != Roof::Flat && matches!(roof_mat, RoofMat::Membrane | RoofMat::Gravel) {
        roof_mat = RoofMat::Shingles;
    }
    if roof == Roof::Flat && matches!(roof_mat, RoofMat::Shingles | RoofMat::ClayTile | RoofMat::Slate) {
        roof_mat = RoofMat::Membrane;
    }
    let ridge = t.get("roof:direction").and_then(|v| v.parse::<f32>().ok()).map(|deg| {
        let a = deg.to_radians();
        // Direction the roof slopes down to; the ridge runs across it.
        Vec2::new(a.sin(), -a.cos()).perp()
    });
    let ridge = ridge.or_else(|| match t.get("roof:orientation") {
        Some("across") => Some(geom::obb(&outer).1.perp()),
        _ => None,
    });
    Part {
        outer,
        holes,
        min_h,
        wall_top,
        roof,
        roof_h,
        ridge,
        wall,
        wall_col,
        roof_mat,
        roof_col: t.get("roof:colour").and_then(colour),
        windows,
        levels,
    }
}

/// All buildings of a tile, in the tile's local metres.
pub fn buildings(osm: &OsmTile, local: &Local, in_tile: &dyn Fn(Vec2) -> bool) -> Vec<Building> {
    let (lat0, lon0) = (local.lat0, local.lon0);
    let reg = region(lat0, lon0);
    let ring = |r: &Vec<(f64, f64)>| -> Vec<Vec2> { geom::clean(r.iter().map(|&(a, b)| local.xz(a, b)).collect()) };
    // Points of interest and entrances.
    let pois: Vec<(Vec2, &Tags)> = osm
        .nodes
        .iter()
        .filter(|n| ["shop", "amenity", "office", "craft", "tourism", "healthcare"].iter().any(|k| n.tags.get(k).is_some()))
        .map(|n| (local.xz(n.lat, n.lon), &n.tags))
        .collect();
    let doors: Vec<(Vec2, &Tags)> = osm
        .nodes
        .iter()
        .filter(|n| n.tags.get("entrance").is_some() || n.tags.get("door").is_some())
        .map(|n| (local.xz(n.lat, n.lon), &n.tags))
        .collect();
    let mut shells = Vec::new();
    let mut parts_raw = Vec::new();
    let mut indoor = Vec::new();
    for a in &osm.areas {
        let Some(o) = a.outer.first() else { continue };
        let outer = ring(o);
        if outer.len() < 3 || geom::area(&outer) < 4.0 {
            continue;
        }
        let holes: Vec<Vec<Vec2>> = a.inner.iter().map(ring).filter(|r| r.len() >= 3).collect();
        if usx_core::osm::is_indoor(&a.tags) && !usx_core::osm::is_building(&a.tags) {
            let level =
                a.tags.get("level").and_then(|v| v.split([';', ',']).next()).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(0.0);
            indoor.push(IndoorArea {
                ring: outer,
                level: level.round() as i32,
                corridor: a.tags.is("indoor", "corridor"),
                name: a.tags.get("name").map(str::to_string),
            });
            continue;
        }
        if a.tags.get("building:part").is_some_and(|v| v != "no") && a.tags.get("building").is_none() {
            parts_raw.push((outer, holes, a.tags.clone(), a.id));
        } else {
            shells.push((outer, holes, a.tags.clone(), a.id));
        }
    }
    let mut out = Vec::with_capacity(shells.len());
    for (outer, holes, tags, id) in shells {
        let c = geom::centroid(&outer);
        if !in_tile(c) {
            continue;
        }
        let area = geom::area(&outer);
        let inside: Vec<&Tags> = pois.iter().filter(|(p, _)| geom::contains(&outer, *p)).map(|(_, t)| *t).collect();
        let (kind, detail) = classify(&tags, &inside, area);
        let seed = hash64(id as u64);
        let blevels = levels_of(&tags);
        // Parts inside this outline replace it (the OSM convention for 3D buildings).
        let my_parts: Vec<Part> = parts_raw
            .iter()
            .filter(|(o, _, _, _)| geom::contains(&outer, geom::centroid(o)))
            .map(|(o, h, t, pid)| {
                let (pk, _) = classify(t, &[], geom::area(o));
                let k = if matches!(pk, Kind::Shed | Kind::House | Kind::Retail | Kind::Warehouse) { kind } else { pk };
                let mut p = make_part(o.clone(), h.clone(), t, k, reg, hash64(*pid as u64), None);
                // Parts share the building's colours unless they say otherwise.
                if t.get("building:colour").is_none() && t.get("building:material").is_none() {
                    let base = make_part(outer.clone(), Vec::new(), &tags, kind, reg, seed, None);
                    p.wall = base.wall;
                    p.wall_col = base.wall_col;
                    p.windows = base.windows;
                }
                p
            })
            .collect();
        let main = make_part(outer.clone(), holes.clone(), &tags, kind, reg, seed, None);
        let levels = main.levels.max(my_parts.iter().map(|p| p.levels).max().unwrap_or(0));
        let height = (main.wall_top + main.roof_h).max(my_parts.iter().map(|p| p.wall_top + p.roof_h).fold(0.0, f32::max));
        let parts = if my_parts.is_empty() { vec![main] } else { my_parts };
        // Entrances: mapped ones snapped to the outline, else one on the side facing
        // the street (the longest edge nearest the outline's... chosen later by the
        // caller who knows the roads).
        let mut entrances = Vec::new();
        for (p, t) in &doors {
            if geom::dist_to_ring(&outer, *p) > 1.5 {
                continue;
            }
            let (e, along) = nearest_edge(&outer, *p);
            let kind = match t.get("entrance") {
                Some("service" | "emergency" | "exit") => EntranceKind::Service,
                Some("garage") => EntranceKind::Garage,
                _ => EntranceKind::Main,
            };
            entrances.push(Entrance { edge: e, along, kind });
        }
        let underground = tags.num("building:levels:underground").unwrap_or(0.0);
        let basement = underground >= 1.0 || kind == Kind::House && hash01(seed ^ 0xba5e) < 0.45;
        out.push(Building {
            id,
            kind,
            name: tags.get("name").map(str::to_string),
            indoor: indoor.iter().filter(|r| geom::contains(&outer, geom::centroid(&r.ring))).cloned().collect(),
            outline: outer,
            holes,
            parts,
            levels: blevels.map(|l| l.round().max(1.0) as u32).unwrap_or(levels),
            basement,
            height,
            entrances,
            seed,
            detail,
            area,
            wikidata: tags.get("wikidata").map(str::to_string),
        });
    }
    out
}

/// The outline edge nearest a point, and how far along it the point projects.
pub fn nearest_edge(r: &[Vec2], p: Vec2) -> (usize, f32) {
    let n = r.len();
    let mut best = (0, 0.0, f32::MAX);
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        let d = p.distance(a + ab * t);
        if d < best.2 {
            best = (i, t * ab.length(), d);
        }
    }
    (best.0, best.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_kinds() {
        assert_eq!(colour("#ff0000"), Some([1.0, 0.0, 0.0]));
        assert!(colour("terracotta").is_some());
        let t = Tags(vec![("building".into(), "yes".into())]);
        assert_eq!(classify(&t, &[], 150.0).0, Kind::House);
        let shop = Tags(vec![("shop".into(), "supermarket".into())]);
        assert_eq!(classify(&t, &[&shop], 3000.0).0, Kind::Supermarket);
        let ff = Tags(vec![("amenity".into(), "fast_food".into())]);
        assert_eq!(classify(&t, &[&ff], 300.0).0, Kind::FastFood);
    }
}
