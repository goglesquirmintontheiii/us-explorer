// Trees and shrubs: the standard mesh vertex shader plus wind. uv_b carries how far a
// vertex sways (0 at the foot of the trunk, ~1 at the crown's edge) and its phase, so
// branches bend together and leaves flutter on top.

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
}

struct VegParams {
    // xy: wind direction (east, north); z: strength 0 (calm) .. 1 (gale); w: unused.
    wind: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> veg: VegParams;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0)).xyz;
    let base = world_from_local[3].xyz;
    // Up is away from the planet centre (6371 km below the floating origin).
    let up = normalize(base + vec3<f32>(0.0, 6371000.0, 0.0));
    let size = length(world_from_local[1].xyz);
#ifdef VERTEX_UVS_B
    let sway = vertex.uv_b.x;
    let phase = vertex.uv_b.y + dot(base.xz, vec2<f32>(0.071, 0.053));
#else
    let sway = 0.0;
    let phase = 0.0;
#endif
    let t = globals.time;
    let s = clamp(veg.wind.z, 0.0, 1.0);
    // The origin frame has x east and z south.
    var dir = vec3<f32>(veg.wind.x, 0.0, -veg.wind.y);
    dir = dir - up * dot(dir, up);
    // Slow bending with gusts, plus a faster flutter of the outer leaves.
    let gust = 0.65 + 0.35 * sin(t * 0.37 + phase * 0.2) * sin(t * 0.23 + 1.7);
    let bend = (0.15 + 0.85 * s) * s * gust * (0.75 + 0.25 * sin(t * 1.4 + phase));
    let sway2 = sway * sway;
    let flutter = sin(t * (6.0 + 4.0 * s) + phase * 11.0 + wp.y * 1.7) * clamp(sway - 0.6, 0.0, 1.0) * (0.015 + 0.05 * s);
    let side = normalize(cross(up, dir + vec3<f32>(1e-4, 0.0, 0.0)));
    wp = wp + dir * bend * sway2 * size * 0.06 + (up * 0.5 + side) * flutter * size * 0.15
        + side * sin(t * 0.9 + phase) * sway2 * size * 0.004 * (0.3 + s);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex.instance_index);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, world_from_local[3]);
#endif
    return out;
}
