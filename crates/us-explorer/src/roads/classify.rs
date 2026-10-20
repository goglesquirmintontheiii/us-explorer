//! What an OSM way is, as far as drawing it goes: class, width, lanes, surface.

use usx_core::osm::Tags;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Motorway,
    Trunk,
    Primary,
    Secondary,
    Tertiary,
    Unclassified,
    Residential,
    Living,
    Service,
    Track,
    Pedestrian,
    Cycleway,
    Footway,
    Path,
    Steps,
    Rail,
    LightRail,
    Tram,
    Subway,
}

impl Class {
    pub fn from_tags(t: &Tags) -> Option<(Class, bool)> {
        if t.flag("area") {
            return None;
        }
        if let Some(r) = t.get("railway") {
            return Some((
                match r {
                    "rail" | "narrow_gauge" | "monorail" => Class::Rail,
                    "light_rail" => Class::LightRail,
                    "tram" => Class::Tram,
                    "subway" => Class::Subway,
                    _ => return None,
                },
                false,
            ));
        }
        let mut h = t.get("highway")?;
        let mut construction = false;
        if h == "construction" {
            construction = true;
            h = t.get("construction").unwrap_or("residential");
        }
        let base = h.strip_suffix("_link").unwrap_or(h);
        let c = match base {
            "motorway" => Class::Motorway,
            "trunk" => Class::Trunk,
            "primary" => Class::Primary,
            "secondary" => Class::Secondary,
            "tertiary" => Class::Tertiary,
            "unclassified" | "road" | "busway" => Class::Unclassified,
            "residential" => Class::Residential,
            "living_street" => Class::Living,
            "service" => Class::Service,
            "track" => Class::Track,
            "pedestrian" => Class::Pedestrian,
            "cycleway" => Class::Cycleway,
            "footway" | "bridleway" | "corridor" => Class::Footway,
            "path" => Class::Path,
            "steps" => Class::Steps,
            _ => return None,
        };
        // Indoor corridors and platforms aren't outdoor surfaces.
        if base == "corridor" || t.get("level").is_some_and(|l| l != "0") && c >= Class::Pedestrian {
            return None;
        }
        Some((c, construction))
    }

    /// Carries motor traffic (gets lanes, markings, junction surfaces).
    pub fn vehicle(self) -> bool {
        self <= Class::Track
    }
    pub fn rail(self) -> bool {
        self >= Class::Rail
    }
    pub fn path(self) -> bool {
        !self.vehicle() && !self.rail()
    }
    /// A road big enough to get edge lines, a centre line and signals.
    pub fn major(self) -> bool {
        self <= Class::Tertiary
    }
    /// Draw order, so overlapping surfaces never fight: bigger roads on top.
    pub fn lift(self) -> f32 {
        match self {
            Class::Motorway | Class::Trunk => 0.16,
            Class::Primary | Class::Secondary => 0.15,
            Class::Tertiary | Class::Unclassified | Class::Residential | Class::Living => 0.14,
            Class::Service | Class::Track => 0.12,
            Class::Rail | Class::LightRail | Class::Subway => 0.11,
            Class::Tram => 0.145,
            _ => 0.10,
        }
    }
    pub fn rank(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Surface {
    Asphalt = 0,
    Concrete = 1,
    Pavers = 2,
    Gravel = 3,
    Dirt = 4,
    Wood = 5,
    Metal = 6,
}

impl Surface {
    pub fn from_tags(t: &Tags, c: Class) -> Surface {
        match t.get("surface") {
            Some("asphalt" | "chipseal" | "paved") => Surface::Asphalt,
            Some("concrete" | "concrete:plates" | "concrete:lanes") => Surface::Concrete,
            Some("paving_stones" | "sett" | "cobblestone" | "unhewn_cobblestone" | "bricks" | "brick") => Surface::Pavers,
            Some("gravel" | "fine_gravel" | "pebblestone" | "compacted" | "rock") => Surface::Gravel,
            Some("dirt" | "ground" | "earth" | "mud" | "sand" | "grass" | "unpaved" | "woodchips") => Surface::Dirt,
            Some("wood") => Surface::Wood,
            Some("metal" | "metal_grid") => Surface::Metal,
            _ => match c {
                Class::Track => Surface::Gravel,
                Class::Path => Surface::Dirt,
                Class::Footway | Class::Steps | Class::Pedestrian => Surface::Concrete,
                _ => Surface::Asphalt,
            },
        }
    }
}

/// 0 = excellent ... 7 = impassable. Untagged minor rural roads are a bit worse.
pub fn smoothness(t: &Tags, c: Class) -> u8 {
    match t.get("smoothness") {
        Some("excellent") => 0,
        Some("good") => 1,
        Some("intermediate") => 3,
        Some("bad") => 4,
        Some("very_bad") => 5,
        Some("horrible") => 6,
        Some("very_horrible" | "impassable") => 7,
        _ => match c {
            Class::Unclassified | Class::Track => 3,
            Class::Service | Class::Residential => 2,
            _ => 1,
        },
    }
}

/// Lanes and widths for a road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    /// Lanes running with the way's direction and against it.
    pub fwd: u8,
    pub bwd: u8,
    pub lane_w: f32,
    /// Paved shoulder outside the edge line on the left / right of the whole road.
    pub shoulder_l: f32,
    pub shoulder_r: f32,
    pub oneway: bool,
}

impl Profile {
    pub fn width(&self) -> f32 {
        (self.fwd + self.bwd) as f32 * self.lane_w + self.shoulder_l + self.shoulder_r
    }
    pub fn half(&self) -> f32 {
        self.width() * 0.5
    }
}

pub fn profile(t: &Tags, c: Class, link: bool) -> Profile {
    // oneway=-1 is reversed by the caller before we get here; treat as oneway.
    let oneway = matches!(t.get("oneway"), Some("yes" | "true" | "1" | "-1"))
        || (matches!(c, Class::Motorway) && !t.is("oneway", "no"))
        || t.is("junction", "roundabout")
        || t.is("junction", "circular");
    let lane_w: f32 = match c {
        Class::Motorway | Class::Trunk => 3.65,
        Class::Primary | Class::Secondary => 3.5,
        Class::Tertiary | Class::Unclassified => 3.3,
        Class::Residential | Class::Living => 4.2, // unmarked, with room for parked cars
        Class::Service => 3.0,
        Class::Track => 2.8,
        Class::Pedestrian => 3.0,
        Class::Cycleway | Class::Footway | Class::Steps => 1.0,
        Class::Path => 0.8,
        Class::Rail | Class::LightRail | Class::Tram | Class::Subway => 1.5,
    };
    let lanes = t.num("lanes").map(|l| l.clamp(1.0, 12.0) as u8);
    let lf = t.num("lanes:forward").map(|l| l.clamp(0.0, 8.0) as u8);
    let lb = t.num("lanes:backward").map(|l| l.clamp(0.0, 8.0) as u8);
    let default_total = match c {
        Class::Motorway | Class::Trunk if oneway => {
            if link {
                1
            } else {
                2
            }
        }
        Class::Motorway | Class::Trunk => 4,
        _ if c.path() || c.rail() => 2,
        _ if oneway => 1,
        Class::Service | Class::Track => 1,
        _ => 2,
    };
    let (fwd, bwd) = if oneway {
        (lanes.or(lf).unwrap_or(default_total).max(1), 0)
    } else if let (Some(f), Some(b)) = (lf, lb) {
        (f.max(1), b.max(1))
    } else {
        let total = lanes.unwrap_or(default_total);
        if total <= 1 && (c == Class::Service || c == Class::Track || c.path() || c.rail()) {
            (1, 0)
        } else {
            let b = lb.unwrap_or(total / 2).max(1);
            (lf.unwrap_or(total.saturating_sub(b)).max(1), b)
        }
    };
    let (sl, sr) = match c {
        Class::Motorway | Class::Trunk if oneway => (1.2, 3.0),
        Class::Motorway | Class::Trunk => (3.0, 3.0),
        Class::Primary | Class::Secondary => (0.6, 0.6),
        Class::Tertiary | Class::Unclassified => (0.3, 0.3),
        _ => (0.0, 0.0),
    };
    let mut p = Profile { fwd, bwd, lane_w, shoulder_l: sl, shoulder_r: sr, oneway };
    // An explicit width wins: scale lanes and shoulders to fit it.
    if let Some(w) = t.num("width").filter(|w| *w > 0.5 && *w < 60.0) {
        let k = w as f32 / p.width();
        p.lane_w *= k;
        p.shoulder_l *= k;
        p.shoulder_r *= k;
    }
    // Paths are a single strip of their own width.
    if c.path() {
        let w = t.num("width").map(|w| w as f32).unwrap_or(match c {
            Class::Path => 1.4,
            Class::Pedestrian => 6.0,
            Class::Steps => 2.0,
            _ => 2.0,
        });
        p = Profile { fwd: 1, bwd: 0, lane_w: w.clamp(0.6, 30.0), shoulder_l: 0.0, shoulder_r: 0.0, oneway: false };
    }
    if c.rail() {
        p = Profile { fwd: 1, bwd: 0, lane_w: if c == Class::Tram { 2.6 } else { 3.2 }, shoulder_l: 0.0, shoulder_r: 0.0, oneway: false };
    }
    p
}

/// Turn arrows for each lane, left to right in the direction of travel: bit 0 left,
/// bit 1 through, bit 2 right, 3 bits per lane, up to 6 lanes.
pub fn turn_bits(v: Option<&str>) -> u32 {
    let Some(v) = v else { return 0 };
    let mut out = 0u32;
    for (i, lane) in v.split('|').take(6).enumerate() {
        let mut b = 0u32;
        for part in lane.split(';') {
            b |= match part.trim() {
                "left" | "slight_left" | "sharp_left" | "merge_to_left" => 1,
                "through" => 2,
                "right" | "slight_right" | "sharp_right" | "merge_to_right" => 4,
                _ => 0,
            };
        }
        out |= b << (i * 3);
    }
    out
}

/// OSM `layer` (bridges default to 1, tunnels to -1).
pub fn layer(t: &Tags) -> i32 {
    let base = t.num("layer").map(|l| l as i32).unwrap_or(0);
    if t.flag("bridge") && base <= 0 {
        1
    } else if t.flag("tunnel") && !t.is("tunnel", "building_passage") && base >= 0 {
        -1
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[(&str, &str)]) -> Tags {
        Tags(v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect())
    }

    #[test]
    fn classes_and_profiles() {
        let m = tags(&[("highway", "motorway"), ("lanes", "3")]);
        let (c, _) = Class::from_tags(&m).unwrap();
        assert_eq!(c, Class::Motorway);
        let p = profile(&m, c, false);
        assert!(p.oneway && p.fwd == 3 && p.bwd == 0);
        assert!((p.width() - (3.0 * 3.65 + 4.2)).abs() < 1e-4);

        let r = tags(&[("highway", "residential")]);
        let p = profile(&r, Class::Residential, false);
        assert_eq!((p.fwd, p.bwd), (1, 1));

        let w = tags(&[("highway", "primary"), ("lanes", "5"), ("lanes:forward", "3"), ("lanes:backward", "2")]);
        let p = profile(&w, Class::Primary, false);
        assert_eq!((p.fwd, p.bwd), (3, 2));

        let x = tags(&[("highway", "construction"), ("construction", "secondary")]);
        assert_eq!(Class::from_tags(&x), Some((Class::Secondary, true)));
        assert!(Class::from_tags(&tags(&[("highway", "pedestrian"), ("area", "yes")])).is_none());
    }

    #[test]
    fn turn_lanes() {
        assert_eq!(turn_bits(Some("left|through|through;right")), 1 | (2 << 3) | (6 << 6));
        assert_eq!(layer(&tags(&[("bridge", "yes")])), 1);
        assert_eq!(layer(&tags(&[("tunnel", "yes")])), -1);
        assert_eq!(layer(&tags(&[("bridge", "yes"), ("layer", "3")])), 3);
    }
}
