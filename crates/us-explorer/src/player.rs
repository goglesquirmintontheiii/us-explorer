//! The player: walking with gravity, Minecraft creative-style flight (double-tap Jump),
//! no-clip, and the three camera views. The camera's position is written to
//! [`Viewer`], which everything else (terrain streaming, the overlay) follows.

use crate::camera::{InputBlocked, MouseLock};
use crate::controls::Intent;
use crate::globe::Viewer;
use crate::map::TeleportTo;
use crate::terrain::Teleported;
use crate::terrain::Terrain;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use usx_core::geo::{EnuFrame, Geodetic};
use usx_core::settings::CameraView;

pub const EYE_HEIGHT: f64 = 1.62;
pub const WALK_SPEED: f64 = 1.9;
pub const SPRINT_SPEED: f64 = 6.0;
pub const JUMP_SPEED: f64 = 3.8;
pub const GRAVITY: f64 = 9.81;
/// Two taps of Jump within this many seconds toggle flight.
pub const DOUBLE_TAP: f64 = 0.3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Walk,
    Fly,
    Noclip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pose {
    #[default]
    Idle,
    Walk,
    Run,
    Jump,
    Fall,
    Hover,
    Fly,
    Swim,
}

#[derive(Resource)]
pub struct Player {
    /// Feet position, ECEF.
    pub feet: DVec3,
    /// Look direction: degrees clockwise from north, and above the horizon.
    pub heading: f64,
    pub pitch: f64,
    /// Body facing (turns towards the movement direction in third person).
    pub body_heading: f64,
    /// Velocity in the local frame at the feet: x east, y up, z south.
    pub vel: DVec3,
    pub mode: Mode,
    pub on_ground: bool,
    /// Swimming in the sea (terrain below sea level and body under the surface).
    pub in_water: bool,
    pub fly_speed: f64,
    pub view: CameraView,
    pub orbit_dist: f64,
    pub orbit_yaw: f64,
    pub orbit_pitch: f64,
    pub pose: Pose,
    /// Walk cycle phase in radians.
    pub gait: f32,
    /// After a teleport: hold this height above ground until detailed terrain loads.
    pub hold_agl: Option<f64>,
    pub ground: Option<f64>,
    last_jump_tap: f64,
    /// Seconds spent off the ground.
    pub air_time: f32,
}

impl Player {
    pub fn geo(&self) -> Geodetic {
        Geodetic::from_ecef(self.feet)
    }

    /// Put the player somewhere new, flying, `agl` metres above the ground.
    pub fn teleport(&mut self, lat: f64, lon: f64, agl: f64, ground_guess: Option<f64>) {
        self.feet = Geodetic::new(lat, lon, ground_guess.unwrap_or(0.0) + agl).to_ecef();
        self.vel = DVec3::ZERO;
        self.mode = if agl > 3.0 { Mode::Fly } else { Mode::Walk };
        self.hold_agl = Some(agl);
        self.ground = None;
    }

    pub fn agl(&self) -> Option<f64> {
        self.ground.map(|g| self.geo().h - g)
    }

    pub fn horizontal_speed(&self) -> f64 {
        DVec3::new(self.vel.x, 0.0, self.vel.z).length()
    }
}

/// Rotation for a heading/pitch in a local frame (Y up, -Z north).
pub fn look_rotation(heading: f64, pitch: f64) -> DQuat {
    DQuat::from_rotation_y(-heading.to_radians()) * DQuat::from_rotation_x(pitch.to_radians())
}

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        let viewer = app.world().resource::<Viewer>();
        let s = &app.world().resource::<crate::GameSettings>().0;
        let mut feet = viewer.geo();
        let fresh = viewer.hold_agl.is_some();
        // Resuming: stand (or hover) at the saved height above the ground once it loads.
        let hold = viewer.hold_agl.or(Some(s.last_agl));
        if !fresh {
            feet.h -= EYE_HEIGHT;
        }
        let p = Player {
            feet: feet.to_ecef(),
            heading: viewer.heading,
            pitch: viewer.pitch,
            body_heading: viewer.heading,
            vel: DVec3::ZERO,
            mode: if fresh || s.last_agl > 3.0 { Mode::Fly } else { Mode::Walk },
            on_ground: false,
            in_water: false,
            fly_speed: 20.0,
            view: s.camera_view,
            orbit_dist: 6.0,
            orbit_yaw: 200.0,
            orbit_pitch: -15.0,
            pose: Pose::Idle,
            gait: 0.0,
            hold_agl: hold,
            ground: None,
            last_jump_tap: -10.0,
            air_time: 0.0,
        };
        app.insert_resource(p)
            .add_systems(Update, (toggle_lock, handle_teleports, look, simulate, choose_pose).chain().after(crate::ui::UiInput))
            .add_systems(PostUpdate, place_camera.before(crate::globe::GlobeSet));
    }
}

fn handle_teleports(
    mut ev: MessageReader<TeleportTo>,
    terrain: Res<Terrain>,
    mut p: ResMut<Player>,
    mut tp: MessageWriter<Teleported>,
    mut settings: ResMut<crate::GameSettings>,
) {
    for t in ev.read() {
        p.teleport(t.lat, t.lon, t.agl.max(0.0), terrain.height_at(t.lat, t.lon));
        if let Some(h) = t.heading {
            p.heading = h;
        }
        p.pitch = if t.agl > 50.0 { -15.0 } else { 0.0 };
        tp.write(Teleported);
        settings.0.push_recent(usx_core::settings::Bookmark {
            name: format!("{:.4}, {:.4}", t.lat, t.lon),
            lat: t.lat,
            lon: t.lon,
            agl: t.agl,
            heading: p.heading,
        });
        settings.0.save();
    }
}

fn toggle_lock(
    intent: Res<Intent>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    blocked: Res<InputBlocked>,
    menus: Res<crate::ui::Menus>,
    mut lock: ResMut<MouseLock>,
    mut cursor: Query<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
) {
    if intent.lock_mouse {
        lock.0 = !lock.0;
    }
    // Clicking into the world grabs the mouse, like most games.
    if mouse.just_pressed(MouseButton::Left) && !blocked.pointer && !menus.any_open() {
        lock.0 = true;
    }
    if keys.just_pressed(KeyCode::Escape) || menus.any_open() {
        lock.0 = false;
    }
    let want = lock.0 || (mouse.pressed(MouseButton::Right) && !blocked.pointer);
    for mut c in &mut cursor {
        let mode = if want { bevy::window::CursorGrabMode::Locked } else { bevy::window::CursorGrabMode::None };
        if c.grab_mode != mode {
            c.grab_mode = mode;
            c.visible = !want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn look(
    intent: Res<Intent>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mouse: Res<ButtonInput<MouseButton>>,
    lock: Res<MouseLock>,
    blocked: Res<InputBlocked>,
    mut s: ResMut<crate::GameSettings>,
    mut p: ResMut<Player>,
) {
    if s.0.camera_view != p.view {
        p.view = s.0.camera_view;
    }
    let looking = lock.0 || (mouse.pressed(MouseButton::Right) && !blocked.pointer);
    let sens = 0.08 * s.0.mouse_sensitivity as f64;
    let inv = if s.0.invert_look_y { -1.0 } else { 1.0 };
    let mut d = intent.look.as_dvec2();
    if looking {
        d += motion.delta.as_dvec2() * DVec2::new(sens, -sens * inv);
    }
    if p.view == CameraView::Orbit {
        p.orbit_yaw = (p.orbit_yaw + d.x).rem_euclid(360.0);
        p.orbit_pitch = (p.orbit_pitch + d.y).clamp(-85.0, 60.0);
    } else {
        p.heading = (p.heading + d.x).rem_euclid(360.0);
        p.pitch = (p.pitch + d.y).clamp(-89.0, 89.0);
    }
    if !blocked.pointer && scroll.delta.y != 0.0 {
        let k = 1.25f64.powf(scroll.delta.y as f64);
        match (p.view, p.mode) {
            (CameraView::Orbit, _) => p.orbit_dist = (p.orbit_dist / k).clamp(2.0, 400.0),
            (_, Mode::Fly | Mode::Noclip) => p.fly_speed = (p.fly_speed * k).clamp(WALK_SPEED, 278.0),
            _ => {}
        }
    }
    if intent.camera_view {
        p.view = match p.view {
            CameraView::FirstPerson => CameraView::ThirdPerson,
            CameraView::ThirdPerson => CameraView::Orbit,
            CameraView::Orbit => CameraView::FirstPerson,
        };
        if p.view == CameraView::Orbit {
            p.orbit_yaw = (p.heading + 180.0).rem_euclid(360.0);
        }
        s.0.camera_view = p.view;
        s.0.save();
    }
}

use bevy::math::DVec2;

/// Ground under the feet: the terrain, plus any walkable surface (bridge decks from
/// Phase 3) within a step's reach.
const SEA_LEVEL: f64 = 0.0;
const SWIM_SPEED: f64 = 1.2;
const SWIM_SPRINT: f64 = 2.2;

pub fn ground_height(terrain: &Terrain, walk: &crate::walkable::Walkables, g: Geodetic) -> Option<(f64, u8)> {
    let t = terrain.floor_and_zoom(g.lat, g.lon);
    let w = walk.surface_below(g.lat, g.lon, g.h + 0.6);
    match (t, w) {
        (Some((th, z)), Some(wh)) => Some((th.max(wh), z)),
        (t, None) => t,
        (None, Some(wh)) => Some((wh, 20)),
    }
}

#[allow(clippy::too_many_arguments)]
fn simulate(
    time: Res<Time>,
    intent: Res<Intent>,
    terrain: Res<Terrain>,
    walk: Res<crate::walkable::Walkables>,
    settings: Res<crate::GameSettings>,
    stats: Res<crate::terrain::TerrainStats>,
    mut p: ResMut<Player>,
) {
    let dt = time.delta_secs_f64().min(0.1);
    let now = time.elapsed_secs_f64();
    let mut g = p.geo();
    let frame = EnuFrame::at(g);
    let ground = ground_height(&terrain, &walk, g);
    p.ground = ground.map(|x| x.0);

    // Hold a fixed height over the ground until detailed terrain has arrived.
    if let (Some(agl), Some((h, z))) = (p.hold_agl, ground) {
        g.h = h + agl;
        p.feet = g.to_ecef();
        let standing = agl < 0.5;
        p.on_ground = standing;
        p.air_time = 0.0;
        if standing {
            p.mode = Mode::Walk;
        }
        p.vel = DVec3::ZERO;
        if z >= 14 {
            p.hold_agl = None;
        }
        return;
    }

    if intent.noclip {
        p.mode = if p.mode == Mode::Noclip { Mode::Fly } else { Mode::Noclip };
    }
    if intent.jump_pressed {
        if now - p.last_jump_tap < DOUBLE_TAP && p.mode != Mode::Noclip {
            p.mode = if p.mode == Mode::Fly { Mode::Walk } else { Mode::Fly };
            p.last_jump_tap = -10.0;
        } else {
            p.last_jump_tap = now;
        }
    }

    let yaw = DQuat::from_rotation_y(-p.heading.to_radians());
    let fwd = yaw * DVec3::NEG_Z;
    let right = yaw * DVec3::X;
    let wish = (fwd * intent.move_axis.y as f64 + right * intent.move_axis.x as f64).clamp_length_max(1.0);

    // The sea surface is height 0; terrarium tiles carry bathymetry below it.
    p.in_water = p.mode == Mode::Walk && ground.is_some_and(|(h, _)| h < SEA_LEVEL - 1.2) && g.h < SEA_LEVEL - 0.9;
    match p.mode {
        Mode::Walk if p.in_water => {
            let speed = if intent.sprint { SWIM_SPRINT } else { SWIM_SPEED };
            let target = wish * speed;
            let k = (3.0 * dt).min(1.0);
            p.vel.x += (target.x - p.vel.x) * k;
            p.vel.z += (target.z - p.vel.z) * k;
            // Float with the head out; Space swims up, Shift dives.
            let float_h = SEA_LEVEL - 1.35;
            let ty = ((float_h - g.h) * 1.5).clamp(-1.5, 1.5) + intent.up as f64 * 1.6;
            p.vel.y += (ty - p.vel.y) * (2.5 * dt).min(1.0);
            p.on_ground = false;
            p.air_time = 0.0;
        }
        Mode::Walk => {
            let speed = if intent.sprint { SPRINT_SPEED } else { WALK_SPEED };
            let target = wish * speed;
            let accel = if p.on_ground { 12.0 } else { 2.5 };
            let k = (accel * dt).min(1.0);
            p.vel.x += (target.x - p.vel.x) * k;
            p.vel.z += (target.z - p.vel.z) * k;
            if p.on_ground && intent.jump_pressed {
                p.vel.y = JUMP_SPEED;
                p.on_ground = false;
            }
            if ground.is_some() {
                p.vel.y -= GRAVITY * dt;
            } else {
                // Don't fall through ground that hasn't loaded yet.
                p.vel.y = p.vel.y.max(0.0);
            }
        }
        Mode::Fly | Mode::Noclip => {
            let mut speed = p.fly_speed * if intent.sprint { 4.0 } else { 1.0 };
            // Faster when high up, so crossing the country doesn't take all day.
            if let Some(agl) = p.agl() {
                speed *= (agl / 1500.0).clamp(1.0, 40.0);
            }
            if settings.0.cap_speed_while_loading && stats.pending > 40 {
                speed = speed.min(250.0);
            }
            let target = DVec3::new(wish.x, intent.up as f64, wish.z) * speed;
            // Near-instant response: flight stops when the keys are released.
            let k = (18.0 * dt).min(1.0);
            let v = p.vel;
            p.vel = v + (target - v) * k;
        }
    }

    // Integrate in the local frame, then back to ECEF.
    let local_move = DVec3::new(p.vel.x, p.vel.y, p.vel.z) * dt;
    p.feet = frame.to_ecef(local_move);
    let mut g2 = p.geo();
    if let Some((h, _)) = ground_height(&terrain, &walk, g2) {
        p.ground = Some(h);
        if p.mode != Mode::Noclip && g2.h <= h {
            g2.h = h;
            p.feet = g2.to_ecef();
            if p.vel.y < 0.0 {
                p.vel.y = 0.0;
            }
            if p.mode == Mode::Fly && settings.0.land_on_touch && intent.up < 0.0 {
                p.mode = Mode::Walk;
            }
            p.on_ground = p.mode == Mode::Walk;
        } else if p.mode == Mode::Walk {
            // Stick to the ground when walking down slopes instead of hopping off them.
            let snap = 0.35 + p.horizontal_speed() * dt * 1.5;
            if p.on_ground && p.vel.y <= 0.0 && g2.h - h < snap {
                g2.h = h;
                p.feet = g2.to_ecef();
                p.vel.y = 0.0;
            } else {
                p.on_ground = false;
            }
        } else {
            p.on_ground = false;
        }
    }
    if p.on_ground {
        p.air_time = 0.0;
    } else {
        p.air_time += dt as f32;
    }

    // In third person the body turns to face where it walks; in first person it faces the view.
    let hs = p.horizontal_speed();
    if p.view == CameraView::FirstPerson || p.mode != Mode::Walk && hs < 0.5 {
        p.body_heading = p.heading;
    } else if hs > 0.3 {
        let target = (-p.vel.x).atan2(p.vel.z).to_degrees() + 180.0;
        let target = (target + 360.0) % 360.0;
        let diff = ((target - p.body_heading + 540.0) % 360.0) - 180.0;
        p.body_heading = (p.body_heading + diff * (10.0 * dt).min(1.0)).rem_euclid(360.0);
    }
}

fn choose_pose(time: Res<Time>, mut p: ResMut<Player>) {
    let hs = p.horizontal_speed();
    p.pose = match p.mode {
        Mode::Walk if p.in_water => Pose::Swim,
        Mode::Fly | Mode::Noclip => {
            if hs > 3.0 {
                Pose::Fly
            } else {
                Pose::Hover
            }
        }
        Mode::Walk if !p.on_ground && p.air_time > 0.08 => {
            if p.vel.y > 0.0 {
                Pose::Jump
            } else {
                Pose::Fall
            }
        }
        Mode::Walk if hs > 3.2 => Pose::Run,
        Mode::Walk if hs > 0.25 => Pose::Walk,
        Mode::Walk => Pose::Idle,
    };
    // Gait advances with distance travelled: about one stride per 1.4 m walking.
    let stride = match p.pose {
        Pose::Run => 2.3,
        Pose::Swim => 0.9,
        _ => 1.4,
    };
    let dg = (hs / stride * std::f64::consts::TAU * time.delta_secs_f64()) as f32;
    p.gait = (p.gait + dg) % std::f32::consts::TAU;
}

/// Camera position and look for the current view, written into [`Viewer`].
fn place_camera(p: Res<Player>, mut viewer: ResMut<Viewer>, terrain: Res<Terrain>) {
    let g = p.geo();
    let frame = EnuFrame::at(g);
    let to_ecef = frame.ecef_to_local.transpose();
    let eye_local = DVec3::Y * EYE_HEIGHT;
    let look = look_rotation(p.heading, p.pitch);
    let (cam_local, rot) = match p.view {
        CameraView::FirstPerson => (eye_local + look * DVec3::new(0.0, 0.0, -0.15), look),
        CameraView::ThirdPerson => {
            let target = eye_local + look * DVec3::new(0.45, 0.15, 0.0);
            (target + look * DVec3::new(0.0, 0.0, 3.4), look)
        }
        CameraView::Orbit => {
            let o = look_rotation(p.orbit_yaw, p.orbit_pitch);
            let center = DVec3::Y * 1.0;
            let cam = center + o * DVec3::new(0.0, 0.0, p.orbit_dist);
            let dir = (center - cam).normalize();
            let heading = (-dir.x).atan2(dir.z).to_degrees() + 180.0;
            let pitch = dir.y.asin().to_degrees();
            (cam, look_rotation(heading, pitch))
        }
    };
    let mut cam_ecef = to_ecef * cam_local + p.feet;
    // Keep third-person and orbit cameras out of the ground.
    if p.view != CameraView::FirstPerson {
        let mut cg = Geodetic::from_ecef(cam_ecef);
        if let Some(h) = terrain.height_at(cg.lat, cg.lon)
            && cg.h < h + 0.4
        {
            cg.h = h + 0.4;
            cam_ecef = cg.to_ecef();
        }
    }
    let f = rot * DVec3::NEG_Z;
    viewer.ecef = cam_ecef;
    viewer.heading = ((-f.x).atan2(f.z).to_degrees() + 180.0).rem_euclid(360.0);
    viewer.pitch = f.y.clamp(-1.0, 1.0).asin().to_degrees();
    viewer.velocity = to_ecef * p.vel;
    viewer.ground = p.ground;
    viewer.hold_agl = p.hold_agl;
}
