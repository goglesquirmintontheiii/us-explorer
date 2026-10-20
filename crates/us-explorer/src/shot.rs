//! Scripted screenshots for progress reports and CI: fly to each spot, wait until
//! terrain has finished streaming, capture, and exit when done.

use crate::globe::Viewer;
use crate::terrain::{Teleported, TerrainStats};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use std::time::Instant;
use usx_core::geo::Geodetic;

#[derive(Clone, Debug)]
pub struct ShotSpec {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub agl: f64,
    pub heading: f64,
    pub pitch: f64,
}

impl ShotSpec {
    /// `name=denver;lat=39.7;lon=-105;agl=300;heading=270;pitch=-10`
    pub fn parse(s: &str) -> Option<Self> {
        let mut sp = ShotSpec {
            name: "shot".into(),
            lat: f64::NAN,
            lon: f64::NAN,
            agl: 300.0,
            heading: 0.0,
            pitch: -10.0,
        };
        for kv in s.split(';') {
            let (k, v) = kv.split_once('=')?;
            match k.trim() {
                "name" => sp.name = v.trim().to_string(),
                "lat" => sp.lat = v.parse().ok()?,
                "lon" => sp.lon = v.parse().ok()?,
                "agl" => sp.agl = v.parse().ok()?,
                "heading" => sp.heading = v.parse().ok()?,
                "pitch" => sp.pitch = v.parse().ok()?,
                _ => {}
            }
        }
        (sp.lat.is_finite() && sp.lon.is_finite()).then_some(sp)
    }
}

#[derive(Resource)]
struct ShotRunner {
    queue: Vec<ShotSpec>,
    current: Option<(ShotSpec, Instant)>,
    captured_at: Option<u32>,
    frame: u32,
    dir: std::path::PathBuf,
}

pub struct ShotPlugin;

impl Plugin for ShotPlugin {
    fn build(&self, app: &mut App) {
        let args = app.world().resource::<crate::Args>().clone();
        if args.shots.is_empty() {
            return;
        }
        let dir = args
            .shot_dir
            .clone()
            .unwrap_or_else(|| "screenshots".into());
        let _ = std::fs::create_dir_all(&dir);
        let mut queue = args.shots.clone();
        queue.reverse();
        app.insert_resource(ShotRunner {
            queue,
            current: None,
            captured_at: None,
            frame: 0,
            dir,
        })
        .add_systems(Update, run);
    }
}

fn run(
    mut commands: Commands,
    mut r: ResMut<ShotRunner>,
    mut viewer: ResMut<Viewer>,
    stats: Res<TerrainStats>,
    mut tp: MessageWriter<Teleported>,
    mut exit: MessageWriter<AppExit>,
) {
    r.frame += 1;
    if r.current.is_none() {
        let Some(next) = r.queue.pop() else {
            if r.captured_at.is_none_or(|f| r.frame > f + 10) {
                exit.write(AppExit::Success);
            }
            return;
        };
        viewer.ecef = Geodetic::new(next.lat, next.lon, 3000.0).to_ecef();
        viewer.ground = None;
        tp.write(Teleported);
        r.current = Some((next, Instant::now()));
        r.captured_at = None;
        return;
    }
    let (spec, started) = r.current.clone().unwrap();
    // Hold the requested height above whatever ground has loaded so far.
    let mut g = viewer.geo();
    g.lat = spec.lat;
    g.lon = spec.lon;
    if let Some(ground) = viewer.ground {
        g.h = ground + spec.agl;
    }
    viewer.ecef = g.to_ecef();
    viewer.heading = spec.heading;
    viewer.pitch = spec.pitch;
    viewer.velocity = bevy::math::DVec3::ZERO;
    let t = started.elapsed().as_secs_f32();
    let settled = t > 6.0 && stats.idle_frames > 90;
    if r.captured_at.is_none() && (settled || t > 180.0) {
        let path = r.dir.join(format!("{}.png", spec.name));
        info!(
            "screenshot {} after {t:.1}s (settled: {settled}, deepest zoom {})",
            path.display(),
            stats.deepest_visible
        );
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        r.captured_at = Some(r.frame);
    }
    if r.captured_at.is_some_and(|f| r.frame > f + 8) {
        r.current = None;
    }
}
