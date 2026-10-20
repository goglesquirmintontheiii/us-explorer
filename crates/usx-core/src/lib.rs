//! Engine-independent core of US Explorer: globe math, tile math, networking with a
//! bandwidth cap, the disk cache, settings, the update check and the geocoder.
//! Nothing in here depends on Bevy, so the weather service and tools can share it.

pub mod cache;
pub mod geo;
pub mod geocode;
pub mod net;
pub mod protocol;
pub mod settings;
pub mod sources;
pub mod tile;
pub mod update;

/// Re-exported so the game decodes images with the same build of the crate.
pub use image;

/// Version of this build, taken from Cargo (`0.N.0` is phase N).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// GitHub repository the game updates from.
pub const REPO: &str = "goglesquirmintontheiii/us-explorer";
/// User-Agent sent with every request, as tile and geocoding servers ask for.
pub fn user_agent() -> String {
    format!("USExplorer/{VERSION} (+https://github.com/{REPO})")
}
