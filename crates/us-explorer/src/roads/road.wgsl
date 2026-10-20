// Road surfaces and markings, all procedural from per-vertex data (see roads/build.rs):
//   uv    = (lateral metres from the centre line, positive right of travel; metres along)
//   uv_b  = (segment length, or -(RGB565 + 1) paint for structures;
//            element + 8 * seed (8 bits) + 2048 * street-light layout (4 bits) + 32768 * high flags)
//   color = (half width; packed lanes and shoulders; low flags; packed turn arrows)
// Every packed value stays below 2^20 so interpolation can't corrupt it (we round).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

struct RoadParams {
    // 0 by day, 1 at night: tunnel lights and retroreflective paint.
    night: f32,
    // 0 dry .. 1 soaked (puddles in potholes, darker asphalt).
    wet: f32,
    time: f32,
    _pad: f32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> road: RoadParams;

const ONEWAY: u32 = 1u;
const XWALK_START: u32 = 2u;
const XWALK_END: u32 = 4u;
const STOP_START: u32 = 8u;
const STOP_END: u32 = 16u;
const JUNCTION: u32 = 32u;
const PATH: u32 = 64u;
const RAIL: u32 = 128u;
const CENTER_DOUBLE: u32 = 256u;
const CENTER_DASHED: u32 = 512u;
const EDGE_LINES: u32 = 1024u;
const LANE_LINES: u32 = 2048u;
const SIDEWALK: u32 = 262144u;
const STRUCTURE: u32 = 524288u;
// High flags (bit 20 up) arrive shifted down by 20.
const DECK: u32 = 1u;
const TUNNEL_LIGHT: u32 = 2u;
const STEPS: u32 = 8u;
const WATER: u32 = 16u;

const E_ROAD: u32 = 0u;
const E_JUNCTION: u32 = 1u;
const E_SIDEWALK: u32 = 2u;
const E_CURB: u32 = 3u;
const E_STRUCTURE: u32 = 4u;
const E_RAIL_BED: u32 = 5u;
const E_RAIL: u32 = 6u;
const E_TUNNEL: u32 = 7u;

fn hash(c: vec2<f32>) -> f32 {
    let q = vec2<u32>(vec2<i32>(floor(c)) + vec2<i32>(65536));
    var h = q.x * 0x27d4eb2du ^ (q.y + 0x9e3779b9u) * 0x85ebca6bu;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    return f32(h & 0xffffu) / 65535.0;
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    return vnoise(p) * 0.5 + vnoise(p * 2.03) * 0.3 + vnoise(p * 4.1) * 0.2;
}

// Anti-aliased band: 1 inside [a, b], soft over one pixel.
fn band(x: f32, a: f32, b: f32) -> f32 {
    let w = max(fwidth(x), 1e-4);
    return clamp(min(x - a, b - x) / w + 0.5, 0.0, 1.0);
}

// Dashes along v: `on` metres painted out of every `period`.
fn dashes(v: f32, on: f32, period: f32) -> f32 {
    return band(v - period * floor(v / period), 0.0, on);
}

// One turn arrow: a stem, a quarter-circle bend toward side `s`, a sideways head.
fn turn_arrow(p: vec2<f32>, s: f32) -> f32 {
    var a = band(p.x, -0.13, 0.13) * band(p.y, 0.0, 2.6);
    let c = vec2(s * 0.75, 2.6);
    let r = length(p - c);
    a = max(a, band(r, 0.62, 0.88) * select(0.0, 1.0, (p.x - c.x) * s < 0.0 && p.y > 2.6));
    // Head pointing sideways from x = 0.75 to 2.15 (in the turn direction).
    let hx = (p.x * s - 0.75) / 1.4;
    a = max(a, band(hx, 0.0, 1.0) * band(abs(p.y - 3.35), -1.0, 0.6 * (1.0 - hx)));
    return a;
}

// Lane-use arrow glyph in metres at MUTCD-like proportions: x across the lane centre,
// y along from the tail (0) to the tip (~5 m), pointing +y.
fn arrow(p: vec2<f32>, bits: u32) -> f32 {
    var a = 0.0;
    if (bits & 2u) != 0u {
        a = max(a, band(p.x, -0.13, 0.13) * band(p.y, 0.0, 3.3));
        // Head: triangle from y 3.3 to 5.0, 1.0 m wide at its base.
        let t = (p.y - 3.3) / 1.7;
        a = max(a, band(p.y, 3.3, 5.0) * band(abs(p.x), -1.0, 0.5 * (1.0 - t)));
    }
    if (bits & 1u) != 0u {
        a = max(a, turn_arrow(p, -1.0));
    }
    if (bits & 4u) != 0u {
        a = max(a, turn_arrow(p, 1.0));
    }
    return a;
}

// Pools of light from the inferred street lamps (layout as build::lamp_layout).
fn lamp_pool(s: f32, v: f32, hw: f32, len: f32, glow: u32) -> f32 {
    if (glow & 1u) == 0u {
        return 0.0;
    }
    let major = (glow & 2u) != 0u;
    let spacing = select(45.0, 36.0, major);
    let first = spacing * (0.5 + 0.125 * f32((glow >> 2u) & 3u));
    let arm = clamp(hw * 0.5, 1.2, 3.5);
    let reach = hw + 0.9 + select(0.0, 1.0, major) - arm;
    var total = 0.0;
    let k0 = floor((v - first) / spacing);
    for (var dk = 0.0; dk <= 1.0; dk += 1.0) {
        let k = k0 + dk;
        let lv = first + k * spacing;
        if k < 0.0 || lv >= len - 5.0 {
            continue;
        }
        let side = select(1.0, -1.0, major && (i32(k) % 2) == 1);
        let d = vec2(s - side * reach, v - lv);
        total += exp(-dot(d, d) / (2.0 * 7.5 * 7.5));
    }
    return total;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let s = in.uv.x;
    let v = in.uv.y;
    let len = in.uv_b.x;
    let code = u32(round(in.uv_b.y));
    let elem = code & 7u;
    let seed = f32((code >> 3u) & 0xffu);
    let glow = (code >> 11u) & 15u;
    let hi = code >> 15u;
#ifdef VERTEX_COLORS
    let hw = in.color.x;
    let lanes = u32(round(in.color.y));
    let flags = u32(round(in.color.z));
    let turns = u32(round(in.color.w));
#else
    let hw = 3.0;
    let lanes = 17u;
    let flags = 0u;
    let turns = 0u;
#endif
    let surface = (flags >> 12u) & 7u;
    let smoothness = (flags >> 15u) & 7u;
    let dist = distance(in.world_position.xyz, view.world_position);
    let near = 1.0 - smoothstep(60.0, 900.0, dist);
    // Noise space: along-road metres (stable across floating-origin rebases).
    var q = vec2(s, v + seed * 37.0);
    if elem == E_JUNCTION {
        q = in.uv + vec2(seed * 13.0, 0.0);
    }

    var base = vec3(0.21, 0.21, 0.22);
    var rough = 0.88;
    var paint = 0.0;
    var paint_col = vec3(0.92, 0.92, 0.9);
    var emissive = vec3(0.0);
    var bump = vec2(0.0);

    // ---------------------------------------------------------------- surfaces
    let grain = vnoise(q * 9.0) * 0.6 + vnoise(q * 31.0) * 0.4;
    let blotch = fbm(q * 0.15);
    switch surface {
        case 1u: { // concrete: lighter, with joints every 4.5 m
            base = vec3(0.56, 0.55, 0.52) * (0.9 + 0.12 * blotch + 0.05 * grain);
            let joint = 1.0 - band(v - 4.5 * floor(v / 4.5), 0.03, 4.47);
            base *= 1.0 - 0.35 * joint * near;
            rough = 0.8;
        }
        case 2u: { // pavers / bricks
            let b = vec2(q.x * 4.0 + 0.5 * floor(q.y * 8.0), q.y * 8.0);
            let mortar = 1.0 - band(fract(b.x), 0.06, 0.94) * band(fract(b.y), 0.08, 0.92);
            base = mix(vec3(0.45, 0.33, 0.28), vec3(0.55, 0.45, 0.38), hash(floor(b))) * (0.9 + 0.1 * grain);
            base = mix(base, vec3(0.3, 0.29, 0.27), mortar * near);
            rough = 0.85;
        }
        case 3u: { // gravel
            base = vec3(0.47, 0.44, 0.39) * (0.75 + 0.35 * vnoise(q * 22.0) + 0.1 * blotch);
            rough = 0.97;
            bump = vec2(vnoise(q * 22.0) - 0.5, vnoise(q.yx * 22.0) - 0.5) * 0.8;
        }
        case 4u: { // dirt
            base = vec3(0.42, 0.34, 0.25) * (0.8 + 0.25 * fbm(q * 1.5) + 0.08 * grain);
            rough = 0.98;
        }
        case 5u: { // wood boardwalk
            let plank = fract(v / 0.15);
            base = vec3(0.45, 0.33, 0.22) * (0.85 + 0.15 * hash(vec2(floor(v / 0.15), seed)));
            base *= 1.0 - 0.4 * (1.0 - band(plank, 0.06, 0.94)) * near;
        }
        case 6u: { // metal grating
            base = vec3(0.35, 0.36, 0.37);
            rough = 0.45;
        }
        default: { // asphalt: aggregate grain, patches, tyre-darkened lanes
            base = vec3(0.12, 0.12, 0.125) * (0.85 + 0.25 * grain * near + 0.14 * blotch);
            // Old asphalt is greyer.
            base = mix(base, vec3(0.26, 0.26, 0.25), 0.3 * fbm(q * 0.03 + 7.0));
        }
    }

    // ---------------------------------------------------------------- structures
    if (flags & STRUCTURE) != 0u || elem == E_STRUCTURE || elem == E_TUNNEL {
        let p = in.uv * vec2(1.0, 1.0);
        base = vec3(0.62, 0.61, 0.58) * (0.84 + 0.16 * fbm(p * 0.7 + seed) + 0.04 * grain);
        // Weathering streaks.
        base *= 1.0 - 0.18 * smoothstep(0.55, 0.9, vnoise(vec2(p.x * 3.0, 0.0) + seed));
        rough = 0.85;
        // Painted steel (truss, towers, cables): colour carried in uv_b.x.
        if len < -0.5 {
            let c = u32(round(-len - 1.0));
            let rgb = vec3(f32((c >> 11u) & 31u) / 31.0, f32((c >> 5u) & 63u) / 63.0, f32(c & 31u) / 31.0);
            base = rgb * rgb * (0.92 + 0.08 * grain);
            rough = 0.55;
            pbr_input.material.metallic = 0.3;
        }
        if elem == E_TUNNEL {
            // Tiled walls, a light strip along the ceiling.
            base = vec3(0.78, 0.77, 0.72) * (0.92 + 0.08 * grain);
            let tile = 1.0 - band(fract(v / 0.3), 0.04, 0.96) * band(fract(s / 0.3), 0.04, 0.96);
            base *= 1.0 - 0.2 * tile * near;
            if (hi & TUNNEL_LIGHT) != 0u && abs(s) < 0.4 {
                let lamp = band(v - 6.0 * floor(v / 6.0), 0.0, 3.0);
                emissive = vec3(1.0, 0.85, 0.6) * lamp * 9000.0;
            }
        }
    }

    // ---------------------------------------------------------------- sidewalks, curbs
    if elem == E_SIDEWALK {
        base = vec3(0.6, 0.59, 0.56) * (0.88 + 0.1 * blotch + 0.05 * grain);
        let slab = (1.0 - band(fract(v / 1.5), 0.012, 0.988)) * near;
        base *= 1.0 - 0.3 * slab;
        rough = 0.82;
    }
    if elem == E_CURB {
        base = vec3(0.66, 0.65, 0.62) * (0.9 + 0.08 * grain);
        rough = 0.8;
    }

    // ---------------------------------------------------------------- rail
    if elem == E_RAIL_BED {
        // Ballast, with sleepers every 0.6 m across the middle.
        base = vec3(0.4, 0.38, 0.35) * (0.7 + 0.45 * vnoise(q * 18.0));
        let sleeper = band(fract(v / 0.6), 0.0, 0.42) * band(abs(s), -1.0, 1.3);
        base = mix(base, vec3(0.28, 0.24, 0.2) * (0.85 + 0.2 * grain), sleeper);
        rough = 0.95;
    }
    if elem == E_RAIL {
        base = vec3(0.45, 0.42, 0.4);
        rough = 0.35;
        pbr_input.material.metallic = 0.85;
    }

    // ---------------------------------------------------------------- markings
    let is_road = elem == E_ROAD && (flags & (PATH | RAIL)) == 0u;
    if is_road {
        let nf = lanes & 15u;
        let nb = (lanes >> 4u) & 15u;
        let sl = f32((lanes >> 8u) & 63u) * 0.1;
        let sr = f32((lanes >> 14u) & 63u) * 0.1;
        let ntot = f32(max(nf + nb, 1u));
        let lw = max((2.0 * hw - sl - sr) / ntot, 1.0);
        let left = -hw + sl;          // left edge of the travelled way
        let centre = left + f32(nb) * lw;
        let line = 0.12;
        let yellow = vec3(0.95, 0.72, 0.12);
        // Paint fades in near the ends of the segment at junction mouths (no lines across).
        if (flags & EDGE_LINES) != 0u {
            let le = band(s, left - line * 0.5, left + line * 0.5);
            let re = band(s, hw - sr - line * 0.5, hw - sr + line * 0.5);
            let le_col = select(vec3(0.92), yellow, (flags & ONEWAY) != 0u);
            if le > 0.0 { paint = max(paint, le); paint_col = le_col; }
            if re > 0.0 { paint = max(paint, re); paint_col = vec3(0.92); }
        }
        if (flags & CENTER_DOUBLE) != 0u {
            let c = max(band(s, centre - 0.2, centre - 0.08), band(s, centre + 0.08, centre + 0.2));
            if c > 0.0 { paint = max(paint, c); paint_col = yellow; }
        } else if (flags & CENTER_DASHED) != 0u {
            let c = band(s, centre - line * 0.5, centre + line * 0.5) * dashes(v + seed, 3.0, 12.0);
            if c > 0.0 { paint = max(paint, c); paint_col = yellow; }
        }
        if (flags & LANE_LINES) != 0u {
            for (var k = 1u; k < 8u; k++) {
                if k < nb {
                    let x = left + f32(k) * lw;
                    paint = max(paint, band(s, x - line * 0.5, x + line * 0.5) * dashes(v + seed, 3.0, 12.0));
                }
                if k < nf {
                    let x = centre + f32(k) * lw;
                    let d = band(s, x - line * 0.5, x + line * 0.5) * dashes(v + seed, 3.0, 12.0);
                    if d > 0.0 { paint = max(paint, d); paint_col = vec3(0.92); }
                }
            }
        }
        // Stop lines (0.5 m wide) across the arriving lanes, set back behind any crosswalk.
        let to_end = len - v;
        let xw_end = (flags & XWALK_END) != 0u;
        let xw_start = (flags & XWALK_START) != 0u;
        if (flags & STOP_END) != 0u {
            let back = select(0.6, 4.2, xw_end);
            let st = band(to_end, back, back + 0.5) * band(s, centre, hw - sr);
            if st > 0.0 { paint = max(paint, st); paint_col = vec3(0.93); }
        }
        if (flags & STOP_START) != 0u {
            let back = select(0.6, 4.2, xw_start);
            let st = band(v, back, back + 0.5) * band(s, left, centre);
            if st > 0.0 { paint = max(paint, st); paint_col = vec3(0.93); }
        }
        // Continental crosswalks: bars along the road, 0.6 m on / 0.6 m off, 3 m deep.
        let bars = band(fract((s + 0.3) / 1.2), 0.0, 0.5);
        if xw_end {
            let c = band(to_end, 0.3, 3.3) * bars * band(s, -hw + 0.3, hw - 0.3);
            if c > 0.0 { paint = max(paint, c); paint_col = vec3(0.93); }
        }
        if xw_start {
            let c = band(v, 0.3, 3.3) * bars * band(s, -hw + 0.3, hw - 0.3);
            if c > 0.0 { paint = max(paint, c); paint_col = vec3(0.93); }
        }
        // Turn arrows, 6 m long, 9 m before the stop line, in each arriving lane.
        let tf = turns & 0x1ffu;
        let tb = (turns >> 9u) & 0x1ffu;
        // Arrows are 5 m long, their tips 8 m before the stop line.
        if tf != 0u && to_end > 7.5 && to_end < 13.5 {
            for (var k = 0u; k < 3u; k++) {
                let bits = (tf >> (k * 3u)) & 7u;
                if bits != 0u && k < nf {
                    let cx = centre + (f32(k) + 0.5) * lw;
                    let a = arrow(vec2(s - cx, 13.0 - to_end), bits);
                    if a > 0.0 { paint = max(paint, a); paint_col = vec3(0.93); }
                }
            }
        }
        if tb != 0u && v > 7.5 && v < 13.5 {
            for (var k = 0u; k < 3u; k++) {
                let bits = (tb >> (k * 3u)) & 7u;
                if bits != 0u && k < nb {
                    // Backward lanes are numbered from their own left (our right of centre).
                    let cx = centre - (f32(k) + 0.5) * lw;
                    let a = arrow(vec2(-(s - cx), 13.0 - v), bits);
                    if a > 0.0 { paint = max(paint, a); paint_col = vec3(0.93); }
                }
            }
        }
        // Worn paint.
        paint *= 0.75 + 0.25 * vnoise(q * vec2(3.0, 0.7));

        // Potholes and patches, mirrored on the CPU by build::pothole().
        let cell = floor(v / 6.0);
        let pp = array<f32, 8>(0.0, 0.004, 0.012, 0.03, 0.07, 0.13, 0.2, 0.3);
        var pr = pp[0];
        switch smoothness { case 1u: { pr = pp[1]; } case 2u: { pr = pp[2]; } case 3u: { pr = pp[3]; } case 4u: { pr = pp[4]; }
            case 5u: { pr = pp[5]; } case 6u: { pr = pp[6]; } case 7u: { pr = pp[7]; } default: {} }
        for (var dc = -1.0; dc <= 1.0; dc += 1.0) {
            let c = cell + dc;
            if hash(vec2(seed * 3.0 + 1.0, c)) < pr {
                let pv = c * 6.0 + 1.0 + hash(vec2(seed * 3.0 + 2.0, c)) * 4.0;
                let ps = (hash(vec2(seed * 3.0 + 3.0, c)) * 1.6 - 0.8) * hw;
                let r = 0.25 + 0.45 * hash(vec2(seed * 3.0 + 4.0, c));
                let d = length(vec2(s - ps, (v - pv) * 0.8)) / r;
                if d < 1.0 {
                    let wob = 1.0 + 0.25 * (vnoise(vec2(s, v) * 6.0) - 0.5);
                    if d < wob * 0.9 {
                        base = vec3(0.1, 0.095, 0.09) * (0.8 + 0.4 * vnoise(q * 40.0));
                        rough = 0.98;
                        paint = 0.0;
                        // Rain fills them.
                        if road.wet > 0.2 {
                            base = mix(base, vec3(0.05, 0.06, 0.07), road.wet);
                            rough = mix(rough, 0.05, road.wet);
                        }
                    } else {
                        base *= 0.75;
                    }
                }
            }
            // Crack-sealed patches, more common than potholes.
            if hash(vec2(seed * 5.0 + 7.0, c)) < pr * 3.0 + 0.02 {
                let pv = c * 6.0 + 3.0;
                let ps = (hash(vec2(seed * 5.0 + 8.0, c)) - 0.5) * hw;
                let box_ = band(abs(s - ps), -1.0, 0.6 + hash(vec2(c, seed)) * 1.0) * band(abs(v - pv), -1.0, 0.8 + hash(vec2(seed, c)) * 1.5);
                base = mix(base, base * 0.7, box_ * 0.8);
            }
        }
    }
    // Plain paths: a little darker at the edges.
    if elem == E_ROAD && (flags & PATH) != 0u {
        base *= 0.9 + 0.1 * band(abs(s), -1.0, hw - 0.15);
        if (hi & STEPS) != 0u {
            base *= 0.75 + 0.25 * band(fract(v / 0.32), 0.1, 1.0);
        }
    }

    // Wet roads: darker and glossier.
    let wet = road.wet * select(1.0, 0.6, surface >= 3u);
    base *= 1.0 - 0.45 * wet;
    rough = mix(rough, 0.25, wet * 0.8);

    var painted = mix(base, paint_col * (0.85 + 0.15 * grain), clamp(paint, 0.0, 1.0));

    // ---------------------------------------------------------------- inland water
    if (hi & WATER) != 0u {
        let t = road.time;
        let w = in.uv;
        // Two drifting ripple layers for the normal; deeper colour away from the light.
        let r1 = vec2(vnoise(w * 0.35 + vec2(t * 0.21, t * 0.13)), vnoise(w.yx * 0.35 - vec2(t * 0.17, t * 0.09)));
        let r2 = vec2(vnoise(w * 1.7 + vec2(-t * 0.5, t * 0.37)), vnoise(w.yx * 1.7 + vec2(t * 0.41, -t * 0.29)));
        let slope = (r1 - 0.5) * 0.35 + (r2 - 0.5) * 0.18 * (0.3 + 0.7 * near);
        let tn = normalize(cross(pbr_input.N, vec3(0.0, 0.0, 1.0)) + vec3(1e-4));
        let bn = cross(pbr_input.N, tn);
        pbr_input.N = normalize(pbr_input.N + tn * slope.x + bn * slope.y);
        painted = vec3(0.025, 0.07, 0.08) * (0.85 + 0.3 * fbm(w * 0.02));
        pbr_input.material.base_color = vec4(painted, 1.0);
        pbr_input.material.perceptual_roughness = 0.07 + 0.15 * road.wet;
        pbr_input.material.metallic = 0.0;
        pbr_input.material.emissive = vec4(0.0, 0.0, 0.0, 1.0);
        var wout: FragmentOutput;
        wout.color = apply_pbr_lighting(pbr_input);
        wout.color = main_pass_post_lighting_processing(pbr_input, wout.color);
        return wout;
    }

    // Bumpy surface catches the light up close.
    if near > 0.0 {
        let e = 0.05;
        let h0 = vnoise(q * 9.0);
        let hx = vnoise((q + vec2(e, 0.0)) * 9.0);
        let hy = vnoise((q + vec2(0.0, e)) * 9.0);
        let t = normalize(cross(pbr_input.N, vec3(0.0, 1.0, 0.0)) + vec3(1e-4));
        let b = cross(pbr_input.N, t);
        pbr_input.N = normalize(pbr_input.N + (t * ((hx - h0) / e + bump.x) + b * ((hy - h0) / e + bump.y)) * 0.012 * near);
    }

    pbr_input.material.base_color = vec4(painted, 1.0);
    pbr_input.material.perceptual_roughness = mix(rough, 0.6, paint * 0.5);
    // Street lights light the road and sidewalk below them, seen from any distance.
    if road.night > 0.01 && (elem == E_ROAD || elem == E_SIDEWALK || elem == E_CURB) {
        let pool = lamp_pool(s, v, hw, len, glow);
        emissive += painted * vec3(1.0, 0.78, 0.52) * pool * 9000.0 * road.night;
    }
    // Lights are in daytime units; exposure opens up ~10 stops at night, so scale down.
    pbr_input.material.emissive = vec4(emissive * exp2(-10.0 * road.night), 1.0);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
