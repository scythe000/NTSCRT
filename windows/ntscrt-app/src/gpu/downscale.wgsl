// Downscale kernels, ported from the MSL source embedded in
// `Sources/CrtCore/Downscaler.swift`. Same weight functions, same
// ratio-scaled support, same two-pass separable structure — so a given
// method resolves to the same pixels it does on the Mac build.
//
// nearest and area run as a single dispatch. nearest+, bilinear, bicubic and
// lanczos are proper decimation filters: their kernel support scales with the
// downscale ratio (a fixed-footprint interpolator skips source pixels when
// minifying and aliases badly — the classic "bilinear downscale looks like
// nearest" bug), and they run as two separable passes (horizontal into a
// scratch texture, then vertical) so large ratios stay cheap.

struct Dims {
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> d: Dims;

fn load_clamped(x: i32, y: i32) -> vec4<f32> {
    let cx = clamp(x, 0, i32(textureDimensions(src).x) - 1);
    let cy = clamp(y, 0, i32(textureDimensions(src).y) - 1);
    return textureLoad(src, vec2<i32>(cx, cy), 0);
}

// ---------- nearest ----------

@compute @workgroup_size(8, 8, 1)
fn downscale_nearest(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= d.dw || gid.y >= d.dh) { return; }
    // Matches the Metal version's normalized nearest sample: the texel whose
    // centre the destination centre falls into.
    let sx = i32((f32(gid.x) + 0.5) * f32(d.sw) / f32(d.dw));
    let sy = i32((f32(gid.y) + 0.5) * f32(d.sh) / f32(d.dh));
    textureStore(dst, vec2<i32>(i32(gid.x), i32(gid.y)), load_clamped(sx, sy));
}

// ---------- weight functions ----------
// t is in filter-space (source distance divided by the downscale ratio), so
// each filter keeps its natural support: tent +/-1, Mitchell +/-2,
// lanczos3 +/-3 — scaled back up by the ratio in the passes below.

fn w_bilinear(t: f32) -> f32 {
    return max(0.0, 1.0 - abs(t));
}

// Stabilized nearest: a narrow Gaussian (sigma 0.35 output pixels). Wide
// enough that sub-pixel motion no longer flips which source pixel wins
// (kills temporal shimmer on video), narrow enough to keep most of nearest's
// per-pixel punch.
fn w_nearestaa(t: f32) -> f32 {
    return exp(t * t * -4.0816);
}

// Mitchell-Netravali B = C = 1/3.
fn w_bicubic(t: f32) -> f32 {
    let x = abs(t);
    let B = 1.0 / 3.0;
    let C = 1.0 / 3.0;
    let x2 = x * x;
    let x3 = x2 * x;
    if (x < 1.0) {
        return ((12.0 - 9.0 * B - 6.0 * C) * x3
              + (-18.0 + 12.0 * B + 6.0 * C) * x2
              + (6.0 - 2.0 * B)) * (1.0 / 6.0);
    }
    if (x < 2.0) {
        return ((-B - 6.0 * C) * x3
              + (6.0 * B + 30.0 * C) * x2
              + (-12.0 * B - 48.0 * C) * x
              + (8.0 * B + 24.0 * C)) * (1.0 / 6.0);
    }
    return 0.0;
}

fn sinc(x: f32) -> f32 {
    if (abs(x) < 1e-6) { return 1.0; }
    let xp = x * 3.14159265358979323846;
    return sin(xp) / xp;
}

fn w_lanczos(t: f32) -> f32 {
    if (abs(t) >= 3.0) { return 0.0; }
    return sinc(t) * sinc(t / 3.0);
}

fn weight(kind: u32, t: f32) -> f32 {
    switch kind {
        case 0u: { return w_nearestaa(t); }
        case 1u: { return w_bilinear(t); }
        case 2u: { return w_bicubic(t); }
        default: { return w_lanczos(t); }
    }
}

fn radius_for(kind: u32) -> f32 {
    switch kind {
        case 0u: { return 1.0; }  // nearest+
        case 1u: { return 1.0; }  // bilinear
        case 2u: { return 2.0; }  // bicubic
        default: { return 3.0; }  // lanczos
    }
}

// ---------- separable ratio-scaled passes ----------
// Horizontal: (sw x sh) -> (dw x sh). Vertical: (dw x sh) -> (dw x dh).
// scale = src/dst ratio (clamped >= 1 so magnification degrades to plain
// interpolation); support = filter radius * scale.
//
// The kernel choice is a specialization constant rather than four separate
// entry points — WGSL has no preprocessor to stamp out the Metal macro.

override FILTER_KIND: u32 = 1u;

@compute @workgroup_size(8, 8, 1)
fn downscale_separable_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= d.dw || gid.y >= d.sh) { return; }
    let scale = max(1.0, f32(d.sw) / f32(d.dw));
    let center = (f32(gid.x) + 0.5) * f32(d.sw) / f32(d.dw) - 0.5;
    let support = radius_for(FILTER_KIND) * scale;
    let lo = i32(ceil(center - support));
    let hi = i32(floor(center + support));
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var x = lo; x <= hi; x = x + 1) {
        let w = weight(FILTER_KIND, (f32(x) - center) / scale);
        acc = acc + load_clamped(x, i32(gid.y)) * w;
        wsum = wsum + w;
    }
    textureStore(dst, vec2<i32>(i32(gid.x), i32(gid.y)), acc / max(wsum, 1e-6));
}

@compute @workgroup_size(8, 8, 1)
fn downscale_separable_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= d.dw || gid.y >= d.dh) { return; }
    let scale = max(1.0, f32(d.sh) / f32(d.dh));
    let center = (f32(gid.y) + 0.5) * f32(d.sh) / f32(d.dh) - 0.5;
    let support = radius_for(FILTER_KIND) * scale;
    let lo = i32(ceil(center - support));
    let hi = i32(floor(center + support));
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var y = lo; y <= hi; y = y + 1) {
        let w = weight(FILTER_KIND, (f32(y) - center) / scale);
        acc = acc + load_clamped(i32(gid.x), y) * w;
        wsum = wsum + w;
    }
    textureStore(dst, vec2<i32>(i32(gid.x), i32(gid.y)), acc / max(wsum, 1e-6));
}

// ---------- area (box average) ----------
// Averages all source pixels covered by each destination pixel. The right
// choice when the downscale ratio is large; produces smooth, anti-aliased
// results without ringing. Also the integrator for the supersampled
// scanline pass.

@compute @workgroup_size(8, 8, 1)
fn downscale_area(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= d.dw || gid.y >= d.dh) { return; }
    let sx0 = f32(gid.x) * f32(d.sw) / f32(d.dw);
    let sx1 = f32(gid.x + 1u) * f32(d.sw) / f32(d.dw);
    let sy0 = f32(gid.y) * f32(d.sh) / f32(d.dh);
    let sy1 = f32(gid.y + 1u) * f32(d.sh) / f32(d.dh);
    let x0 = i32(floor(sx0));
    let x1 = i32(ceil(sx1));
    let y0 = i32(floor(sy0));
    let y1 = i32(ceil(sy1));
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var y = y0; y < y1; y = y + 1) {
        let fy = clamp(min(f32(y) + 1.0, sy1) - max(f32(y), sy0), 0.0, 1.0);
        for (var x = x0; x < x1; x = x + 1) {
            let fx = clamp(min(f32(x) + 1.0, sx1) - max(f32(x), sx0), 0.0, 1.0);
            let w = fx * fy;
            acc = acc + load_clamped(x, y) * w;
            wsum = wsum + w;
        }
    }
    textureStore(dst, vec2<i32>(i32(gid.x), i32(gid.y)), acc / max(wsum, 1e-6));
}
