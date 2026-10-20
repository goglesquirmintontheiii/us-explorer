//! Offline region download: pre-fill the disk cache for an area so it plays with no
//! network (useful on the laptop). Runs at low priority on its own small pool.

use super::load;
use bevy::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use usx_core::net::WorkPool;
use usx_core::sources::{self, ImagerySource, TERRARIUM_MAX_ZOOM};
use usx_core::tile::{TileId, tiles_in_box};

pub struct RegionJob {
    pub id: TileId,
    pub imagery: ImagerySource,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct RegionProgress {
    pub done: AtomicU64,
    pub failed: AtomicU64,
    pub bytes_before: AtomicU64,
}

#[derive(Resource, Default)]
pub struct RegionDownload {
    pool: Option<WorkPool<RegionJob, ()>>,
    pub total: u64,
    pub progress: Arc<RegionProgress>,
    pub cancel: Arc<AtomicBool>,
    pub label: String,
}

impl RegionDownload {
    /// Tiles needed for a circle of `radius_km` around a point, down to `max_zoom`.
    pub fn plan(lat: f64, lon: f64, radius_km: f64, max_zoom: u8) -> Vec<TileId> {
        let dlat = radius_km / 111.0;
        let dlon = radius_km / (111.0 * lat.to_radians().cos().max(0.05));
        let mut v = Vec::new();
        for z in 0..=max_zoom {
            v.extend(tiles_in_box(
                lat - dlat,
                lon - dlon,
                lat + dlat,
                lon + dlon,
                z,
            ));
        }
        v
    }

    /// Rough download size: ~22 KB imagery + ~90 KB elevation (shallow zooms only).
    pub fn estimate_mb(tiles: &[TileId]) -> f64 {
        tiles
            .iter()
            .map(|t| {
                if t.z <= TERRARIUM_MAX_ZOOM {
                    0.112
                } else {
                    0.022
                }
            })
            .sum()
    }

    pub fn start(&mut self, tiles: Vec<TileId>, imagery: ImagerySource, label: String) {
        self.cancel.store(true, Ordering::Relaxed);
        let progress = Arc::new(RegionProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let p2 = progress.clone();
        let pool = WorkPool::new("region", 4, move |_, job: RegionJob| {
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let t = job.id;
            let mut ok = load::fetch(
                &format!("img/{}/{}.jpg", job.imagery.key(), t.path()),
                &job.imagery.url(t),
            )
            .is_ok();
            if t.z <= TERRARIUM_MAX_ZOOM {
                ok &= load::fetch(
                    &format!("dem/terrarium/{}.png", t.path()),
                    &sources::terrarium_url(t),
                )
                .is_ok();
            }
            if ok {
                p2.done.fetch_add(1, Ordering::Relaxed);
            } else {
                p2.failed.fetch_add(1, Ordering::Relaxed);
            }
        });
        self.total = tiles.len() as u64;
        for (i, t) in tiles.into_iter().enumerate() {
            // Shallow zooms first so a half-finished download is still useful.
            pool.submit(
                i as u64,
                t.z as f32,
                RegionJob {
                    id: t,
                    imagery,
                    cancel: cancel.clone(),
                },
            );
        }
        self.pool = Some(pool);
        self.progress = progress;
        self.cancel = cancel;
        self.label = label;
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.pool = None;
        self.total = 0;
    }

    pub fn active(&self) -> bool {
        self.pool.is_some()
    }

    pub fn finished(&self) -> u64 {
        self.progress.done.load(Ordering::Relaxed) + self.progress.failed.load(Ordering::Relaxed)
    }
}

pub struct RegionPlugin;

impl Plugin for RegionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RegionDownload>()
            .add_systems(Update, drain);
    }
}

fn drain(mut r: ResMut<RegionDownload>) {
    if let Some(p) = &r.pool {
        while p.try_recv().is_some() {}
        if r.finished() >= r.total {
            r.pool = None;
        }
    }
}
