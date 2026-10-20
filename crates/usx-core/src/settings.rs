//! User settings, stored as TOML in the platform config directory
//! (`~/.config/us-explorer/settings.toml` on Linux, `%APPDATA%` on Windows).

use crate::sources::ImagerySource;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Preset {
    Low,
    Medium,
    High,
    Ultra,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Low, Preset::Medium, Preset::High, Preset::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }
    /// Target size of one imagery texel on screen, in pixels. Bigger = coarser, cheaper.
    pub fn texel_pixels(self) -> f32 {
        match self {
            Self::Low => 2.6,
            Self::Medium => 1.8,
            Self::High => 1.25,
            Self::Ultra => 0.9,
        }
    }
    /// Radius of full-detail terrain around the player, metres.
    pub fn detail_radius_m(self) -> f64 {
        match self {
            Self::Low => 1_500.0,
            Self::Medium => 3_000.0,
            Self::High => 5_000.0,
            Self::Ultra => 8_000.0,
        }
    }
    pub fn max_tiles(self) -> usize {
        match self {
            Self::Low => 220,
            Self::Medium => 380,
            Self::High => 600,
            Self::Ultra => 900,
        }
    }
    pub fn shadows(self) -> bool {
        !matches!(self, Self::Low)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Units {
    Imperial,
    Metric,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Outfit {
    Casual,
    Rain,
    Winter,
    Hiker,
}

impl Outfit {
    pub const ALL: [Outfit; 4] = [Outfit::Casual, Outfit::Rain, Outfit::Winter, Outfit::Hiker];
    pub fn label(self) -> &'static str {
        match self {
            Self::Casual => "Casual",
            Self::Rain => "Rain jacket",
            Self::Winter => "Winter coat",
            Self::Hiker => "Hiking gear",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CameraView {
    FirstPerson,
    ThirdPerson,
    Orbit,
}

/// A saved place.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Height above the ground, metres.
    pub agl: f64,
    pub heading: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MapStyle {
    Streets,
    Imagery,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub preset: Preset,
    pub units: Units,
    pub imagery: ImagerySource,
    /// Use USGS 3DEP lidar elevation where available (more downloads, sharper ground).
    pub lidar_elevation: bool,
    pub cache_dir: Option<PathBuf>,
    pub cache_size_gb: f64,
    /// Total download cap for the game and its helper processes, megabits per second.
    pub bandwidth_mbps: f64,
    pub check_updates: bool,
    pub skipped_version: Option<String>,
    pub show_debug_overlay: bool,
    pub vsync: bool,
    pub fov_deg: f32,
    pub mouse_sensitivity: f32,
    /// Slow flight down while terrain is still streaming in.
    pub cap_speed_while_loading: bool,
    pub last_position: Option<crate::geo::Geodetic>,
    pub last_heading_deg: f64,
    /// Height above the ground at the last save, so a flight resumes in the air.
    pub last_agl: f64,
    /// Action name -> key name; missing actions use their default keys.
    pub key_bindings: std::collections::BTreeMap<String, String>,
    pub bookmarks: Vec<Bookmark>,
    pub recent_places: Vec<Bookmark>,
    pub outfit: Outfit,
    pub camera_view: CameraView,
    /// Touching the ground while flying switches back to walking.
    pub land_on_touch: bool,
    pub show_minimap: bool,
    pub minimap_rotates: bool,
    pub map_style: MapStyle,
    pub invert_look_y: bool,
    /// Roads, bridges and roadside objects from OpenStreetMap.
    pub roads: bool,
    /// Sun shadows from terrain and objects.
    pub shadows: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            preset: Preset::High,
            units: Units::Imperial,
            imagery: ImagerySource::Esri,
            roads: true,
            shadows: true,
            lidar_elevation: true,
            cache_dir: None,
            cache_size_gb: 50.0,
            bandwidth_mbps: 500.0,
            check_updates: true,
            skipped_version: None,
            show_debug_overlay: true,
            vsync: true,
            fov_deg: 70.0,
            mouse_sensitivity: 1.0,
            cap_speed_while_loading: false,
            last_position: None,
            last_heading_deg: 0.0,
            last_agl: 0.0,
            key_bindings: Default::default(),
            bookmarks: Vec::new(),
            recent_places: Vec::new(),
            outfit: Outfit::Casual,
            camera_view: CameraView::ThirdPerson,
            land_on_touch: true,
            show_minimap: true,
            minimap_rotates: true,
            map_style: MapStyle::Streets,
            invert_look_y: false,
        }
    }
}

fn dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("io.github", "goglesquirmintontheiii", "us-explorer")
}

pub fn config_path() -> PathBuf {
    dirs().map(|d| d.config_dir().join("settings.toml")).unwrap_or_else(|| PathBuf::from("settings.toml"))
}

pub fn default_cache_dir() -> PathBuf {
    dirs().map(|d| d.cache_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("cache"))
}

pub fn data_dir() -> PathBuf {
    dirs().map(|d| d.data_dir().to_path_buf()).unwrap_or_else(|| PathBuf::from("data"))
}

impl Settings {
    /// Remember a place in the recent list (newest first, no near-duplicates, max 12).
    pub fn push_recent(&mut self, b: Bookmark) {
        self.recent_places.retain(|r| (r.lat - b.lat).abs() > 0.001 || (r.lon - b.lon).abs() > 0.001);
        self.recent_places.insert(0, b);
        self.recent_places.truncate(12);
    }

    pub fn load() -> Self {
        std::fs::read_to_string(config_path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = config_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = toml::to_string_pretty(self) {
            let tmp = p.with_extension("toml.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(tmp, p);
            }
        }
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.cache_dir.clone().unwrap_or_else(default_cache_dir)
    }

    pub fn cache_bytes(&self) -> u64 {
        (self.cache_size_gb.max(1.0) * 1e9) as u64
    }
}
