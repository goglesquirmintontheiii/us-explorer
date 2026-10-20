//! US Explorer: walk, drive and fly the real United States under live weather.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod body;
mod camera;
mod controls;
mod globe;
mod map;
mod player;
mod shot;
mod sun;
mod terrain;
mod ui;
mod updater;
mod walkable;
mod weather_link;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};
use usx_core::settings::Settings;

/// Settings shared with every system; saved when changed through the menu.
#[derive(Resource, Clone)]
pub struct GameSettings(pub Settings);

/// Command-line options.
#[derive(Resource, Clone, Default)]
pub struct Args {
    pub start: Option<(f64, f64)>,
    pub start_agl: Option<f64>,
    pub shots: Vec<shot::ShotSpec>,
    pub shot_dir: Option<std::path::PathBuf>,
    pub time: Option<f64>,
    pub window: Option<(u32, u32)>,
    pub no_weather_service: bool,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--at" => a.start = it.next().and_then(|s| usx_core::geo::parse_lat_lon(&s)),
            "--agl" => a.start_agl = it.next().and_then(|s| s.parse().ok()),
            "--shot" => {
                if let Some(s) = it.next().and_then(|s| shot::ShotSpec::parse(&s)) {
                    a.shots.push(s)
                }
            }
            "--shot-dir" => a.shot_dir = it.next().map(Into::into),
            "--time" => a.time = it.next().and_then(|s| sun::parse_utc(&s)),
            "--window" => {
                a.window = it.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
            }
            "--no-weather-service" => a.no_weather_service = true,
            "--version" => {
                println!("US Explorer {}", usx_core::VERSION);
                std::process::exit(0);
            }
            "--help" | "-h" => {
                println!(
                    "US Explorer {}\n\n  --at LAT,LON        start at a place\n  --agl METRES        start height above ground\n  --time ISO8601      fixed UTC time (e.g. 2026-06-21T18:00:00Z)\n  --window WxH        window size\n  --shot SPEC         take a screenshot and exit; SPEC is name=..;lat=..;lon=..;agl=..;heading=..;pitch=..\n  --shot-dir DIR      where screenshots go",
                    usx_core::VERSION
                );
                std::process::exit(0);
            }
            _ => {}
        }
    }
    a
}

fn main() {
    // The same executable doubles as the weather service.
    if std::env::args().nth(1).as_deref() == Some("--weather-service") {
        usx_weather::run(std::env::args().skip(2));
        return;
    }
    usx_core::update::cleanup_old();
    let args = parse_args();
    let settings = Settings::load();
    usx_core::net::limiter().set_mbps(settings.bandwidth_mbps * 0.9);
    let (w, h) = args.window.unwrap_or((1600, 900));

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("US Explorer {}", usx_core::VERSION),
                    resolution: WindowResolution::new(w, h),
                    present_mode: if settings.vsync && args.shots.is_empty() { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync },
                    ..default()
                }),
                ..default()
            })
            .set(bevy::log::LogPlugin { filter: "wgpu=warn,naga=warn,bevy_render=info,us_explorer=info".into(), ..default() }),
    )
    .insert_resource(GameSettings(settings))
    .insert_resource(args)
    .add_plugins((
        bevy_egui::EguiPlugin::default(),
        bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
        globe::GlobePlugin,
        sun::SunPlugin,
        camera::CameraPlugin,
        controls::ControlsPlugin,
        walkable::WalkablePlugin,
        player::PlayerPlugin,
        body::BodyPlugin,
        map::MapPlugin,
        terrain::TerrainPlugin,
        weather_link::WeatherLinkPlugin,
        updater::UpdaterPlugin,
        ui::UiPlugin,
        shot::ShotPlugin,
    ));
    app.run();
}
