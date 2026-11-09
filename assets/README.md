# Fonts

Rasterised at build time by `lib/gfx/mayland/fontgen.rs` — nothing here is read
at run time, and none of it ships in the image except as glyph pixels.

| file | face | where it is used | licence |
|---|---|---|---|
| `JetBrainsMono-Regular.ttf` | JetBrains Mono Regular | the console, and every fixed-cell client | OFL 1.1 |
| `ui-regular.ttf` | Inter Regular (v20, from Google Fonts) | interface body and captions | OFL 1.1 |
| `ui-bold.ttf` | Inter Bold (v20, from Google Fonts) | interface titles, headings and buttons | OFL 1.1 |

Inter covers U+0400–U+045F, which is what the Russian interface needs; the
generator asks for ASCII, U+0410–U+044F and Ё/ё.

Either `ui-*.ttf` may be removed: `fontgen::generate_ui` falls back to the mono
face and everything still builds. Replacing them with another pair is the whole
of changing the interface font — the sizes live in `UI_FACES`.
