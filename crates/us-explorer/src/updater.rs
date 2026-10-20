//! Checks GitHub for a newer release and installs it in one click.

use bevy::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use usx_core::update::{self, Release};

pub enum UpdateState {
    Idle,
    Checking(crossbeam_channel::Receiver<Result<Option<Release>, String>>),
    UpToDate,
    Available(Release),
    Downloading {
        rel: Release,
        done: Arc<AtomicU64>,
        total: Arc<AtomicU64>,
        rx: crossbeam_channel::Receiver<Result<std::path::PathBuf, String>>,
    },
    Failed(String),
}

#[derive(Resource)]
pub struct Updater {
    pub state: UpdateState,
    /// The popup is hidden after "Later" until the next check finds something.
    pub dismissed: bool,
    last_check: Option<Instant>,
}

impl Updater {
    pub fn check_now(&mut self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(update::check().map_err(|e| e.to_string()));
        });
        self.state = UpdateState::Checking(rx);
        self.last_check = Some(Instant::now());
    }

    pub fn install(&mut self) {
        let UpdateState::Available(rel) = &self.state else {
            return;
        };
        let rel = rel.clone();
        let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(rel.appimage_size)));
        let (d2, t2, r2) = (done.clone(), total.clone(), rel.clone());
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let r = update::install(&r2, |n, len| {
                d2.store(n, Ordering::Relaxed);
                if let Some(l) = len {
                    t2.store(l, Ordering::Relaxed);
                }
            });
            let _ = tx.send(r);
        });
        self.state = UpdateState::Downloading { rel, done, total, rx };
    }
}

pub struct UpdaterPlugin;

impl Plugin for UpdaterPlugin {
    fn build(&self, app: &mut App) {
        let enabled =
            app.world().resource::<crate::GameSettings>().0.check_updates && app.world().resource::<crate::Args>().shots.is_empty();
        let mut u = Updater { state: UpdateState::Idle, dismissed: false, last_check: None };
        if enabled {
            u.check_now();
        }
        app.insert_resource(u).add_systems(Update, poll);
    }
}

fn poll(mut u: ResMut<Updater>, settings: Res<crate::GameSettings>) {
    let next = match &u.state {
        UpdateState::Checking(rx) => rx.try_recv().ok().map(|r| match r {
            Ok(Some(rel)) => {
                let skipped = settings.0.skipped_version.as_deref() == Some(rel.version.to_string().as_str());
                if skipped { UpdateState::UpToDate } else { UpdateState::Available(rel) }
            }
            Ok(None) => UpdateState::UpToDate,
            Err(e) => UpdateState::Failed(format!("Couldn't check for updates: {e}")),
        }),
        UpdateState::Downloading { rx, .. } => rx.try_recv().ok().map(|r| match r {
            Ok(path) => update::relaunch(&path),
            Err(e) => UpdateState::Failed(e),
        }),
        _ => None,
    };
    if let Some(n) = next {
        if matches!(n, UpdateState::Available(_)) {
            u.dismissed = false;
        }
        u.state = n;
    }
    // Look again every six hours during long sessions.
    if settings.0.check_updates
        && matches!(u.state, UpdateState::UpToDate | UpdateState::Failed(_))
        && u.last_check.is_some_and(|t| t.elapsed() > Duration::from_secs(6 * 3600))
    {
        u.check_now();
    }
}
