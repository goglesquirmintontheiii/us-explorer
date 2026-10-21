// Tree impostors: one camera-facing card per tree, turning about the local vertical.
// Each vertex holds the tree's base point; uv_b holds its corner offset (metres across,
// metres up). Cards closer than `near` are folded away (the 3D tree is drawn there) and
// those beyond `far` too.

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
}

struct BillboardParams {
    // x: near cut, y: far cut, z: wind strength, w: unused.
    cuts: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> bb: BillboardParams;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let base = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0)).xyz;
    let up = normalize(base + vec3<f32>(0.0, 6371000.0, 0.0));
    let to_cam = view.world_position - base;
    let dist = length(to_cam);
    var flat = to_cam - up * dot(to_cam, up);
    if length(flat) < 1e-3 {
        flat = vec3<f32>(0.0, 0.0, 1.0);
    }
    let fwd = normalize(flat);
    let right = normalize(cross(up, fwd));
#ifdef VERTEX_UVS_B
    let corner = vertex.uv_b;
#else
    let corner = vec2<f32>(0.0);
#endif
    // A gentle sway of the top.
    let t = globals.time;
    let k = corner.y / max(abs(corner.y) + 1.0, 1.0);
    let sway = sin(t * 1.1 + dot(base.xz, vec2<f32>(0.09, 0.07))) * k * 0.15 * bb.cuts.z;
    var p = base + right * (corner.x + sway) + up * corner.y;
    // Fade out just inside the near cut (by shrinking), fold away entirely closer still.
    let near = bb.cuts.x;
    if dist < near || dist > bb.cuts.y {
        p = base;
    }
    out.world_position = vec4<f32>(p, 1.0);
    out.position = position_world_to_clip(p);
    out.world_normal = normalize(up * 0.7 + fwd * 0.5);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = vec4<f32>(right, 1.0);
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
