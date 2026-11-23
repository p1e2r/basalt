// Title screen panorama: a cube around the camera, six faces in a 2D array texture
// (0 front -Z, 1 right +X, 2 back +Z, 3 left -X, 4 up, 5 down). dir.x = tan(fov/2), dir.y = aspect.

struct P { dir: vec4<f32>, rot: mat4x4<f32> };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var t: texture_2d_array<f32>;
@group(0) @binding(2) var s: sampler;

struct VO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_full(@builtin(vertex_index) i: u32) -> VO {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: VO;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment fn fs_pano(v: VO) -> @location(0) vec4<f32> {
    let ndc = vec2<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0);
    let d = normalize((p.rot * vec4<f32>(ndc.x * p.dir.x * p.dir.y, ndc.y * p.dir.x, -1.0, 0.0)).xyz);
    let a = abs(d);
    var face = 0;
    var uv = vec2<f32>(0.0);
    if (a.z >= a.x && a.z >= a.y) {
        if (d.z < 0.0) { face = 0; uv = vec2<f32>(d.x / a.z, -d.y / a.z); }
        else { face = 2; uv = vec2<f32>(-d.x / a.z, -d.y / a.z); }
    } else if (a.x >= a.y) {
        if (d.x > 0.0) { face = 1; uv = vec2<f32>(d.z / a.x, -d.y / a.x); }
        else { face = 3; uv = vec2<f32>(-d.z / a.x, -d.y / a.x); }
    } else {
        if (d.y > 0.0) { face = 4; uv = vec2<f32>(d.x / a.y, -d.z / a.y); }
        else { face = 5; uv = vec2<f32>(d.x / a.y, d.z / a.y); }
    }
    return vec4<f32>(textureSampleLevel(t, s, uv * 0.5 + 0.5, face, 0.0).rgb, 1.0);
}
