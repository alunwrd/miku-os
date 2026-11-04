// A window, composed over what is under it
//
// Alpha, rounded corners, a border and a drop shadow are one operation: inside
// the window's curve it is blended over the backdrop, outside it the backdrop
// is darkened by how close it is, and the border goes over both.
//
// The corner and the border are Hyprland's (`surface_coverage`, `border_alpha`
// below); the shadow is a falloff from the signed distance to the same curve.
//
// The grid covers the window plus a margin wide enough for the shadow, so one
// launch paints the window and its shadow together. Pixels in the margin read
// no source pixel.
//
// It reads the backdrop from two buffers: `under` holds it sharp and `blurred`
// holds the same region after the kawase chain (see the shadow below).
//
// `dst` is written and never read, so the region under a window need not be
// laid down before every pass, and a shadow cannot darken over its own output.

typedef unsigned int u32;

struct Px {
    float b, g, r, a;
};

__device__ inline Px unpack(u32 v) {
    Px p;
    p.b = (float)(v & 0xffu);
    p.g = (float)((v >> 8) & 0xffu);
    p.r = (float)((v >> 16) & 0xffu);
    p.a = (float)((v >> 24) & 0xffu);
    return p;
}

__device__ inline u32 pack(float b, float g, float r) {
    int bi = (int)(b + 0.5f), gi = (int)(g + 0.5f), ri = (int)(r + 0.5f);
    bi = bi < 0 ? 0 : (bi > 255 ? 255 : bi);
    gi = gi < 0 ? 0 : (gi > 255 ? 255 : gi);
    ri = ri < 0 ? 0 : (ri > 255 ? 255 : ri);
    return 0xff000000u | ((u32)ri << 16) | ((u32)gi << 8) | (u32)bi;
}

/// Distance from a point to a rectangle with rounded corners, negative inside
///
/// For the shadow only: a circle's distance is near enough the superellipse for
/// a gradient tens of pixels wide. The window's edge is `surface_coverage`'s.
///
/// The rectangle is centred on the origin with half-extents `hw`, `hh`. The
/// standard rounded-box distance: shrink the box by the corner radius, take the
/// distance to that, and subtract the radius back
__device__ inline float rounded_box(float px, float py, float hw, float hh, float r) {
    float qx = fabsf(px) - (hw - r);
    float qy = fabsf(py) - (hh - r);
    float mx = qx > 0.0f ? qx : 0.0f;
    float my = qy > 0.0f ? qy : 0.0f;
    float outside = sqrtf(mx * mx + my * my);
    float inside = (qx > qy ? qx : qy);
    if (inside > 0.0f)
        inside = 0.0f;
    return outside + inside - r;
}

// The window's outline, as Hyprland draws it
//
// `libmay::shape` (`lib/mayland/src/shape.rs`) is the definition, from
// Hyprland's `rounding.glsl` and `border.glsl`; these are the same lines, so
// the card and the processor cut one corner (`dev/mayland-window-shape.md`).
// Coordinates are a pixel's centre from the box's top-left. `__powf` is the
// instruction GLSL's `pow` compiles to here

#define SMOOTHING (3.14159265358979323846f / 5.34665792551f)

__device__ inline float smooth01(float t) {
    t = t < 0.0f ? 0.0f : (t > 1.0f ? 1.0f : t);
    return t * t * (3.0f - 2.0f * t);
}

__device__ inline float superellipse(float x, float y, float p) {
    return __powf(__powf(x, p) + __powf(y, p), 1.0f / p);
}

/// How much of the window's own pixel its corner leaves: `rounding()`
__device__ inline float surface_coverage(float px, float py, float w, float h,
                                         float r, float p) {
    if (r <= 0.0f)
        return 1.0f;
    float qx = fabsf(px - w * 0.5f) - (w * 0.5f - r) + 1.0f / w;
    float qy = fabsf(py - h * 0.5f) - (h * 0.5f - r) + 1.0f / h;
    if (qx + qy <= r)
        return 1.0f;
    float d = superellipse(qx > 0.0f ? qx : 0.0f, qy > 0.0f ? qy : 0.0f, p);
    if (d > r + SMOOTHING)
        return 0.0f;
    return 1.0f - smooth01((d - r + SMOOTHING) / (SMOOTHING * 2.0f));
}

/// How much of the border's colour a pixel of its box takes: `getBorder`
__device__ inline float border_alpha(float px, float py, float bw, float bh,
                                     float radius, float outer, float p, float thick) {
    float fx = fabsf(px - bw * 0.5f);
    float fy = fabsf(py - bh * 0.5f);
    float cx = fx - (bw * 0.5f - radius) + 1.0f / bw;
    float cy = fy - (bh * 0.5f - radius) + 1.0f / bh;
    float ox = fx - (bw * 0.5f - outer) + 1.0f / bw;
    float oy = fy - (bh * 0.5f - outer) + 1.0f / bh;
    if ((cx < cy ? cx : cy) > 0.0f && radius > 0.0f) {
        float d = superellipse(cx, cy, p);
        if (d < radius - thick * 0.5f)
            return smooth01((d - radius + thick + SMOOTHING) / (SMOOTHING * 2.0f));
        if ((ox < oy ? ox : oy) > 0.0f)
            return 1.0f - smooth01((superellipse(ox, oy, p) - outer + SMOOTHING) / (SMOOTHING * 2.0f));
    }
    float m = px < bw - px ? px : bw - px;
    m = m < py ? m : py;
    m = m < bh - py ? m : bh - py;
    return m > thick ? 0.0f : 1.0f;
}

/// Composes one window over the surface
///
/// `gw`/`gh` are the grid, and `ox`/`oy` are where the window's top-left sits
/// inside it, signed, because a grid covering only what changed can start
/// inside the window. `dst`, `under` and `blurred` point at the top-left of
/// that grid and are indexed by it; `src` points at the window's top-left and
/// is reached through the offsets.
///
/// `shadow_falloff` is how far the shadow reaches.
///
/// `scale` is what a `popin` animation moves: the window is drawn into a box
/// that fraction of its rectangle, centred, so it grows into place. The grid is
/// still the whole rectangle and its margin, and between the box and the
/// rectangle is whatever is behind it. At 1 the branch below reads one-to-one
/// without a filter
extern "C" __global__ void composite(u32 *dst, const u32 *under, const u32 *blurred,
                                     const u32 *src,
                                     int dstride, int ustride, int bstride, int sstride,
                                     int w, int h,
                                     int gw, int gh, int ox, int oy,
                                     float alpha, float corner,
                                     float shadow, float shadow_falloff,
                                     int src_has_alpha, float scale,
                                     float power, float dim,
                                     u32 border_colour, float border) {
    // The grid is whatever rectangle the caller chose, and the window's corner
    // sits at `(ox, oy)` inside it. Both go negative when the rectangle starts
    // inside the window
    int total_w = gw;
    int total_h = gh;
    int x = blockIdx.x * blockDim.x + threadIdx.x;
    int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= total_w || y >= total_h)
        return;

    // Where this pixel sits relative to the window's centre
    float hw = (float)w * 0.5f;
    float hh = (float)h * 0.5f;
    float px = (float)x + 0.5f - (float)ox - hw;
    float py = (float)y + 0.5f - (float)oy - hh;

    // The box the window is drawn in this frame: the whole rectangle, or that
    // fraction of it around the same centre while a `popin` runs. The corner,
    // the edge and the shadow are measured from this box, so what is around the
    // window is what is behind it, as on the processor's path
    // (`compose::placed`)
    float s = scale < 0.999f ? scale : 1.0f;
    float bw = hw * s;
    float bh = hh * s;

    // The radius every curve is built on: `corner · power / 2`, what Hyprland's
    // `rounding()` makes of the configured number. The shadow's distance field
    // is measured from the same curve
    float base_r = corner * (power * 0.5f);
    float r = base_r;
    float lim = bw < bh ? bw : bh;
    if (r > lim)
        r = lim;
    float d = rounded_box(px, py, bw, bh, r);

    Px sharp = unpack(under[y * ustride + x]);

    // Where this pixel's centre is in the window's box, from its top-left
    float W = bw * 2.0f, H = bh * 2.0f;
    float lx = px + bw, ly = py + bh;
    float cov = 0.0f;
    if (lx > 0.0f && lx < W && ly > 0.0f && ly < H) {
        float rs = base_r - 1.0f;
        cov = surface_coverage(lx, ly, W, H, rs > 0.0f ? rs : 0.0f, power);
    }

    float ob, og, orr;
    if (cov > 0.0f) {
        // The backdrop this pixel is blended over: the blurred one where the
        // window covers it, the sharp one at the very edge of the corner arc.
        // `blurred` everywhere would leave a one-pixel soft fringe around the
        // arc
        Px soft = unpack(blurred[y * bstride + x]);
        Px base;
        base.b = soft.b * cov + sharp.b * (1.0f - cov);
        base.g = soft.g * cov + sharp.g * (1.0f - cov);
        base.r = soft.r * cov + sharp.r * (1.0f - cov);
        int sx = x - ox;
        int sy = y - oy;
        // Inside the box means inside the window, so these are in range by
        // construction. Clamped anyway: the arithmetic runs through two signed
        // offsets
        if (sx < 0) sx = 0;
        if (sy < 0) sy = 0;
        if (sx > w - 1) sx = w - 1;
        if (sy > h - 1) sy = h - 1;
        Px win;
        if (scale >= 0.999f) {
            win = unpack(src[sy * sstride + sx]);
        } else {
            // The whole buffer fitted into the smaller box, bilinearly, because
            // nearest-neighbour drops stems on text. The window's centre is the
            // fixed point. Inside the box by construction, so the clamps below
            // only catch the last half pixel
            float u = px / s + hw - 0.5f;
            float v = py / s + hh - 0.5f;
            if (u < 0.0f) u = 0.0f;
            if (v < 0.0f) v = 0.0f;
            if (u > (float)(w - 1)) u = (float)(w - 1);
            if (v > (float)(h - 1)) v = (float)(h - 1);
            int u0 = (int)u, v0 = (int)v;
            int u1 = u0 + 1 < w ? u0 + 1 : u0;
            int v1 = v0 + 1 < h ? v0 + 1 : v0;
            float fu = u - (float)u0, fv = v - (float)v0;
            Px p00 = unpack(src[v0 * sstride + u0]);
            Px p10 = unpack(src[v0 * sstride + u1]);
            Px p01 = unpack(src[v1 * sstride + u0]);
            Px p11 = unpack(src[v1 * sstride + u1]);
            float w00 = (1.0f - fu) * (1.0f - fv);
            float w10 = fu * (1.0f - fv);
            float w01 = (1.0f - fu) * fv;
            float w11 = fu * fv;
            win.b = p00.b * w00 + p10.b * w10 + p01.b * w01 + p11.b * w11;
            win.g = p00.g * w00 + p10.g * w10 + p01.g * w01 + p11.g * w11;
            win.r = p00.r * w00 + p10.r * w10 + p01.r * w01 + p11.r * w11;
            win.a = p00.a * w00 + p10.a * w10 + p01.a * w01 + p11.a * w11;
        }
        // The window's alpha channel multiplies the one asked for, so a client
        // drawing something transparent stays transparent, but only when the
        // buffer has an alpha channel. In XRGB8888 that byte means nothing and
        // is usually zero
        float sa = src_has_alpha ? win.a * (1.0f / 255.0f) : 1.0f;
        float a = alpha * cov * sa;
        // Dimmed first, as Hyprland's tint comes before its corner and its
        // opacity: the window's own colour, not what shows through it
        float tint = 1.0f - dim;
        ob = win.b * tint * a + base.b * (1.0f - a);
        og = win.g * tint * a + base.g * (1.0f - a);
        orr = win.r * tint * a + base.r * (1.0f - a);
    } else if (shadow > 0.0f && d < shadow_falloff) {
        // Outside: the shadow. Nothing to read from the window, and the
        // backdrop is darkened by how close it is to the edge.
        //
        // Over the sharp backdrop. The blur has to cover the margin as well as
        // the window, since a window's edge samples pixels beyond it, but
        // nothing translucent is over the margin, and writing the blurred
        // margin back would leave a square of smeared text around every window
        float t = 1.0f - d / shadow_falloff;
        // Squared, so the shadow is dense at the edge and fades away quickly
        // rather than ending in a visible ring
        float k = 1.0f - shadow * t * t;
        ob = sharp.b * k;
        og = sharp.g * k;
        orr = sharp.r * k;
    } else {
        // Past the shadow's reach, or no shadow at all: the backdrop, carried
        // through unchanged.
        //
        // `dst` is not the source, so a pixel nobody writes would be whatever
        // this surface held two frames ago. Every pixel of the grid is written
        ob = sharp.b;
        og = sharp.g;
        orr = sharp.r;
    }

    // The border, over all of it: drawn after the window, as Hyprland's is,
    // in the window's box grown by the border on every side. Its colour's own
    // alpha and the window's opacity both weigh it; the dim does not
    if (border >= 1.0f) {
        float bx = lx + border, by = ly + border;
        float BW = W + 2.0f * border, BH = H + 2.0f * border;
        if (bx > 0.0f && bx < BW && by > 0.0f && by < BH) {
            float inner = base_r == 0.0f ? 0.0f : base_r + border;
            float pull = 2.0f - power;
            float outer = base_r + border - border * 0.41421356237f * (pull > 0.0f ? pull : 0.0f);
            float ba = border_alpha(bx, by, BW, BH, inner, outer, power, border);
            if (ba > 0.0f) {
                float k = ba * (float)(border_colour >> 24) * (1.0f / 255.0f) * alpha;
                ob = (float)(border_colour & 0xffu) * k + ob * (1.0f - k);
                og = (float)((border_colour >> 8) & 0xffu) * k + og * (1.0f - k);
                orr = (float)((border_colour >> 16) & 0xffu) * k + orr * (1.0f - k);
            }
        }
    }
    dst[y * dstride + x] = pack(ob, og, orr);
}
