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
    mesh_view_bindings::view,
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
    let p = in.uv_b;
    let lum = dot(base, vec3(0.299, 0.587, 0.114));

    // Rock on steep ground: imagery taken from above smears on cliffs.
    let steep = smoothstep(0.28, 0.55, slope);
    if steep > 0.0 {
        let strata = vnoise(vec2(p.x + p.y, dot(in.world_position.xyz, up) * 4.0), 16.0);
        let rock = vec3(lum) * vec3(1.06, 1.0, 0.93) * (0.72 + 0.5 * fbm(p, 8.0)) * (0.85 + 0.3 * strata);
        base = mix(base, mix(rock, base, 0.2), steep);
    }

    // Close-up detail, faded out with distance so it never shimmers far away.
    let near = 1.0 - smoothstep(40.0, 420.0, dist);
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
    }

    // Shadows baked into the imagery double up with real lighting; lift them a little.
    let shadow = 1.0 - smoothstep(0.03, 0.16, lum);
    base = base + (vec3(lum * 1.6 + 0.02) - base) * shadow * 0.35;

    pbr_input.material.base_color = vec4(base, 1.0);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
