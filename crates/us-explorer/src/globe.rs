//! Floating origin over a full-scale WGS84 globe.
//!
//! The [`Viewer`] holds the true position in f64 ECEF. Rendering happens in f32 metres
//! relative to [`WorldOrigin`], a local East-Up-South frame that jumps to the viewer
//! whenever it drifts more than [`REBASE_DISTANCE`] away, so precision never degrades.

use bevy::math::{DMat3, DQuat, DVec3};
use bevy::prelude::*;
use usx_core::geo::{EnuFrame, Geodetic};

pub const REBASE_DISTANCE: f64 = 2_000.0;

#[derive(Resource)]
pub struct WorldOrigin {
    pub frame: EnuFrame,
    pub geo: Geodetic,
    /// Bumped on every rebase so anchored entities know to move.
    pub generation: u64,
}

impl WorldOrigin {
    pub fn at(geo: Geodetic) -> Self {
        Self { frame: EnuFrame::at(geo), geo, generation: 1 }
    }
    pub fn local(&self, ecef: DVec3) -> Vec3 {
        self.frame.to_local(ecef).as_vec3()
    }
    pub fn rotation(&self) -> Quat {
        self.frame.rotation().as_quat()
    }
}

/// An entity whose mesh is built in ECEF axes around an ECEF anchor point.
#[derive(Component)]
pub struct GeoAnchor {
    pub ecef: DVec3,
    pub placed_generation: u64,
}

impl GeoAnchor {
    pub fn new(ecef: DVec3) -> Self {
        Self { ecef, placed_generation: 0 }
    }
}

/// The player's (or free camera's) true position and view direction.
#[derive(Resource)]
pub struct Viewer {
    pub ecef: DVec3,
    /// Degrees clockwise from true north.
    pub heading: f64,
    /// Degrees above the horizon.
    pub pitch: f64,
    /// ECEF velocity, m/s (used for prefetching).
    pub velocity: DVec3,
    /// Terrain height under the viewer when known (metres above sea level).
    pub ground: Option<f64>,
    /// After a teleport: stay this high above the ground until detailed terrain loads.
    pub hold_agl: Option<f64>,
}

impl Viewer {
    pub fn geo(&self) -> Geodetic {
        Geodetic::from_ecef(self.ecef)
    }
    pub fn agl(&self) -> Option<f64> {
        self.ground.map(|g| self.geo().h - g)
    }
    /// The view rotation in the viewer's own local frame (Y up, -Z north).
    pub fn local_rotation(&self) -> DQuat {
        DQuat::from_rotation_y(-self.heading.to_radians()) * DQuat::from_rotation_x(self.pitch.to_radians())
    }
    /// Basis vectors (forward, right, up) in ECEF.
    pub fn basis_ecef(&self) -> (DVec3, DVec3, DVec3) {
        let f = EnuFrame::at(self.geo());
        let to_ecef = f.ecef_to_local.transpose();
        let r = self.local_rotation();
        (to_ecef * (r * DVec3::NEG_Z), to_ecef * (r * DVec3::X), to_ecef * DVec3::Y)
    }
}

/// Origin rebasing and anchor placement; runs before cameras and lights are placed.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobeSet;

pub struct GlobePlugin;

impl Plugin for GlobePlugin {
    fn build(&self, app: &mut App) {
        let (args, settings) = (app.world().resource::<crate::Args>().clone(), app.world().resource::<crate::GameSettings>().0.clone());
        // Default start: Denver, looking west at the Front Range.
        let (lat, lon) = args.start.or(settings.last_position.map(|p| (p.lat, p.lon))).unwrap_or((39.7392, -104.9903));
        let h = if args.start.is_some() { 2000.0 } else { settings.last_position.map(|p| p.h).unwrap_or(2000.0) };
        let start = Geodetic::new(lat, lon, h);
        app.insert_resource(WorldOrigin::at(start))
            .insert_resource(Viewer {
                ecef: start.to_ecef(),
                heading: if args.start.is_some() { 0.0 } else { settings.last_heading_deg },
                pitch: -12.0,
                velocity: DVec3::ZERO,
                ground: None,
                hold_agl: (args.start.is_some() || settings.last_position.is_none()).then_some(args.start_agl.unwrap_or(600.0)),
            })
            .add_systems(PostUpdate, (rebase, place_anchors).chain().in_set(GlobeSet).before(TransformSystems::Propagate));
    }
}

/// Move the origin to the viewer when it has drifted too far.
fn rebase(viewer: Res<Viewer>, mut origin: ResMut<WorldOrigin>) {
    if origin.frame.to_local(viewer.ecef).length() > REBASE_DISTANCE {
        let g = viewer.geo();
        origin.frame = EnuFrame::at(g);
        origin.geo = g;
        origin.generation += 1;
    }
}

/// Re-place anchored meshes after a rebase (or when first spawned).
fn place_anchors(origin: Res<WorldOrigin>, mut q: Query<(&mut GeoAnchor, &mut Transform)>) {
    let rot = origin.rotation();
    for (mut a, mut t) in &mut q {
        if a.placed_generation != origin.generation {
            t.translation = origin.local(a.ecef);
            t.rotation = rot;
            a.placed_generation = origin.generation;
        }
    }
}

/// The viewer's rotation expressed in the origin frame.
pub fn viewer_rotation_in_origin(viewer: &Viewer, origin: &WorldOrigin) -> Quat {
    let vf = EnuFrame::at(viewer.geo());
    let m: DMat3 = origin.frame.ecef_to_local * vf.ecef_to_local.transpose();
    (DQuat::from_mat3(&m) * viewer.local_rotation()).as_quat()
}
