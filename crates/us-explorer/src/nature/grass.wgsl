// Grass around the player: patches of blades (or, on lower settings, flat tufts) laid
// on a grid. A small map texture centred on the player says where grass grows, how
// tall and what colour (taken from the aerial imagery and the season); blades where the
// map has none fold away to nothing.
//
//   uv    = (across the blade 0..1, up the blade 0..1) or the tuft card's uv
//   uv_b  = the blade's root in the patch (x, z metres)
//   color = (random 0..1, stiffness, height factor, width factor)
// Map texel: rgb = colour (sRGB), a = density * 16 + height class (both 0..15).

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
}

struct GrassParams {
    // Map centre in render space (xyz) and metres per texel (w).
    centre: vec4<f32>,
    // Map east and south directions in render space; w: texels across.
    east: vec4<f32>,
    south: vec4<f32>,
    // xy wind direction (east, north), z strength, w: 1 for flat tufts.
    wind: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> grass: GrassParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var grass_map: texture_2d<f32>;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var p = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0)).xyz;
#ifdef VERTEX_UVS_B
    let root_l = vec3<f32>(vertex.uv_b.x, 0.0, vertex.uv_b.y);
#else
    let root_l = vec3<f32>(0.0);
#endif
    let root = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(root_l, 1.0)).xyz;
    let up = normalize(root + vec3<f32>(0.0, 6371000.0, 0.0));
#ifdef VERTEX_COLORS
    let rnd = vertex.color;
#else
    let rnd = vec4<f32>(0.5);
#endif
#ifdef VERTEX_UVS_A
    let t = vertex.uv.y;
#else
    let t = 0.0;
#endif
    // Look up the map under the blade's root.
    let n = grass.east.w;
    let rel = root - grass.centre.xyz;
    let tx = floor(dot(rel, grass.east.xyz) / grass.centre.w + n * 0.5);
    let ty = floor(dot(rel, grass.south.xyz) / grass.centre.w + n * 0.5);
    var texel = vec4<f32>(0.0);
    if tx >= 0.0 && ty >= 0.0 && tx < n && ty < n {
        texel = textureLoad(grass_map, vec2<i32>(i32(tx), i32(ty)), 0);
    }
    let packed = u32(round(texel.a * 255.0));
    let density = f32(packed >> 4u) / 15.0;
    let hclass = f32(packed & 15u);
    // Height classes: 0 -> 6 cm .. 15 -> 2 m (roughly geometric).
    let height = 0.06 * pow(1.26, hclass) * (0.6 + 0.8 * rnd.z);
    // Thin out with distance (the far ring has fewer, wider blades already).
    let dist = distance(root, view.world_position);
    let keep = density * (1.0 - smoothstep(grass.east.w * grass.centre.w * 0.36, grass.east.w * grass.centre.w * 0.48, dist));
    var col = srgb_to_linear(texel.rgb);
    if rnd.x > keep {
        // No grass here: fold the blade to its root.
        p = root;
    } else {
        // Scale the blade (it was built 1 m tall) and bend it in the wind.
        let local_up = p - root;
        let along = dot(local_up, up);
        let flat = local_up - up * along;
        let tall = along * height;
        var dir = vec3<f32>(grass.wind.x, 0.0, -grass.wind.y);
        dir = normalize(dir - up * dot(dir, up) + vec3<f32>(1e-5, 0.0, 0.0));
        let s = grass.wind.z;
        let tt = globals.time;
        let phase = dot(root.xz, vec2<f32>(0.37, 0.29)) + rnd.x * 6.28;
        let gust = 0.6 + 0.4 * sin(tt * 0.9 + dot(root.xz, vec2<f32>(0.11, 0.07)));
        let bend = (0.1 + 0.9 * s) * gust * (0.6 + 0.4 * sin(tt * (2.2 + 2.0 * s) + phase)) * (1.2 - rnd.y * 0.6);
        let w = select(1.0, 0.6 + rnd.w * 0.8, grass.wind.w < 0.5);
        p = root + flat * select(w, height * 0.8 + 0.2, grass.wind.w > 0.5) + up * tall + dir * bend * t * t * height * 0.45;
        // Darker towards the roots, a little yellow at the tips; each blade varies.
        col = col * (0.55 + 0.6 * t) * (0.85 + 0.3 * rnd.x) * vec3<f32>(1.0 + 0.08 * t, 1.0 + 0.04 * t, 0.95);
    }
    out.world_position = vec4<f32>(p, 1.0);
    out.position = position_world_to_clip(p);
    // Normals lean up so the grass shades like a lawn, not like a comb.
#ifdef VERTEX_NORMALS
    let n_blade = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
    out.world_normal = normalize(up * 0.75 + n_blade * 0.35);
#else
    out.world_normal = up;
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = vec4<f32>(cross(up, out.world_normal), 1.0);
#endif
#ifdef VERTEX_COLORS
    out.color = vec4<f32>(col, 1.0);
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, world_from_local[3]);
#endif
    return out;
}
