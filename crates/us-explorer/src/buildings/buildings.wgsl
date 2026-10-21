// Building surfaces, all procedural from per-vertex data (see buildings/exterior.rs):
//   uv    = walls: (metres along the facade segment, metres above the ground floor);
//           roofs: (metres along the eave, metres up the slope)
//   uv_b  = (facade segment length, packed code: element | material << 4 |
//            windows << 8 | floor height * 8 << 11 | storefront << 17 | gable << 18)
//   color = (rgb colour, wall height * 4 | seed << 14)
// Windows show a fake room behind the glass (interior mapping) and light up at night;
// buildings shown with real interiors discard the glass so you see inside.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

struct BuildingParams {
    // 0 by day, 1 at night.
    night: f32,
    // 1: see through the windows (a real interior is behind them).
    cutout: f32,
    wet: f32,
    time: f32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> bld: BuildingParams;

const E_WALL: u32 = 0u;
const E_ROOF: u32 = 1u;
const E_FOUNDATION: u32 = 2u;
const E_TRIM: u32 = 3u;
const E_GLASS: u32 = 4u;
const E_DOOR: u32 = 5u;
const E_GARAGE: u32 = 6u;
const E_EQUIP: u32 = 7u;
const E_CHIMNEY: u32 = 8u;
const E_PORCH: u32 = 9u;
const E_POST: u32 = 10u;
const E_SOFFIT: u32 = 11u;
const E_SIGN: u32 = 12u;
const E_AWNING: u32 = 13u;

// Wall materials.
const W_BRICK: u32 = 0u;
const W_SIDING: u32 = 1u;
const W_STUCCO: u32 = 2u;
const W_CONCRETE: u32 = 3u;
const W_GLASS: u32 = 4u;
const W_STONE: u32 = 5u;
const W_SHINGLE: u32 = 6u;
const W_METAL: u32 = 7u;
const W_PLASTER: u32 = 8u;

// Roof materials.
const R_SHINGLES: u32 = 0u;
const R_CLAY: u32 = 1u;
const R_METAL: u32 = 2u;
const R_MEMBRANE: u32 = 3u;
const R_GRAVEL: u32 = 4u;
const R_SLATE: u32 = 5u;
const R_COPPER: u32 = 6u;
const R_GLASS: u32 = 7u;

// Window styles.
const WIN_HOUSE: u32 = 0u;
const WIN_PUNCHED: u32 = 1u;
const WIN_RIBBON: u32 = 2u;
const WIN_CURTAIN: u32 = 3u;
const WIN_CLERESTORY: u32 = 4u;
const WIN_TALL: u32 = 5u;
const WIN_NONE: u32 = 6u;

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

fn band(x: f32, a: f32, b: f32) -> f32 {
    let w = max(fwidth(x), 1e-4);
    return clamp(min(x - a, b - x) / w + 0.5, 0.0, 1.0);
}

struct Layout {
    bays: f32,
    w: f32,
    sill: f32,
    h: f32,
};

// Mirrors exterior::window_layout.
fn win_layout(style: u32, len: f32, fh: f32) -> Layout {
    var bay = 1.0;
    var w = 0.0;
    var sill = 0.0;
    var h = 0.0;
    switch style {
        case 0u: { bay = 3.4; w = 0.95; sill = 0.9; h = 1.35; }
        case 1u: { bay = 2.7; w = 1.25; sill = 0.85; h = 1.5; }
        case 2u: { bay = 1.6; w = 1.48; sill = 0.95; h = max(fh - 1.6, 1.2); }
        case 3u: { bay = 1.55; w = 1.47; sill = 0.0; h = fh - 0.2; }
        case 4u: { bay = 6.0; w = 3.6; sill = 0.0; h = 1.1; }
        case 5u: { bay = 3.6; w = 1.3; sill = 1.0; h = max(fh - 2.2, 1.8); }
        default: { }
    }
    var out: Layout;
    if style == WIN_NONE || len < 1.6 {
        out.bays = 0.0;
        return out;
    }
    let n = max(round(len / bay), 1.0);
    let b = len / n;
    out.bays = n;
    out.w = max(min(w, b - 0.35), 0.0);
    out.sill = sill;
    out.h = h;
    return out;
}

struct Win {
    // Inside a window pane (0..1, anti-aliased).
    pane: f32,
    // Inside the frame (pane plus the frame border).
    frame: f32,
    // Position inside the pane: x 0..1 across, y 0..1 up.
    local: vec2<f32>,
    size: vec2<f32>,
    // Which room (bay, floor), for variation.
    room: vec2<f32>,
    arched: f32,
};

fn windows(u: f32, v: f32, len: f32, fh: f32, style: u32, storefront: bool, wall_top: f32) -> Win {
    var r: Win;
    let l = win_layout(style, len, fh);
    if l.bays < 0.5 || l.w <= 0.0 {
        return r;
    }
    let bay = len / l.bays;
    let bi = clamp(floor(u / bay), 0.0, l.bays - 1.0);
    let cx = (bi + 0.5) * bay;
    var gf = fh;
    if storefront { gf = 4.6; }
    let top = wall_top - select(0.6, 0.3, style == WIN_HOUSE);
    var x0 = cx - l.w * 0.5;
    var x1 = cx + l.w * 0.5;
    var y0 = -1.0;
    var y1 = -1.0;
    var floor_i = 0.0;
    if storefront && v < gf {
        let sw = max(bay - 0.3, 0.5);
        x0 = cx - sw * 0.5;
        x1 = cx + sw * 0.5;
        y0 = 0.45;
        y1 = gf - 1.0;
    } else if style == WIN_CLERESTORY {
        y0 = max(wall_top - 1.8, 2.4);
        y1 = max(wall_top - 0.7, 3.4);
    } else if v < gf {
        y0 = l.sill;
        y1 = min(l.sill + l.h, gf - 0.25);
    } else {
        floor_i = floor((v - gf) / fh);
        let base = gf + floor_i * fh;
        floor_i += 1.0;
        if base + fh * 0.6 < top {
            y0 = base + l.sill;
            y1 = min(base + l.sill + l.h, top);
        }
    }
    if y1 <= y0 + 0.4 {
        return r;
    }
    var fw = 0.07;
    if style == WIN_CURTAIN { fw = 0.04; }
    r.frame = band(u, x0 - fw, x1 + fw) * band(v, y0 - fw, y1 + fw);
    r.pane = band(u, x0, x1) * band(v, y0, y1);
    // Arched tops for tall windows.
    if style == WIN_TALL && v > y1 - (x1 - x0) * 0.5 {
        let c = vec2(cx, y1 - (x1 - x0) * 0.5);
        let d = length(vec2(u, v) - c);
        r.pane *= 1.0 - smoothstep((x1 - x0) * 0.5 - 0.02, (x1 - x0) * 0.5, d);
        r.frame *= 1.0 - smoothstep((x1 - x0) * 0.5 + fw - 0.02, (x1 - x0) * 0.5 + fw, d);
        r.arched = 1.0;
    }
    r.local = vec2((u - x0) / (x1 - x0), (v - y0) / (y1 - y0));
    r.size = vec2(x1 - x0, y1 - y0);
    r.room = vec2(bi, floor_i);
    return r;
}

// A room seen through a window: march the view ray into a box behind the glass and
// shade whichever face it hits. `p` is the point on the glass in room metres
// (x across, y up), `d` the view ray in (across, up, into the wall).
fn fake_room(p: vec2<f32>, d: vec3<f32>, room: vec2<f32>, size: vec2<f32>, depth: f32, seed: f32, lit: f32) -> vec3<f32> {
    let w = size.x + 1.4;
    let h = size.y + 1.2;
    let o = vec3(p.x + 0.7, p.y + 0.9, 0.0);
    let dd = vec3(d.x, d.y, max(d.z, 1e-3));
    // Distances to the planes: back wall, side walls, floor/ceiling.
    let tz = depth / dd.z;
    let tx = select((w - o.x) / dd.x, -o.x / dd.x, dd.x < 0.0);
    let ty = select((h - o.y) / dd.y, -o.y / dd.y, dd.y < 0.0);
    let t = min(tz, min(abs(tx), abs(ty)));
    let q = o + dd * t;
    let rh = hash(room + vec2(seed * 7.0, 3.0));
    let wallc = mix(vec3(0.75, 0.72, 0.66), vec3(0.58, 0.64, 0.7), rh) * (0.85 + 0.15 * hash(room + vec2(1.0, seed)));
    var c = wallc;
    if t == ty {
        if dd.y < 0.0 {
            // Floor: wood or carpet.
            c = mix(vec3(0.42, 0.3, 0.2), vec3(0.45, 0.43, 0.4), step(0.5, hash(room + vec2(5.0, seed))));
        } else {
            // Ceiling with a light.
            c = vec3(0.85, 0.85, 0.83);
            let lc = length(q.xz - vec2(w * 0.5, depth * 0.5));
            c += vec3(1.0, 0.9, 0.7) * (1.0 - smoothstep(0.2, 0.45, lc)) * lit * 2.0;
        }
    } else if t == tz {
        // Back wall: a door or furniture silhouette now and then.
        c = wallc * 0.9;
        let fx = fract(rh * 13.0) * (w - 1.5) + 0.3;
        let furn = band(q.x, fx, fx + 1.2) * band(q.y, 0.0, 0.9 + rh * 0.8);
        c = mix(c, vec3(0.25, 0.2, 0.16) + rh * 0.2, furn * step(0.35, rh));
    } else {
        c = wallc * 0.8;
    }
    // Darker toward the back (room not lit by the window).
    c *= 0.55 + 0.45 * exp(-t * 0.25);
    // Blinds part way down in some rooms.
    let blind = hash(room + vec2(seed, 11.0));
    if blind > 0.6 && p.y > size.y * (1.0 - (blind - 0.6) * 2.0) {
        c = mix(vec3(0.82, 0.8, 0.74), vec3(0.95, 0.88, 0.7) * (1.0 + lit), 0.5 + 0.5 * step(0.5, fract(p.y * 18.0)));
    }
    return c;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let u = in.uv.x;
    let v = in.uv.y;
    let len = in.uv_b.x;
    let code = u32(round(in.uv_b.y));
    let elem = code & 15u;
    let mat = (code >> 4u) & 15u;
    let wstyle = (code >> 8u) & 7u;
    let fh = f32((code >> 11u) & 63u) / 8.0;
    let storefront = ((code >> 17u) & 1u) != 0u;
    let gable = ((code >> 18u) & 1u) != 0u;
#ifdef VERTEX_COLORS
    let base_col = in.color.rgb;
    let packed = u32(round(in.color.a));
#else
    let base_col = vec3(0.7);
    let packed = 0u;
#endif
    let wall_top = f32(packed & 16383u) / 4.0;
    let seed = f32((packed >> 14u) & 1023u);
    let dist = distance(in.world_position.xyz, view.world_position);
    let near = 1.0 - smoothstep(40.0, 600.0, dist);
    let q = vec2(u, v);

    var base = base_col;
    var rough = 0.85;
    var metal = 0.0;
    var emissive = vec3(0.0);
    var bump = vec2(0.0);
    let grain = vnoise(q * 11.0 + seed) * 0.6 + vnoise(q * 37.0) * 0.4;

    if elem == E_WALL || elem == E_CHIMNEY {
        switch mat {
            case 0u: { // brick: running bond, 0.2 x 0.075 m courses, recessed mortar
                let row = floor(v / 0.075);
                let bu = u / 0.2 + 0.5 * (row % 2.0);
                let cell = vec2(floor(bu), row);
                let fu = fract(bu);
                let fv = fract(v / 0.075);
                let mortar = 1.0 - band(fu, 0.05, 0.95) * band(fv, 0.13, 0.95);
                let tone = 0.82 + 0.3 * hash(cell + seed) + 0.08 * grain;
                base = mix(base_col * tone, vec3(0.7, 0.68, 0.64), mortar * near);
                bump = vec2(select(0.0, sign(0.5 - fu), mortar > 0.5), select(0.0, sign(0.5 - fv), mortar > 0.5)) * 0.4 * near;
                rough = 0.9;
            }
            case 1u: { // lap siding: 0.18 m boards, a shadow line under each
                let fv = fract(v / 0.18);
                let shade = 0.78 + 0.22 * smoothstep(0.0, 0.25, fv);
                base = base_col * shade * (0.95 + 0.05 * vnoise(vec2(u * 0.6, floor(v / 0.18))));
                bump = vec2(0.0, (0.5 - fv) * 0.5) * near;
                rough = 0.6;
            }
            case 2u, 8u: { // stucco / plaster
                base = base_col * (0.9 + 0.1 * fbm(q * 3.0 + seed) + 0.05 * grain);
                bump = (vec2(vnoise(q * 20.0), vnoise(q.yx * 20.0)) - 0.5) * 0.35 * near;
                rough = 0.95;
            }
            case 3u: { // concrete panels with joints and tie holes
                let pj = 1.0 - band(fract(u / 3.0), 0.006, 0.994) * band(fract(v / 1.5), 0.012, 0.988);
                base = base_col * (0.88 + 0.12 * fbm(q * 0.8 + seed)) * (1.0 - 0.25 * pj * near);
                let ties = 1.0 - smoothstep(0.02, 0.035, length(fract(q / vec2(0.75, 0.75)) - 0.5));
                base *= 1.0 - 0.3 * ties * near;
                rough = 0.9;
            }
            case 4u: { // glass curtain wall: spandrels and mullions handled by the window code
                base = base_col * 0.6;
                rough = 0.12;
                metal = 0.4;
            }
            case 5u: { // stone: irregular coursed blocks
                let row = floor(v / 0.4);
                let bu = u / (0.6 + 0.3 * hash(vec2(row, seed))) + hash(vec2(row, 3.0));
                let cell = vec2(floor(bu), row);
                let mortar = 1.0 - band(fract(bu), 0.04, 0.96) * band(fract(v / 0.4), 0.06, 0.96);
                base = mix(base_col * (0.8 + 0.35 * hash(cell)) * (0.9 + 0.1 * fbm(q * 4.0)), vec3(0.6, 0.58, 0.55), mortar * near);
                rough = 0.95;
            }
            case 6u: { // wood shingles: staggered 0.15 x 0.13 m
                let row = floor(v / 0.13);
                let bu = u / 0.15 + 0.5 * (row % 2.0) + 0.3 * hash(vec2(row, seed));
                let cell = vec2(floor(bu), row);
                let edge = 1.0 - band(fract(bu), 0.04, 0.96);
                base = base_col * (0.75 + 0.4 * hash(cell)) * (1.0 - 0.3 * edge * near) * (0.85 + 0.15 * smoothstep(0.0, 0.3, fract(v / 0.13)));
                rough = 0.9;
            }
            case 7u: { // corrugated metal
                let c = sin(u * 6.2832 / 0.076);
                base = base_col * (0.88 + 0.08 * c);
                bump = vec2(c * 0.5, 0.0) * near;
                rough = 0.45;
                metal = 0.6;
            }
            default: {}
        }
        // Weathering: a little darker toward the ground and streaks under sills.
        base *= 0.92 + 0.08 * smoothstep(0.0, 1.5, v);

        // Windows (not on gable ends or chimneys).
        if elem == E_WALL && !gable {
            let wn = windows(u, v, len, fh, wstyle, storefront, wall_top);
            var curtain = wstyle == WIN_CURTAIN;
            if wn.frame > 0.0 {
                // Frame colour: white on houses, dark metal elsewhere.
                var frame_col = vec3(0.92, 0.92, 0.9);
                if wstyle != WIN_HOUSE { frame_col = vec3(0.16, 0.17, 0.18); }
                if curtain { frame_col = vec3(0.28, 0.3, 0.32); }
                base = mix(base, frame_col, wn.frame);
                rough = mix(rough, 0.4, wn.frame);
                // Shutters beside some house windows.
                if wstyle == WIN_HOUSE && hash(vec2(seed, 41.0)) > 0.55 {
                    let sw = wn.size.x * 0.45;
                    let lx = wn.local.x * wn.size.x;
                    let sh = band(wn.local.y, 0.0, 1.0) * (band(lx, -sw - 0.08, -0.08) + band(lx, wn.size.x + 0.08, wn.size.x + sw + 0.08));
                    let shut_col = mix(vec3(0.1, 0.12, 0.1), vec3(0.15, 0.2, 0.32), step(0.75, hash(vec2(seed, 43.0))));
                    base = mix(base, shut_col * (0.85 + 0.15 * step(0.5, fract(wn.local.y * 12.0))), clamp(sh, 0.0, 1.0));
                }
            }
            if wn.pane > 0.0 {
                // See-through windows with a real interior behind.
                if bld.cutout > 0.5 && wn.pane > 0.5 {
                    discard;
                }
                // Muntins on house windows, horizontal divider on ribbons.
                var bars = 0.0;
                if wstyle == WIN_HOUSE {
                    bars = band(abs(wn.local.y - 0.5) * wn.size.y, -1.0, 0.025) + band(abs(wn.local.x - 0.5) * wn.size.x, -1.0, 0.02);
                } else if curtain {
                    bars = band(abs(wn.local.y - 0.33) * wn.size.y, -1.0, 0.02);
                }
                // The inset: a shadow along the top and left of each pane.
                let inset_shadow = smoothstep(0.0, 0.08 / wn.size.y, 1.0 - wn.local.y) * smoothstep(0.0, 0.08 / wn.size.x, wn.local.x);
                // View ray in wall space: across = u direction, up, into the wall.
                let n = normalize(in.world_normal);
                let up = vec3(0.0, 1.0, 0.0);
                let tu = normalize(cross(n, up) + vec3(1e-5));
                let vd = normalize(in.world_position.xyz - view.world_position);
                let rd = vec3(dot(vd, tu), dot(vd, up), -dot(vd, n));
                let room_seed = seed + wn.room.x * 0.37;
                // Some rooms lit after dark (more in towns' commercial cores late evening).
                let lit_r = hash(wn.room + vec2(seed * 3.0, 17.0));
                let lit = select(0.0, 1.0, lit_r < 0.38) * bld.night;
                var depth = 3.5;
                if curtain || wstyle == WIN_RIBBON { depth = 7.0; }
                if storefront && v < 4.6 { depth = 9.0; }
                let room = fake_room(wn.local * wn.size, rd, wn.room, wn.size, depth, room_seed, lit);
                let glass_tint = vec3(0.55, 0.62, 0.65);
                base = mix(base, vec3(0.02, 0.025, 0.03), wn.pane);
                rough = mix(rough, 0.04, wn.pane);
                metal = mix(metal, 0.1, wn.pane);
                bump = mix(bump, vec2(0.0), wn.pane);
                // Interior light comes out through the glass (daylit rooms by day,
                // lamp-lit ones at night).
                let day_in = room * glass_tint * 700.0 * (1.0 - bld.night);
                let night_in = room * vec3(1.0, 0.82, 0.6) * 2600.0 * lit;
                emissive += (day_in + night_in) * wn.pane * (1.0 - bars) * (0.75 + 0.25 * inset_shadow);
                base = mix(base, vec3(0.9), bars * wn.pane);
            } else if curtain {
                // Spandrel panels between floors on curtain walls.
                base = mix(base_col * 0.45, base_col, 0.5);
                rough = 0.18;
                metal = 0.5;
            }
        } else if gable && elem == E_WALL && mat == W_SIDING {
            // Gable ends of houses: a small round vent.
            let c = vec2(len * 0.5, wall_top + 1.2);
            let d = length(q - c);
            base = mix(base, vec3(0.9), 1.0 - smoothstep(0.28, 0.3, d));
        }
    } else if elem == E_ROOF {
        switch mat {
            case 0u: { // asphalt shingles: 0.14 m courses, 0.33 m tabs, offset per course
                let row = floor(v / 0.14);
                let bu = u / 0.33 + 0.5 * (row % 2.0);
                let cell = vec2(floor(bu), row);
                let fv = fract(v / 0.14);
                let slot = 1.0 - band(fract(bu), 0.03, 0.97);
                let lip = smoothstep(0.0, 0.18, fv);
                base = base_col * (0.75 + 0.35 * hash(cell + seed) + 0.15 * grain) * (0.7 + 0.3 * lip) * (1.0 - 0.35 * slot * near);
                bump = vec2(0.0, (0.5 - fv) * 0.9) * near;
                rough = 0.95;
            }
            case 1u: { // clay barrel tiles
                let fu = fract(u / 0.25);
                let row = floor(v / 0.33);
                let fv = fract(v / 0.33);
                let curve = sin(fu * 3.14159);
                base = base_col * (0.7 + 0.35 * curve) * (0.85 + 0.25 * hash(vec2(floor(u / 0.25), row) + seed)) * (0.75 + 0.25 * smoothstep(0.0, 0.2, fv));
                bump = vec2(cos(fu * 3.14159) * 0.8, (0.5 - fv) * 0.5) * near;
                rough = 0.7;
            }
            case 2u, 6u: { // standing seam metal (copper weathers green)
                let fu = fract(u / 0.45);
                let seam = 1.0 - band(fu, 0.02, 0.98);
                base = base_col * (0.9 + 0.1 * vnoise(q * 0.5)) * (1.0 - 0.2 * seam);
                bump = vec2(seam * 0.8, 0.0) * near;
                rough = 0.35;
                metal = 0.7;
                if mat == R_COPPER { base = mix(vec3(0.33, 0.55, 0.47), vec3(0.45, 0.3, 0.2), vnoise(q * 0.3) * 0.3); metal = 0.2; }
            }
            case 3u: { // membrane: big seams
                let s = 1.0 - band(fract(u / 3.0), 0.003, 0.997);
                base = base_col * (0.9 + 0.1 * fbm(q * 0.3)) * (1.0 - 0.12 * s);
                rough = 0.8;
            }
            case 4u: { // gravel ballast
                base = base_col * (0.75 + 0.35 * vnoise(q * 24.0) + 0.1 * fbm(q * 0.5));
                rough = 0.98;
            }
            case 5u: { // slate
                let row = floor(v / 0.22);
                let bu = u / 0.3 + 0.5 * (row % 2.0);
                let edge = 1.0 - band(fract(bu), 0.03, 0.97);
                base = vec3(0.25, 0.27, 0.3) * (0.8 + 0.3 * hash(vec2(floor(bu), row))) * (1.0 - 0.3 * edge * near) * (0.8 + 0.2 * smoothstep(0.0, 0.2, fract(v / 0.22)));
                rough = 0.6;
            }
            case 7u: { // glass roof
                base = vec3(0.05, 0.07, 0.08);
                rough = 0.05;
            }
            default: {}
        }
    } else if elem == E_FOUNDATION {
        base = vec3(0.6, 0.58, 0.55) * (0.85 + 0.15 * fbm(q * 2.0));
        rough = 0.95;
    } else if elem == E_DOOR {
        // Closed doors: panel doors for houses, glass doors with a push bar for shops.
        let w = len;
        let x = u;
        if storefront || w > 1.4 {
            let leaf = band(fract(x / (w * 0.5)) * w * 0.5, 0.06, w * 0.5 - 0.06) * band(v, 0.1, 2.1);
            base = mix(base_col, vec3(0.03, 0.035, 0.04), leaf);
            rough = mix(0.4, 0.05, leaf);
            let room = vec3(0.3, 0.28, 0.25);
            emissive += room * 500.0 * leaf * (1.0 - bld.night) + vec3(1.0, 0.85, 0.6) * 1500.0 * leaf * bld.night;
            base = mix(base, vec3(0.75), band(v, 1.0, 1.06) * leaf);
        } else {
            // Six-panel door with a brass knob.
            let px = fract(x / (w * 0.5));
            let py = v / 2.1;
            let panel = band(px, 0.15, 0.85) * (band(py, 0.07, 0.3) + band(py, 0.38, 0.6) + band(py, 0.68, 0.93));
            base = base_col * (1.0 - 0.18 * panel) * (0.95 + 0.05 * grain);
            let knob = 1.0 - smoothstep(0.03, 0.04, length(vec2(x - w * 0.86, v - 1.0)));
            base = mix(base, vec3(0.8, 0.65, 0.3), knob);
            metal = knob;
            rough = 0.5;
        }
    } else if elem == E_GARAGE {
        // Sectional garage door: four raised-panel sections.
        let sec = fract(v / 0.54);
        let pnl = band(fract(u / (len / max(round(len / 1.2), 1.0))), 0.08, 0.92) * band(sec, 0.12, 0.88);
        base = base_col * (0.85 + 0.1 * pnl) * (0.8 + 0.2 * smoothstep(0.0, 0.08, sec));
        rough = 0.5;
    } else if elem == E_EQUIP {
        let grille = step(0.5, fract(v * 20.0));
        base = vec3(0.7, 0.71, 0.7) * (0.85 + 0.15 * grille * step(0.3, v - floor(v)));
        rough = 0.5;
        metal = 0.6;
    } else if elem == E_PORCH {
        let plank = fract(u / 0.14);
        base = base_col * (0.85 + 0.15 * hash(vec2(floor(u / 0.14), seed))) * (1.0 - 0.3 * (1.0 - band(plank, 0.04, 0.96)));
        rough = 0.8;
    } else if elem == E_SOFFIT || elem == E_POST || elem == E_TRIM {
        base = base_col * (0.92 + 0.08 * grain);
        rough = 0.7;
    } else if elem == E_SIGN {
        base = base_col;
        rough = 0.4;
        // Back-lit sign band at night (blank: no branding).
        emissive += vec3(0.9, 0.9, 0.85) * 1200.0 * bld.night * band(v, 0.12, 0.68);
    } else if elem == E_AWNING {
        let stripe = step(0.5, fract(u / 0.45));
        base = base_col * (0.85 + 0.25 * stripe * step(0.5, hash(vec2(seed, 9.0))));
        rough = 0.9;
    } else if elem == E_GLASS {
        base = vec3(0.02, 0.025, 0.03);
        rough = 0.04;
    }

    // Rain darkens and glosses.
    base *= 1.0 - 0.35 * bld.wet;
    rough = mix(rough, 0.2, bld.wet * 0.7);

    if near > 0.0 && dot(bump, bump) > 0.0 {
        let n = pbr_input.N;
        let t = normalize(cross(n, vec3(0.0, 1.0, 0.0)) + vec3(1e-4));
        let b = cross(n, t);
        pbr_input.N = normalize(n + (t * bump.x + b * bump.y) * 0.25 * near);
    }
    pbr_input.material.base_color = vec4(base, 1.0);
    pbr_input.material.perceptual_roughness = rough;
    pbr_input.material.metallic = metal;
    pbr_input.material.emissive = vec4(emissive * exp2(-10.0 * bld.night), 1.0);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
