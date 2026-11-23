struct U {
    vp: mat4x4<f32>,
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,
    fog_color: vec4<f32>,
    sky_color: vec4<f32>,
    // fog_start, fog_end, daylight, time
    params: vec4<f32>,
    // width, height, underwater, unused
    screen: vec4<f32>,
    sun: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) light: vec4<f32>,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) dist: f32,
};

fn curve(l: f32) -> f32 { return l / (4.0 - 3.0 * l); }

@vertex fn vs_chunk(v: VIn) -> VOut {
    var o: VOut;
    let rel = v.pos - u.cam.xyz;
    o.clip = u.vp * vec4<f32>(rel, 1.0);
    o.uv = v.uv;
    // u.screen.w >= 0: a sky-less dimension with that ambient light (nether 0.1): no sky light,
    // block light brightened like vanilla (ambient + (1 - ambient) * curve)
    let skyless = u.screen.w >= 0.0;
    let amb = max(u.screen.w, 0.0);
    let sky = select(curve(v.light.x) * u.params.z, 0.0, skyless);
    let blk = select(curve(v.light.y), amb + (1.0 - amb) * curve(v.light.y), skyless);
    let sky_rgb = vec3<f32>(sky) * mix(vec3<f32>(0.55, 0.6, 1.0), vec3<f32>(1.0), u.params.z);
    let blk_rgb = vec3<f32>(blk, blk * (0.6 * blk + 0.4) * 0.9 + 0.08 * blk, blk * (blk * blk * 0.6 + 0.3));
    let l = clamp(max(sky_rgb, blk_rgb) + sky_rgb * blk_rgb * 0.2, vec3<f32>(0.0), vec3<f32>(1.0));
    o.color = vec4<f32>(v.color.rgb * (0.04 + 0.96 * l), v.color.a);
    o.dist = length(rel);
    return o;
}

fn fogged(c: vec3<f32>, dist: f32) -> vec3<f32> {
    let f = clamp((dist - u.params.x) / (u.params.y - u.params.x), 0.0, 1.0);
    return mix(c, u.fog_color.rgb, f);
}

@fragment fn fs_opaque(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    return vec4<f32>(fogged(t.rgb * i.color.rgb, i.dist), 1.0);
}
@fragment fn fs_cutout(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    if (t.a < 0.5) { discard; }
    return vec4<f32>(fogged(t.rgb * i.color.rgb, i.dist), 1.0);
}
@fragment fn fs_crack(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    if (t.a < 0.1) { discard; }
    return vec4<f32>(t.rgb, 1.0);
}
@fragment fn fs_trans(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    if (t.a < 0.01) { discard; }
    return vec4<f32>(fogged(t.rgb * i.color.rgb, i.dist), t.a * i.color.a);
}

// ---- sky ----
struct SOut { @builtin(position) clip: vec4<f32>, @location(0) ndc: vec2<f32> };
@vertex fn vs_sky(@builtin(vertex_index) vi: u32) -> SOut {
    var o: SOut;
    let p = vec2<f32>(f32((vi << 1u) & 2u) * 2.0 - 1.0, f32(vi & 2u) * 2.0 - 1.0);
    o.clip = vec4<f32>(p, 1.0, 1.0);
    o.ndc = p;
    return o;
}
@fragment fn fs_sky(i: SOut) -> @location(0) vec4<f32> {
    if (u.screen.w >= 0.0) { return vec4<f32>(u.fog_color.rgb, 1.0); }
    let w = u.inv_vp * vec4<f32>(i.ndc, 1.0, 1.0);
    let d = normalize(w.xyz / w.w);
    let t = smoothstep(-0.02, 0.35, d.y);
    var c = mix(u.fog_color.rgb, u.sky_color.rgb, t);
    // sun / moon disc
    let sd = normalize(u.sun.xyz);
    let m = 0.0;
    let right = normalize(cross(sd, vec3<f32>(0.0, 0.0, 1.0)));
    let upv = cross(right, sd);
    let lx = dot(d, right); let ly = dot(d, upv);
    if (dot(d, sd) > 0.0 && abs(lx) < 0.055 && abs(ly) < 0.055) { c = mix(c, vec3<f32>(1.0, 0.98, 0.85), 0.95); }
    if (dot(d, sd) < 0.0 && abs(lx) < 0.04 && abs(ly) < 0.04) { c = mix(c, vec3<f32>(0.85, 0.88, 0.95), 0.9 * (1.0 - u.params.z)); }
    return vec4<f32>(c + m, 1.0);
}

// ---- ui ----
@vertex fn vs_ui(v: VIn) -> VOut {
    var o: VOut;
    o.clip = vec4<f32>(v.pos.x / u.screen.x * 2.0 - 1.0, 1.0 - v.pos.y / u.screen.y * 2.0, 0.0, 1.0);
    o.uv = v.uv; o.color = v.color; o.dist = 0.0;
    return o;
}
@fragment fn fs_ui(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    let c = t * i.color;
    if (c.a < 0.01) { discard; }
    return c;
}
