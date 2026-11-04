// Dual kawase, as in Hyprland
//
// Two passes undoing each other's resolution: a chain of halvings, each
// gathering five taps, then a chain of doublings gathering eight. The blur
// radius grows geometrically with the number of levels while the work shrinks
// geometrically.
//
// `blur1.glsl` and `blur2.glsl` are the originals and the tap weights are
// theirs. The coordinate system is not: GL hands a shader normalised
// coordinates and an interpolating sampler. Here both are written out: texel
// centres in pixels, and a bilinear read by hand.
//
// It reads a rectangle of pixels with a stride and writes a rectangle of pixels
// with a stride: pitch-linear in video memory, with no textures, samplers,
// swizzling or tiling.

typedef unsigned int u32;

struct Px {
    float b, g, r, a;
};

// A8R8G8B8, which is what the display scans and what every surface holds
__device__ inline Px unpack(u32 v) {
    Px p;
    p.b = (float)(v & 0xffu);
    p.g = (float)((v >> 8) & 0xffu);
    p.r = (float)((v >> 16) & 0xffu);
    p.a = (float)((v >> 24) & 0xffu);
    return p;
}

__device__ inline u32 pack(Px p) {
    // Rounded rather than truncated: six passes each losing half a level of
    // every channel drift visibly towards black
    int b = (int)(p.b + 0.5f), g = (int)(p.g + 0.5f);
    int r = (int)(p.r + 0.5f), a = (int)(p.a + 0.5f);
    b = b < 0 ? 0 : (b > 255 ? 255 : b);
    g = g < 0 ? 0 : (g > 255 ? 255 : g);
    r = r < 0 ? 0 : (r > 255 ? 255 : r);
    a = a < 0 ? 0 : (a > 255 ? 255 : a);
    return ((u32)a << 24) | ((u32)r << 16) | ((u32)g << 8) | (u32)b;
}

__device__ inline Px fetch(const u32 *src, int stride, int w, int h, int x, int y) {
    // Clamped to the edge. The taps reach outside the rectangle, and what they
    // find there has to be the edge pixel rather than the next row: a wrap
    // shows as a bright seam down one side
    x = x < 0 ? 0 : (x >= w ? w - 1 : x);
    y = y < 0 ? 0 : (y >= h ? h - 1 : y);
    return unpack(src[y * stride + x]);
}

/// One bilinear sample at a floating point texel-centre coordinate
///
/// The sampler GL provides. Every tap in both passes lands between pixels,
/// where a five-tap kernel gets the coverage of a much wider one
__device__ inline Px sample(const u32 *src, int stride, int w, int h, float x, float y) {
    float fx = x - 0.5f, fy = y - 0.5f;
    int x0 = (int)floorf(fx), y0 = (int)floorf(fy);
    float tx = fx - (float)x0, ty = fy - (float)y0;

    Px p00 = fetch(src, stride, w, h, x0, y0);
    Px p10 = fetch(src, stride, w, h, x0 + 1, y0);
    Px p01 = fetch(src, stride, w, h, x0, y0 + 1);
    Px p11 = fetch(src, stride, w, h, x0 + 1, y0 + 1);

    float w00 = (1.0f - tx) * (1.0f - ty), w10 = tx * (1.0f - ty);
    float w01 = (1.0f - tx) * ty, w11 = tx * ty;

    Px o;
    o.b = p00.b * w00 + p10.b * w10 + p01.b * w01 + p11.b * w11;
    o.g = p00.g * w00 + p10.g * w10 + p01.g * w01 + p11.g * w11;
    o.r = p00.r * w00 + p10.r * w10 + p01.r * w01 + p11.r * w11;
    o.a = p00.a * w00 + p10.a * w10 + p01.a * w01 + p11.a * w11;
    return o;
}

__device__ inline void add(Px *acc, Px v, float k) {
    acc->b += v.b * k;
    acc->g += v.g * k;
    acc->r += v.r * k;
    acc->a += v.a * k;
}

/// Halves the resolution. Five taps: the centre at weight four and four
/// diagonals at one, over eight
extern "C" __global__ void blur_down(u32 *dst, const u32 *src,
                                     int dstride, int sstride,
                                     int dw, int dh, int sw, int sh, float radius) {
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= dw || y >= dh)
        return;

    // Where this destination texel's centre falls in the source, which is twice
    // as large in each direction
    float sx = ((float)x + 0.5f) * 2.0f;
    float sy = ((float)y + 0.5f) * 2.0f;
    float o = radius;

    Px acc = {0.0f, 0.0f, 0.0f, 0.0f};
    add(&acc, sample(src, sstride, sw, sh, sx, sy), 4.0f);
    add(&acc, sample(src, sstride, sw, sh, sx - o, sy - o), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx + o, sy + o), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx + o, sy - o), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx - o, sy + o), 1.0f);

    acc.b *= 0.125f; acc.g *= 0.125f; acc.r *= 0.125f; acc.a *= 0.125f;
    dst[y * dstride + x] = pack(acc);
}

/// Doubles the resolution. Eight taps in a diamond: the corners at two, the
/// points at one, over twelve
extern "C" __global__ void blur_up(u32 *dst, const u32 *src,
                                   int dstride, int sstride,
                                   int dw, int dh, int sw, int sh, float radius) {
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= dw || y >= dh)
        return;

    float sx = ((float)x + 0.5f) * 0.5f;
    float sy = ((float)y + 0.5f) * 0.5f;
    float o = radius;

    Px acc = {0.0f, 0.0f, 0.0f, 0.0f};
    add(&acc, sample(src, sstride, sw, sh, sx - 2.0f * o, sy), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx - o, sy + o), 2.0f);
    add(&acc, sample(src, sstride, sw, sh, sx, sy + 2.0f * o), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx + o, sy + o), 2.0f);
    add(&acc, sample(src, sstride, sw, sh, sx + 2.0f * o, sy), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx + o, sy - o), 2.0f);
    add(&acc, sample(src, sstride, sw, sh, sx, sy - 2.0f * o), 1.0f);
    add(&acc, sample(src, sstride, sw, sh, sx - o, sy - o), 2.0f);

    float k = 1.0f / 12.0f;
    acc.b *= k; acc.g *= k; acc.r *= k; acc.a *= k;
    dst[y * dstride + x] = pack(acc);
}
