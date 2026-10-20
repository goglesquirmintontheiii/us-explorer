//! Simulation clock, real sun position and the physically based sky.

use crate::globe::{GlobeSet, WorldOrigin};
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, light_consts::lux};
use bevy::math::DVec3;
use bevy::prelude::*;
use usx_core::geo::EnuFrame;

/// The game's clock (UTC seconds). Real time by default; `offset` lets the debug
/// menu (and later the replay system) move it.
#[derive(Resource)]
pub struct SimClock {
    pub unix: f64,
    pub offset: f64,
    pub fixed: bool,
}

impl SimClock {
    pub fn now_real() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
    pub fn time(&self) -> f64 {
        self.unix + self.offset
    }
}

#[derive(Component)]
pub struct Sun;

#[derive(Component)]
pub struct Planet;

pub struct SunPlugin;

impl Plugin for SunPlugin {
    fn build(&self, app: &mut App) {
        let fixed = app.world().resource::<crate::Args>().time;
        app.insert_resource(SimClock {
            unix: fixed.unwrap_or_else(SimClock::now_real),
            offset: 0.0,
            fixed: fixed.is_some(),
        })
        .add_systems(Startup, spawn_sky)
        .add_systems(Update, tick_clock)
        .add_systems(
            PostUpdate,
            (place_sun, place_planet)
                .after(GlobeSet)
                .before(TransformSystems::Propagate),
        );
    }
}

fn spawn_sky(mut commands: Commands, mut media: ResMut<Assets<ScatteringMedium>>) {
    let medium = media.add(ScatteringMedium::default());
    commands.spawn((Planet, Atmosphere::earth(medium), Transform::default()));
    commands.spawn((
        Sun,
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::default(),
    ));
}

fn tick_clock(mut clock: ResMut<SimClock>) {
    if !clock.fixed {
        clock.unix = SimClock::now_real();
    }
}

fn place_sun(
    clock: Res<SimClock>,
    origin: Res<WorldOrigin>,
    mut q: Query<&mut Transform, With<Sun>>,
) {
    let dir = origin
        .frame
        .dir_to_local(sun_dir_ecef(clock.time()))
        .as_vec3();
    for mut t in &mut q {
        *t = Transform::IDENTITY.looking_to(-dir, Vec3::Y);
    }
}

/// The atmosphere is a sphere; put its centre so the sphere touches the ground under
/// the origin. A planet-centre placement would be off by up to 18 km on the ellipsoid.
fn place_planet(
    origin: Res<WorldOrigin>,
    mut q: Query<(&Atmosphere, &mut Transform), With<Planet>>,
) {
    let mut g = origin.geo;
    g.h = 0.0;
    let ground = origin.frame.to_local(g.to_ecef());
    for (a, mut t) in &mut q {
        t.translation = (ground - DVec3::Y * a.inner_radius as f64).as_vec3();
    }
}

/// Unit vector from Earth's centre towards the sun, in ECEF, for a UTC unix time.
/// Low-precision solar ephemeris (Astronomical Almanac), good to ~0.01 degrees.
pub fn sun_dir_ecef(unix: f64) -> DVec3 {
    let n = unix / 86_400.0 + 2_440_587.5 - 2_451_545.0;
    let l = (280.460 + 0.985_647_4 * n).rem_euclid(360.0);
    let g = (357.528 + 0.985_600_3 * n).rem_euclid(360.0).to_radians();
    let lambda = (l + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    let eps = (23.439 - 0.000_000_4 * n).to_radians();
    let ra = (eps.cos() * lambda.sin()).atan2(lambda.cos());
    let dec = (eps.sin() * lambda.sin()).asin();
    let gmst = (280.460_618_37 + 360.985_647_366_29 * n)
        .rem_euclid(360.0)
        .to_radians();
    let lon = ra - gmst;
    DVec3::new(dec.cos() * lon.cos(), dec.cos() * lon.sin(), dec.sin())
}

/// Sun elevation above the horizon, degrees, at a place.
pub fn sun_elevation(unix: f64, frame: &EnuFrame) -> f64 {
    frame.dir_to_local(sun_dir_ecef(unix)).y.asin().to_degrees()
}

/// Parse `YYYY-MM-DDTHH:MM[:SS][Z]` (UTC) into unix seconds.
pub fn parse_utc(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once(['T', ' ']).unwrap_or((s, "00:00"));
    let d: Vec<i64> = date
        .split('-')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let t: Vec<f64> = time
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    if d.len() != 3 || t.len() < 2 {
        return None;
    }
    let days = days_from_civil(d[0], d[1], d[2]);
    Some(days as f64 * 86_400.0 + t[0] * 3600.0 + t[1] * 60.0 + t.get(2).copied().unwrap_or(0.0))
}

/// Howard Hinnant's days-from-civil.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Format unix seconds as `YYYY-MM-DD HH:MM UTC`.
pub fn format_utc(unix: f64) -> String {
    let days = (unix / 86_400.0).floor() as i64;
    let secs = unix - days as f64 * 86_400.0;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        (secs / 3600.0) as u32,
        ((secs % 3600.0) / 60.0) as u32
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use usx_core::geo::Geodetic;

    #[test]
    fn noon_sun_is_high_in_summer_denver() {
        // 2026-06-21 19:00 UTC is just after solar noon in Denver.
        let t = parse_utc("2026-06-21T19:00:00Z").unwrap();
        let e = sun_elevation(t, &EnuFrame::at(Geodetic::new(39.74, -104.99, 0.0)));
        assert!((e - 73.5).abs() < 1.5, "{e}");
        let night = sun_elevation(
            parse_utc("2026-06-21T07:00Z").unwrap(),
            &EnuFrame::at(Geodetic::new(39.74, -104.99, 0.0)),
        );
        assert!(night < -10.0);
        assert_eq!(format_utc(t), "2026-06-21 19:00 UTC");
    }
}
