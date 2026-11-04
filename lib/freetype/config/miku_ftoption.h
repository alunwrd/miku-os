/*
 * FreeType's options for MikuOS: upstream's, less what lib/freetype does not
 * build. Named by FT_CONFIG_OPTIONS_H in build.rs, so the vendored tree is
 * left as it was released.
 *
 * - no file I/O: a face is always made from memory (FT_New_Memory_Face),
 *   so ftsystem.c compiles without stdio's streams;
 * - no compressed fonts (gzip, LZW) and no WOFF, which needs zlib;
 * - no OT-SVG glyphs, which need a renderer of their own;
 * - no Mac resource forks, which are files.
 *
 * Nothing here changes how a TrueType or CFF glyph is hinted or rendered:
 * that has to match Arch's FreeType, built with upstream's defaults and
 * HarfBuzz (`dev/aosh-negi.md`, "What changes in aosh's decisions").
 */
#ifndef MIKU_FTOPTION_H_
#define MIKU_FTOPTION_H_

#include <freetype/config/ftoption.h>

#define FT_CONFIG_OPTION_DISABLE_STREAM_SUPPORT
/* HarfBuzz, as in Arch's FreeType (there loaded at run time, here the
 * functions src/hb.rs defines), for the autohinter's GSUB coverage */
#define FT_CONFIG_OPTION_USE_HARFBUZZ
#undef FT_CONFIG_OPTION_USE_LZW
#undef FT_CONFIG_OPTION_USE_ZLIB
#undef FT_CONFIG_OPTION_SVG
#undef FT_CONFIG_OPTION_MAC_FONTS
#undef FT_CONFIG_OPTION_GUESSING_EMBEDDED_RFORK

#endif
