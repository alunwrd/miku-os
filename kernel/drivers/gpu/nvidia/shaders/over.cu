// One surface laid over another
//
// The shell below the windows (the wallpaper and the layers in BACKGROUND and
// BOTTOM) is kept as one image in video memory
// (`dev/mayland-below-windows.md`), built by this: the bottom surface copied
// in, every one above it laid over with its alpha. Hyprland draws them the same
// way, one textured quad after another, before the windows and the blur.
//
// Not `composite`: that is a window, with a corner, a shadow and antialiasing
// along its edge. A layer covering the screen has no edge, and softening one
// would leave a half-blended border round the screen.
//
// Straight alpha, as `paint::over` and the processor's compose blend it: the
// two paths must agree on every pixel.

typedef unsigned int u32;

/// What the source's fourth byte is, and whether there is a source at all
enum {
    /// An alpha channel: ARGB8888
    MODE_ALPHA = 0,
    /// Padding: XRGB8888, opaque but for the alpha it is drawn with
    MODE_OPAQUE = 1,
    /// No source: the rectangle is cleared to black, for what no surface covers
    MODE_CLEAR = 2,
};

/// `src` laid over `dst`, `w` by `h` pixels. Both point at the rectangle's
/// top-left and are indexed by their own strides, in pixels
extern "C" __global__ void over(u32 *dst, const u32 *src, int dstride, int sstride,
                                int w, int h, float alpha, int mode) {
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= w || y >= h)
        return;
    u32 *out = dst + y * dstride + x;
    if (mode == MODE_CLEAR) {
        *out = 0xff000000u;
        return;
    }
    u32 s = src[y * sstride + x];
    float a = alpha;
    if (mode == MODE_ALPHA)
        a *= (float)(s >> 24) * (1.0f / 255.0f);
    // The two ends exactly, so a copy is a copy and nothing is drawn where
    // nothing was asked for
    if (a >= 1.0f) {
        *out = s | 0xff000000u;
        return;
    }
    if (a <= 0.0f)
        return;
    u32 d = *out;
    u32 v = 0xff000000u;
    for (int shift = 0; shift < 24; shift += 8) {
        float sc = (float)((s >> shift) & 0xffu);
        float dc = (float)((d >> shift) & 0xffu);
        int c = (int)(sc * a + dc * (1.0f - a) + 0.5f);
        c = c < 0 ? 0 : (c > 255 ? 255 : c);
        v |= (u32)c << shift;
    }
    *out = v;
}
