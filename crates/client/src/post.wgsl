// Menu backdrops: separable gaussian blur, and the title screen panorama (6 faces of a cube,
// stored as a 2D array texture: 0 front, 1 right, 2 back, 3 left, 4 up, 5 down).

struct P { dir: vec4<f32>, rot: mat4x4<f32> };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var t: texture_2d<f32>;
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

// dir.xy = one texel along the blur axis, dir.z = sigma in texels, dir.w = output brightness
@fragment fn fs_blur(v: VO) -> @location(0) vec4<f32> {
    let sigma = max(p.dir.z, 0.5);
    let r = i32(min(ceil(sigma * 2.5), 40.0));
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = -r; k <= r; k++) {
        let w = exp(-f32(k * k) / (2.0 * sigma * sigma));
        acc += textureSampleLevel(t, s, v.uv + p.dir.xy * f32(k), 0.0).rgb * w;
        wsum += w;
    }
    return vec4<f32>(acc / wsum * p.dir.w, 1.0);
}
