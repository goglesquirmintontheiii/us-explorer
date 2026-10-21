//! Ambient sound: the city's hum, a highway's roar, wind in the trees and birds by day,
//! crickets on warm nights, surf on the shore and wind higher up. Each is a loop
//! synthesised at start-up (no sound files to ship), mixed by what's around you.

use crate::globe::Viewer;
use crate::nature::{Nature, conditions::Conditions};
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use std::sync::Arc;
use usx_core::tile::TileId;

const RATE: u32 = 22_050;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    City,
    Highway,
    Leaves,
    Birds,
    Crickets,
    Surf,
    Wind,
}

const KINDS: [Kind; 7] = [Kind::City, Kind::Highway, Kind::Leaves, Kind::Birds, Kind::Crickets, Kind::Surf, Kind::Wind];

#[derive(Component)]
struct Loop(Kind);

#[derive(Resource, Default)]
struct Mix {
    target: [f32; 7],
    level: [f32; 7],
    timer: f32,
    rx: Option<crossbeam_channel::Receiver<Vec<(Kind, Vec<u8>)>>>,
    started: bool,
}

pub struct AmbiencePlugin;

impl Plugin for AmbiencePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Mix>().add_systems(Update, (start, measure, apply).chain());
    }
}

fn start(mut mix: ResMut<Mix>, mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>, args: Res<crate::Args>, settings: Res<crate::GameSettings>) {
    if mix.started || !args.shots.is_empty() || !settings.0.ambient_sound {
        return;
    }
    let Some(rx) = &mix.rx else {
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(KINDS.iter().map(|&k| (k, wav(&synth(k)))).collect());
        });
        mix.rx = Some(rx);
        return;
    };
    let Ok(clips) = rx.try_recv() else { return };
    for (k, bytes) in clips {
        let h = sources.add(AudioSource { bytes: Arc::from(bytes) });
        commands.spawn((Loop(k), AudioPlayer(h), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0))));
    }
    mix.started = true;
    mix.rx = None;
}

/// Work out what's around you twice a second.
#[allow(clippy::too_many_arguments)]
fn measure(
    mut mix: ResMut<Mix>,
    time: Res<Time>,
    viewer: Res<Viewer>,
    terrain: Res<crate::terrain::Terrain>,
    nature: Option<Res<Nature>>,
    cond: Res<Conditions>,
    day: Res<crate::sun::DayNight>,
    inside: Option<Res<crate::buildings::interior::Inside>>,
) {
    if !mix.started {
        return;
    }
    mix.timer -= time.delta_secs();
    if mix.timer > 0.0 {
        return;
    }
    mix.timer = 0.5;
    let g = viewer.geo();
    let agl = viewer.agl().unwrap_or(2.0).max(0.0) as f32;
    let high = (1.0 - agl / 250.0).clamp(0.0, 1.0);
    let m_lat = 1.0 / 111_320.0;
    let m_lon = m_lat / g.lat.to_radians().cos();
    // Sea around you: samples on rings at 60, 200 and 500 m.
    let mut surf = 0.0f32;
    for (r, w) in [(60.0, 1.0), (200.0, 0.6), (500.0, 0.25)] {
        let mut wet = 0;
        let mut n = 0;
        for i in 0..12 {
            let a = i as f64 / 12.0 * std::f64::consts::TAU;
            if let Some(h) = terrain.height_at(g.lat + a.cos() * r * m_lat, g.lon + a.sin() * r * m_lon) {
                n += 1;
                if h.abs() < 0.4 {
                    wet += 1;
                }
            }
        }
        if n > 0 {
            surf = surf.max(wet as f32 / n as f32 * w * 1.6);
        }
    }
    // Highways: wide road corridors within 500 m.
    let mut road_d = f64::MAX;
    let mut busy = 0.0f32;
    let t15 = TileId::containing(g.lat, g.lon, usx_core::osm::ZOOM).0;
    if let Some(c) = crate::roads::corridors_for(t15) {
        for cor in &c.0 {
            let (s, w, n, e) = cor.bbox;
            if g.lat < s - 0.006 || g.lat > n + 0.006 || g.lon < w - 0.006 || g.lon > e + 0.006 {
                continue;
            }
            for p in cor.pts.windows(2) {
                let (ax, ay) = ((p[0].1 - g.lon) / m_lon, (p[0].0 - g.lat) / m_lat);
                let (bx, by) = ((p[1].1 - g.lon) / m_lon, (p[1].0 - g.lat) / m_lat);
                let (dx, dy) = (bx - ax, by - ay);
                let l2 = (dx * dx + dy * dy).max(1e-6);
                let t = (-(ax * dx + ay * dy) / l2).clamp(0.0, 1.0);
                let d = ((ax + dx * t).powi(2) + (ay + dy * t).powi(2)).sqrt();
                if cor.half >= 6.5 {
                    road_d = road_d.min(d);
                } else if d < 60.0 {
                    busy += 0.02 * (1.0 - d as f32 / 60.0);
                }
            }
        }
    }
    let highway = if road_d < 600.0 { (1.0 - road_d as f32 / 600.0).powf(1.6) } else { 0.0 };
    // City: buildings within ~250 m.
    let mut blds = 0usize;
    if let Some(fp) = crate::buildings::footprints_for(t15) {
        for f in fp.iter() {
            let (s, w, _, _) = f.bbox;
            if ((s - g.lat) / m_lat).abs() < 250.0 && ((w - g.lon) / m_lon).abs() < 250.0 {
                blds += 1;
            }
        }
    }
    let city = ((blds as f32 / 70.0).min(1.0) * 0.8 + busy.min(0.4)).min(1.0);
    // Trees within ~80 m.
    let mut trees = 0usize;
    if let Some(nat) = &nature {
        let c = TileId::containing(g.lat, g.lon, 17).0;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let id = TileId { z: 17, x: (c.x as i64 + dx) as u32, y: (c.y as i64 + dy) as u32 };
                if let Some(cell) = nat.cell(id) {
                    let me = cell.local.xz(g.lat, g.lon);
                    trees += cell.trees.iter().filter(|t| t.p.distance_squared(me) < 80.0 * 80.0).count();
                }
            }
        }
    }
    let woods = (trees as f32 / 45.0).min(1.0) * (1.0 - city * 0.6);
    let daylight = ((day.elevation + 4.0) / 8.0).clamp(0.0, 1.0);
    let warm = ((cond.temperature_c - 10.0) / 8.0).clamp(0.0, 1.0);
    let wind = (cond.wind_speed / 14.0).clamp(0.0, 1.0);
    let indoors = inside.is_some_and(|i| i.0.is_some());
    let muffle = if indoors { 0.12 } else { 1.0 };
    let t = &mut mix.target;
    t[0] = city * 0.55 * high * muffle;
    t[1] = highway * 0.7 * high * muffle;
    t[2] = woods * (0.25 + 0.75 * wind) * 0.6 * high * muffle;
    t[3] = (woods.max(0.25 * (1.0 - city))) * daylight * 0.45 * high * muffle;
    t[4] = (woods.max(0.4 * (1.0 - city))) * (1.0 - daylight) * warm * 0.35 * high * muffle;
    t[5] = surf.min(1.0) * (0.5 + 0.5 * (cond.wave_height / 1.5).min(1.0)) * 0.8 * (1.0 - agl / 600.0).clamp(0.0, 1.0) * muffle;
    t[6] = ((0.08 + 0.5 * wind) * (0.4 + 0.6 * (agl / 150.0).min(1.0))).min(0.8) * muffle;
}

fn apply(mut mix: ResMut<Mix>, time: Res<Time>, settings: Res<crate::GameSettings>, mut sinks: Query<(&Loop, &mut AudioSink)>) {
    let on = settings.0.ambient_sound;
    let master = settings.0.volume.clamp(0.0, 1.0);
    let k = (time.delta_secs() * 0.8).min(1.0);
    for i in 0..7 {
        let t = if on { mix.target[i] } else { 0.0 };
        mix.level[i] += (t - mix.level[i]) * k;
    }
    for (l, mut s) in &mut sinks {
        let i = KINDS.iter().position(|&k| k == l.0).unwrap_or(0);
        let v = mix.level[i] * master;
        if let Volume::Linear(cur) = s.volume()
            && (cur - v).abs() < 0.003
        {
            continue;
        }
        s.set_volume(Volume::Linear(v));
    }
}

// ------------------------------------------------------------------- synthesis

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn white(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

/// One-pole low-pass.
struct Lp {
    y: f32,
    a: f32,
}

impl Lp {
    fn new(fc: f32) -> Self {
        Self { y: 0.0, a: 1.0 - (-std::f32::consts::TAU * fc / RATE as f32).exp() }
    }
    fn set(&mut self, fc: f32) {
        self.a = 1.0 - (-std::f32::consts::TAU * fc / RATE as f32).exp();
    }
    fn run(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// A seamless loop: synthesise a little extra and cross-fade the tail into the head.
fn synth(kind: Kind) -> Vec<f32> {
    let secs = match kind {
        Kind::Surf => 24.0,
        Kind::Birds => 20.0,
        _ => 12.0,
    };
    let n = (secs * RATE as f32) as usize;
    let fade = RATE as usize;
    let total = n + fade;
    let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ kind as u64 * 7919);
    let mut out = vec![0.0f32; total];
    let fs = RATE as f32;
    match kind {
        Kind::City => {
            let (mut lp1, mut lp2, mut lp3) = (Lp::new(90.0), Lp::new(400.0), Lp::new(1500.0));
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / fs;
                let rumble = lp1.run(r.white()) * 2.4;
                let swell = 0.6 + 0.4 * (t * 0.37).sin() * (t * 0.13 + 1.0).sin();
                let traffic = (lp2.run(r.white()) - lp3.run(r.white()) * 0.3) * swell;
                *o = rumble + traffic * 0.8;
            }
            // A couple of distant horns.
            for _ in 0..2 {
                let at = (r.next() * (secs - 2.0) * fs) as usize;
                let len = (0.35 * fs) as usize;
                for j in 0..len {
                    let t = j as f32 / fs;
                    let env = (t * 30.0).min(1.0) * (1.0 - j as f32 / len as f32).powf(0.3);
                    out[at + j] += ((t * 415.0 * std::f32::consts::TAU).sin().signum() * 0.3 + (t * 520.0 * std::f32::consts::TAU).sin() * 0.5) * env * 0.05;
                }
            }
        }
        Kind::Highway => {
            let mut lp = Lp::new(500.0);
            let mut lp2 = Lp::new(120.0);
            let passes: Vec<(f32, f32, f32)> = (0..14).map(|_| (r.next() * secs, 1.2 + r.next() * 2.5, 0.5 + r.next())).collect();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / fs;
                let mut near = 0.0;
                for &(c, w, a) in &passes {
                    let mut d = (t - c).abs();
                    d = d.min(secs - d);
                    near += a * (-(d / w).powi(2)).exp();
                }
                lp.set(300.0 + 900.0 * near.min(1.5));
                let x = r.white();
                *o = lp.run(x) * (0.35 + near) * 1.3 + lp2.run(x) * 1.2;
            }
        }
        Kind::Leaves => {
            let (mut lp, mut hp) = (Lp::new(5000.0), Lp::new(900.0));
            let mut gust = Lp::new(0.3);
            for o in out.iter_mut() {
                let x = r.white();
                let rustle = lp.run(x) - hp.run(x);
                let g = 0.5 + gust.run(r.white() * 40.0).abs();
                *o = rustle * g * 0.7;
            }
        }
        Kind::Birds => {
            // Songs: short runs of chirps sweeping up or down.
            let mut t0 = 0.3;
            while t0 < secs - 1.5 {
                let base = 2200.0 + r.next() * 2600.0;
                let notes = 2 + (r.next() * 6.0) as usize;
                let sweep = (r.next() - 0.4) * 2400.0;
                let note_len = 0.05 + r.next() * 0.1;
                let gap = 0.04 + r.next() * 0.08;
                let amp = 0.05 + r.next() * 0.12;
                for k in 0..notes {
                    let start = ((t0 + k as f32 * (note_len + gap)) * fs) as usize;
                    let len = (note_len * fs) as usize;
                    let mut ph = 0.0f32;
                    for j in 0..len {
                        let u = j as f32 / len as f32;
                        let f = base + sweep * u + 150.0 * (u * 40.0).sin();
                        ph += f / fs * std::f32::consts::TAU;
                        let env = (u * std::f32::consts::PI).sin().powi(2);
                        if let Some(o) = out.get_mut(start + j) {
                            *o += ph.sin() * env * amp;
                        }
                    }
                }
                t0 += notes as f32 * (note_len + gap) + 0.6 + r.next() * 2.5;
            }
        }
        Kind::Crickets => {
            for voice in 0..3 {
                let f = 4300.0 + voice as f32 * 260.0 + r.next() * 100.0;
                let rate = 0.45 + r.next() * 0.3;
                let offset = r.next();
                let amp = 0.06 / (1.0 + voice as f32 * 0.5);
                for (i, o) in out.iter_mut().enumerate() {
                    let t = i as f32 / fs;
                    let cycle = (t / rate + offset).fract() * rate;
                    // Three pulses at ~30 Hz, then silence.
                    let pulse = if cycle < 0.1 { ((cycle * 30.0 * std::f32::consts::PI).sin()).max(0.0).powi(2) } else { 0.0 };
                    *o += (t * f * std::f32::consts::TAU).sin() * pulse * amp;
                }
            }
        }
        Kind::Surf => {
            let (mut lp, mut lp2) = (Lp::new(600.0), Lp::new(200.0));
            let period = 8.0;
            let waves = (secs / period) as usize;
            let jit: Vec<f32> = (0..waves).map(|_| 0.7 + r.next() * 0.6).collect();
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / fs;
                let k = ((t / period) as usize) % waves;
                let u = (t % period) / period;
                // The break: a quick swell, then the long hiss of the wash running up.
                let crash = (u / 0.08).min(1.0) * (-(u - 0.08).max(0.0) * 5.0).exp();
                let wash = (-(u - 0.15).max(0.0) * 2.2).exp() * (u / 0.15).min(1.0);
                let a = jit[k];
                lp.set(400.0 + 2600.0 * crash);
                let x = r.white();
                *o = (lp.run(x) * (0.25 + crash * 1.2 + wash * 0.6) + lp2.run(x) * 0.8) * a;
            }
        }
        Kind::Wind => {
            let (mut lp, mut gust) = (Lp::new(380.0), Lp::new(0.25));
            let mut hp = Lp::new(60.0);
            for o in out.iter_mut() {
                let x = r.white();
                let g = 0.4 + (gust.run(r.white() * 60.0)).abs();
                lp.set(250.0 + 350.0 * g.min(1.5));
                let y = lp.run(x);
                *o = (y - hp.run(y)) * g * 2.0;
            }
        }
    }
    // Cross-fade the overhang into the start.
    for j in 0..fade {
        let f = j as f32 / fade as f32;
        out[j] = out[j] * f + out[n + j] * (1.0 - f);
    }
    out.truncate(n);
    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-6);
    let gain = 0.85 / peak;
    out.iter_mut().for_each(|x| *x *= gain);
    out
}

/// 16-bit mono WAV bytes.
fn wav(s: &[f32]) -> Vec<u8> {
    let data = s.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for &x in s {
        b.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loops_are_sane() {
        for k in KINDS {
            let s = synth(k);
            assert!(s.iter().all(|x| x.is_finite()));
            let rms = (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
            assert!(rms > 0.01, "{k:?} silent");
            // Seamless: the end runs smoothly into the start.
            let jump = (s[0] - s[s.len() - 1]).abs();
            assert!(jump < 0.5, "{k:?} {jump}");
            if std::env::var("USX_WAV").is_ok() {
                std::fs::write(format!("/tmp/{k:?}.wav"), wav(&s)).unwrap();
            }
        }
    }
}
