// The night sky drawn over the atmosphere: the moon (lit by the real sun, so its
// phase is right), the Milky Way, city sky glow and the aurora. A sphere around the
// camera, pinned to the far plane so everything in the world is in front of it.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    mesh_view_bindings::{view, globals},
}

struct SkyParams {
    // Rotation from world directions to equatorial (J2000) ones.
    eq_from_world: mat4x4<f32>,
    // xyz: direction to the moon; w: its drawn angular radius (radians).
    moon: vec4<f32>,
    // xyz: direction to the sun; w: moon brightness (cd/m2, full-moon luminance).
    sun: vec4<f32>,
    // x: darkness 0..1, y: light pollution 0..1, z: cloud cover 0..1, w: unused.
    night: vec4<f32>,
    // xyz: horizontal direction of the magnetic pole; w: aurora strength 0..1.
    aurora: vec4<f32>,
    // x: lower edge elevation (radians), y: how much of the sky it covers 0..1.
    aurora2: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyParams;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) dir: vec3<f32>,
};

@vertex
fn vertex(v: Vertex) -> Out {
    var out: Out;
    let m = mesh_functions::get_world_from_local(v.instance_index);
    let d = normalize((m * vec4<f32>(v.position, 0.0)).xyz);
    var clip = view.clip_from_world * vec4<f32>(view.world_position + d * 1000.0, 1.0);
    clip.z = 0.0;
    out.position = clip;
    out.dir = d;
    return out;
}

fn hash3(p: vec3<f32>) -> f32 {
    let q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yxz + 33.33);
    return fract((r.x + r.y) * r.z);
}

fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(mix(hash3(i), hash3(i + vec3(1.0, 0.0, 0.0)), u.x), mix(hash3(i + vec3(0.0, 1.0, 0.0)), hash3(i + vec3(1.0, 1.0, 0.0)), u.x), u.y);
    let b = mix(mix(hash3(i + vec3(0.0, 0.0, 1.0)), hash3(i + vec3(1.0, 0.0, 1.0)), u.x), mix(hash3(i + vec3(0.0, 1.0, 1.0)), hash3(i + vec3(1.0, 1.0, 1.0)), u.x), u.y);
    return mix(a, b, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var s = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i++) {
        s += a * noise3(q);
        q = q * 2.03 + vec3(1.7, 9.2, 3.1);
        a *= 0.5;
    }
    return s / 0.9375;
}

@fragment
fn fragment(in: Out) -> @location(0) vec4<f32> {
    let d = normalize(in.dir);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    let el = dot(d, up);
    let dark = sky.night.x;
    let lp = sky.night.y;
    let cloud = sky.night.z;
    let clear = 1.0 - 0.9 * cloud;
    let t = globals.time;
    // Thicker air towards the horizon.
    let air = smoothstep(-0.03, 0.2, el);
    var c = vec3<f32>(0.0);

    if dark > 0.01 {
        // The Milky Way, in galactic coordinates.
        let e = (sky.eq_from_world * vec4<f32>(d, 0.0)).xyz;
        let gal = mat3x3<f32>(
            vec3<f32>(-0.0548755604, 0.4941094279, -0.8676661490),
            vec3<f32>(-0.8734370902, -0.4448296300, -0.1980763734),
            vec3<f32>(-0.4838350155, 0.7469822445, 0.4559837762),
        );
        let g = gal * e;
        let b = asin(clamp(g.z, -1.0, 1.0));
        let l = atan2(g.y, g.x);
        let core = pow(0.5 + 0.5 * cos(l), 2.0);
        let w = 0.09 + 0.11 * core;
        let band = exp(-(b * b) / (w * w));
        let clumps = 0.45 + 0.9 * fbm(g * 7.0);
        // The Great Rift and other dust lanes darken the band along its middle.
        let dust = smoothstep(0.35, 0.75, fbm(g * 11.0 + 4.0)) * exp(-pow((b - 0.02) / 0.045, 2.0));
        let bulge = exp(-(l * l + 5.0 * b * b) / 0.06);
        let mw = (band * (0.3 + 0.7 * core) * clumps * (1.0 - 0.8 * dust) + 0.8 * bulge * (1.0 - 0.6 * dust));
        let tint = mix(vec3<f32>(0.62, 0.68, 0.85), vec3<f32>(0.95, 0.82, 0.62), core);
        let seen = dark * pow(1.0 - lp, 2.5) * clear * air;
        c += tint * mw * 0.035 * seen;

        // Glow of the town below the sky: orange near the horizon, brighter under cloud.
        let glow = lp * dark * exp(-max(el, 0.0) * 5.0) * (0.6 + 1.6 * cloud);
        c += vec3<f32>(0.85, 0.5, 0.26) * glow * 0.03;
        // Faint natural airglow.
        c += vec3<f32>(0.25, 0.45, 0.3) * 0.0025 * dark * exp(-max(el, 0.0) * 8.0) * clear;

        // The aurora: curtains towards the magnetic pole, green below and red above.
        let ai = sky.aurora.w;
        if ai > 0.001 && el > -0.02 {
            let mn = sky.aurora.xyz;
            let h = normalize(d - up * el + vec3<f32>(1e-5, 0.0, 0.0));
            let az = acos(clamp(dot(h, mn), -1.0, 1.0));
            let side = select(-az, az, dot(cross(mn, h), up) > 0.0);
            let elev = asin(clamp(el, -1.0, 1.0));
            let spread = mix(exp(-pow(az / 1.0, 2.0)), 1.0, sky.aurora2.y);
            let edge = sky.aurora2.x + 0.05 * sin(side * 2.7 + t * 0.07) + 0.035 * (noise3(vec3<f32>(side * 5.0, t * 0.05, 1.0)) - 0.5);
            let u = elev - edge;
            let curtain = smoothstep(-0.015, 0.02, u) * exp(-max(u, 0.0) / 0.3);
            let rays = 0.35 + 0.65 * noise3(vec3<f32>(side * 38.0, u * 1.5, t * 0.25));
            let folds = 0.6 + 0.4 * sin(side * 9.0 + t * 0.15 + 2.0 * noise3(vec3<f32>(side * 3.0, t * 0.03, 5.0)));
            let col = mix(vec3<f32>(0.12, 1.0, 0.4), vec3<f32>(0.75, 0.12, 0.4), smoothstep(0.08, 0.45, u));
            c += col * curtain * rays * folds * spread * ai * 0.09 * dark * clear;
        }
    }

    // The moon, lit by the sun.
    let md = sky.moon.xyz;
    let r = sky.moon.w;
    let cosang = dot(d, md);
    if cosang > 0.99 {
        let t1 = normalize(cross(md, up + vec3<f32>(0.0, 0.0, 1e-4)));
        let t2 = cross(t1, md);
        let x = dot(d, t1) / r;
        let y = dot(d, t2) / r;
        let rr = x * x + y * y;
        let ext = mix(vec3<f32>(1.0, 0.5, 0.25), vec3<f32>(1.0), smoothstep(0.0, 0.3, el)) * smoothstep(-0.01, 0.02, el);
        if rr < 1.0 {
            let nz = sqrt(1.0 - rr);
            let n = t1 * x + t2 * y - md * nz;
            let lit = max(dot(n, sky.sun.xyz), 0.0);
            // Maria: the same face always points at us.
            let maria = 0.62 + 0.38 * smoothstep(0.35, 0.65, fbm(vec3<f32>(x * 2.2 + 3.0, y * 2.2, 0.5)));
            let edge = smoothstep(1.0, 0.94, rr);
            let shine = sky.sun.w * (lit * maria + 0.0004) * edge;
            c = c * (1.0 - edge) + vec3<f32>(0.98, 0.96, 0.9) * shine * view.exposure * ext * (1.0 - 0.8 * cloud);
        } else {
            // A soft halo, mostly at night.
            let a = acos(clamp(cosang, -1.0, 1.0));
            c += vec3<f32>(0.7, 0.75, 0.85) * exp(-a / 0.03) * 0.02 * sky.sun.w / 2500.0 * dark * ext * (1.0 + 2.0 * cloud);
        }
    }
    return vec4<f32>(c, 0.0);
}
