// Terrain shading: satellite imagery as albedo, with procedural ground detail faded in
// near the camera (grass, asphalt, dirt, rock) and rock on steep slopes at any range,
// so imagery stops looking blurry at your feet or stretched on cliffs.
//
// uv_b holds Web Mercator metres wrapped to a 4096 m period, continuous across tiles,
// so every noise below is built to repeat exactly on that period.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::{view, globals},
}

struct TerrainParams {
    // Swell: direction of travel (east, south), height (m), period (s).
    swell: vec4<f32>,
    // Wind: direction of travel (east, south), speed (m/s), wind-sea height (m).
    wind: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sea: TerrainParams;

// One deep-water wave: returns (height, d/dx, d/dy) at p (metres, x east, y south).
// The wave vector is snapped to whole cycles per PERIOD so waves line up across tiles
// (each tile's detail coordinates are offset by whole periods).
fn wave(p: vec2<f32>, dir: vec2<f32>, wavelength: f32, amp: f32, t: f32, phase0: f32) -> vec3<f32> {
    let q = 6.2831853 / PERIOD;
    var kv = round(dir * (6.2831853 / wavelength) / q) * q;
    if dot(kv, kv) < 1e-10 {
        kv = vec2<f32>(q, 0.0);
    }
    let k = length(kv);
    let w = sqrt(9.81 * k);
    let ph = dot(kv, p) - w * t + phase0;
    let c = cos(ph);
    return vec3<f32>(amp * sin(ph), amp * c * kv.x, amp * c * kv.y);
}

const PERIOD: f32 = 4096.0;

fn hash(c: vec2<f32>) -> f32 {
    let q = vec2<u32>(vec2<i32>(c));
    var h = q.x * 0x27d4eb2du ^ (q.y + 0x9e3779b9u) * 0x85ebca6bu;
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    return f32(h & 0xffffu) / 65535.0;
}

// Value noise with cells of `cell` metres, wrapped so it tiles every PERIOD metres.
fn vnoise(p: vec2<f32>, cell: f32) -> f32 {
    let n = PERIOD / cell;
    let x = p / cell;
    let i = floor(x);
    let f = x - i;
    let u = f * f * (3.0 - 2.0 * f);
    let i0 = i - n * floor(i / n);
    let i1 = (i0 + 1.0) - n * floor((i0 + 1.0) / n);
    let a = hash(i0);
    let b = hash(vec2(i1.x, i0.y));
    let c = hash(vec2(i0.x, i1.y));
    let d = hash(i1);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>, cell: f32) -> f32 {
    return vnoise(p, cell) * 0.5 + vnoise(p, cell * 0.5) * 0.3 + vnoise(p, cell * 0.25) * 0.2;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var base = pbr_input.material.base_color.rgb;

    // Up is away from the planet centre, which sits ~6371 km below the floating origin.
    let up = normalize(in.world_position.xyz + vec3(0.0, 6371000.0, 0.0));
    let slope = 1.0 - clamp(dot(normalize(in.world_normal), up), 0.0, 1.0);
    let dist = distance(in.world_position.xyz, view.world_position);
    // Skirts (crack fillers hanging under tile edges) carry a large offset in uv_b.x.
    let skirt = in.uv_b.x > 500000.0;
    let p = in.uv_b - vec2(select(0.0, 1048576.0, skirt), 0.0);
    let lum = dot(base, vec3(0.299, 0.587, 0.114));

    // The sea: vertex colour alpha carries the water depth as 1 / (1 + depth), so the
    // colour itself stays white (the standard path multiplies it into the imagery).
#ifdef VERTEX_COLORS
    let depth = 1.0 / max(in.color.a, 1e-4) - 1.0;
#else
    let depth = 0.0;
#endif
    let water = select(smoothstep(0.03, 0.5, depth), 0.0, skirt);
    // Land just above the sea carries its height the same way (alpha above 1).
    let above_sea = select(-1.0, -depth / max(1.0 + depth, 1e-3), depth < 0.0);

    // Rock on steep ground: imagery taken from above smears on cliffs.
    let steep = smoothstep(0.28, 0.55, slope) * (1.0 - water);
    if steep > 0.0 && !skirt {
        let strata = vnoise(vec2(p.x + p.y, dot(in.world_position.xyz, up) * 4.0), 16.0);
        let rock = vec3(lum) * vec3(1.06, 1.0, 0.93) * (0.72 + 0.5 * fbm(p, 8.0)) * (0.85 + 0.3 * strata);
        base = mix(base, mix(rock, base, 0.2), steep);
    }

    // Close-up detail, faded out with distance so it never shimmers far away.
    let near = (1.0 - smoothstep(40.0, 420.0, dist)) * (1.0 - water);
    if near > 0.001 {
        let green = clamp((base.g - max(base.r, base.b)) * 9.0, 0.0, 1.0);
        let chroma = max(max(base.r, base.g), base.b) - min(min(base.r, base.g), base.b);
        let grey = (1.0 - clamp(chroma * 7.0, 0.0, 1.0)) * (1.0 - green);
        let brown = clamp((base.r - base.b) * 5.0, 0.0, 1.0) * (1.0 - green);

        let fine = vnoise(p, 0.25);
        let blades = vnoise(p, 0.125);
        let grass = base * (0.78 + 0.36 * mix(fbm(p, 2.0), blades, 0.55)) * vec3(0.97, 1.03, 0.95);
        let asphalt = base * (0.9 + 0.2 * (fine * 0.6 + vnoise(p, 1.0) * 0.4));
        let dirt = base * (0.8 + 0.32 * fbm(p, 1.5)) * (0.94 + 0.12 * fine);
        var detail = base * (0.86 + 0.28 * fbm(p, 1.0));
        detail = mix(detail, dirt, brown);
        detail = mix(detail, asphalt, grey);
        detail = mix(detail, grass, green);
        base = mix(base, detail, near);

        // Small bumps so close ground catches the light instead of looking painted.
        // Detail x runs east (+X) and detail y runs south (+Z) near the origin.
        let e = 0.08;
        let hc = fbm(p, 1.0) + 0.5 * fine;
        let hx = fbm(p + vec2(e, 0.0), 1.0) + 0.5 * vnoise(p + vec2(e, 0.0), 0.25);
        let hy = fbm(p + vec2(0.0, e), 1.0) + 0.5 * vnoise(p + vec2(0.0, e), 0.25);
        let strength = near * mix(0.35, 0.12, grey) * (1.0 - steep);
        let bump = vec3((hx - hc) / e, 0.0, (hy - hc) / e) * strength * 0.25;
        pbr_input.N = normalize(pbr_input.N - bump);
    }

    // Darken skirts so any crack they fill reads as shadow, not a pale flap.
    if skirt {
        base = base * 0.45;
    }

    // Shadows baked into the imagery double up with real lighting; lift them a little.
    let shadow = (1.0 - smoothstep(0.03, 0.16, lum)) * (1.0 - water);
    base = base + (vec3(lum * 1.6 + 0.02) - base) * shadow * 0.35;

    // Beaches: sand just above the waterline is darker and glossy, with the swash of
    // each wave running up and back.
    if above_sea >= 0.0 && above_sea < 2.5 && !skirt {
        let sandy = clamp((lum - 0.35) * 4.0, 0.0, 1.0) * clamp(1.0 - abs(base.g - base.r) * 6.0, 0.0, 1.0) * clamp((base.r - base.b) * 6.0, 0.0, 1.0);
        if sandy > 0.0 {
            let t = globals.time;
            let period = max(sea.swell.w, 4.0);
            let run = max(sea.swell.z, 0.2) * 0.6;
            // Swash height above the still waterline over a wave's cycle.
            let cyc = fract(t / period + dot(p, vec2<f32>(0.003, 0.002)));
            let swash = run * smoothstep(0.0, 0.35, cyc) * (1.0 - smoothstep(0.35, 1.0, cyc));
            let wet = (1.0 - smoothstep(0.0, 0.6 + run, above_sea)) * sandy;
            let film = (1.0 - smoothstep(swash - 0.05, swash + 0.05, above_sea)) * sandy;
            base = mix(base, base * vec3<f32>(0.62, 0.6, 0.58), wet * 0.8);
            pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 0.25, wet * 0.6);
            // The thin sheet of water itself: glossy, a little foam at its edge.
            let edge = smoothstep(swash - 0.12, swash, above_sea) * (1.0 - smoothstep(swash, swash + 0.04, above_sea));
            base = mix(base, base * vec3<f32>(0.5, 0.55, 0.6), film * 0.6);
            base = mix(base, vec3<f32>(0.9, 0.92, 0.93), edge * 0.7 * sandy);
            pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 0.05, film);
            pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3(0.5), film);
        }
    }

    if water > 0.0 {
        let t = globals.time;
        // Ocean surface as a few real waves: the swell (from buoy and wave-model data:
        // its period sets the wavelength), two neighbours of it, and the wind sea, plus
        // fine chop. Normals from their slopes.
        let sdir = normalize(sea.swell.xy + vec2(1e-4, 0.0));
        let wdir = normalize(sea.wind.xy + vec2(1e-4, 0.0));
        let swell_len = 1.56 * sea.swell.w * sea.swell.w;
        let wind_t = max(0.6 * sqrt(max(sea.wind.z, 0.5)) + 1.0, 1.5);
        let wind_len = 1.56 * wind_t * wind_t;
        let ha = sea.swell.z * 0.5;
        let hw = sea.wind.w * 0.5;
        let rot = mat2x2<f32>(0.94, 0.34, -0.34, 0.94);
        // Shallow water: waves slow down and shorten towards the shore.
        let shoal = clamp(sqrt(max(depth, 0.3) / (swell_len * 0.5)), 0.25, 1.0);
        var wv = wave(p, sdir, swell_len * shoal, ha * 0.6, t, 0.0);
        wv += wave(p, rot * sdir, swell_len * 0.71 * shoal, ha * 0.3, t, 1.7);
        wv += wave(p, transpose(rot) * sdir, swell_len * 0.53 * shoal, ha * 0.2, t, 4.1);
        wv += wave(p, wdir, wind_len, hw * 0.6, t, 2.3);
        wv += wave(p, rot * wdir, wind_len * 0.6, hw * 0.35, t, 5.3);
        wv += wave(p, transpose(rot) * wdir, wind_len * 0.37, hw * 0.25, t, 0.9);
        // Fine chop (noise), stronger in more wind.
        let chop_s = 0.25 + min(sea.wind.z / 10.0, 1.5);
        let wave_n = (fbm(p + wdir * t * 1.3, 9.0) * 0.55
            + vnoise(p + wdir * t * 0.8 + vec2(-t * 0.3, t * 0.2), 3.0) * 0.3
            + vnoise(p + vec2(t * 0.5, -t * 0.9), 1.1) * 0.15);
        let e = 0.2;
        let wx = fbm(p + vec2(e, 0.0) + wdir * t * 1.3, 9.0) * 0.55 + vnoise(p + vec2(e, 0.0) + wdir * t * 0.8 + vec2(-t * 0.3, t * 0.2), 3.0) * 0.3
            + vnoise(p + vec2(e + t * 0.5, -t * 0.9), 1.1) * 0.15;
        let wy = fbm(p + vec2(0.0, e) + wdir * t * 1.3, 9.0) * 0.55 + vnoise(p + vec2(0.0, e) + wdir * t * 0.8 + vec2(-t * 0.3, t * 0.2), 3.0) * 0.3
            + vnoise(p + vec2(t * 0.5, e - t * 0.9), 1.1) * 0.15;
        // Detail fades with distance (and in very shallow water) so it never sparkles.
        let far = 1.0 - smoothstep(150.0, 2500.0, dist);
        let amp = far * 0.9 * smoothstep(0.1, 1.5, depth) * chop_s * 0.35 + 0.05;
        let big = (1.0 - smoothstep(800.0, 6000.0, dist)) * smoothstep(0.2, 1.0, depth);
        // Detail x runs east, y south; the tangent frame near the origin follows the
        // world's x and z.
        let slope_v = vec2<f32>(wv.y, wv.z) * big + vec2<f32>((wx - wave_n) / e, (wy - wave_n) / e) * amp;
        let wn = vec3(slope_v.x, 0.0, slope_v.y) * water;
        pbr_input.N = normalize(pbr_input.N - wn);
        // Colour: the imagery (reefs, sand, depth) seen through the water, darker and
        // bluer with depth; whitecaps in a breeze, and surf breaking where the waves
        // run into shallow water.
        let deep = vec3(0.012, 0.05, 0.075);
        var wc = mix(base * vec3(0.8, 0.92, 1.0), deep, smoothstep(2.0, 50.0, depth) * 0.75);
        let crest_h = wv.x / max(ha * 1.1 + hw * 1.2, 0.05);
        let caps = smoothstep(5.0, 11.0, sea.wind.z);
        let crest = smoothstep(0.55, 0.85, crest_h + (wave_n - 0.5) * 0.6) * smoothstep(3.0, 15.0, depth) * (0.08 + 0.55 * caps);
        // Breakers: lines of foam parallel to the shore, in water shallower than about
        // twice the wave height, moving in with the swell.
        let hs = max(sea.swell.z, 0.1);
        let surf_zone = (1.0 - smoothstep(hs * 0.8, hs * 2.6 + 0.6, depth)) * smoothstep(0.02, 0.25, depth);
        let surf_phase = sqrt(max(depth, 0.01)) * 6.2831853 / max(sqrt(9.81) * sea.swell.w * 0.35, 0.5) * 6.0 + t * 6.2831853 / max(sea.swell.w, 3.0);
        let breaker = smoothstep(0.55, 0.95, sin(surf_phase) * 0.5 + 0.5 + (vnoise(p * 0.25 + vec2(t * 0.1, 0.0), 1.5) - 0.5) * 0.5) * surf_zone * min(hs * 1.5, 1.0);
        let shore = (1.0 - smoothstep(0.05, 0.9, depth)) * smoothstep(0.45, 0.7, vnoise(p * 1.0 + vec2(t * 0.7, 0.0), 1.5));
        let foam = clamp(crest + breaker + shore * 0.8, 0.0, 1.0);
        wc = mix(wc, vec3(0.86, 0.9, 0.92), foam);
        base = mix(base, wc, water);
        pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, 0.06 + foam * 0.6, water);
        pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3(0.45), water * (1.0 - foam));
    }

    pbr_input.material.base_color = vec4(base, 1.0);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
