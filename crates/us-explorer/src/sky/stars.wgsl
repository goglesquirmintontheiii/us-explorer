// Stars and planets: each a small camera-facing square of a few pixels, pinned to the
// far plane. Dimmer stars vanish under light pollution and cloud; all of them twinkle,
// more so near the horizon.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    mesh_view_bindings::{view, globals},
}

struct StarParams {
    // x: darkness 0..1, y: faintest magnitude visible, z: clear sky 0..1, w: size scale.
    p: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> stars: StarParams;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) corner: vec2<f32>,
    @location(1) colour: vec3<f32>,
    @location(2) bright: f32,
};

@vertex
fn vertex(v: Vertex) -> Out {
    var out: Out;
    let m = mesh_functions::get_world_from_local(v.instance_index);
    let d = normalize((m * vec4<f32>(v.position, 0.0)).xyz);
#ifdef VERTEX_UVS_A
    let corner = v.uv;
#else
    let corner = vec2<f32>(0.0);
#endif
#ifdef VERTEX_UVS_B
    let info = v.uv_b;
#else
    let info = vec2<f32>(6.0, 0.0);
#endif
#ifdef VERTEX_COLORS
    let col = v.color.rgb;
#else
    let col = vec3<f32>(1.0);
#endif
    let mag = info.x;
    let seed = info.y;
    let el = d.y;
    // Brightness on a compressed scale so both Sirius and 6th-magnitude stars read.
    let limit = stars.p.y;
    let fade = clamp((limit - mag) / 1.2, 0.0, 1.0);
    let air = smoothstep(-0.02, 0.18, el) * (0.35 + 0.65 * smoothstep(0.0, 0.5, el));
    let twinkle = 1.0 + (0.18 + 0.4 * (1.0 - smoothstep(0.0, 0.5, el))) * sin(globals.time * (9.0 + 7.0 * fract(seed * 7.13)) + seed * 61.0);
    var b = pow(10.0, -0.26 * (mag - 1.0)) * fade * air * twinkle * stars.p.x * stars.p.z;
    let size = (1.3 + 1.1 * clamp(sqrt(max(b, 0.0)), 0.0, 2.2)) * stars.p.w;
    var clip = view.clip_from_world * vec4<f32>(view.world_position + d * 1000.0, 1.0);
    clip = vec4<f32>(clip.xy + corner * size * 2.0 / view.viewport.zw * clip.w, 0.0, clip.w);
    if b < 0.002 {
        clip = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    out.position = clip;
    out.corner = corner;
    out.colour = col;
    out.bright = b;
    return out;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    let r2 = dot(in.corner, in.corner);
    let a = exp(-r2 * 3.2);
    return vec4<f32>(in.colour * in.bright * a * 0.55, 0.0);
}
