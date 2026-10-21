//! Every external data source the game uses, with URL templates and attribution.
//! The credits screen is generated from [`CREDITS`], so adding a source here is the
//! only way to use it.

use crate::tile::TileId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ImagerySource {
    /// Esri World Imagery: ~0.5 m across the US, 0.3 m in many metros.
    Esri,
    /// USGS National Map "Imagery Only" (NAIP-based), public domain.
    Usgs,
}

impl ImagerySource {
    pub fn key(self) -> &'static str {
        match self {
            Self::Esri => "esri",
            Self::Usgs => "usgs",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Esri => "Esri World Imagery",
            Self::Usgs => "USGS / NAIP imagery",
        }
    }
    pub fn max_zoom(self) -> u8 {
        match self {
            Self::Esri => 19,
            Self::Usgs => 16,
        }
    }
    pub fn url(self, t: TileId) -> String {
        match self {
            Self::Esri => {
                format!("https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{}/{}/{}", t.z, t.y, t.x)
            }
            Self::Usgs => {
                format!("https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/tile/{}/{}/{}", t.z, t.y, t.x)
            }
        }
    }
}

/// Terrarium-encoded elevation tiles on AWS Open Data (USGS 3DEP over the US,
/// SRTM/ETOPO elsewhere, bathymetry offshore). Height = R*256 + G + B/256 - 32768.
pub const TERRARIUM_MAX_ZOOM: u8 = 15;
pub fn terrarium_url(t: TileId) -> String {
    format!("https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{}/{}/{}.png", t.z, t.x, t.y)
}

/// USGS 3DEP dynamic elevation service; serves the best available DEM (down to 1 m
/// lidar) for any box. Used for zooms beyond the terrarium pyramid.
pub fn usgs_3dep_url(t: TileId, size: u32) -> String {
    let (w, s, e, n) = merc_bounds_m(t);
    format!(
        "https://elevation.nationalmap.gov/arcgis/rest/services/3DEPElevation/ImageServer/exportImage?bbox={w},{s},{e},{n}&bboxSR=3857&imageSR=3857&size={size},{size}&format=tiff&pixelType=F32&interpolation=RSP_BilinearInterpolation&noData=-9999&f=image"
    )
}

/// A tile's bounds in EPSG:3857 metres (west, south, east, north).
pub fn merc_bounds_m(t: TileId) -> (f64, f64, f64, f64) {
    const HALF: f64 = 20_037_508.342_789_244;
    let size = 2.0 * HALF / (1u64 << t.z) as f64;
    let w = -HALF + t.x as f64 * size;
    let n = HALF - t.y as f64 * size;
    (w, n - size, w + size, n)
}

/// Esri World Street Map, used for the minimap and full map.
pub fn street_map_url(t: TileId) -> String {
    format!("https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/tile/{}/{}/{}", t.z, t.y, t.x)
}

pub const NOMINATIM: &str = "https://nominatim.openstreetmap.org/search";
pub const GITHUB_API: &str = "https://api.github.com";

pub struct Credit {
    pub name: &'static str,
    pub used_for: &'static str,
    pub attribution: &'static str,
    pub url: &'static str,
}

pub const CREDITS: &[Credit] = &[
    Credit {
        name: "Esri World Imagery",
        used_for: "Ground imagery (default)",
        attribution: "Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community",
        url: "https://www.arcgis.com/home/item.html?id=10df2279f9684e4a9f6a7f08febac2a9",
    },
    Credit {
        name: "Esri World Street Map",
        used_for: "Minimap and full map",
        attribution: "Sources: Esri, HERE, Garmin, USGS, Intermap, INCREMENT P, NRCan, Esri Japan, METI, Esri China (Hong Kong), Esri Korea, Esri (Thailand), NGCC, (c) OpenStreetMap contributors, and the GIS User Community",
        url: "https://www.arcgis.com/home/item.html?id=3b93337983e9436f8db950e38a8629af",
    },
    Credit {
        name: "USGS The National Map: Imagery Only",
        used_for: "Ground imagery (alternative)",
        attribution: "USGS The National Map: Orthoimagery. Data refreshed regularly; public domain.",
        url: "https://basemap.nationalmap.gov/",
    },
    Credit {
        name: "Terrain Tiles on AWS (Tilezen / Mapzen)",
        used_for: "Elevation",
        attribution: "3DEP, SRTM, GMTED2010, ETOPO1 and other sources, compiled by Mapzen / Tilezen",
        url: "https://registry.opendata.aws/terrain-tiles/",
    },
    Credit {
        name: "USGS 3D Elevation Program (3DEP)",
        used_for: "High-resolution elevation (lidar)",
        attribution: "U.S. Geological Survey, 3D Elevation Program; public domain",
        url: "https://www.usgs.gov/3d-elevation-program",
    },
    Credit {
        name: "OpenStreetMap",
        used_for: "Roads, bridges, railways, signs and roadside objects, buildings, trees, parks, fences, power lines, wind turbines, buoys and piers; place search (Nominatim)",
        attribution: "© OpenStreetMap contributors, ODbL",
        url: "https://www.openstreetmap.org/copyright",
    },
    Credit {
        name: "Overpass API",
        used_for: "Fetching OpenStreetMap data (overpass-api.de, private.coffee, kumi.systems)",
        attribution: "Overpass API by Roland Olbricht and the public instance operators",
        url: "https://wiki.openstreetmap.org/wiki/Overpass_API",
    },
    Credit {
        name: "Open-Meteo",
        used_for: "Wind, cloud cover, temperature, time zone and the sea state (waves and swell)",
        attribution: "Weather data by Open-Meteo.com, CC BY 4.0 (from national weather services incl. NOAA and DWD)",
        url: "https://open-meteo.com/",
    },
    Credit {
        name: "NOAA National Data Buoy Center",
        used_for: "Wave height, period and direction from the nearest buoy",
        attribution: "NOAA National Data Buoy Center; public domain",
        url: "https://www.ndbc.noaa.gov/",
    },
    Credit {
        name: "NOAA Space Weather Prediction Center",
        used_for: "Aurora forecast (OVATION) and planetary K index",
        attribution: "NOAA SWPC; public domain",
        url: "https://www.swpc.noaa.gov/",
    },
    Credit {
        name: "NASA GIBS: MODIS NDVI and VIIRS Black Marble",
        used_for: "Seasons (vegetation greenness) and light pollution in the night sky",
        attribution: "We acknowledge the use of imagery provided by services from NASA's Global Imagery Browse Services (GIBS), part of NASA's Earth Science Data and Information System (ESDIS).",
        url: "https://earthdata.nasa.gov/gibs",
    },
    Credit {
        name: "Yale Bright Star Catalogue",
        used_for: "The 7,000 stars in the night sky",
        attribution: "Hoffleit & Warren, Yale University Observatory (5th revised ed.), via brettonw/YaleBrightStarCatalog",
        url: "http://tdc-www.harvard.edu/catalogs/bsc5.html",
    },
    Credit {
        name: "JPL approximate planetary positions",
        used_for: "Where the planets are",
        attribution: "E. M. Standish, JPL Solar System Dynamics; public domain",
        url: "https://ssd.jpl.nasa.gov/planets/approx_pos.html",
    },
    Credit {
        name: "DejaVu Fonts",
        used_for: "Lettering on road signs",
        attribution: "DejaVu fonts, Bitstream Vera license / public domain changes",
        url: "https://dejavu-fonts.github.io/",
    },
    Credit { name: "Bevy Engine", used_for: "Game engine", attribution: "Bevy contributors, MIT / Apache-2.0", url: "https://bevy.org" },
    Credit {
        name: "egui",
        used_for: "Menus and overlays",
        attribution: "Emil Ernerfeldt and contributors, MIT / Apache-2.0",
        url: "https://github.com/emilk/egui",
    },
];
