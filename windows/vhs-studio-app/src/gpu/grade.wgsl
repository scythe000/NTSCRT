// Colour grade on the chain input. One texel in, one texel out; the math is
// the same, in the same order, as `vhs_studio_core::grade::grade_pixel`, which
// is the CPU reference the tests check presets against. Change one, change
// the other.

struct Grade {
    // hue_shift(rad), keep, keep_hue(rad), keep_width(rad)
    a: vec4<f32>,
    // saturation, contrast, brightness, gamma
    b: vec4<f32>,
    // tint, shadow.rgb
    c: vec4<f32>,
    // highlight.rgb, solarize
    d: vec4<f32>,
    // solarize_threshold, invert, 0, 0
    e: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> g: Grade;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@compute @workgroup_size(8, 8, 1)
fn grade_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let texel = textureLoad(src, vec2<i32>(id.xy), 0);
    var c = texel.rgb;

    // 1. hue shift: rotate the chroma plane of YIQ.
    let y = luma(c);
    let i = 0.596 * c.r - 0.274 * c.g - 0.322 * c.b;
    let q = 0.211 * c.r - 0.523 * c.g + 0.312 * c.b;
    let s = sin(g.a.x);
    let co = cos(g.a.x);
    let i2 = i * co - q * s;
    let q2 = i * s + q * co;
    c = vec3<f32>(
        y + 0.956 * i2 + 0.621 * q2,
        y - 0.272 * i2 - 0.647 * q2,
        y - 1.106 * i2 + 1.703 * q2,
    );

    // 2. colour highlight: keep the band around keep_hue, grey the rest.
    if (g.a.y > 0.0) {
        let pixel_hue = atan2(q2, i2);
        var d = abs(pixel_hue - g.a.z) % TAU;
        if (d > PI) {
            d = TAU - d;
        }
        var inside = 1.0 - smoothstep(g.a.w * 0.5, g.a.w, d);
        let chroma = sqrt(i2 * i2 + q2 * q2);
        inside = inside * smoothstep(0.0, 0.08, chroma);
        let l = luma(c);
        c = mix(vec3<f32>(l), c, mix(1.0 - g.a.y, 1.0, inside));
    }

    // 3. saturation
    let l = luma(c);
    c = mix(vec3<f32>(l), c, g.b.x);

    // 4. contrast and brightness
    c = (c - 0.5) * g.b.y + 0.5 + g.b.z;

    // 5. gamma
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / g.b.w));

    // 6. tint (duotone)
    if (g.c.x > 0.0) {
        let lt = clamp(luma(c), 0.0, 1.0);
        let duo = mix(g.c.yzw, g.d.xyz, lt);
        c = mix(c, duo, g.c.x);
    }

    // 7. solarize
    if (g.d.w > 0.0) {
        let flipped = select(c, 1.0 - c, c > vec3<f32>(g.e.x));
        c = mix(c, flipped, g.d.w);
    }

    // 8. invert
    c = mix(c, 1.0 - c, g.e.y);

    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), texel.a));
}
