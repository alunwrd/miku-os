/*
 * What aosh asks FreeType: glyphs in the words Qt uses, and paths
 *
 * Every call here is one `QFontEngineFT` makes on Arch (qtbase 6.11,
 * src/gui/text/freetype/qfontengine_ft.cpp), so a glyph comes out of FreeType
 * the way it comes out for Qt:
 *
 * - a face is made from memory, at a named instance's or a Text's
 *   `font.variableAxes` coordinates (`QFreetypeFace::getFace`), and sized in
 *   26.6 points at 72 dpi, which is pixels (`computeSize`, `FT_Set_Char_Size`);
 * - a glyph is loaded with an identity transform and the pen's fraction of a
 *   pixel as the transform's delta, `FT_LOAD_DEFAULT |
 *   FT_LOAD_IGNORE_GLOBAL_ADVANCE_WIDTH | FT_LOAD_NO_BITMAP`, and
 *   `FT_LOAD_TARGET_LIGHT` for `hintslight` or `FT_LOAD_NO_HINTING` for a
 *   size Qt draws as outlines (over 64 px): `loadFlags`, `loadGlyph`;
 * - a weight with no face of its own is `FT_GlyphSlot_Embolden`, after the
 *   load and before anything is read from the slot;
 * - the advance Qt lays out with is `linearHoriAdvance >> 10`, its hinted
 *   metrics are the slot's, and the coverage is `FT_RENDER_MODE_LIGHT`.
 *
 * What is read from the slot is copied out, because the slot is reused by the
 * next load; the bitmap is the slot's own, good until then.
 *
 * A Canvas's paths are filled by FreeType's smooth rasteriser, which hands
 * back each row's coverage as spans, written here into the caller's mask
 * (`aft_path_fill`); a line's outline is aosh's own, filled the same way.
 */
#include <string.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include FT_MULTIPLE_MASTERS_H
#include FT_SYNTHESIS_H
#include FT_OUTLINE_H

typedef struct {
    /* 26.6 pixels */
    long linear;
    long advance;
    long bearing_x;
    long width;
    /* the coverage, when it was rendered */
    int left;
    int top;
    unsigned w;
    unsigned h;
    int pitch;
    const unsigned char *buffer;
} aft_glyph;

enum { AFT_HINT = 1, AFT_EMBOLDEN = 2, AFT_RENDER = 4 };

int aft_library_new(FT_Library *out)
{
    return FT_Init_FreeType(out);
}

void aft_library_done(FT_Library lib)
{
    FT_Done_FreeType(lib);
}

int aft_face_new(FT_Library lib, const unsigned char *data, long len, long index, FT_Face *out)
{
    return FT_New_Memory_Face(lib, data, len, index, out);
}

void aft_face_done(FT_Face face)
{
    FT_Done_Face(face);
}

/* `n` coordinates, 16.16, in the face's axis order (none for a face that is
 * not variable), then the size, 26.6 */
int aft_face_setup(FT_Face face, const FT_Fixed *coords, unsigned n, long size)
{
    if (n) {
        int e = FT_Set_Var_Design_Coordinates(face, n, (FT_Fixed *)coords);
        if (e)
            return e;
    }
    return FT_Set_Char_Size(face, size, size, 0, 0);
}

/* Another size for the same face: Qt keeps one face for every size of a
 * font and sets the size each time it uses it (QFreetypeFace::lockFace) */
int aft_face_size(FT_Face face, long size)
{
    return FT_Set_Char_Size(face, size, size, 0, 0);
}

int aft_glyph_load(FT_Face face, unsigned gid, int flags, long subx, aft_glyph *g)
{
    FT_Matrix m = { 0x10000, 0, 0, 0x10000 };
    FT_Vector v = { subx, 0 };
    FT_Set_Transform(face, &m, &v);

    FT_Int32 lf = FT_LOAD_DEFAULT | FT_LOAD_IGNORE_GLOBAL_ADVANCE_WIDTH | FT_LOAD_NO_BITMAP
                  | ((flags & AFT_HINT) ? FT_LOAD_TARGET_LIGHT : FT_LOAD_NO_HINTING);
    int e = FT_Load_Glyph(face, gid, lf);
    /* Qt tries again with bitmaps allowed: a face that is nothing else */
    if (e) {
        lf &= ~FT_LOAD_NO_BITMAP;
        e = FT_Load_Glyph(face, gid, lf);
    }
    if (e)
        return e;

    FT_GlyphSlot s = face->glyph;
    if (flags & AFT_EMBOLDEN)
        FT_GlyphSlot_Embolden(s);

    g->linear = s->linearHoriAdvance >> 10;
    g->advance = s->advance.x;
    g->bearing_x = s->metrics.horiBearingX;
    g->width = s->metrics.width;
    g->left = g->top = 0;
    g->w = g->h = 0;
    g->pitch = 0;
    g->buffer = 0;

    if (flags & AFT_RENDER) {
        e = FT_Render_Glyph(s, FT_RENDER_MODE_LIGHT);
        if (e)
            return e;
        /* A byte of coverage a pixel is all aosh reads; a face's own
         * monochrome bitmap is not that */
        if (s->bitmap.pixel_mode != FT_PIXEL_MODE_GRAY && s->bitmap.rows && s->bitmap.width)
            return -1;
        g->left = s->bitmap_left;
        g->top = s->bitmap_top;
        g->w = s->bitmap.width;
        g->h = s->bitmap.rows;
        g->pitch = s->bitmap.pitch;
        g->buffer = s->bitmap.buffer;
    }
    return 0;
}

/* A glyph's outline at the face's size, as Qt reads one for a path
 * (QFontEngineFT::getUnscaledGlyph, which QRawFont::pathForGlyph and Qt
 * Quick's distance-field cache come to): the face set to its units per em,
 * the glyph loaded with the default hinting (`hinted`) and scaled after.
 * The outline's own points are written, not FreeType's walk of them, whose
 * implied on-curve points are halved in whole 64ths where Qt halves them
 * exactly: the number of contours, each contour's last point, then x, y
 * (26.6, y up) and the tag of each point. The number of longs it takes is
 * returned whether or not `cap` held them; -1 if the glyph would not load
 * or is no outline */
long aft_glyph_outline(FT_Face face, unsigned gid, int hinted, long *out, long cap)
{
    FT_Set_Transform(face, 0, 0);
    if (FT_Load_Glyph(face, gid, (hinted ? FT_LOAD_DEFAULT : FT_LOAD_NO_HINTING) | FT_LOAD_NO_BITMAP))
        return -1;
    FT_GlyphSlot s = face->glyph;
    if (s->format != FT_GLYPH_FORMAT_OUTLINE)
        return -1;
    const FT_Outline *o = &s->outline;
    long need = 1 + o->n_contours + 3L * o->n_points;
    if (need > cap)
        return need;
    long k = 0;
    out[k++] = o->n_contours;
    for (int c = 0; c < o->n_contours; c++)
        out[k++] = o->contours[c];
    for (int i = 0; i < o->n_points; i++) {
        out[k++] = o->points[i].x;
        out[k++] = o->points[i].y;
        out[k++] = FT_CURVE_TAG(o->tags[i]);
    }
    return need;
}

/* A mask `w` by `h`, a byte a pixel, rows going down */
typedef struct {
    unsigned char *cov;
    int w;
    int h;
} aft_mask;

static void aft_spans(int y, int count, const FT_Span *spans, void *user)
{
    aft_mask *m = user;
    if (y < 0 || y >= m->h)
        return;
    unsigned char *row = m->cov + (long)y * m->w;
    for (int i = 0; i < count; i++) {
        int x0 = spans[i].x, x1 = spans[i].x + spans[i].len;
        if (x0 < 0)
            x0 = 0;
        if (x1 > m->w)
            x1 = m->w;
        for (int x = x0; x < x1; x++)
            row[x] = spans[i].coverage;
    }
}

/* `o`'s coverage into the mask, antialiased. The outline's y goes down the
 * mask: the rasteriser hands spans back at the outline's own rows */
static int aft_render(FT_Library lib, FT_Outline *o, int w, int h, unsigned char *cov)
{
    aft_mask m = { cov, w, h };
    FT_Raster_Params p;
    memset(&p, 0, sizeof p);
    p.source = o;
    p.flags = FT_RASTER_FLAG_AA | FT_RASTER_FLAG_DIRECT | FT_RASTER_FLAG_CLIP;
    p.gray_spans = aft_spans;
    p.user = &m;
    p.clip_box.xMin = 0;
    p.clip_box.yMin = 0;
    p.clip_box.xMax = w;
    p.clip_box.yMax = h;
    return FT_Outline_Render(lib, o, &p);
}

/* A path (points in 26.6, each a line's end, FT_CURVE_TAG_ON, and where
 * each subpath ends) filled by the non-zero rule, or the even-odd one */
int aft_path_fill(FT_Library lib, FT_Vector *points, unsigned char *tags, int n_points, unsigned short *ends,
                  int n_ends, int even_odd, int w, int h, unsigned char *cov)
{
    FT_Outline o;
    o.n_points = (unsigned short)n_points;
    o.n_contours = (unsigned short)n_ends;
    o.points = points;
    o.tags = tags;
    o.contours = ends;
    o.flags = even_odd ? FT_OUTLINE_EVEN_ODD_FILL : FT_OUTLINE_NONE;
    return aft_render(lib, &o, w, h, cov);
}
