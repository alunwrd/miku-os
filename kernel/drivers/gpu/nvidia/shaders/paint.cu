// nvcc: --fmad=false
//
// aosh's painter on the GPU (`dev/aosh-gpu.md`)
//
// aosh records what its processor painter would do (the same calls, with what
// is worked out once per column, per row or per corner already worked out,
// `aosh/src/scene/gpu/record.rs`), and this does the rest, a thread per pixel.
// The arithmetic is the processor's, in the same order:
// `aosh/src/scene/gpu/raster.rs` and `paint.rs` are what each step is checked
// against, pixel for pixel, on the build host (`aosh/src/scene/gpu/check.rs`).
// The first line of this file keeps nvcc from fusing a multiply and an add,
// which would round once where the processor rounds twice.
//
// It runs for any process, without CAP_GPU, so nothing it is handed is trusted:
// every buffer comes with its size (`PARAM_BUFFER_SIZED`) and every read and
// write is checked against it. An operation pointing outside is skipped, a word
// past the end reads as nought, and a pixel past a target's edge is not
// written.
//
// The commands, all 32-bit words (the layout `record.rs` writes):
//
//   header (16)    magic, version, tiles, tiles_at, targets, targets_at,
//                  textures, textures_at, ops, ops_at, lists_at, data_at,
//                  words, 0, 0, 0
//   target (5)     byte offset in its pool, stride in pixels, width, height,
//                  which of the four pools (`dst0` to `dst3`)
//   texture (5)    byte offset in `tex`, stride (pixels, or bytes for a
//                  mask), width, height, format (0 premultiplied ARGB, 1 A8)
//   tile (8)       target, x, y, width, height (16 at most), list (from
//                  lists_at), count, 0
//   op (16)        kind, clip x0 y0 x1 y1, data (from data_at), words,
//                  opacity, and eight words of the kind's own
//
// A tile is a block, its pixels the threads. Each starts clear, runs the tile's
// operations in order, and is written once, as straight alpha: what a window's
// buffer holds.

typedef unsigned int u32;
typedef unsigned long long u64;
typedef long long i64;

#define MAGIC 0x544E5041u
#define VERSION 1u

// Far apart: close together, nvcc makes a choice among them a jump table, and a
// jump table lives in constant bank 2, which the kernel's launches do not bind
// (`compute.rs`, `build_qmd`)
#define OP_FILL 0x0110u
#define OP_TRIS 0x0230u
#define OP_QUAD 0x0470u
#define OP_QUAD_TURNED 0x08f0u
#define OP_SHADED 0x1170u
#define OP_MASK 0x2330u
#define OP_STYLED 0x4410u
#define OP_SHADED_TURNED 0x8230u

#define SH_BOX 0x0150u
#define SH_MASK 0x0290u
#define SH_INK 0x04d0u
#define SH_MIX 0x0830u
#define SH_GAUSS 0x1050u
#define SH_RADIAL 0x2090u
#define SH_MULTI 0x40f0u

#define NO_TEXTURE 0xFFFFFFFFu

struct Cmd {
    const u32 *w;
    u64 n;
    __device__ u32 at(u64 i) const { return i < n ? w[i] : 0u; }
    __device__ float f(u64 i) const { return __uint_as_float(at(i)); }
};

struct Tex {
    const unsigned char *base;
    u64 bytes;
    // `v as f32 / 255.0` for every byte, made once per block: a division
    // rounded as the processor rounds it is a dozen instructions and a call,
    // and it is in every texel
    const float *by255;
};

// A texture's entry, read once per use
struct Texture {
    u64 off;
    u32 stride, w, h, format;
    bool ok;
};

__device__ Texture texture(const Cmd &c, u32 i) {
    Texture t;
    u32 n = c.at(6), at = c.at(7);
    t.ok = i < n;
    u64 e = (u64)at + (u64)i * 5;
    t.off = c.at(e);
    t.stride = c.at(e + 1);
    t.w = c.at(e + 2);
    t.h = c.at(e + 3);
    t.format = c.at(e + 4);
    t.ok = t.ok && t.w > 0 && t.h > 0 && t.stride >= t.w;
    return t;
}

// A premultiplied texel, as a word: nought outside the texture or the buffer
__device__ u32 texel_word(const Tex &x, const Texture &t, u32 i, u32 j) {
    if (!t.ok || t.format != 0 || i >= t.w || j >= t.h) return 0u;
    u64 o = t.off + ((u64)j * t.stride + i) * 4;
    if (o + 4 > x.bytes) return 0u;
    return *(const u32 *)(x.base + o);
}

__device__ u32 mask_byte(const Tex &x, const Texture &t, int i, int j) {
    if (!t.ok || t.format != 1 || i < 0 || j < 0 || (u32)i >= t.w || (u32)j >= t.h) return 0u;
    u64 o = t.off + (u64)j * t.stride + (u32)i;
    if (o + 1 > x.bytes) return 0u;
    return x.base[o];
}

// `Texture::at`: red, green, blue, alpha, each over 255
__device__ float4 texel(const Tex &x, const Texture &t, u32 i, u32 j) {
    u32 c = texel_word(x, t, i, j);
    return make_float4(x.by255[(c >> 16) & 0xffu], x.by255[(c >> 8) & 0xffu], x.by255[c & 0xffu], x.by255[c >> 24]);
}

// A tap: the texel at or before, the one after, how far between
struct Tap {
    u32 i0, i1;
    float t;
};

__device__ Tap tap_word(const Cmd &c, u64 at) {
    Tap p;
    u32 w = c.at(at);
    p.i0 = w & 0xffffu;
    p.i1 = w >> 16;
    p.t = c.f(at + 1);
    return p;
}

// `subtexel`
__device__ float subtexel(float t) {
    return (float)__float2int_rz(t * 256.0f + 0.5f) / 256.0f;
}

// `Tap::new` for a place worked out in `f32`, linear
__device__ Tap tap_at(float f, u32 n) {
    Tap p;
    i64 last = (i64)n - 1;
    float f0 = floorf(f);
    i64 i = __float2ll_rz(f0);
    float t = subtexel(f - f0);
    if (t >= 1.0f) {
        i += 1;
        t = 0.0f;
    }
    i64 a = i < 0 ? 0 : (i > last ? last : i);
    i64 b = i + 1 < 0 ? 0 : (i + 1 > last ? last : i + 1);
    p.i0 = (u32)a;
    p.i1 = (u32)b;
    p.t = t;
    return p;
}

__device__ float4 lerp4(float4 a, float4 b, float t) {
    return make_float4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t);
}

// `Texture::read`. The processor reads one, two or four texels as the taps
// fall; this always reads four. It is the same number: a lerp by nought,
// `a + (b - a) * 0`, is `a` exactly
__device__ float4 read(const Tex &x, const Texture &t, Tap c, Tap r) {
    float4 top = lerp4(texel(x, t, c.i0, r.i0), texel(x, t, c.i1, r.i0), c.t);
    float4 bottom = lerp4(texel(x, t, c.i0, r.i1), texel(x, t, c.i1, r.i1), c.t);
    return lerp4(top, bottom, r.t);
}

// `Texture::read_alpha`: alpha times a 255th, which is not alpha over 255
__device__ float alpha_of(const Tex &x, const Texture &t, u32 i, u32 j) {
    return (float)(texel_word(x, t, i, j) >> 24) * (1.0f / 255.0f);
}

__device__ float read_alpha(const Tex &x, const Texture &t, Tap c, Tap r) {
    float a00 = alpha_of(x, t, c.i0, r.i0), a01 = alpha_of(x, t, c.i0, r.i1);
    float top = a00 + (alpha_of(x, t, c.i1, r.i0) - a00) * c.t;
    float bottom = a01 + (alpha_of(x, t, c.i1, r.i1) - a01) * c.t;
    return top + (bottom - top) * r.t;
}

// `Texture::sample`
__device__ float4 sample(const Tex &x, const Texture &t, float u, float v, bool linear) {
    float fx = u * (float)t.w - 0.5f, fy = v * (float)t.h - 0.5f;
    if (!linear) {
        i64 i = __float2ll_rz(floorf(fx + 0.5f)), j = __float2ll_rz(floorf(fy + 0.5f));
        i64 li = (i64)t.w - 1, lj = (i64)t.h - 1;
        i = i < 0 ? 0 : (i > li ? li : i);
        j = j < 0 ? 0 : (j > lj ? lj : j);
        return texel(x, t, (u32)i, (u32)j);
    }
    return read(x, t, tap_at(fx, t.w), tap_at(fy, t.h));
}

// `blend`: source-over of a premultiplied colour, rounded to 8 bits
__device__ u32 blend(u32 d, float r, float g, float b, float a) {
    if (a <= 0.0f && r <= 0.0f && g <= 0.0f && b <= 0.0f) return d;
    float inv = 1.0f - a;
    u32 out = 0;
    float s[4] = {a, r, g, b};
    for (int k = 0; k < 4; k++) {
        u32 shift = 24 - 8 * k;
        float v = s[k] * 255.0f + (float)((d >> shift) & 0xffu) * inv;
        u32 q = __float2uint_rz(v + 0.5f);
        out |= (q > 255u ? 255u : q) << shift;
    }
    return out;
}

__device__ u32 blend4(u32 d, float4 c, float o) {
    return blend(d, c.x * o, c.y * o, c.z * o, c.w * o);
}

// `paint::over`: integer source-over of `src` (alpha, red, green, blue) at
// coverage `cov`
__device__ u32 over(u32 dst, const u32 *src, u32 cov) {
    if (cov == 0) return dst;
    u32 s[4];
    for (int k = 0; k < 4; k++) s[k] = cov >= 255 ? src[k] : (src[k] * cov + 127) / 255;
    u32 inv = 255 - s[0];
    u32 out = 0;
    for (int k = 0; k < 4; k++) {
        u32 shift = 24 - 8 * k;
        u32 v = s[k] + ((((dst >> shift) & 0xffu) * inv + 127) / 255);
        out |= (v > 255u ? 255u : v) << shift;
    }
    return out;
}

// A word of four bytes, the first in the low bits
__device__ void unpack(u32 w, u32 *out) {
    out[0] = w & 0xffu;
    out[1] = (w >> 8) & 0xffu;
    out[2] = (w >> 16) & 0xffu;
    out[3] = w >> 24;
}

// `paint::to_straight`
__device__ u32 to_straight(u32 p) {
    u32 a = p >> 24;
    if (a == 0) return 0;
    if (a == 255) return p;
    u32 out = a << 24;
    for (int k = 0; k < 3; k++) {
        u32 shift = 16 - 8 * k;
        u32 v = ((((p >> shift) & 0xffu) * 255 + a / 2) / a);
        out |= (v > 255u ? 255u : v) << shift;
    }
    return out;
}

// Triangles: `raster::triangle`, a centre at a time

// Whether an edge a -> b of a positively wound triangle owns the centres on it
__device__ bool top_left(i64 ax, i64 ay, i64 bx, i64 by) {
    i64 ex = bx - ax, ey = by - ay;
    return (ey == 0 && ex > 0) || ey < 0;
}

// Whether the centre (cx, cy) is in the triangle, and the corners' weights
// there in the order they were given
__device__ bool covers(i64 *px, i64 *py, i64 cx, i64 cy, float *l) {
    i64 area = (px[1] - px[0]) * (py[2] - py[0]) - (py[1] - py[0]) * (px[2] - px[0]);
    if (area == 0) return false;
    bool flipped = area < 0;
    i64 x[3] = {px[0], px[1], px[2]}, y[3] = {py[0], py[1], py[2]};
    if (flipped) {
        i64 tx = x[1], ty = y[1];
        x[1] = x[2]; y[1] = y[2];
        x[2] = tx; y[2] = ty;
        area = -area;
    }
    i64 w[3];
    for (int k = 0; k < 3; k++) {
        // Edge k faces corner k: from k + 1 to k + 2
        int a = k == 2 ? 0 : k + 1, b = k == 0 ? 2 : (k == 1 ? 0 : 1);
        i64 dx = -(y[b] - y[a]), dy = x[b] - x[a];
        i64 c = -(dy * y[a]) - dx * x[a];
        i64 t = top_left(x[a], y[a], x[b], y[b]) ? 0 : 1;
        w[k] = c + dy * cy + dx * cx;
        if (w[k] < t) return false;
    }
    float inv = 1.0f / (float)area;
    l[0] = (float)w[0] * inv;
    float l1 = (float)w[1] * inv, l2 = (float)w[2] * inv;
    l[1] = flipped ? l2 : l1;
    l[2] = flipped ? l1 : l2;
    return true;
}

__device__ float4 vertex_colour(const Tex &x, u32 c) {
    return make_float4(x.by255[c & 0xffu], x.by255[(c >> 8) & 0xffu], x.by255[(c >> 16) & 0xffu], x.by255[c >> 24]);
}

// `raster::fill`: triangles `first` to `first + count` of a geometry at a
// pixel. A geometry is recorded as runs of neighbouring triangles, each an
// operation reaching only its own tiles (`record.rs`, `tris`), so a pixel
// tests the triangles near it and not a rounded rectangle's every one
__device__ u32 tris(const Cmd &c, const Tex &tx, u64 d, u64 words, u32 verts, u32 first, u32 count, float opacity, i64 cx, i64 cy,
                    u32 px) {
    u64 tri_at = (u64)verts * 3;
    if (tri_at + ((u64)first + count) * 3 > words) return px;
#pragma unroll 1
    for (u32 k = first; k < first + count; k++) {
        u32 idx[3] = {c.at(d + tri_at + k * 3), c.at(d + tri_at + k * 3 + 1), c.at(d + tri_at + k * 3 + 2)};
        if (idx[0] >= verts || idx[1] >= verts || idx[2] >= verts) continue;
        i64 x[3], y[3];
        float4 col[3];
        for (int v = 0; v < 3; v++) {
            x[v] = (i64)(int)c.at(d + idx[v] * 3);
            y[v] = (i64)(int)c.at(d + idx[v] * 3 + 1);
            col[v] = vertex_colour(tx, c.at(d + idx[v] * 3 + 2));
        }
        bool one = col[0].x == col[1].x && col[0].y == col[1].y && col[0].z == col[1].z && col[0].w == col[1].w
            && col[1].x == col[2].x && col[1].y == col[2].y && col[1].z == col[2].z && col[1].w == col[2].w;
        if (one) {
            float r = col[0].x * opacity, g = col[0].y * opacity, b = col[0].z * opacity, a = col[0].w * opacity;
            if (a <= 0.0f && r <= 0.0f && g <= 0.0f && b <= 0.0f) continue;
            float l[3];
            if (covers(x, y, cx, cy, l)) px = blend(px, r, g, b, a);
            continue;
        }
        float l[3];
        if (!covers(x, y, cx, cy, l)) continue;
        float m[4];
        float c0[4] = {col[0].x, col[0].y, col[0].z, col[0].w};
        float c1[4] = {col[1].x, col[1].y, col[1].z, col[1].w};
        float c2[4] = {col[2].x, col[2].y, col[2].z, col[2].w};
        for (int i = 0; i < 4; i++) m[i] = (c0[i] * l[0] + c1[i] * l[1] + c2[i] * l[2]) * opacity;
        px = blend(px, m[0], m[1], m[2], m[3]);
    }
    return px;
}

// Shaders: each `Shader::at`, its columns and rows worked out already

__device__ float smoothstep(float e0, float e1, float x) {
    float t = (x - e0) / (e1 - e0);
    t = t < 0.0f ? 0.0f : (t > 1.0f ? 1.0f : t);
    return t * t * (3.0f - 2.0f * t);
}

__device__ float clamp01(float v) {
    return v < 0.0f ? 0.0f : (v > 1.0f ? 1.0f : v);
}

// `multi::edge`
__device__ float edge(float e0, float e1, float x, bool on) {
    if (e1 <= e0) return (x > e0 || (on && x == e0)) ? 1.0f : 0.0f;
    float t = clamp01((x - e0) / (e1 - e0));
    return t * t * (3.0f - 2.0f * t);
}

// A texture's tap at `u` across it (`Texture::across_by`): between texels,
// or not `linear` the nearest one
__device__ Tap across(const Cmd &c, u32 tex, float u, bool linear, bool down) {
    Texture t = texture(c, tex);
    u32 n = down ? t.h : t.w;
    Tap p;
    p.i0 = 0;
    p.i1 = 0;
    p.t = 0.0f;
    if (n == 0) return p;
    float f = u * (float)n - 0.5f;
    if (!linear) {
        i64 i = __float2ll_rz(floorf(f + 0.5f));
        i64 last = (i64)n - 1;
        i = i < 0 ? 0 : (i > last ? last : i);
        p.i0 = (u32)i;
        p.i1 = (u32)i;
        return p;
    }
    return tap_at(f, n);
}

// A shader at a pixel (`Shader::at`): its numbers at `p`, and its column's
// and its row's values. Upright, those are words the processor worked out
// (`col`, `row`). Turned, they are worked out here, from where the pixel's
// centre is in the quad (`u`, `v`), as the shader's `col` and `row` do: each
// value is had first, by one way or the other, and the shader's arithmetic
// is written once. A blur's taps are a list; turned, each is worked out in
// the loop that reads it
__device__ float4 shade(const Cmd &c, const Tex &x, u32 kind, u64 p, bool turned, u64 col, u64 row, float u, float v) {
    // `shadow::Box_`: how far past the rectangle's straight edge, each way
    if (kind == SH_BOX) {
        float r = c.f(p), ramp = c.f(p + 1);
        float qx, qy;
        if (turned) {
            float sx = c.f(p + 6), sy = c.f(p + 7), hx = c.f(p + 8), hy = c.f(p + 9);
            qx = fmaxf(fabsf(u * sx - sx * 0.5f) - hx + r, 0.0f);
            qy = fmaxf(fabsf(v * sy - sy * 0.5f) - hy + r, 0.0f);
        } else {
            qx = c.f(col);
            qy = c.f(row);
        }
        float d = sqrtf(qx * qx + qy * qy) - r;
        float a = 1.0f - smoothstep(0.0f, ramp, d);
        return make_float4(c.f(p + 2) * a * a, c.f(p + 3) * a * a, c.f(p + 4) * a * a, c.f(p + 5) * a * a);
    }
    // `effect::Mask`
    if (kind == SH_MASK) {
        u32 si = c.at(p), mi = c.at(p + 1);
        Tap sc, mc, sr, mr;
        if (turned) {
            sc = across(c, si, u, true, false);
            mc = across(c, mi, u, true, false);
            sr = across(c, si, v, true, true);
            mr = across(c, mi, v, true, true);
        } else {
            sc = tap_word(c, col);
            mc = tap_word(c, col + 2);
            sr = tap_word(c, row);
            mr = tap_word(c, row + 2);
        }
        Texture src = texture(c, si), mask = texture(c, mi);
        float a = read_alpha(x, mask, mc, mr);
        if (c.at(p + 2)) a = 1.0f - a;
        float4 s = read(x, src, sc, sr);
        return make_float4(s.x * a, s.y * a, s.z * a, s.w * a);
    }
    // `field::Ink`
    if (kind == SH_INK) {
        u32 ti = c.at(p);
        Tap tc = turned ? across(c, ti, u, true, false) : tap_word(c, col);
        Tap tr = turned ? across(c, ti, v, true, true) : tap_word(c, row);
        Texture t = texture(c, ti);
        float lo = c.f(p + 5), hi = c.f(p + 6);
        float d = read_alpha(x, t, tc, tr);
        float tt = clamp01((d - lo) / (hi - lo));
        float a = tt * tt * (3.0f - 2.0f * tt);
        return make_float4(c.f(p + 1) * a, c.f(p + 2) * a, c.f(p + 3) * a, c.f(p + 4) * a);
    }
    // `blur::Mix`
    if (kind == SH_MIX) {
        u32 n = c.at(p);
        float o[4] = {0.0f, 0.0f, 0.0f, 0.0f};
#pragma unroll 1
        for (u32 k = 0; k < n && k < 8; k++) {
            u32 ti = c.at(p + 1 + k * 2);
            Texture t = texture(c, ti);
            float wt = c.f(p + 2 + k * 2);
            Tap tc = turned ? across(c, ti, u, true, false) : tap_word(c, col + k * 2);
            Tap tr = turned ? across(c, ti, v, true, true) : tap_word(c, row + k * 2);
            float4 s = read(x, t, tc, tr);
            o[0] = o[0] + s.x * wt;
            o[1] = o[1] + s.y * wt;
            o[2] = o[2] + s.z * wt;
            o[3] = o[3] + s.w * wt;
        }
        return make_float4(o[0], o[1], o[2], o[3]);
    }
    // `blur::Gauss`: `n` reads, the columns `lc` taps and the rows `lr`.
    // Turned, read `i` is `step` times its place along the pass, across or
    // down, and the other way the pixel's own
    if (kind == SH_GAUSS) {
        u32 ti = c.at(p);
        Texture t = texture(c, ti);
        u32 n = c.at(p + 1), lc = c.at(p + 2), lr = c.at(p + 3);
        float total = c.f(p + 4);
        bool shadow = c.at(p + 5) != 0;
        float thickness = c.f(p + 10);
        u64 weights = p + 11;
        float step = c.f(p + 11 + n);
        bool along_x = c.at(p + 12 + n) != 0;
        u64 places = p + 13 + n;
        if (lc == 0 || lr == 0) return make_float4(0.0f, 0.0f, 0.0f, 0.0f);
        if (shadow) {
            float sum = -0.0f;
#pragma unroll 1
            for (u32 i = 0; i < n && i < 64; i++) {
                u32 ci = i < lc - 1 ? i : lc - 1, ri = i < lr - 1 ? i : lr - 1;
                Tap tc = turned ? across(c, ti, along_x ? u + step * c.f(places + i) : u, true, false) : tap_word(c, col + ci * 2);
                Tap tr = turned ? across(c, ti, along_x ? v : v + step * c.f(places + i), true, true) : tap_word(c, row + ri * 2);
                sum = sum + read_alpha(x, t, tc, tr) * c.f(weights + i);
            }
            sum = sum / total;
            float a = thickness <= 0.0f ? (sum > 0.0f ? 1.0f : 0.0f) : clamp01(sum / thickness);
            return make_float4(c.f(p + 6) * a, c.f(p + 7) * a, c.f(p + 8) * a, c.f(p + 9) * a);
        }
        float o[4] = {0.0f, 0.0f, 0.0f, 0.0f};
#pragma unroll 1
        for (u32 i = 0; i < n && i < 64; i++) {
            u32 ci = i < lc - 1 ? i : lc - 1, ri = i < lr - 1 ? i : lr - 1;
            float wt = c.f(weights + i);
            Tap tc = turned ? across(c, ti, along_x ? u + step * c.f(places + i) : u, true, false) : tap_word(c, col + ci * 2);
            Tap tr = turned ? across(c, ti, along_x ? v : v + step * c.f(places + i), true, true) : tap_word(c, row + ri * 2);
            float4 s = read(x, t, tc, tr);
            o[0] = o[0] + s.x * wt;
            o[1] = o[1] + s.y * wt;
            o[2] = o[2] + s.z * wt;
            o[3] = o[3] + s.w * wt;
        }
        return make_float4(o[0] / total, o[1] / total, o[2] / total, o[3] / total);
    }
    // `gradient::Radial`: a column is its distance across and, with a mask,
    // a tap; a row the same down
    if (kind == SH_RADIAL) {
        Texture ramp = texture(c, c.at(p));
        Tap column = tap_word(c, p + 1);
        u32 m = c.at(p + 3);
        float rx = c.f(p + 4), ry = c.f(p + 5), s = c.f(p + 6), co = c.f(p + 7);
        float dx, dy;
        Tap mc, mr;
        if (turned) {
            dx = u - c.f(p + 8);
            dy = v - c.f(p + 9);
            mc = across(c, m, u, true, false);
            mr = across(c, m, v, true, true);
        } else {
            dx = c.f(col);
            dy = c.f(row);
            mc = tap_word(c, col + 1);
            mr = tap_word(c, row + 1);
        }
        float xx = (dx * co - dy * s) * rx;
        float yy = (dx * s + dy * co) * ry;
        float vv = 2.0f * sqrtf(xx * xx + yy * yy);
        float4 colour = read(x, ramp, column, tap_at(vv * (float)ramp.h - 0.5f, ramp.h));
        if (m != NO_TEXTURE) {
            Texture mask = texture(c, m);
            float a = read_alpha(x, mask, mc, mr);
            return make_float4(colour.x * a, colour.y * a, colour.z * a, colour.w * a);
        }
        return colour;
    }
    // `multi::Multi`
    if (kind == SH_MULTI) {
        u32 si = c.at(p), m = c.at(p + 1);
        Tap sc, mc, sr, mr;
        if (turned) {
            bool smooth = c.at(p + 14) != 0;
            sc = across(c, si, u, true, false);
            sr = across(c, si, v, true, true);
            mc = across(c, m, u, smooth, false);
            mr = across(c, m, v, smooth, true);
        } else {
            sc = tap_word(c, col);
            sr = tap_word(c, row);
            mc = tap_word(c, col + 2);
            mr = tap_word(c, row + 2);
        }
        Texture src = texture(c, si);
        float contrast = c.f(p + 2), brightness = c.f(p + 3), saturation = c.f(p + 4);
        float tint[4] = {c.f(p + 5), c.f(p + 6), c.f(p + 7), c.f(p + 8)};
        float e[4] = {c.f(p + 9), c.f(p + 10), c.f(p + 11), c.f(p + 12)};
        bool inverted = c.at(p + 13) != 0;
        float4 s = read(x, src, sc, sr);
        float a = s.w;
        float rgb[3] = {s.x, s.y, s.z};
        for (int i = 0; i < 3; i++) rgb[i] = (rgb[i] - 0.5f * a) * (1.0f + contrast) + 0.5f * a + brightness * a;
        float grey = 0.299f * rgb[0] + 0.587f * rgb[1] + 0.114f * rgb[2];
        float k = tint[3];
        for (int i = 0; i < 3; i++) {
            float tinted = tint[i] * grey * k + rgb[i] * (1.0f - k);
            rgb[i] = grey + (tinted - grey) * (1.0f + saturation);
        }
        float mm = 1.0f;
        if (m != NO_TEXTURE) {
            Texture mask = texture(c, m);
            float alpha = read_alpha(x, mask, mc, mr);
            mm = edge(e[0], e[1], alpha, false) * edge(e[2], e[3], 1.0f - alpha, true);
            if (inverted) mm = 1.0f - mm;
        }
        return make_float4(clamp01(rgb[0] * mm), clamp01(rgb[1] * mm), clamp01(rgb[2] * mm), clamp01(a * mm));
    }
    return make_float4(0.0f, 0.0f, 0.0f, 0.0f);
}

// One operation at one pixel

// `a[k]` for a `k` the program works out, without indexing (see OP_QUAD_TURNED)
__device__ i64 pick(const i64 *a, int k) {
    return k == 0 ? a[0] : (k == 1 ? a[1] : (k == 2 ? a[2] : a[3]));
}

__device__ u32 run(const Cmd &c, const Tex &x, u64 op, u64 data_at, int px_x, int px_y, u32 px) {
    u32 kind = c.at(op);
    int cx0 = (int)c.at(op + 1), cy0 = (int)c.at(op + 2), cx1 = (int)c.at(op + 3), cy1 = (int)c.at(op + 4);
    if (px_x < cx0 || px_y < cy0 || px_x >= cx1 || px_y >= cy1) return px;
    u64 d = data_at + c.at(op + 5);
    u64 words = c.at(op + 6);
    float opacity = c.f(op + 7);
    u64 a = op + 8;
    i64 cx = (i64)px_x * 256 + 128, cy = (i64)px_y * 256 + 128;
    if (kind == OP_FILL) {
        u32 s[4];
        unpack(c.at(a), s);
        return over(px, s, 255);
    }
    if (kind == OP_TRIS)
        return tris(c, x, d, words, c.at(a), c.at(a + 2), c.at(a + 1), opacity, cx, cy, px);
    // An upright quad: `raster::upright`, its taps worked out
    if (kind == OP_QUAD) {
        Texture t = texture(c, c.at(a));
        int x0 = (int)c.at(a + 1), y0 = (int)c.at(a + 2);
        u32 cols = c.at(a + 3), rows = c.at(a + 4), flags = c.at(a + 5);
        int ci = px_x - x0, ri = px_y - y0;
        if (ci < 0 || ri < 0 || (u32)ci >= cols || (u32)ri >= rows) return px;
        if ((u64)(cols + rows) * 2 > words) return px;
        Tap ct = tap_word(c, d + (u64)ci * 2), rt = tap_word(c, d + (u64)cols * 2 + (u64)ri * 2);
        if (flags & 2u) {
            u32 tw = texel_word(x, t, ct.i0, rt.i0);
            if ((tw >> 24) == 255u && opacity >= 1.0f) return tw;
            if (tw == 0) return px;
            float f16 = (float)((tw >> 16) & 0xffu) / 255.0f * opacity;
            float f8 = (float)((tw >> 8) & 0xffu) / 255.0f * opacity;
            float f0 = (float)(tw & 0xffu) / 255.0f * opacity;
            float f24 = (float)((tw >> 24) & 0xffu) / 255.0f * opacity;
            return blend(px, f16, f8, f0, f24);
        }
        float4 p = texel(x, t, ct.i0, rt.i0), q = texel(x, t, ct.i1, rt.i0);
        float4 r = texel(x, t, ct.i0, rt.i1), s = texel(x, t, ct.i1, rt.i1);
        float P[4] = {p.x, p.y, p.z, p.w}, Q[4] = {q.x, q.y, q.z, q.w}, R[4] = {r.x, r.y, r.z, r.w}, S[4] = {s.x, s.y, s.z, s.w};
        float o[4];
        for (int k = 0; k < 4; k++) {
            float top = P[k] + (Q[k] - P[k]) * ct.t;
            float bottom = R[k] + (S[k] - R[k]) * ct.t;
            o[k] = (top + (bottom - top) * rt.t) * opacity;
        }
        return blend(px, o[0], o[1], o[2], o[3]);
    }
    // A turned quad: its two triangles, the texture sampled at each centre
    if (kind == OP_QUAD_TURNED) {
        Texture t = texture(c, c.at(a));
        u32 flags = c.at(a + 1);
        bool linear = (flags & 1u) != 0, glyph = (flags & 2u) != 0;
        if (words < 12) return px;
        i64 X[4], Y[4];
        for (int k = 0; k < 4; k++) {
            X[k] = (i64)(int)c.at(d + k * 2);
            Y[k] = (i64)(int)c.at(d + k * 2 + 1);
        }
        float u0 = c.f(d + 8), v0 = c.f(d + 9), u1 = c.f(d + 10), v1 = c.f(d + 11);
        // An image's strip is cut top right to bottom left, 0 2 1 and 2 1 3;
        // a glyph's top left to bottom right, 0 2 3 and 3 1 0. Chosen with
        // selects, not by indexing: an array indexed by a number the program
        // works out goes to the stack, and the kernel's launches have none
        for (int k = 0; k < 2; k++) {
            bool second = k == 1;
            // corner 0 1 2 of this triangle, as 0..3 of the quad
            int p0 = glyph ? (second ? 3 : 0) : (second ? 2 : 0);
            int p1 = glyph ? (second ? 1 : 2) : (second ? 1 : 2);
            int p2 = glyph ? (second ? 0 : 3) : (second ? 3 : 1);
            i64 tx[3] = {pick(X, p0), pick(X, p1), pick(X, p2)};
            i64 ty[3] = {pick(Y, p0), pick(Y, p1), pick(Y, p2)};
            float l[3];
            if (!covers(tx, ty, cx, cy, l)) continue;
            // u is u0 at corners 0 and 2, u1 at 1 and 3; v is v0 at 0 and 1
            float ua = (p0 & 1) ? u1 : u0, ub = (p1 & 1) ? u1 : u0, uc = (p2 & 1) ? u1 : u0;
            float va = (p0 & 2) ? v1 : v0, vb = (p1 & 2) ? v1 : v0, vc = (p2 & 2) ? v1 : v0;
            float u = ua * l[0] + ub * l[1] + uc * l[2];
            float v = va * l[0] + vb * l[1] + vc * l[2];
            px = blend4(px, sample(x, t, u, v, linear), opacity);
        }
        return px;
    }
    // A shader on a quad: `raster::shaded`. Upright, its columns' and rows'
    // values are the processor's; turned, it is the quad's two triangles and
    // the values are worked out at each centre. One call of `shade` for
    // both, in one loop: two would be its code twice, and the program is
    // near the size of its slot. The eight words are the same for both -
    // shader, first column, first row, columns, rows, a column's words, a
    // row's, the numbers' - and a turned one's data starts with its corners
    if (kind == OP_SHADED || kind == OP_SHADED_TURNED) {
        bool turned = kind == OP_SHADED_TURNED;
        u32 sk = c.at(a);
        int x0 = (int)c.at(a + 1), y0 = (int)c.at(a + 2);
        u32 cols = c.at(a + 3), rows = c.at(a + 4), cw = c.at(a + 5), rw = c.at(a + 6), pw = c.at(a + 7);
        u64 params = turned ? d + 8 : d;
        if (turned ? words < 8 + (u64)pw : (u64)pw + (u64)cols * cw + (u64)rows * rw > words) return px;
        int ci = px_x - x0, ri = px_y - y0;
        if (!turned && (ci < 0 || ri < 0 || (u32)ci >= cols || (u32)ri >= rows)) return px;
        i64 X[4], Y[4];
        for (int k = 0; k < 4; k++) {
            X[k] = turned ? (i64)(int)c.at(d + k * 2) : 0;
            Y[k] = turned ? (i64)(int)c.at(d + k * 2 + 1) : 0;
        }
#pragma unroll 1
        for (int k = 0; k < (turned ? 2 : 1); k++) {
            float u = 0.0f, v = 0.0f;
            if (turned) {
                bool second = k == 1;
                // 0 2 1, then 2 1 3: an image node's strip
                int p0 = second ? 2 : 0, p1 = second ? 1 : 2, p2 = second ? 3 : 1;
                i64 tx[3] = {pick(X, p0), pick(X, p1), pick(X, p2)};
                i64 ty[3] = {pick(Y, p0), pick(Y, p1), pick(Y, p2)};
                float l[3];
                if (!covers(tx, ty, cx, cy, l)) continue;
                // u is 1 at corners 1 and 3, v at 2 and 3
                float ua = (p0 & 1) ? 1.0f : 0.0f, ub = (p1 & 1) ? 1.0f : 0.0f, uc = (p2 & 1) ? 1.0f : 0.0f;
                float va = (p0 & 2) ? 1.0f : 0.0f, vb = (p1 & 2) ? 1.0f : 0.0f, vc = (p2 & 2) ? 1.0f : 0.0f;
                u = ua * l[0] + ub * l[1] + uc * l[2];
                v = va * l[0] + vb * l[1] + vc * l[2];
            }
            u64 col = d + pw + (u64)(ci < 0 ? 0 : ci) * cw;
            u64 row = d + pw + (u64)cols * cw + (u64)(ri < 0 ? 0 : ri) * rw;
            px = blend4(px, shade(c, x, sk, params, turned, col, row, u, v), opacity);
        }
        return px;
    }
    // `Painter::mask`: a glyph's coverage over, in one colour
    if (kind == OP_MASK) {
        Texture t = texture(c, c.at(a));
        int x0 = (int)c.at(a + 1), y0 = (int)c.at(a + 2);
        u32 s[4];
        unpack(c.at(a + 3), s);
        u32 cov = mask_byte(x, t, px_x - x0, px_y - y0);
        return over(px, s, cov);
    }
    // `Painter::styled_mask`
    if (kind == OP_STYLED) {
        Texture t = texture(c, c.at(a));
        int x0 = (int)c.at(a + 1), y0 = (int)c.at(a + 2);
        u32 col[4], sc[4];
        unpack(c.at(a + 3), col);
        unpack(c.at(a + 4), sc);
        u32 style = c.at(a + 5);
        int l = style == 1 ? 1 : 0, tm = (style == 1 || style == 3) ? 1 : 0;
        int r = style == 1 ? 1 : 0, b = (style == 1 || style == 2) ? 1 : 0;
        int mx = px_x - x0, my = px_y - y0;
        if (mx < -l || my < -tm || mx >= (int)t.w + r || my >= (int)t.h + b) return px;
        int g = (int)mask_byte(x, t, mx, my);
        int around = 0;
        if (style == 2) around = (int)mask_byte(x, t, mx, my - 1);
        else if (style == 3) around = (int)mask_byte(x, t, mx, my + 1);
        else if (style == 1) {
            around = (int)mask_byte(x, t, mx, my + 1) + (int)mask_byte(x, t, mx, my - 1)
                + (int)mask_byte(x, t, mx + 1, my) + (int)mask_byte(x, t, mx - 1, my);
            around = around > 255 ? 255 : around;
        }
        int st = around - g;
        st = st < 0 ? 0 : (st > 255 ? 255 : st);
        if (st == 0 && g == 0) return px;
        u32 m[4];
        for (int k = 0; k < 4; k++) {
            u32 v = ((u32)st * sc[k] + (u32)g * col[k] + 127) / 255;
            m[k] = v > 255u ? 255u : v;
        }
        return over(px, m, 255);
    }
    return px;
}

// A window's buffers are in one of up to four pools of video memory (a big
// window's its own, the small ones sharing, `aosh/src/scene/gpu/device.rs`), so
// a frame of every window is one launch
extern "C" __global__ void paint(const u32 *cmd, u64 cmd_bytes, const unsigned char *tex, u64 tex_bytes,
                                 u32 *dst0, u64 bytes0, u32 *dst1, u64 bytes1, u32 *dst2, u64 bytes2,
                                 u32 *dst3, u64 bytes3) {
    Cmd c;
    c.w = cmd;
    c.n = cmd_bytes / 4;
    __shared__ float by255[256];
    by255[threadIdx.x & 255u] = (float)(threadIdx.x & 255u) / 255.0f;
    __syncthreads();
    Tex x;
    x.base = tex;
    x.bytes = tex_bytes;
    x.by255 = by255;
    if (c.at(0) != MAGIC || c.at(1) != VERSION) return;
    u32 tiles = c.at(2);
    if (blockIdx.x >= tiles) return;
    u64 tile = (u64)c.at(3) + (u64)blockIdx.x * 8;
    u32 target = c.at(tile), tx = c.at(tile + 1), ty = c.at(tile + 2), tw = c.at(tile + 3), th = c.at(tile + 4);
    u32 list = c.at(tile + 5), count = c.at(tile + 6);
    u32 i = threadIdx.x % 16, j = threadIdx.x / 16;
    if (i >= tw || j >= th || tw > 16 || th > 16) return;
    if (target >= c.at(4)) return;
    u64 te = (u64)c.at(5) + (u64)target * 5;
    u64 off = c.at(te);
    u32 stride = c.at(te + 1), width = c.at(te + 2), height = c.at(te + 3), pool = c.at(te + 4);
    if (pool > 3) return;
    // By its two bits, not by its value: a choice among 0, 1, 2 and 3 is a
    // jump table to nvcc, and a jump table is constant bank 2
    bool odd = (pool & 1u) != 0, high = (pool & 2u) != 0;
    u32 *dst = high ? (odd ? dst3 : dst2) : (odd ? dst1 : dst0);
    u64 dst_bytes = high ? (odd ? bytes3 : bytes2) : (odd ? bytes1 : bytes0);
    u32 x_ = tx + i, y_ = ty + j;
    if (x_ >= width || y_ >= height || stride < width) return;
    u64 at = off + ((u64)y_ * stride + x_) * 4;
    if (at + 4 > dst_bytes) return;

    u32 ops = c.at(8);
    u64 ops_at = c.at(9), lists_at = c.at(10), data_at = c.at(11);
    u32 px = 0;
#pragma unroll 1
    for (u32 k = 0; k < count; k++) {
        u32 o = c.at(lists_at + list + k);
        if (o >= ops) continue;
        px = run(c, x, ops_at + (u64)o * 16, data_at, (int)x_, (int)y_, px);
    }
    dst[at / 4] = to_straight(px);
}
