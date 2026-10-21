//! Tree and shrub species, which grow where (by rough ecoregion, elevation and setting),
//! and what the season is doing to them (green-up, fall colour, bare winter branches,
//! summer-dry grass).

/// How a species is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Form {
    /// Branching broadleaf tree.
    Broadleaf,
    /// Long drooping branches (willows).
    Weeping,
    /// Conifer with whorls of branches round a straight trunk.
    Conifer,
    /// Feather-leaved palm (coconut, royal, queen, date).
    Palm,
    /// Fan palm (cabbage palm, Washingtonia).
    FanPalm,
    /// Columnar cactus with arms (saguaro).
    Cactus,
    /// Forked branches ending in spiky leaf clusters (Joshua tree, yucca).
    Joshua,
    /// A round bush.
    Shrub,
}

/// Leaf texture in the leaf atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeafKind {
    /// Oval leaves (elm, cottonwood, ash, most).
    Ovate,
    /// Lobed hand-shaped leaves (maple, sycamore, sweetgum).
    Palmate,
    /// Round-lobed oak leaves.
    Oak,
    /// Small leathery leaves (live oak, olive, mesquite, crape myrtle).
    Small,
    /// Pine needles in tufts.
    Needle,
    /// Short dense needles (spruce, fir, hemlock, douglas fir).
    ShortNeedle,
    /// Scale leaves (junipers, cedars, cypress).
    Scale,
    /// Long narrow leaves (willow, eucalyptus).
    Lance,
    /// One leaflet strip of a feather palm frond.
    Pinnate,
    /// A fan palm frond.
    Fan,
    /// Stiff spikes (Joshua tree, yucca).
    Spike,
}

/// Bark texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bark {
    /// Furrowed grey-brown (oaks, ash, hickory).
    Furrowed,
    /// Smooth grey (maple, beech, young trees).
    Smooth,
    /// White with dark marks (birch, aspen).
    White,
    /// Mottled patches (sycamore, plane, eucalyptus).
    Mottled,
    /// Orange plates (ponderosa, red pine).
    Plated,
    /// Stringy red-brown (cedar, redwood, juniper, cypress).
    Stringy,
    /// Ringed palm trunk.
    PalmRings,
    /// Criss-cross leaf bases (cabbage palm).
    PalmBoots,
    /// Smooth grey concrete-like column (royal palm).
    PalmSmooth,
    /// Green ribbed cactus skin.
    Cactus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Sp {
    RedMaple,
    SugarMaple,
    RedOak,
    WhiteOak,
    LiveOak,
    CoastLiveOak,
    Elm,
    Cottonwood,
    Sycamore,
    Birch,
    Aspen,
    Ash,
    Pecan,
    TulipPoplar,
    Sweetgum,
    Magnolia,
    CrapeMyrtle,
    Dogwood,
    Mesquite,
    PaloVerde,
    Eucalyptus,
    HoneyLocust,
    Willow,
    Mangrove,
    Olive,
    PonderosaPine,
    LodgepolePine,
    LoblollyPine,
    WhitePine,
    DouglasFir,
    RedCedar,
    Spruce,
    Fir,
    Hemlock,
    Juniper,
    Pinyon,
    Redwood,
    BaldCypress,
    ItalianCypress,
    SabalPalm,
    CoconutPalm,
    RoyalPalm,
    WashingtoniaPalm,
    QueenPalm,
    DatePalm,
    Saguaro,
    JoshuaTree,
    Shrub,
    Sagebrush,
    Creosote,
    SawPalmetto,
}

pub struct Def {
    pub name: &'static str,
    pub form: Form,
    /// Typical mature height, metres.
    pub height: f32,
    /// Crown width as a fraction of height.
    pub width: f32,
    /// Bare trunk below the crown, as a fraction of height.
    pub clear: f32,
    pub leaf: LeafKind,
    pub bark: Bark,
    pub evergreen: bool,
    /// Summer leaf colour (linear-ish sRGB).
    pub green: [f32; 3],
    /// Peak fall colour (deciduous only).
    pub fall: [f32; 3],
    /// Bark colour.
    pub bark_col: [f32; 3],
    /// Leaf density, 0.5 (airy) .. 1.5 (dense).
    pub density: f32,
    /// Branches drooping (0) .. upright (1).
    pub upright: f32,
}

const fn d(
    name: &'static str,
    form: Form,
    height: f32,
    width: f32,
    clear: f32,
    leaf: LeafKind,
    bark: Bark,
    evergreen: bool,
    green: [f32; 3],
    fall: [f32; 3],
    bark_col: [f32; 3],
    density: f32,
    upright: f32,
) -> Def {
    Def { name, form, height, width, clear, leaf, bark, evergreen, green, fall, bark_col, density, upright }
}

use Bark as B;
use Form as F;
use LeafKind as L;

const GREY_BARK: [f32; 3] = [0.42, 0.4, 0.37];

pub fn def(s: Sp) -> Def {
    match s {
        Sp::RedMaple => d("red maple", F::Broadleaf, 17.0, 0.7, 0.3, L::Palmate, B::Smooth, false, [0.27, 0.42, 0.16], [0.75, 0.12, 0.08], GREY_BARK, 1.1, 0.6),
        Sp::SugarMaple => d("sugar maple", F::Broadleaf, 22.0, 0.75, 0.28, L::Palmate, B::Furrowed, false, [0.25, 0.42, 0.14], [0.92, 0.48, 0.08], GREY_BARK, 1.25, 0.6),
        Sp::RedOak => d("red oak", F::Broadleaf, 22.0, 0.85, 0.3, L::Oak, B::Furrowed, false, [0.22, 0.36, 0.13], [0.55, 0.16, 0.08], [0.3, 0.28, 0.26], 1.1, 0.5),
        Sp::WhiteOak => d("white oak", F::Broadleaf, 22.0, 0.95, 0.28, L::Oak, B::Furrowed, false, [0.24, 0.37, 0.14], [0.5, 0.28, 0.12], [0.5, 0.48, 0.44], 1.1, 0.4),
        Sp::LiveOak => d("live oak", F::Broadleaf, 14.0, 1.7, 0.18, L::Small, B::Furrowed, true, [0.18, 0.28, 0.11], [0.18, 0.28, 0.11], [0.26, 0.24, 0.22], 1.2, 0.2),
        Sp::CoastLiveOak => d("coast live oak", F::Broadleaf, 13.0, 1.3, 0.2, L::Small, B::Furrowed, true, [0.17, 0.27, 0.1], [0.17, 0.27, 0.1], [0.3, 0.28, 0.26], 1.2, 0.3),
        Sp::Elm => d("American elm", F::Broadleaf, 24.0, 0.8, 0.35, L::Ovate, B::Furrowed, false, [0.26, 0.4, 0.15], [0.78, 0.65, 0.2], [0.36, 0.32, 0.28], 1.0, 0.85),
        Sp::Cottonwood => d("cottonwood", F::Broadleaf, 26.0, 0.8, 0.3, L::Ovate, B::Furrowed, false, [0.3, 0.45, 0.17], [0.85, 0.72, 0.18], [0.45, 0.42, 0.38], 0.9, 0.6),
        Sp::Sycamore => d("sycamore", F::Broadleaf, 24.0, 0.8, 0.3, L::Palmate, B::Mottled, false, [0.3, 0.44, 0.18], [0.55, 0.42, 0.2], [0.75, 0.73, 0.66], 0.95, 0.55),
        Sp::Birch => d("paper birch", F::Broadleaf, 15.0, 0.5, 0.3, L::Ovate, B::White, false, [0.32, 0.47, 0.17], [0.88, 0.75, 0.2], [0.88, 0.86, 0.82], 0.8, 0.7),
        Sp::Aspen => d("quaking aspen", F::Broadleaf, 15.0, 0.35, 0.45, L::Ovate, B::White, false, [0.32, 0.5, 0.18], [0.95, 0.75, 0.12], [0.82, 0.83, 0.76], 0.8, 0.8),
        Sp::Ash => d("green ash", F::Broadleaf, 18.0, 0.75, 0.3, L::Ovate, B::Furrowed, false, [0.25, 0.4, 0.15], [0.65, 0.5, 0.15], GREY_BARK, 0.95, 0.6),
        Sp::Pecan => d("pecan", F::Broadleaf, 26.0, 0.85, 0.3, L::Lance, B::Furrowed, false, [0.27, 0.4, 0.15], [0.72, 0.58, 0.18], [0.4, 0.36, 0.3], 0.9, 0.55),
        Sp::TulipPoplar => d("tulip poplar", F::Broadleaf, 30.0, 0.45, 0.45, L::Palmate, B::Furrowed, false, [0.3, 0.45, 0.16], [0.88, 0.75, 0.18], GREY_BARK, 1.0, 0.85),
        Sp::Sweetgum => d("sweetgum", F::Broadleaf, 22.0, 0.55, 0.3, L::Palmate, B::Furrowed, false, [0.24, 0.4, 0.15], [0.62, 0.12, 0.2], GREY_BARK, 1.1, 0.75),
        Sp::Magnolia => d("southern magnolia", F::Broadleaf, 17.0, 0.55, 0.12, L::Ovate, B::Smooth, true, [0.12, 0.25, 0.09], [0.12, 0.25, 0.09], [0.45, 0.43, 0.4], 1.35, 0.7),
        Sp::CrapeMyrtle => d("crape myrtle", F::Broadleaf, 6.0, 0.8, 0.35, L::Small, B::Mottled, false, [0.26, 0.4, 0.15], [0.8, 0.3, 0.12], [0.62, 0.52, 0.45], 1.0, 0.9),
        Sp::Dogwood => d("dogwood", F::Broadleaf, 7.0, 1.0, 0.25, L::Ovate, B::Smooth, false, [0.26, 0.4, 0.15], [0.6, 0.12, 0.12], GREY_BARK, 0.95, 0.5),
        Sp::Mesquite => d("mesquite", F::Broadleaf, 7.0, 1.3, 0.15, L::Small, B::Furrowed, false, [0.33, 0.42, 0.19], [0.5, 0.48, 0.25], [0.3, 0.26, 0.22], 0.65, 0.35),
        Sp::PaloVerde => d("palo verde", F::Broadleaf, 7.0, 1.2, 0.15, L::Small, B::Smooth, true, [0.45, 0.52, 0.25], [0.45, 0.52, 0.25], [0.48, 0.6, 0.32], 0.5, 0.45),
        Sp::Eucalyptus => d("eucalyptus", F::Broadleaf, 30.0, 0.5, 0.4, L::Lance, B::Mottled, true, [0.33, 0.43, 0.3], [0.33, 0.43, 0.3], [0.72, 0.66, 0.58], 0.75, 0.7),
        Sp::HoneyLocust => d("honey locust", F::Broadleaf, 18.0, 0.9, 0.3, L::Small, B::Furrowed, false, [0.33, 0.47, 0.17], [0.88, 0.76, 0.22], [0.3, 0.27, 0.25], 0.7, 0.55),
        Sp::Willow => d("weeping willow", F::Weeping, 13.0, 1.1, 0.25, L::Lance, B::Furrowed, false, [0.38, 0.5, 0.2], [0.78, 0.72, 0.25], [0.4, 0.36, 0.3], 1.0, 0.1),
        Sp::Mangrove => d("red mangrove", F::Broadleaf, 6.0, 1.2, 0.2, L::Ovate, B::Smooth, true, [0.17, 0.3, 0.12], [0.17, 0.3, 0.12], [0.35, 0.3, 0.25], 1.3, 0.4),
        Sp::Olive => d("olive", F::Broadleaf, 7.0, 1.0, 0.25, L::Lance, B::Furrowed, true, [0.38, 0.45, 0.33], [0.38, 0.45, 0.33], [0.42, 0.4, 0.36], 0.85, 0.55),
        Sp::PonderosaPine => d("ponderosa pine", F::Conifer, 30.0, 0.35, 0.5, L::Needle, B::Plated, true, [0.2, 0.32, 0.14], [0.2, 0.32, 0.14], [0.6, 0.38, 0.22], 0.8, 0.55),
        Sp::LodgepolePine => d("lodgepole pine", F::Conifer, 22.0, 0.22, 0.55, L::Needle, B::Furrowed, true, [0.2, 0.32, 0.15], [0.2, 0.32, 0.15], [0.42, 0.36, 0.3], 0.8, 0.6),
        Sp::LoblollyPine => d("loblolly pine", F::Conifer, 28.0, 0.3, 0.6, L::Needle, B::Plated, true, [0.23, 0.35, 0.14], [0.23, 0.35, 0.14], [0.45, 0.32, 0.24], 0.85, 0.6),
        Sp::WhitePine => d("eastern white pine", F::Conifer, 26.0, 0.45, 0.3, L::Needle, B::Furrowed, true, [0.21, 0.35, 0.2], [0.21, 0.35, 0.2], [0.35, 0.32, 0.3], 0.85, 0.45),
        Sp::DouglasFir => d("Douglas fir", F::Conifer, 40.0, 0.3, 0.25, L::ShortNeedle, B::Furrowed, true, [0.13, 0.24, 0.11], [0.13, 0.24, 0.11], [0.36, 0.28, 0.22], 1.15, 0.4),
        Sp::RedCedar => d("western red cedar", F::Conifer, 38.0, 0.33, 0.12, L::Scale, B::Stringy, true, [0.15, 0.28, 0.11], [0.15, 0.28, 0.11], [0.5, 0.32, 0.22], 1.2, 0.25),
        Sp::Spruce => d("spruce", F::Conifer, 28.0, 0.26, 0.06, L::ShortNeedle, B::Furrowed, true, [0.14, 0.25, 0.15], [0.14, 0.25, 0.15], [0.34, 0.3, 0.27], 1.3, 0.35),
        Sp::Fir => d("fir", F::Conifer, 22.0, 0.22, 0.06, L::ShortNeedle, B::Smooth, true, [0.13, 0.25, 0.12], [0.13, 0.25, 0.12], [0.4, 0.38, 0.36], 1.3, 0.5),
        Sp::Hemlock => d("hemlock", F::Conifer, 30.0, 0.32, 0.1, L::ShortNeedle, B::Furrowed, true, [0.14, 0.26, 0.12], [0.14, 0.26, 0.12], [0.38, 0.3, 0.25], 1.2, 0.2),
        Sp::Juniper => d("juniper", F::Broadleaf, 7.0, 0.85, 0.12, L::Scale, B::Stringy, true, [0.2, 0.3, 0.17], [0.2, 0.3, 0.17], [0.45, 0.35, 0.28], 1.1, 0.5),
        Sp::Pinyon => d("pinyon pine", F::Broadleaf, 8.0, 0.9, 0.12, L::Needle, B::Furrowed, true, [0.22, 0.32, 0.17], [0.22, 0.32, 0.17], [0.4, 0.36, 0.32], 1.0, 0.5),
        Sp::Redwood => d("coast redwood", F::Conifer, 70.0, 0.22, 0.35, L::Scale, B::Stringy, true, [0.14, 0.26, 0.11], [0.14, 0.26, 0.11], [0.5, 0.28, 0.18], 1.1, 0.35),
        Sp::BaldCypress => d("bald cypress", F::Conifer, 28.0, 0.35, 0.3, L::Scale, B::Stringy, false, [0.27, 0.4, 0.15], [0.6, 0.32, 0.12], [0.5, 0.38, 0.3], 0.9, 0.5),
        Sp::ItalianCypress => d("Italian cypress", F::Conifer, 14.0, 0.13, 0.03, L::Scale, B::Stringy, true, [0.13, 0.24, 0.11], [0.13, 0.24, 0.11], [0.4, 0.32, 0.26], 1.4, 0.95),
        Sp::SabalPalm => d("cabbage palm", F::FanPalm, 12.0, 0.4, 0.85, L::Fan, B::PalmBoots, true, [0.24, 0.36, 0.15], [0.24, 0.36, 0.15], [0.45, 0.4, 0.33], 1.0, 0.6),
        Sp::CoconutPalm => d("coconut palm", F::Palm, 16.0, 0.55, 0.85, L::Pinnate, B::PalmRings, true, [0.3, 0.42, 0.14], [0.3, 0.42, 0.14], [0.5, 0.45, 0.38], 1.0, 0.25),
        Sp::RoyalPalm => d("royal palm", F::Palm, 20.0, 0.45, 0.8, L::Pinnate, B::PalmSmooth, true, [0.25, 0.38, 0.13], [0.25, 0.38, 0.13], [0.72, 0.71, 0.68], 1.0, 0.4),
        Sp::WashingtoniaPalm => d("Mexican fan palm", F::FanPalm, 22.0, 0.18, 0.9, L::Fan, B::PalmRings, true, [0.27, 0.38, 0.15], [0.27, 0.38, 0.15], [0.42, 0.38, 0.33], 1.0, 0.65),
        Sp::QueenPalm => d("queen palm", F::Palm, 12.0, 0.6, 0.8, L::Pinnate, B::PalmRings, true, [0.27, 0.4, 0.14], [0.27, 0.4, 0.14], [0.55, 0.53, 0.5], 1.0, 0.3),
        Sp::DatePalm => d("date palm", F::Palm, 16.0, 0.5, 0.75, L::Pinnate, B::PalmBoots, true, [0.32, 0.4, 0.27], [0.32, 0.4, 0.27], [0.5, 0.42, 0.33], 1.1, 0.55),
        Sp::Saguaro => d("saguaro", F::Cactus, 10.0, 0.3, 1.0, L::Spike, B::Cactus, true, [0.3, 0.4, 0.22], [0.3, 0.4, 0.22], [0.32, 0.42, 0.24], 1.0, 1.0),
        Sp::JoshuaTree => d("Joshua tree", F::Joshua, 8.0, 0.8, 0.4, L::Spike, B::Furrowed, true, [0.3, 0.38, 0.2], [0.3, 0.38, 0.2], [0.45, 0.4, 0.34], 1.0, 0.7),
        Sp::Shrub => d("shrub", F::Shrub, 2.2, 1.2, 0.0, L::Small, B::Smooth, true, [0.22, 0.34, 0.14], [0.6, 0.35, 0.15], [0.35, 0.3, 0.25], 1.3, 0.5),
        Sp::Sagebrush => d("sagebrush", F::Shrub, 1.3, 1.3, 0.0, L::Small, B::Furrowed, true, [0.52, 0.56, 0.45], [0.52, 0.56, 0.45], [0.4, 0.36, 0.3], 1.0, 0.5),
        Sp::Creosote => d("creosote bush", F::Shrub, 1.6, 1.2, 0.0, L::Small, B::Smooth, true, [0.3, 0.38, 0.18], [0.3, 0.38, 0.18], [0.35, 0.32, 0.28], 0.7, 0.8),
        Sp::SawPalmetto => d("saw palmetto", F::Shrub, 1.4, 1.6, 0.0, L::Fan, B::PalmBoots, true, [0.3, 0.42, 0.2], [0.3, 0.42, 0.2], [0.45, 0.4, 0.33], 1.0, 0.6),
    }
}

/// Broad US ecoregions (simplified from the EPA's), enough to pick plausible species
/// and grass colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Biome {
    Tropical,
    SouthFlorida,
    GulfCoast,
    Southeast,
    Northeast,
    NorthWoods,
    Plains,
    HillCountry,
    SouthTexas,
    Sonoran,
    Mojave,
    Chihuahuan,
    GreatBasin,
    RockyMontane,
    Subalpine,
    Alpine,
    California,
    Redwoods,
    SierraMontane,
    PacificNorthwest,
    InlandNorthwest,
    Boreal,
}

/// The ecoregion at a place (elevation in metres).
pub fn biome_at(lat: f64, lon: f64, elev: f64) -> Biome {
    let e = elev;
    if lat < 23.5 && lon < -150.0 {
        return Biome::Tropical;
    }
    if lat > 51.0 && lon < -129.0 {
        return Biome::Boreal;
    }
    // Mountains: elevation bands by latitude (tree line lower further north).
    let west = lon < -103.0;
    if west {
        let tree_line = 3700.0 - (lat - 35.0).max(0.0) * 60.0;
        if e > tree_line {
            return Biome::Alpine;
        }
        if e > tree_line - 900.0 {
            return Biome::Subalpine;
        }
    }
    // Pacific coast states.
    if lon < -116.5 {
        if lat >= 42.0 {
            return if lon < -121.4 { Biome::PacificNorthwest } else if e > 1300.0 { Biome::RockyMontane } else { Biome::InlandNorthwest };
        }
        if lat >= 37.0 && lon < -122.2 && lat < 42.0 && e < 900.0 {
            return Biome::Redwoods;
        }
        if e > 1300.0 && lat > 35.0 {
            return Biome::SierraMontane;
        }
        // The deserts east of the mountains.
        if lon > -118.4 && lat < 37.5 && !(lat < 34.6 && lon < -117.2) {
            return if lat < 34.0 { Biome::Sonoran } else { Biome::Mojave };
        }
        return Biome::California;
    }
    if west {
        // Interior West.
        if lat < 34.6 && lon < -110.8 && e < 1300.0 {
            return Biome::Sonoran;
        }
        if lat < 37.5 && lon < -112.5 && e < 1600.0 {
            return Biome::Mojave;
        }
        if lat < 33.6 && lon > -108.8 && e < 1700.0 {
            return Biome::Chihuahuan;
        }
        if lon > -105.3 && e < 1900.0 {
            return Biome::Plains;
        }
        if e > 2000.0 {
            return Biome::RockyMontane;
        }
        if lat > 42.0 && lon < -111.0 {
            return Biome::InlandNorthwest;
        }
        return if lat < 34.0 { Biome::Chihuahuan } else { Biome::GreatBasin };
    }
    // East of the Rockies.
    if lat < 27.3 && lon > -82.6 {
        return Biome::SouthFlorida;
    }
    if lon < -97.0 && (lat < 29.2 || lat < 29.6 && lon < -99.2) {
        return Biome::SouthTexas;
    }
    if (29.0..32.6).contains(&lat) && (-100.8..-97.0).contains(&lon) {
        return Biome::HillCountry;
    }
    if lon < -97.5 || lon < -96.0 && lat > 40.0 {
        return Biome::Plains;
    }
    // Gulf and south Atlantic coastal plain.
    let coast_n = 30.8 + (lon + 81.0).max(0.0) * 0.6;
    if lat < coast_n && lon > -97.0 {
        return Biome::GulfCoast;
    }
    if lat < 37.3 {
        return Biome::Southeast;
    }
    let north = if lon > -80.0 { 43.6 } else { 45.2 };
    if lat > north || lat > 42.5 && e > 600.0 {
        return Biome::NorthWoods;
    }
    Biome::Northeast
}

/// Where a tree is growing, which changes the mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    /// Streets, yards and city parks: planted trees.
    Urban,
    /// Woods and wild land.
    Wild,
    /// Beside rivers, lakes and in wetlands.
    Riparian,
    /// Beaches and dunes.
    Coastal,
}

/// Species and weights for a biome and setting.
pub fn mix(b: Biome, s: Setting) -> &'static [(Sp, f32)] {
    use Biome::*;
    use Setting::*;
    use Sp::*;
    match (b, s) {
        (Tropical, Coastal) => &[(CoconutPalm, 5.0), (RoyalPalm, 1.0), (Shrub, 1.0)],
        (Tropical, _) => &[(CoconutPalm, 2.0), (RoyalPalm, 1.0), (Mangrove, 0.5), (LiveOak, 1.0), (Eucalyptus, 0.5), (Shrub, 1.0)],
        (SouthFlorida, Coastal) => &[(CoconutPalm, 4.0), (SabalPalm, 3.0), (RoyalPalm, 1.0), (SawPalmetto, 1.0), (Mangrove, 0.5)],
        (SouthFlorida, Riparian) => &[(Mangrove, 3.0), (SabalPalm, 2.0), (BaldCypress, 1.0), (LiveOak, 1.0)],
        (SouthFlorida, Urban) => &[
            (RoyalPalm, 2.5),
            (CoconutPalm, 2.0),
            (SabalPalm, 2.5),
            (QueenPalm, 1.5),
            (LiveOak, 2.0),
            (Mangrove, 0.3),
            (Shrub, 1.0),
        ],
        (SouthFlorida, Wild) => &[(SabalPalm, 3.0), (LoblollyPine, 2.0), (LiveOak, 2.0), (SawPalmetto, 2.0), (BaldCypress, 0.5)],
        (GulfCoast, Coastal) => &[(SabalPalm, 3.0), (LiveOak, 2.0), (LoblollyPine, 1.0), (SawPalmetto, 1.5), (Shrub, 1.0)],
        (GulfCoast, Riparian) => &[(BaldCypress, 3.0), (Willow, 1.0), (LiveOak, 1.5), (Sweetgum, 1.0)],
        (GulfCoast, Urban) => &[(LiveOak, 4.0), (Magnolia, 1.5), (CrapeMyrtle, 2.0), (SabalPalm, 1.5), (WashingtoniaPalm, 0.6), (RedMaple, 0.8), (Shrub, 1.0)],
        (GulfCoast, Wild) => &[(LoblollyPine, 4.0), (LiveOak, 2.0), (Sweetgum, 1.0), (Magnolia, 0.7), (SabalPalm, 1.0), (SawPalmetto, 1.0)],
        (Southeast, Riparian) => &[(Sycamore, 2.0), (Willow, 1.0), (RedMaple, 1.5), (Sweetgum, 1.0), (BaldCypress, 0.6)],
        (Southeast, Urban) => &[
            (RedOak, 2.0),
            (WhiteOak, 1.0),
            (RedMaple, 2.0),
            (CrapeMyrtle, 2.0),
            (Magnolia, 1.0),
            (Dogwood, 1.0),
            (LoblollyPine, 1.0),
            (Shrub, 1.0),
        ],
        (Southeast, _) => &[
            (LoblollyPine, 3.0),
            (RedOak, 1.5),
            (WhiteOak, 1.5),
            (Sweetgum, 1.5),
            (TulipPoplar, 1.5),
            (RedMaple, 1.0),
            (Dogwood, 0.5),
            (Pecan, 0.4),
        ],
        (Northeast, Riparian) => &[(Sycamore, 2.0), (Willow, 1.0), (RedMaple, 1.5), (Cottonwood, 1.0)],
        (Northeast, Urban) => &[
            (RedMaple, 2.0),
            (SugarMaple, 1.0),
            (RedOak, 1.5),
            (HoneyLocust, 1.5),
            (Sycamore, 1.0),
            (Elm, 1.0),
            (Ash, 0.6),
            (Spruce, 0.4),
            (Dogwood, 0.6),
            (Shrub, 1.0),
        ],
        (Northeast, _) => &[
            (SugarMaple, 2.0),
            (RedMaple, 2.0),
            (RedOak, 2.0),
            (WhiteOak, 1.0),
            (TulipPoplar, 1.0),
            (Ash, 1.0),
            (WhitePine, 0.8),
            (Hemlock, 0.6),
            (Birch, 0.4),
        ],
        (NorthWoods, Riparian) => &[(Birch, 1.0), (RedMaple, 1.0), (Willow, 0.5), (Spruce, 1.0), (Aspen, 1.0)],
        (NorthWoods, Urban) => &[(SugarMaple, 2.0), (RedMaple, 1.5), (Spruce, 1.0), (Birch, 1.0), (Ash, 1.0), (HoneyLocust, 0.6), (Shrub, 1.0)],
        (NorthWoods, _) => &[(SugarMaple, 2.0), (Birch, 1.5), (Aspen, 1.5), (WhitePine, 1.5), (Spruce, 2.0), (Fir, 1.5), (Hemlock, 1.0), (RedMaple, 1.0)],
        (Plains, Wild) => &[(Juniper, 1.0), (Cottonwood, 0.6), (Shrub, 1.5)],
        (Plains, Riparian) => &[(Cottonwood, 4.0), (Willow, 1.5), (Elm, 1.0), (Ash, 1.0)],
        (Plains, _) => &[(Elm, 2.0), (Ash, 1.5), (HoneyLocust, 1.5), (Cottonwood, 1.5), (Spruce, 1.0), (Juniper, 0.6), (Shrub, 1.0)],
        (HillCountry, Riparian) => &[(Pecan, 2.0), (BaldCypress, 2.0), (Sycamore, 1.0), (Elm, 1.0)],
        (HillCountry, Urban) => &[
            (LiveOak, 4.0),
            (Elm, 1.5),
            (Pecan, 1.0),
            (CrapeMyrtle, 1.5),
            (Mesquite, 0.6),
            (WashingtoniaPalm, 0.5),
            (SabalPalm, 0.3),
            (Juniper, 0.6),
            (Shrub, 1.0),
        ],
        (HillCountry, _) => &[(LiveOak, 3.0), (Juniper, 3.0), (Mesquite, 1.0), (Elm, 0.8), (Shrub, 1.5)],
        (SouthTexas, Urban) => &[(LiveOak, 2.0), (WashingtoniaPalm, 1.5), (SabalPalm, 1.0), (Mesquite, 1.0), (CrapeMyrtle, 1.0), (Shrub, 1.0)],
        (SouthTexas, Coastal) => &[(SabalPalm, 1.0), (Shrub, 2.0), (LiveOak, 0.5)],
        (SouthTexas, _) => &[(Mesquite, 4.0), (LiveOak, 1.0), (Shrub, 3.0)],
        (Sonoran, Urban) => &[(WashingtoniaPalm, 2.0), (DatePalm, 1.0), (PaloVerde, 1.5), (Mesquite, 1.5), (Olive, 0.6), (Eucalyptus, 0.6), (Shrub, 1.0)],
        (Sonoran, Riparian) => &[(Cottonwood, 2.0), (Mesquite, 2.0), (Willow, 1.0)],
        (Sonoran, _) => &[(Saguaro, 2.5), (PaloVerde, 2.0), (Mesquite, 1.5), (Creosote, 4.0)],
        (Mojave, Urban) => &[(WashingtoniaPalm, 2.0), (DatePalm, 1.0), (Mesquite, 1.0), (Olive, 0.6), (ItalianCypress, 0.5), (Shrub, 1.0)],
        (Mojave, Riparian) => &[(Cottonwood, 2.0), (Mesquite, 1.0), (Willow, 1.0)],
        (Mojave, _) => &[(JoshuaTree, 1.5), (Creosote, 5.0), (Sagebrush, 1.0)],
        (Chihuahuan, Urban) => &[(Mesquite, 1.5), (Ash, 1.0), (Elm, 1.0), (WashingtoniaPalm, 0.8), (Juniper, 0.5), (Shrub, 1.0)],
        (Chihuahuan, Riparian) => &[(Cottonwood, 3.0), (Willow, 1.0)],
        (Chihuahuan, _) => &[(Creosote, 4.0), (Mesquite, 2.0), (Juniper, 0.6)],
        (GreatBasin, Urban) => &[(Cottonwood, 1.5), (Ash, 1.0), (HoneyLocust, 1.0), (Spruce, 1.0), (Juniper, 0.6), (Shrub, 1.0)],
        (GreatBasin, Riparian) => &[(Cottonwood, 3.0), (Willow, 2.0)],
        (GreatBasin, _) => &[(Sagebrush, 5.0), (Juniper, 2.0), (Pinyon, 2.0)],
        (RockyMontane, Urban) => &[(Spruce, 2.0), (Cottonwood, 1.5), (Aspen, 1.0), (Ash, 1.0), (HoneyLocust, 1.0), (Shrub, 1.0)],
        (RockyMontane, Riparian) => &[(Cottonwood, 2.0), (Aspen, 2.0), (Willow, 1.0), (Spruce, 1.0)],
        (RockyMontane, _) => &[(PonderosaPine, 4.0), (DouglasFir, 2.0), (Aspen, 1.5), (LodgepolePine, 1.0), (Juniper, 0.5)],
        (Subalpine, Riparian) => &[(Willow, 1.0), (Spruce, 2.0), (Aspen, 1.0)],
        (Subalpine, _) => &[(Spruce, 4.0), (Fir, 3.0), (LodgepolePine, 2.0), (Aspen, 1.0)],
        (Alpine, _) => &[(Shrub, 1.0)],
        (California, Coastal) => &[(WashingtoniaPalm, 1.5), (Eucalyptus, 1.0), (CoastLiveOak, 1.0), (Shrub, 2.0)],
        (California, Riparian) => &[(Sycamore, 2.0), (Willow, 1.5), (Cottonwood, 1.0), (CoastLiveOak, 1.0)],
        (California, Urban) => &[
            (WashingtoniaPalm, 2.0),
            (QueenPalm, 1.0),
            (CoastLiveOak, 1.5),
            (Eucalyptus, 1.5),
            (Sycamore, 1.0),
            (Olive, 0.8),
            (ItalianCypress, 0.6),
            (CrapeMyrtle, 0.8),
            (Shrub, 1.0),
        ],
        (California, _) => &[(CoastLiveOak, 4.0), (WhiteOak, 1.0), (Eucalyptus, 0.6), (Shrub, 3.0)],
        (Redwoods, Riparian) => &[(Redwood, 2.0), (Sycamore, 1.0), (Willow, 0.5)],
        (Redwoods, Urban) => &[(CoastLiveOak, 1.5), (Eucalyptus, 1.5), (Redwood, 1.0), (WashingtoniaPalm, 0.6), (Shrub, 1.0)],
        (Redwoods, _) => &[(Redwood, 3.0), (DouglasFir, 2.0), (CoastLiveOak, 1.0)],
        (SierraMontane, Urban) => &[(PonderosaPine, 2.0), (DouglasFir, 1.0), (CoastLiveOak, 1.0), (Shrub, 1.0)],
        (SierraMontane, _) => &[(PonderosaPine, 3.0), (Fir, 2.0), (DouglasFir, 1.5), (RedCedar, 1.0), (CoastLiveOak, 0.6)],
        (PacificNorthwest, Riparian) => &[(Cottonwood, 1.5), (Willow, 1.0), (RedCedar, 1.5), (RedMaple, 1.0)],
        (PacificNorthwest, Urban) => &[(DouglasFir, 2.0), (RedMaple, 1.5), (RedCedar, 1.0), (Birch, 1.0), (Dogwood, 0.6), (Sweetgum, 0.6), (Shrub, 1.0)],
        (PacificNorthwest, _) => &[(DouglasFir, 4.0), (RedCedar, 2.0), (Hemlock, 2.0), (RedMaple, 0.6)],
        (InlandNorthwest, Urban) => &[(Spruce, 1.0), (Ash, 1.0), (HoneyLocust, 1.0), (PonderosaPine, 1.0), (Elm, 1.0), (Shrub, 1.0)],
        (InlandNorthwest, Riparian) => &[(Cottonwood, 3.0), (Willow, 1.5), (Aspen, 1.0)],
        (InlandNorthwest, _) => &[(PonderosaPine, 3.0), (Sagebrush, 3.0), (DouglasFir, 1.0), (Juniper, 1.0)],
        (Boreal, _) => &[(Spruce, 4.0), (Birch, 2.0), (Aspen, 1.5), (Fir, 1.0)],
    }
}

/// Pick from a weighted list with a 0..1 number.
pub fn pick(list: &[(Sp, f32)], r: f32) -> Sp {
    let total: f32 = list.iter().map(|x| x.1).sum();
    let mut t = r * total;
    for &(s, w) in list {
        if t < w {
            return s;
        }
        t -= w;
    }
    list.last().map(|x| x.0).unwrap_or(Sp::Shrub)
}

/// Species from OSM `genus`/`species`/`taxon`/`leaf_type` tags, when they say.
pub fn from_tags(t: &usx_core::osm::Tags) -> Option<Sp> {
    let name = t.get("species").or(t.get("taxon")).or(t.get("genus")).unwrap_or("").to_ascii_lowercase();
    let g = name.split_whitespace().next().unwrap_or("");
    let sp = name.split_whitespace().nth(1).unwrap_or("");
    let s = match g {
        "acer" => {
            if sp == "saccharum" || sp == "platanoides" {
                Sp::SugarMaple
            } else {
                Sp::RedMaple
            }
        }
        "quercus" => match sp {
            "virginiana" | "fusiformis" => Sp::LiveOak,
            "agrifolia" | "wislizeni" | "chrysolepis" => Sp::CoastLiveOak,
            "alba" | "macrocarpa" | "bicolor" | "lobata" | "garryana" => Sp::WhiteOak,
            _ => Sp::RedOak,
        },
        "ulmus" | "zelkova" | "celtis" => Sp::Elm,
        "populus" => {
            if sp == "tremuloides" {
                Sp::Aspen
            } else {
                Sp::Cottonwood
            }
        }
        "platanus" => Sp::Sycamore,
        "betula" => Sp::Birch,
        "fraxinus" => Sp::Ash,
        "carya" => Sp::Pecan,
        "juglans" => Sp::Pecan,
        "liriodendron" => Sp::TulipPoplar,
        "liquidambar" => Sp::Sweetgum,
        "magnolia" => Sp::Magnolia,
        "lagerstroemia" => Sp::CrapeMyrtle,
        "cornus" | "cercis" | "prunus" | "malus" | "pyrus" => Sp::Dogwood,
        "prosopis" | "acacia" | "vachellia" => Sp::Mesquite,
        "parkinsonia" | "cercidium" => Sp::PaloVerde,
        "eucalyptus" | "corymbia" => Sp::Eucalyptus,
        "gleditsia" | "robinia" | "sophora" | "styphnolobium" => Sp::HoneyLocust,
        "salix" => Sp::Willow,
        "rhizophora" | "avicennia" | "laguncularia" => Sp::Mangrove,
        "olea" => Sp::Olive,
        "pinus" => match sp {
            "ponderosa" | "jeffreyi" | "resinosa" => Sp::PonderosaPine,
            "contorta" => Sp::LodgepolePine,
            "taeda" | "elliottii" | "palustris" | "echinata" => Sp::LoblollyPine,
            "strobus" => Sp::WhitePine,
            "edulis" | "monophylla" => Sp::Pinyon,
            _ => Sp::PonderosaPine,
        },
        "pseudotsuga" => Sp::DouglasFir,
        "thuja" | "calocedrus" | "chamaecyparis" => Sp::RedCedar,
        "picea" => Sp::Spruce,
        "abies" => Sp::Fir,
        "tsuga" => Sp::Hemlock,
        "juniperus" => Sp::Juniper,
        "sequoia" | "sequoiadendron" => Sp::Redwood,
        "taxodium" => Sp::BaldCypress,
        "cupressus" | "hesperocyparis" => Sp::ItalianCypress,
        "sabal" => Sp::SabalPalm,
        "cocos" => Sp::CoconutPalm,
        "roystonea" => Sp::RoyalPalm,
        "washingtonia" => Sp::WashingtoniaPalm,
        "syagrus" => Sp::QueenPalm,
        "phoenix" => Sp::DatePalm,
        "carnegiea" => Sp::Saguaro,
        "yucca" => Sp::JoshuaTree,
        "larrea" => Sp::Creosote,
        "artemisia" => Sp::Sagebrush,
        "serenoa" => Sp::SawPalmetto,
        "arecaceae" | "palm" => Sp::SabalPalm,
        _ => return None,
    };
    Some(s)
}

/// What the season is doing at a place: for deciduous trees and for grass.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Season {
    /// Leaves on deciduous trees, 0 (bare) .. 1 (full).
    pub leaves: f32,
    /// Fall colour, 0 (green) .. 1 (peak).
    pub fall: f32,
    /// Fresh spring green (lighter, yellower) 0..1.
    pub spring: f32,
    /// Grass dryness: 0 green .. 1 straw.
    pub dry: f32,
    /// Grass dormant in winter (tan) 0..1.
    pub dormant: f32,
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Season for a day of the year (1..366) at a place. A phenology model: green-up moves
/// north and uphill through spring at about 3.5 days per degree of latitude; fall
/// colour peaks from late September in the north to mid November on the Gulf; summer
/// drought browns grass in the West.
pub fn season(b: Biome, lat: f64, elev: f64, doy: f32) -> Season {
    let lat = lat as f32;
    let ek = (elev as f32 / 1000.0).max(0.0);
    let tropical = matches!(b, Biome::Tropical | Biome::SouthFlorida);
    // Spring leaf-out (day of year) and fall peak.
    let green_up = (55.0 + (lat - 25.0).max(0.0) * 3.6 + ek * 14.0).clamp(20.0, 175.0);
    let peak = (268.0 + (47.0 - lat) * 3.2 - ek * 15.0).clamp(240.0, 330.0);
    let (leaves, fall, spring) = if tropical {
        (1.0, 0.0, 0.0)
    } else {
        let on = smooth(green_up - 8.0, green_up + 14.0, doy);
        let off = 1.0 - smooth(peak + 4.0, peak + 22.0, doy);
        let fall = smooth(peak - 24.0, peak - 2.0, doy) * (1.0 - smooth(peak + 18.0, peak + 30.0, doy));
        let spring = smooth(green_up - 4.0, green_up + 6.0, doy) * (1.0 - smooth(green_up + 18.0, green_up + 40.0, doy));
        (on.min(off).max(0.0), fall, spring)
    };
    // Grass: Mediterranean summer-dry, the plains and deserts brown late summer, the
    // north goes dormant in winter.
    let dry = match b {
        Biome::California | Biome::Redwoods => smooth(120.0, 165.0, doy) * (1.0 - smooth(300.0, 345.0, doy)),
        Biome::Sonoran | Biome::Mojave | Biome::Chihuahuan | Biome::GreatBasin => 0.65 + 0.25 * smooth(140.0, 190.0, doy),
        Biome::Plains | Biome::HillCountry | Biome::SouthTexas | Biome::InlandNorthwest => {
            smooth(190.0, 240.0, doy) * 0.6 * (1.0 - smooth(300.0, 330.0, doy)) + 0.15
        }
        Biome::RockyMontane => smooth(200.0, 250.0, doy) * 0.5,
        _ => smooth(205.0, 240.0, doy) * 0.15,
    };
    let dormant = if tropical || matches!(b, Biome::GulfCoast) && lat < 29.5 {
        0.0
    } else {
        let start = (peak + 25.0).min(345.0);
        let end = green_up - 5.0;
        (smooth(start - 10.0, start + 20.0, doy)).max(1.0 - smooth(end - 20.0, end + 10.0, doy)) * if lat < 32.0 { 0.6 } else { 0.9 }
    };
    Season { leaves, fall, spring, dry, dormant }
}

/// Day of year (1..366) from unix seconds.
pub fn day_of_year(unix: f64) -> f32 {
    let days = (unix / 86_400.0).floor() as i64;
    // Civil from days (Hinnant), then days since Jan 1.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy_mar = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let y = yoe + era * 400;
    let mp = (5 * doy_mar + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    // Day of year from the March-based count.
    let doy = if doy_mar >= 306 { doy_mar - 306 + 1 } else { doy_mar + 59 + 1 + leap as i64 };
    doy as f32 + (unix / 86_400.0).fract() as f32
}

/// Leaf colour of a species in a season (and how much of the crown is there).
pub fn leaf_colour(df: &Def, s: &Season, var: f32) -> ([f32; 3], f32) {
    let mut c = df.green;
    if !df.evergreen {
        // Spring: fresh yellow-green.
        let fresh = [c[0] * 1.25 + 0.04, c[1] * 1.15 + 0.03, c[2] * 0.8];
        for i in 0..3 {
            c[i] += (fresh[i] - c[i]) * s.spring;
        }
        // Fall colour comes in tree by tree (var shifts each one).
        let f = (s.fall * (1.3 + var * 0.6) - var * 0.3).clamp(0.0, 1.0);
        for i in 0..3 {
            c[i] += (df.fall[i] - c[i]) * f;
        }
    }
    let amount = if df.evergreen { 1.0 } else { (s.leaves * (1.2 + var * 0.4) - var * 0.2).clamp(0.0, 1.0) };
    (c, amount)
}

/// Lawn and grass colour for a biome in a season (before tinting from imagery).
pub fn grass_colour(b: Biome, s: &Season) -> [f32; 3] {
    let green = match b {
        Biome::SouthFlorida | Biome::GulfCoast | Biome::Tropical => [0.2, 0.36, 0.11],
        Biome::PacificNorthwest | Biome::Redwoods => [0.22, 0.4, 0.13],
        Biome::Plains | Biome::HillCountry | Biome::SouthTexas => [0.3, 0.38, 0.14],
        Biome::Sonoran | Biome::Mojave | Biome::Chihuahuan | Biome::GreatBasin => [0.36, 0.38, 0.2],
        Biome::California => [0.28, 0.38, 0.13],
        _ => [0.22, 0.38, 0.12],
    };
    let straw = match b {
        Biome::California | Biome::Redwoods => [0.68, 0.56, 0.32],
        Biome::Sonoran | Biome::Mojave | Biome::Chihuahuan => [0.62, 0.55, 0.38],
        _ => [0.58, 0.52, 0.34],
    };
    let dormant = [0.5, 0.46, 0.3];
    let mut c = green;
    for i in 0..3 {
        c[i] += (straw[i] - c[i]) * s.dry;
        c[i] += (dormant[i] - c[i]) * s.dormant;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions() {
        assert_eq!(biome_at(25.78, -80.13, 2.0), Biome::SouthFlorida);
        assert_eq!(biome_at(29.42, -98.49, 200.0), Biome::HillCountry);
        assert_eq!(biome_at(39.74, -104.99, 1600.0), Biome::Plains);
        assert_eq!(biome_at(33.45, -112.07, 340.0), Biome::Sonoran);
        assert_eq!(biome_at(36.17, -115.14, 610.0), Biome::Mojave);
        assert_eq!(biome_at(34.05, -118.25, 90.0), Biome::California);
        assert_eq!(biome_at(47.6, -122.33, 50.0), Biome::PacificNorthwest);
        assert_eq!(biome_at(40.75, -73.99, 10.0), Biome::Northeast);
        assert_eq!(biome_at(44.5, -72.5, 400.0), Biome::NorthWoods);
        assert_eq!(biome_at(33.75, -84.39, 300.0), Biome::Southeast);
        assert_eq!(biome_at(30.27, -97.74, 150.0), Biome::HillCountry);
        assert_eq!(biome_at(29.95, -90.07, 2.0), Biome::GulfCoast);
        assert_eq!(biome_at(39.6, -105.9, 3400.0), Biome::Subalpine);
        assert_eq!(biome_at(37.8, -122.48, 50.0), Biome::Redwoods);
    }

    #[test]
    fn fall_in_vermont_october() {
        let s = season(Biome::NorthWoods, 44.5, 400.0, 280.0);
        assert!(s.fall > 0.5, "{s:?}");
        let jan = season(Biome::NorthWoods, 44.5, 400.0, 20.0);
        assert!(jan.leaves < 0.05);
        let jul = season(Biome::NorthWoods, 44.5, 400.0, 190.0);
        assert!(jul.leaves > 0.95 && jul.fall < 0.05);
        let miami = season(Biome::SouthFlorida, 25.8, 2.0, 20.0);
        assert_eq!(miami.leaves, 1.0);
        let la = season(Biome::California, 34.0, 100.0, 220.0);
        assert!(la.dry > 0.9);
    }

    #[test]
    fn doy() {
        let t = crate::sun::parse_utc("2026-10-21T12:00:00Z").unwrap();
        assert!((day_of_year(t) - 294.5).abs() < 0.01, "{}", day_of_year(t));
        let t = crate::sun::parse_utc("2024-03-01T00:00:00Z").unwrap();
        assert!((day_of_year(t) - 61.0).abs() < 0.01, "{}", day_of_year(t));
    }
}
