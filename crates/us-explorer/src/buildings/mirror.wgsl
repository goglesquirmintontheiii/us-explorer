// Mirrors: show the reflection camera's image at this fragment's place on screen.
#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = (in.position.xy - view.viewport.xy) / view.viewport.zw;
    let c = textureSample(tex, samp, uv).rgb;
    // A touch of silvering tint; the image is already tone mapped.
    return vec4<f32>(c * vec3<f32>(0.93, 0.95, 0.96) * 1.1, 1.0);
}
