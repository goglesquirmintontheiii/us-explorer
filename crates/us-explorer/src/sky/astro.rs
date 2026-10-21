//! Positions of the moon and the bright planets (low-precision ephemerides, good to a
//! fraction of a degree, which is plenty for the eye), sidereal time, and US local time
//! with daylight saving.

use bevy::math::DVec3;
use std::f64::consts::TAU;

/// Days since J2000.0 (2000-01-01 12:00 TT, near enough UTC) for a unix time.
pub fn j2000_days(unix: f64) -> f64 {
    unix / 86_400.0 + 2_440_587.5 - 2_451_545.0
}

/// Greenwich mean sidereal time, radians.
pub fn gmst(unix: f64) -> f64 {
    (280.460_618_37 + 360.985_647_366_29 * j2000_days(unix)).rem_euclid(360.0).to_radians()
}

/// Mean obliquity of the ecliptic, radians.
fn obliquity(d: f64) -> f64 {
    (23.439_291 - 0.000_000_356 * d).to_radians()
}

/// Ecliptic (longitude, latitude, radians) to an equatorial unit vector (x to the
/// vernal equinox, z to the celestial north pole).
fn ecliptic_to_eq(lon: f64, lat: f64, eps: f64) -> DVec3 {
    let v = DVec3::new(lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin());
    DVec3::new(v.x, v.y * eps.cos() - v.z * eps.sin(), v.y * eps.sin() + v.z * eps.cos())
}

/// Equatorial (inertial) vector to Earth-fixed (ECEF) at a time.
pub fn eq_to_ecef(v: DVec3, unix: f64) -> DVec3 {
    let g = gmst(unix);
    let (s, c) = g.sin_cos();
    DVec3::new(c * v.x + s * v.y, -s * v.x + c * v.y, v.z)
}

/// The moon: geocentric equatorial position in Earth radii.
pub fn moon_eq(unix: f64) -> DVec3 {
    let t = j2000_days(unix) / 36_525.0;
    let s = |a: f64, b: f64| (a + b * t).to_radians().sin();
    let c = |a: f64, b: f64| (a + b * t).to_radians().cos();
    let lon = 218.32 + 481_267.881 * t + 6.29 * s(135.0, 477_198.87) - 1.27 * s(259.3, -413_335.36) + 0.66 * s(235.7, 890_534.22)
        + 0.21 * s(269.9, 954_397.74)
        - 0.19 * s(357.5, 35_999.05)
        - 0.11 * s(186.5, 966_404.03);
    let lat = 5.13 * s(93.3, 483_202.02) + 0.28 * s(228.2, 960_400.89) - 0.28 * s(318.3, 6_003.15) - 0.17 * s(217.6, -407_332.21);
    let par = 0.9508 + 0.0518 * c(135.0, 477_198.87) + 0.0095 * c(259.3, -413_335.36) + 0.0078 * c(235.7, 890_534.22)
        + 0.0028 * c(269.9, 954_397.74);
    let r = 1.0 / par.to_radians().sin();
    ecliptic_to_eq(lon.to_radians(), lat.to_radians(), obliquity(j2000_days(unix))) * r
}

/// The sun: geocentric equatorial unit vector.
pub fn sun_eq(unix: f64) -> DVec3 {
    let n = j2000_days(unix);
    let l = (280.460 + 0.985_647_4 * n).rem_euclid(360.0);
    let g = (357.528 + 0.985_600_3 * n).rem_euclid(360.0).to_radians();
    let lambda = (l + 1.915 * g.sin() + 0.020 * (2.0 * g).sin()).to_radians();
    ecliptic_to_eq(lambda, 0.0, obliquity(n))
}

/// Fraction of the moon's disc that is lit (0 new .. 1 full), and whether it's waxing.
pub fn moon_phase(unix: f64) -> (f64, bool) {
    let m = moon_eq(unix).normalize();
    let s = sun_eq(unix);
    let elong = m.dot(s).clamp(-1.0, 1.0).acos();
    let lit = (1.0 - elong.cos()) / 2.0;
    // Waxing while the moon is east of the sun (ahead in ecliptic longitude).
    let waxing = s.cross(m).z > 0.0;
    (lit, waxing)
}

/// Orbital elements (J2000, with rates per century): a, e, I, L, long. perihelion, node.
struct Elements([f64; 6], [f64; 6]);

const EARTH: Elements = Elements(
    [1.000_002_61, 0.016_711_23, -0.000_015_31, 100.464_571_66, 102.937_681_93, 0.0],
    [0.000_005_62, -0.000_043_92, -0.012_946_68, 35_999.372_449_81, 0.323_273_64, 0.0],
);

/// The five naked-eye planets: name, elements, absolute magnitude, colour.
pub const PLANETS: [(&str, f64, [f32; 3]); 5] = [
    ("Mercury", -0.42, [1.0, 0.93, 0.85]),
    ("Venus", -4.40, [1.0, 0.98, 0.9]),
    ("Mars", -1.52, [1.0, 0.62, 0.42]),
    ("Jupiter", -9.40, [1.0, 0.95, 0.85]),
    ("Saturn", -8.88, [1.0, 0.92, 0.72]),
];

const ELEMENTS: [Elements; 5] = [
    Elements(
        [0.387_099_27, 0.205_635_93, 7.004_979_02, 252.250_323_5, 77.457_796_28, 48.330_765_93],
        [0.000_000_37, 0.000_019_06, -0.005_947_49, 149_472.674_111_75, 0.160_476_89, -0.125_340_81],
    ),
    Elements(
        [0.723_335_66, 0.006_776_72, 3.394_676_05, 181.979_099_5, 131.602_467_18, 76.679_842_55],
        [0.000_003_9, -0.000_041_07, -0.000_788_9, 58_517.815_387_29, 0.002_683_29, -0.277_694_18],
    ),
    Elements(
        [1.523_710_34, 0.093_394_1, 1.849_691_42, -4.553_432_05, -23.943_629_59, 49.559_538_91],
        [0.000_018_47, 0.000_078_82, -0.008_131_31, 19_140.302_684_99, 0.444_410_88, -0.292_573_43],
    ),
    Elements(
        [5.202_887, 0.048_386_24, 1.304_396_95, 34.396_440_51, 14.728_479_83, 100.473_909_09],
        [-0.000_116_07, -0.000_132_53, -0.001_837_14, 3_034.746_127_75, 0.212_526_68, 0.204_691_06],
    ),
    Elements(
        [9.536_675_94, 0.053_861_79, 2.485_991_87, 49.954_244_23, 92.598_878_31, 113.662_424_48],
        [-0.001_250_6, -0.000_509_91, 0.001_936_09, 1_222.493_622_01, -0.418_972_16, -0.288_677_94],
    ),
];

/// Heliocentric ecliptic position (AU).
fn helio(el: &Elements, t: f64) -> DVec3 {
    let e: Vec<f64> = (0..6).map(|i| el.0[i] + el.1[i] * t).collect();
    let (a, ecc) = (e[0], e[1]);
    let (inc, l, peri, node) = (e[2].to_radians(), e[3].to_radians(), e[4].to_radians(), e[5].to_radians());
    let w = peri - node;
    let m = (l - peri).rem_euclid(TAU);
    let mut ea = m + ecc * m.sin();
    for _ in 0..6 {
        ea -= (ea - ecc * ea.sin() - m) / (1.0 - ecc * ea.cos());
    }
    let (xp, yp) = (a * (ea.cos() - ecc), a * (1.0 - ecc * ecc).sqrt() * ea.sin());
    let (cw, sw, cn, sn, ci, si) = (w.cos(), w.sin(), node.cos(), node.sin(), inc.cos(), inc.sin());
    DVec3::new(
        (cw * cn - sw * sn * ci) * xp + (-sw * cn - cw * sn * ci) * yp,
        (cw * sn + sw * cn * ci) * xp + (-sw * sn + cw * cn * ci) * yp,
        (sw * si) * xp + (cw * si) * yp,
    )
}

/// Each planet's geocentric equatorial unit vector and apparent magnitude.
pub fn planets(unix: f64) -> [(DVec3, f32); 5] {
    let d = j2000_days(unix);
    let t = d / 36_525.0;
    let earth = helio(&EARTH, t);
    let eps = obliquity(d);
    std::array::from_fn(|i| {
        let p = helio(&ELEMENTS[i], t);
        let g = p - earth;
        let (r, delta) = (p.length(), g.length());
        // Phase angle (sun-planet-earth) dims the inner planets when they're crescents.
        let phase = ((r * r + delta * delta - earth.length_squared()) / (2.0 * r * delta)).clamp(-1.0, 1.0).acos().to_degrees();
        let mag = PLANETS[i].1 + 5.0 * (r * delta).log10() + if i < 2 { 0.02 * phase } else { 0.0 };
        let u = g / delta;
        let eq = DVec3::new(u.x, u.y * eps.cos() - u.z * eps.sin(), u.y * eps.sin() + u.z * eps.cos());
        (eq, mag as f32)
    })
}

/// US time zone at a place: (standard offset hours, observes daylight saving, standard
/// and daylight abbreviations). From the zone name when known (Open-Meteo reports it),
/// else from the map: Hawaii, Alaska, Arizona and the four mainland bands.
pub fn us_zone(name: Option<&str>, lat: f64, lon: f64) -> (f64, bool, &'static str, &'static str) {
    const ET: (f64, bool, &str, &str) = (-5.0, true, "EST", "EDT");
    const CT: (f64, bool, &str, &str) = (-6.0, true, "CST", "CDT");
    const MT: (f64, bool, &str, &str) = (-7.0, true, "MST", "MDT");
    const AZ: (f64, bool, &str, &str) = (-7.0, false, "MST", "MST");
    const PT: (f64, bool, &str, &str) = (-8.0, true, "PST", "PDT");
    const AK: (f64, bool, &str, &str) = (-9.0, true, "AKST", "AKDT");
    const HI: (f64, bool, &str, &str) = (-10.0, false, "HST", "HST");
    const AT: (f64, bool, &str, &str) = (-4.0, false, "AST", "AST");
    if let Some(n) = name {
        let z = match n {
            "America/Phoenix" => Some(AZ),
            "Pacific/Honolulu" => Some(HI),
            "America/Puerto_Rico" | "America/St_Thomas" => Some(AT),
            "America/Adak" => Some((-10.0, true, "HST", "HDT")),
            "America/Los_Angeles" => Some(PT),
            "America/Denver" | "America/Boise" => Some(MT),
            "America/Chicago" | "America/Menominee" => Some(CT),
            "America/New_York" | "America/Detroit" => Some(ET),
            _ if n.starts_with("America/Indiana/Knox") || n.starts_with("America/Indiana/Tell_City") => Some(CT),
            _ if n.starts_with("America/North_Dakota") => Some(CT),
            _ if n.starts_with("America/Indiana") || n.starts_with("America/Kentucky") => Some(ET),
            "America/Anchorage" | "America/Juneau" | "America/Sitka" | "America/Nome" | "America/Yakutat" | "America/Metlakatla" => Some(AK),
            _ => None,
        };
        if let Some(z) = z {
            return z;
        }
    }
    if lon < -154.0 && lat < 25.0 {
        HI
    } else if lat > 51.0 && lon < -129.0 {
        AK
    } else if lat < 19.0 && lon > -68.0 {
        AT
    } else if (31.3..37.0).contains(&lat) && (-114.8..-109.05).contains(&lon) {
        AZ
    } else if lon > -84.5 || (lon > -86.8 && lat > 37.5 && lat < 41.8) || (lon > -87.6 && lat > 41.8) {
        ET
    } else if lon > -101.5 || (lon > -104.0 && lat < 32.0 && lon > -104.9 && lat > 29.0 && lon > -104.5) {
        CT
    } else if lon > -114.5 || (lat > 42.0 && lon > -117.0) {
        MT
    } else {
        PT
    }
}

/// Is US daylight saving in force at this instant (second Sunday in March, 2 am local
/// standard time, to the first Sunday in November, 2 am local daylight time)?
pub fn us_dst(unix: f64, std_offset_h: f64) -> bool {
    let local = unix + std_offset_h * 3600.0;
    let s = crate::sun::format_utc(local);
    let y: i64 = s[..4].parse().unwrap_or(2026);
    let sunday = |month: i64, nth: i64| -> f64 {
        let first = crate::sun::parse_utc(&format!("{y:04}-{month:02}-01T00:00Z")).unwrap_or(0.0);
        // 1970-01-01 was a Thursday.
        let dow = ((first / 86_400.0).floor() as i64 + 4).rem_euclid(7);
        first + (((7 - dow) % 7) + 7 * (nth - 1)) as f64 * 86_400.0
    };
    let start = sunday(3, 2) + 2.0 * 3600.0;
    // Ends at 2 am daylight time = 1 am standard time.
    let end = sunday(11, 1) + 3600.0;
    local >= start && local < end
}

/// Local clock time, e.g. "14:32 CDT".
pub fn local_time(unix: f64, zone: Option<&str>, lat: f64, lon: f64) -> String {
    let (off, dst, st, dt) = us_zone(zone, lat, lon);
    let on = dst && us_dst(unix, off);
    let t = unix + (off + if on { 1.0 } else { 0.0 }) * 3600.0;
    let s = crate::sun::format_utc(t);
    format!("{} {}", &s[11..16], if on { dt } else { st })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sun::parse_utc;

    fn radec(v: DVec3) -> (f64, f64) {
        let v = v.normalize();
        (v.y.atan2(v.x).to_degrees().rem_euclid(360.0) / 15.0, v.z.asin().to_degrees())
    }

    #[test]
    fn moon_and_planets() {
        // 2026-01-03 10:03 UTC: full moon (Wolf Moon) in Gemini, RA ~7h, Dec ~+27.
        let t = parse_utc("2026-01-03T10:03Z").unwrap();
        let (lit, _) = moon_phase(t);
        assert!(lit > 0.97, "{lit}");
        let (ra, dec) = radec(moon_eq(t));
        assert!((ra - 7.0).abs() < 0.5 && (dec - 26.0).abs() < 3.0, "{ra} {dec}");
        // 2026-01-18: new moon.
        assert!(moon_phase(parse_utc("2026-01-18T19:52Z").unwrap()).0 < 0.03);
        // Jupiter at opposition 2026-01-10 in Gemini (RA ~7h20m, Dec +22), magnitude -2.7.
        let p = planets(parse_utc("2026-01-10T00:00Z").unwrap());
        let (ra, dec) = radec(p[3].0);
        assert!((ra - 7.35).abs() < 0.3 && (dec - 22.3).abs() < 2.0, "{ra} {dec}");
        assert!((p[3].1 + 2.7).abs() < 0.4, "{}", p[3].1);
        // Venus is never far from the sun.
        let s = sun_eq(parse_utc("2026-06-01T00:00Z").unwrap());
        let v = planets(parse_utc("2026-06-01T00:00Z").unwrap())[1].0;
        assert!(v.dot(s).acos().to_degrees() < 48.0);
    }

    #[test]
    fn clocks() {
        // 2026: DST from 8 March to 1 November.
        let t = parse_utc("2026-07-04T19:00Z").unwrap();
        assert_eq!(local_time(t, Some("America/Chicago"), 29.4, -98.5), "14:00 CDT");
        assert_eq!(local_time(t, None, 29.4, -98.5), "14:00 CDT");
        assert_eq!(local_time(t, None, 33.45, -112.07), "12:00 MST");
        assert_eq!(local_time(t, None, 40.7, -74.0), "15:00 EDT");
        assert_eq!(local_time(t, None, 34.05, -118.24), "12:00 PDT");
        assert_eq!(local_time(t, None, 21.3, -157.85), "09:00 HST");
        let w = parse_utc("2026-12-01T19:00Z").unwrap();
        assert_eq!(local_time(w, None, 39.74, -104.99), "12:00 MST");
        assert!(!us_dst(parse_utc("2026-03-08T06:59Z").unwrap(), -5.0));
        assert!(us_dst(parse_utc("2026-03-08T07:00Z").unwrap(), -5.0));
        assert!(us_dst(parse_utc("2026-11-01T05:59Z").unwrap(), -5.0));
        assert!(!us_dst(parse_utc("2026-11-01T06:00Z").unwrap(), -5.0));
    }
}
