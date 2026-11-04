//! The HarfBuzz functions FreeType's autohinter asks for, answered by rustybuzz
//!
//! Arch's FreeType loads HarfBuzz, and its autohinter uses it to find what
//! glyphs a script reaches through GSUB (Caveat's contextual alternates, a
//! font's small caps) so they are hinted as that script's (`afshaper.c`,
//! `afadjust.c`). Without it those glyphs get no style and no hinting, and are
//! drawn a little off Qt's (`dev/aosh-negi.md`).
//!
//! FreeType built with `FT_CONFIG_OPTION_USE_HARFBUZZ` calls the functions
//! `autofit/ft-hb-decls.h` lists by their HarfBuzz names. These are them, with
//! HarfBuzz's meaning (hb-ot-layout.cc, hb-ot-layout-gsubgpos.hh,
//! hb-ot-tag.cc), over rustybuzz and the ttf-parser under it. Types are
//! `ft-hb-types.h`'s. Only what FreeType calls is here.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::cell::Cell;
use std::collections::BTreeSet;
use std::ffi::{c_char, c_int, c_uint, c_void};

use rustybuzz::ttf_parser::{self, gpos, gsub, opentype_layout as ggg, GlyphId, Tag};

type hb_bool_t = c_int;
type hb_tag_t = u32;
type hb_destroy_func_t = Option<unsafe extern "C" fn(*mut c_void)>;
type hb_reference_table_func_t = Option<unsafe extern "C" fn(*mut hb_face_t, hb_tag_t, *mut c_void) -> *mut hb_blob_t>;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct hb_glyph_info_t {
    codepoint: u32,
    mask: u32,
    cluster: u32,
    var1: u32,
    var2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct hb_glyph_position_t {
    x_advance: i32,
    y_advance: i32,
    x_offset: i32,
    y_offset: i32,
    var: u32,
}

#[repr(C)]
pub struct hb_feature_t {
    tag: hb_tag_t,
    value: u32,
    start: c_uint,
    end: c_uint,
}

const SET_VALUE_INVALID: u32 = u32::MAX;
const TAG_GSUB: hb_tag_t = u32::from_be_bytes(*b"GSUB");
const TAG_DFLT: hb_tag_t = u32::from_be_bytes(*b"DFLT");
const MAX_NESTING: u32 = 64;

/// Bytes, and who to tell when they are no longer needed
pub struct hb_blob_t {
    data: *const u8,
    len: usize,
    user: *mut c_void,
    destroy: hb_destroy_func_t,
    refs: Cell<u32>,
}

pub struct hb_face_t {
    blob: *mut hb_blob_t,
    index: u32,
    upem: u32,
    user: *mut c_void,
    destroy: hb_destroy_func_t,
    refs: Cell<u32>,
}

pub struct hb_font_t {
    face: *mut hb_face_t,
    x_scale: i32,
    y_scale: i32,
}

pub struct hb_set_t(BTreeSet<u32>);

pub struct hb_buffer_t {
    text: String,
    offset: u32,
    infos: Vec<hb_glyph_info_t>,
    positions: Vec<hb_glyph_position_t>,
}

unsafe fn blob_bytes<'a>(b: *mut hb_blob_t) -> &'a [u8] {
    if b.is_null() || (*b).data.is_null() {
        return &[];
    }
    std::slice::from_raw_parts((*b).data, (*b).len)
}

/// The face's font, as ttf-parser reads it: its own face of a collection,
/// at the named instance its index names, as `hb_font_create` sets one
unsafe fn parsed<'a>(face: *mut hb_face_t) -> Option<ttf_parser::Face<'a>> {
    let data = blob_bytes((*face).blob);
    let mut f = ttf_parser::Face::parse(data, (*face).index & 0xFFFF).ok()?;
    for (tag, v) in named_instance(&f, (*face).index) {
        f.set_variation(tag, v);
    }
    Some(f)
}

/// Where each axis stands at named instance `index >> 16` (from 1) of the
/// face's `fvar`; nothing for index 0, the face's own coordinates
fn named_instance(f: &ttf_parser::Face<'_>, index: u32) -> Vec<(Tag, f32)> {
    let n = (index >> 16) as usize;
    let Some(fvar) = f.raw_face().table(Tag::from_bytes(b"fvar")).filter(|_| n > 0) else { return Vec::new() };
    let u16_at = |at: usize| fvar.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let (Some(axes_at), Some(axis_count), Some(axis_size), Some(count), Some(size)) =
        (u16_at(4), u16_at(8), u16_at(10), u16_at(12), u16_at(14)) else { return Vec::new() };
    if n > count {
        return Vec::new();
    }
    let rec = axes_at + axis_count * axis_size + (n - 1) * size;
    (0..axis_count).filter_map(|k| {
        let tag = fvar.get(axes_at + k * axis_size..axes_at + k * axis_size + 4)?;
        let v = fvar.get(rec + 4 + 4 * k..rec + 8 + 4 * k)?;
        Some((Tag::from_bytes(&[tag[0], tag[1], tag[2], tag[3]]), i32::from_be_bytes([v[0], v[1], v[2], v[3]]) as f32 / 65536.0))
    }).collect()
}

// blobs, faces, fonts

#[no_mangle]
pub unsafe extern "C" fn hb_blob_create(data: *const c_char, length: c_uint, _mode: c_int, user: *mut c_void, destroy: hb_destroy_func_t) -> *mut hb_blob_t {
    Box::into_raw(Box::new(hb_blob_t { data: data as *const u8, len: length as usize, user, destroy, refs: Cell::new(1) }))
}

#[no_mangle]
pub unsafe extern "C" fn hb_blob_destroy(blob: *mut hb_blob_t) {
    if blob.is_null() {
        return;
    }
    let r = (*blob).refs.get() - 1;
    (*blob).refs.set(r);
    if r == 0 {
        let b = Box::from_raw(blob);
        if let Some(d) = b.destroy {
            d(b.user);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hb_face_create(blob: *mut hb_blob_t, index: c_uint) -> *mut hb_face_t {
    if !blob.is_null() {
        (*blob).refs.set((*blob).refs.get() + 1);
    }
    let upem = ttf_parser::Face::parse(blob_bytes(blob), index & 0xFFFF).map(|f| f.units_per_em() as u32).unwrap_or(1000);
    Box::into_raw(Box::new(hb_face_t { blob, index, upem, user: std::ptr::null_mut(), destroy: None, refs: Cell::new(1) }))
}

/// A face whose tables are asked for one at a time. FreeType's answers the
/// empty tag with the whole font (`ft-hb-ft.c`), which is what is kept
#[no_mangle]
pub unsafe extern "C" fn hb_face_create_for_tables(table: hb_reference_table_func_t, user: *mut c_void, destroy: hb_destroy_func_t) -> *mut hb_face_t {
    let face = Box::into_raw(Box::new(hb_face_t { blob: std::ptr::null_mut(), index: 0, upem: 1000, user, destroy, refs: Cell::new(1) }));
    if let Some(f) = table {
        (*face).blob = f(face, 0, user);
    }
    (*face).upem = ttf_parser::Face::parse(blob_bytes((*face).blob), 0).map(|f| f.units_per_em() as u32).unwrap_or(1000);
    face
}

#[no_mangle]
pub unsafe extern "C" fn hb_face_destroy(face: *mut hb_face_t) {
    if face.is_null() {
        return;
    }
    let r = (*face).refs.get() - 1;
    (*face).refs.set(r);
    if r == 0 {
        let f = Box::from_raw(face);
        hb_blob_destroy(f.blob);
        if let Some(d) = f.destroy {
            d(f.user);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hb_face_set_index(face: *mut hb_face_t, index: c_uint) {
    (*face).index = index;
}

#[no_mangle]
pub unsafe extern "C" fn hb_face_set_upem(face: *mut hb_face_t, upem: c_uint) {
    (*face).upem = upem.max(1);
}

#[no_mangle]
pub unsafe extern "C" fn hb_font_create(face: *mut hb_face_t) -> *mut hb_font_t {
    (*face).refs.set((*face).refs.get() + 1);
    let upem = (*face).upem as i32;
    Box::into_raw(Box::new(hb_font_t { face, x_scale: upem, y_scale: upem }))
}

#[no_mangle]
pub unsafe extern "C" fn hb_font_destroy(font: *mut hb_font_t) {
    if font.is_null() {
        return;
    }
    let f = Box::from_raw(font);
    hb_face_destroy(f.face);
}

#[no_mangle]
pub unsafe extern "C" fn hb_font_get_face(font: *mut hb_font_t) -> *mut hb_face_t {
    (*font).face
}

#[no_mangle]
pub unsafe extern "C" fn hb_font_set_scale(font: *mut hb_font_t, x: c_int, y: c_int) {
    (*font).x_scale = x;
    (*font).y_scale = y;
}

#[no_mangle]
pub extern "C" fn hb_version_atleast(_major: c_uint, _minor: c_uint, _micro: c_uint) -> hb_bool_t {
    1
}

// sets

#[no_mangle]
pub extern "C" fn hb_set_create() -> *mut hb_set_t {
    Box::into_raw(Box::new(hb_set_t(BTreeSet::new())))
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_destroy(set: *mut hb_set_t) {
    if !set.is_null() {
        drop(Box::from_raw(set));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_add(set: *mut hb_set_t, v: u32) {
    (*set).0.insert(v);
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_del(set: *mut hb_set_t, v: u32) {
    (*set).0.remove(&v);
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_clear(set: *mut hb_set_t) {
    (*set).0.clear();
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_has(set: *const hb_set_t, v: u32) -> hb_bool_t {
    (*set).0.contains(&v) as hb_bool_t
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_is_empty(set: *const hb_set_t) -> hb_bool_t {
    (*set).0.is_empty() as hb_bool_t
}

/// The next value after `*v`, the first for `HB_SET_VALUE_INVALID`
#[no_mangle]
pub unsafe extern "C" fn hb_set_next(set: *const hb_set_t, v: *mut u32) -> hb_bool_t {
    let next = if *v == SET_VALUE_INVALID {
        (*set).0.iter().next().copied()
    } else {
        (*set).0.range(*v + 1..).next().copied()
    };
    match next {
        Some(n) => {
            *v = n;
            1
        }
        None => {
            *v = SET_VALUE_INVALID;
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn hb_set_subtract(set: *mut hb_set_t, other: *const hb_set_t) {
    if set as *const hb_set_t == other {
        (*set).0.clear();
        return;
    }
    for v in &(*other).0 {
        (*set).0.remove(v);
    }
}

// buffers and shaping

#[no_mangle]
pub extern "C" fn hb_buffer_create() -> *mut hb_buffer_t {
    Box::into_raw(Box::new(hb_buffer_t { text: String::new(), offset: 0, infos: Vec::new(), positions: Vec::new() }))
}

#[no_mangle]
pub unsafe extern "C" fn hb_buffer_destroy(buf: *mut hb_buffer_t) {
    if !buf.is_null() {
        drop(Box::from_raw(buf));
    }
}

#[no_mangle]
pub unsafe extern "C" fn hb_buffer_clear_contents(buf: *mut hb_buffer_t) {
    let b = &mut *buf;
    b.text.clear();
    b.offset = 0;
    b.infos.clear();
    b.positions.clear();
}

/// Characters `item_offset` on, `item_length` of them (all, if -1), of the
/// UTF-8 `text`; each glyph's cluster is its byte in `text`
#[no_mangle]
pub unsafe extern "C" fn hb_buffer_add_utf8(buf: *mut hb_buffer_t, text: *const c_char, text_length: c_int, item_offset: c_uint, item_length: c_int) {
    let len = if text_length < 0 { std::ffi::CStr::from_ptr(text).to_bytes().len() } else { text_length as usize };
    let all = std::slice::from_raw_parts(text as *const u8, len);
    let start = (item_offset as usize).min(len);
    let end = if item_length < 0 { len } else { (start + item_length as usize).min(len) };
    let b = &mut *buf;
    b.offset = start as u32;
    b.text.push_str(&String::from_utf8_lossy(&all[start..end]));
}

/// rustybuzz guesses when it shapes, which is the same guess
#[no_mangle]
pub extern "C" fn hb_buffer_guess_segment_properties(_buf: *mut hb_buffer_t) {}

#[no_mangle]
pub unsafe extern "C" fn hb_buffer_get_length(buf: *const hb_buffer_t) -> c_uint {
    let b = &*buf;
    if b.infos.is_empty() { b.text.chars().count() as c_uint } else { b.infos.len() as c_uint }
}

#[no_mangle]
pub unsafe extern "C" fn hb_buffer_get_glyph_infos(buf: *mut hb_buffer_t, length: *mut c_uint) -> *mut hb_glyph_info_t {
    let b = &mut *buf;
    if !length.is_null() {
        *length = b.infos.len() as c_uint;
    }
    b.infos.as_mut_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn hb_buffer_get_glyph_positions(buf: *mut hb_buffer_t, length: *mut c_uint) -> *mut hb_glyph_position_t {
    let b = &mut *buf;
    if !length.is_null() {
        *length = b.positions.len() as c_uint;
    }
    b.positions.as_mut_ptr()
}

#[no_mangle]
pub unsafe extern "C" fn hb_shape(font: *mut hb_font_t, buf: *mut hb_buffer_t, features: *const hb_feature_t, num_features: c_uint) {
    let b = &mut *buf;
    b.infos.clear();
    b.positions.clear();
    let face = (*font).face;
    let data = blob_bytes((*face).blob);
    let Some(mut rb) = rustybuzz::Face::from_slice(data, (*face).index & 0xFFFF) else { return };
    if let Ok(f) = ttf_parser::Face::parse(data, (*face).index & 0xFFFF) {
        for (tag, v) in named_instance(&f, (*face).index) {
            rb.set_variation(tag, v);
        }
    }
    let feats: Vec<rustybuzz::Feature> = (0..num_features as usize).map(|i| {
        let f = &*features.add(i);
        let end = if f.end == c_uint::MAX { usize::MAX } else { f.end as usize };
        rustybuzz::Feature::new(Tag(f.tag), f.value, f.start as usize..end)
    }).collect();
    let mut ub = rustybuzz::UnicodeBuffer::new();
    ub.push_str(&b.text);
    ub.guess_segment_properties();
    let out = rustybuzz::shape(&rb, &feats, ub);
    let upem = (*face).upem.max(1) as i64;
    let (xs, ys) = ((*font).x_scale as i64, (*font).y_scale as i64);
    // Font units to the font's scale, as `em_mult` rounds
    let sx = |v: i32| ((v as i64 * ((xs << 16) / upem) + 32768) >> 16) as i32;
    let sy = |v: i32| ((v as i64 * ((ys << 16) / upem) + 32768) >> 16) as i32;
    for (i, p) in out.glyph_infos().iter().zip(out.glyph_positions()) {
        b.infos.push(hb_glyph_info_t { codepoint: i.glyph_id, cluster: i.cluster + b.offset, ..Default::default() });
        b.positions.push(hb_glyph_position_t {
            x_advance: sx(p.x_advance),
            y_advance: sy(p.y_advance),
            x_offset: sx(p.x_offset),
            y_offset: sy(p.y_offset),
            var: 0,
        });
    }
}

// OpenType layout

/// `hb_ot_tags_from_script_and_language`, for the script alone: a script's
/// OpenType tags, the new Indic one first (hb-ot-tag.cc)
#[no_mangle]
pub unsafe extern "C" fn hb_ot_tags_from_script_and_language(script: u32, _language: *const c_void, script_count: *mut c_uint,
                                                             script_tags: *mut hb_tag_t, language_count: *mut c_uint, _language_tags: *mut hb_tag_t) {
    if !language_count.is_null() {
        *language_count = 0;
    }
    if script_count.is_null() || script_tags.is_null() {
        return;
    }
    let tags = script_tags_of(script);
    let n = tags.len().min(*script_count as usize);
    for (i, t) in tags.iter().take(n).enumerate() {
        *script_tags.add(i) = *t;
    }
    *script_count = n as c_uint;
}

fn script_tags_of(script: u32) -> Vec<hb_tag_t> {
    let tag = |s: &[u8; 4]| u32::from_be_bytes(*s);
    let new = match &script.to_be_bytes() {
        b"Beng" => Some(tag(b"bng2")),
        b"Deva" => Some(tag(b"dev2")),
        b"Gujr" => Some(tag(b"gjr2")),
        b"Guru" => Some(tag(b"gur2")),
        b"Knda" => Some(tag(b"knd2")),
        b"Mlym" => Some(tag(b"mlm2")),
        b"Orya" => Some(tag(b"ory2")),
        b"Taml" => Some(tag(b"tml2")),
        b"Telu" => Some(tag(b"tel2")),
        b"Mymr" => Some(tag(b"mym2")),
        _ => None,
    };
    let mut out = Vec::new();
    if let Some(n) = new {
        if n != tag(b"mym2") {
            out.push((n & !0xFF) | b'3' as u32);
        }
        out.push(n);
    }
    let old = match &script.to_be_bytes() {
        [0, 0, 0, 0] => TAG_DFLT,
        b"Hira" => tag(b"kana"),
        b"Laoo" => tag(b"lao "),
        b"Yiii" => tag(b"yi  "),
        b"Nkoo" => tag(b"nko "),
        b"Vaii" => tag(b"vai "),
        _ => script | 0x2000_0000,
    };
    if old != TAG_DFLT || out.is_empty() {
        out.push(old);
    }
    out
}

/// A tag list ending in `HB_TAG_NONE`, or None for "every one"
unsafe fn tag_list(p: *const hb_tag_t) -> Option<Vec<hb_tag_t>> {
    if p.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while *p.add(i) != 0 {
        out.push(*p.add(i));
        i += 1;
    }
    Some(out)
}

fn layout<'a>(face: &ttf_parser::Face<'a>, table: hb_tag_t) -> Option<ggg::LayoutTable<'a>> {
    if table == TAG_GSUB { face.tables().gsub } else { face.tables().gpos }
}

/// Every lookup the features of these scripts' language systems use (all of
/// them for a null list), and what the table's feature variations put in those
/// features' place (`hb_ot_layout_collect_lookups`)
#[no_mangle]
pub unsafe extern "C" fn hb_ot_layout_collect_lookups(face: *mut hb_face_t, table_tag: hb_tag_t, scripts: *const hb_tag_t,
                                                      languages: *const hb_tag_t, features: *const hb_tag_t, out: *mut hb_set_t) {
    let Some(f) = parsed(face) else { return };
    let Some(table) = layout(&f, table_tag) else { return };
    let (scripts, languages, features) = (tag_list(scripts), tag_list(languages), tag_list(features));
    let mut feature_indexes: BTreeSet<u16> = BTreeSet::new();
    let script_list: Vec<ggg::Script> = match &scripts {
        None => (0..table.scripts.len()).filter_map(|i| table.scripts.get(i)).collect(),
        Some(tags) => tags.iter().filter_map(|t| table.scripts.find(Tag(*t))).collect(),
    };
    let wanted = |tag: Tag| features.as_ref().is_none_or(|fs| fs.contains(&tag.0));
    for script in script_list {
        let mut systems: Vec<ggg::LanguageSystem> = Vec::new();
        match &languages {
            None => {
                systems.extend(script.default_language);
                systems.extend((0..script.languages.len()).filter_map(|i| script.languages.get(i)));
            }
            Some(tags) => {
                for t in tags {
                    if *t == u32::from_be_bytes(*b"dflt") {
                        systems.extend(script.default_language);
                    } else if let Some(l) = script.languages.find(Tag(*t)) {
                        systems.push(l);
                    }
                }
            }
        }
        for l in systems {
            let required = l.required_feature.filter(|&r| table.features.get(r).is_some_and(|ft| wanted(ft.tag)));
            feature_indexes.extend(required);
            for fi in l.feature_indices {
                if table.features.get(fi).is_some_and(|ft| wanted(ft.tag)) {
                    feature_indexes.insert(fi);
                }
            }
        }
    }
    for &fi in &feature_indexes {
        if let Some(ft) = table.features.get(fi) {
            for li in ft.lookup_indices {
                (*out).0.insert(li as u32);
            }
        }
    }
    for li in variation_lookups(&f, table_tag, &feature_indexes) {
        (*out).0.insert(li as u32);
    }
}

/// The lookups of the features the table's FeatureVariations substitute for
/// any of `features`, under every condition set
fn variation_lookups(f: &ttf_parser::Face<'_>, table_tag: hb_tag_t, features: &BTreeSet<u16>) -> Vec<u16> {
    let mut out = Vec::new();
    let Some(t) = f.raw_face().table(Tag(table_tag)) else { return out };
    let u16_at = |at: usize| t.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let u32_at = |at: usize| t.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if u16_at(2).unwrap_or(0) < 1 {
        return out;
    }
    let Some(fv) = u32_at(10).filter(|&o| o != 0) else { return out };
    let Some(records) = u32_at(fv + 4) else { return out };
    for r in 0..records {
        let Some(subst) = u32_at(fv + 8 + r * 8 + 4).filter(|&o| o != 0).map(|o| fv + o) else { continue };
        let Some(count) = u16_at(subst + 4) else { continue };
        for k in 0..count {
            let rec = subst + 6 + k * 6;
            let (Some(index), Some(alt)) = (u16_at(rec), u32_at(rec + 2)) else { continue };
            if !features.contains(&(index as u16)) {
                continue;
            }
            let feature = subst + alt;
            let Some(n) = u16_at(feature + 2) else { continue };
            out.extend((0..n).filter_map(|j| u16_at(feature + 4 + 2 * j).map(|v| v as u16)));
        }
    }
    out
}

/// Where `collect_glyphs` puts what it finds: HarfBuzz's four sets, any of
/// them not wanted
struct Collect<'s> {
    before: Option<&'s mut BTreeSet<u32>>,
    input: Option<&'s mut BTreeSet<u32>>,
    after: Option<&'s mut BTreeSet<u32>>,
    output: Option<&'s mut BTreeSet<u32>>,
    recursed: BTreeSet<u16>,
    nesting: u32,
}

fn add_coverage(set: &mut Option<&mut BTreeSet<u32>>, c: &ggg::Coverage<'_>) {
    let Some(s) = set.as_deref_mut() else { return };
    match c {
        ggg::Coverage::Format1 { glyphs } => s.extend(glyphs.into_iter().map(|g| g.0 as u32)),
        ggg::Coverage::Format2 { records } => {
            for r in *records {
                s.extend(r.start.0 as u32..=r.end.0 as u32);
            }
        }
    }
}

fn coverage_glyphs(c: &ggg::Coverage<'_>) -> Vec<u16> {
    match c {
        ggg::Coverage::Format1 { glyphs } => glyphs.into_iter().map(|g| g.0).collect(),
        ggg::Coverage::Format2 { records } => records.into_iter().flat_map(|r| r.start.0..=r.end.0).collect(),
    }
}

fn add(set: &mut Option<&mut BTreeSet<u32>>, gs: impl IntoIterator<Item = u16>) {
    if let Some(s) = set.as_deref_mut() {
        s.extend(gs.into_iter().map(|g| g as u32));
    }
}

/// The glyphs of a class (HarfBuzz's `collect_class`; class 0 is every
/// glyph the table does not name, which it cannot list)
fn class_glyphs(cd: &ggg::ClassDefinition<'_>, class: u16) -> Vec<u16> {
    match cd {
        ggg::ClassDefinition::Format1 { start, classes } => classes.into_iter().enumerate()
            .filter(|(_, c)| *c == class && class != 0).map(|(i, _)| start.0 + i as u16).collect(),
        ggg::ClassDefinition::Format2 { records } => records.into_iter().filter(|r| r.value == class && class != 0)
            .flat_map(|r| r.start.0..=r.end.0).collect(),
        ggg::ClassDefinition::Empty => Vec::new(),
    }
}

impl Collect<'_> {
    /// A nested lookup, GSUB's only: its output, nothing else, once
    /// (hb_collect_glyphs_context_t::recurse)
    fn recurse(&mut self, table: &ggg::LayoutTable<'_>, gsub: bool, index: u16) {
        if !gsub || self.output.is_none() || self.nesting == 0 || self.recursed.contains(&index) {
            return;
        }
        let output = self.output.take();
        let mut inner = Collect { before: None, input: None, after: None, output, recursed: std::mem::take(&mut self.recursed), nesting: self.nesting - 1 };
        inner.lookup(table, gsub, index);
        self.output = inner.output;
        self.recursed = inner.recursed;
        self.recursed.insert(index);
    }

    fn lookup(&mut self, table: &ggg::LayoutTable<'_>, gsub: bool, index: u16) {
        let Some(lookup) = table.lookups.get(index) else { return };
        for i in 0..lookup.subtables.len() {
            if gsub {
                if let Some(st) = lookup.subtables.get::<gsub::SubstitutionSubtable>(i) {
                    self.substitution(table, st);
                }
            } else if let Some(st) = lookup.subtables.get::<gpos::PositioningSubtable>(i) {
                self.positioning(table, st);
            }
        }
    }

    fn substitution(&mut self, table: &ggg::LayoutTable<'_>, st: gsub::SubstitutionSubtable<'_>) {
        use gsub::SubstitutionSubtable as S;
        match st {
            S::Single(gsub::SingleSubstitution::Format1 { coverage, delta }) => {
                add_coverage(&mut self.input, &coverage);
                add(&mut self.output, coverage_glyphs(&coverage).into_iter().map(|g| (g as i32 + delta as i32) as u16));
            }
            S::Single(gsub::SingleSubstitution::Format2 { coverage, substitutes }) => {
                add_coverage(&mut self.input, &coverage);
                add(&mut self.output, substitutes.into_iter().map(|g| g.0));
            }
            S::Multiple(m) => {
                add_coverage(&mut self.input, &m.coverage);
                for i in 0..m.sequences.len() {
                    if let Some(seq) = m.sequences.get(i) {
                        add(&mut self.output, seq.substitutes.into_iter().map(|g| g.0));
                    }
                }
            }
            S::Alternate(a) => {
                add_coverage(&mut self.input, &a.coverage);
                for i in 0..a.alternate_sets.len() {
                    if let Some(set) = a.alternate_sets.get(i) {
                        add(&mut self.output, set.alternates.into_iter().map(|g| g.0));
                    }
                }
            }
            S::Ligature(l) => {
                add_coverage(&mut self.input, &l.coverage);
                for i in 0..l.ligature_sets.len() {
                    let Some(set) = l.ligature_sets.get(i) else { continue };
                    for k in 0..set.len() {
                        let Some(lig) = set.get(k) else { continue };
                        add(&mut self.input, lig.components.into_iter().map(|g| g.0));
                        add(&mut self.output, [lig.glyph.0]);
                    }
                }
            }
            S::Context(c) => self.context(table, true, c),
            S::ChainContext(c) => self.chained(table, true, c),
            S::ReverseChainSingle(r) => {
                add_coverage(&mut self.input, &r.coverage);
                for i in 0..r.backtrack_coverages.len() {
                    if let Some(c) = r.backtrack_coverages.get(i) {
                        add_coverage(&mut self.before, &c);
                    }
                }
                for i in 0..r.lookahead_coverages.len() {
                    if let Some(c) = r.lookahead_coverages.get(i) {
                        add_coverage(&mut self.after, &c);
                    }
                }
                add(&mut self.output, r.substitutes.into_iter().map(|g| g.0));
            }
        }
    }

    fn positioning(&mut self, table: &ggg::LayoutTable<'_>, st: gpos::PositioningSubtable<'_>) {
        use gpos::PositioningSubtable as P;
        match st {
            P::Single(s) => add_coverage(&mut self.input, &s.coverage()),
            P::Pair(p) => {
                add_coverage(&mut self.input, &p.coverage());
                match p {
                    // The second glyphs of a listed pair: read from the
                    // table by `pair_seconds`, ttf-parser does not list them
                    gpos::PairAdjustment::Format1 { .. } => {}
                    gpos::PairAdjustment::Format2 { classes, .. } => {
                        if let ggg::ClassDefinition::Format1 { start, classes: cs } = classes.1 {
                            add(&mut self.input, (0..cs.len()).map(|i| start.0 + i));
                        } else if let ggg::ClassDefinition::Format2 { records } = classes.1 {
                            for r in records {
                                add(&mut self.input, r.start.0..=r.end.0);
                            }
                        }
                    }
                }
            }
            P::Cursive(c) => add_coverage(&mut self.input, &c.coverage),
            P::MarkToBase(m) => {
                add_coverage(&mut self.input, &m.mark_coverage);
                add_coverage(&mut self.input, &m.base_coverage);
            }
            P::MarkToLigature(m) => {
                add_coverage(&mut self.input, &m.mark_coverage);
                add_coverage(&mut self.input, &m.ligature_coverage);
            }
            P::MarkToMark(m) => {
                add_coverage(&mut self.input, &m.mark1_coverage);
                add_coverage(&mut self.input, &m.mark2_coverage);
            }
            P::Context(c) => self.context(table, false, c),
            P::ChainContext(c) => self.chained(table, false, c),
        }
    }

    fn context(&mut self, table: &ggg::LayoutTable<'_>, gsub: bool, c: ggg::ContextLookup<'_>) {
        use ggg::ContextLookup as C;
        match c {
            C::Format1 { coverage, sets } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..sets.len() {
                    let Some(set) = sets.get(i) else { continue };
                    for k in 0..set.len() {
                        let Some(rule) = set.get(k) else { continue };
                        add(&mut self.input, rule.input.into_iter());
                        for l in rule.lookups {
                            self.recurse(table, gsub, l.lookup_list_index);
                        }
                    }
                }
            }
            C::Format2 { coverage, classes, sets } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..sets.len() {
                    let Some(set) = sets.get(i) else { continue };
                    for k in 0..set.len() {
                        let Some(rule) = set.get(k) else { continue };
                        for class in rule.input {
                            add(&mut self.input, class_glyphs(&classes, class));
                        }
                        for l in rule.lookups {
                            self.recurse(table, gsub, l.lookup_list_index);
                        }
                    }
                }
            }
            C::Format3 { coverage, coverages, lookups } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..coverages.len() {
                    if let Some(c) = coverages.get(i) {
                        add_coverage(&mut self.input, &c);
                    }
                }
                for l in lookups {
                    self.recurse(table, gsub, l.lookup_list_index);
                }
            }
        }
    }

    fn chained(&mut self, table: &ggg::LayoutTable<'_>, gsub: bool, c: ggg::ChainedContextLookup<'_>) {
        use ggg::ChainedContextLookup as C;
        match c {
            C::Format1 { coverage, sets } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..sets.len() {
                    let Some(set) = sets.get(i) else { continue };
                    for k in 0..set.len() {
                        let Some(rule) = set.get(k) else { continue };
                        add(&mut self.before, rule.backtrack.into_iter());
                        add(&mut self.input, rule.input.into_iter());
                        add(&mut self.after, rule.lookahead.into_iter());
                        for l in rule.lookups {
                            self.recurse(table, gsub, l.lookup_list_index);
                        }
                    }
                }
            }
            C::Format2 { coverage, backtrack_classes, input_classes, lookahead_classes, sets } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..sets.len() {
                    let Some(set) = sets.get(i) else { continue };
                    for k in 0..set.len() {
                        let Some(rule) = set.get(k) else { continue };
                        for class in rule.backtrack {
                            add(&mut self.before, class_glyphs(&backtrack_classes, class));
                        }
                        for class in rule.input {
                            add(&mut self.input, class_glyphs(&input_classes, class));
                        }
                        for class in rule.lookahead {
                            add(&mut self.after, class_glyphs(&lookahead_classes, class));
                        }
                        for l in rule.lookups {
                            self.recurse(table, gsub, l.lookup_list_index);
                        }
                    }
                }
            }
            C::Format3 { coverage, backtrack_coverages, input_coverages, lookahead_coverages, lookups } => {
                add_coverage(&mut self.input, &coverage);
                for i in 0..backtrack_coverages.len() {
                    if let Some(c) = backtrack_coverages.get(i) {
                        add_coverage(&mut self.before, &c);
                    }
                }
                for i in 0..input_coverages.len() {
                    if let Some(c) = input_coverages.get(i) {
                        add_coverage(&mut self.input, &c);
                    }
                }
                for i in 0..lookahead_coverages.len() {
                    if let Some(c) = lookahead_coverages.get(i) {
                        add_coverage(&mut self.after, &c);
                    }
                }
                for l in lookups {
                    self.recurse(table, gsub, l.lookup_list_index);
                }
            }
        }
    }
}

unsafe fn set_of<'s>(p: *mut hb_set_t) -> Option<&'s mut BTreeSet<u32>> {
    if p.is_null() { None } else { Some(&mut (*p).0) }
}

#[no_mangle]
pub unsafe extern "C" fn hb_ot_layout_lookup_collect_glyphs(face: *mut hb_face_t, table_tag: hb_tag_t, lookup_index: c_uint,
                                                            before: *mut hb_set_t, input: *mut hb_set_t, after: *mut hb_set_t, output: *mut hb_set_t) {
    let Some(f) = parsed(face) else { return };
    let Some(table) = layout(&f, table_tag) else { return };
    let mut c = Collect {
        before: set_of(before), input: set_of(input), after: set_of(after), output: set_of(output),
        recursed: BTreeSet::new(), nesting: MAX_NESTING,
    };
    c.lookup(&table, table_tag == TAG_GSUB, lookup_index as u16);
    if table_tag != TAG_GSUB {
        if let Some(t) = f.raw_face().table(Tag(table_tag)) {
            add(&mut c.input, pair_seconds(t, lookup_index as usize));
        }
    }
}

/// The second glyph of every pair a GPOS lookup's format 1 pair subtables
/// list, from the table's bytes: lookup list, lookup, extension, PairSet
fn pair_seconds(t: &[u8], lookup: usize) -> Vec<u16> {
    let u16_at = |at: usize| t.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize);
    let u32_at = |at: usize| t.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let mut out = Vec::new();
    let Some(list) = u16_at(8) else { return out };
    let Some(l) = u16_at(list + 2 + 2 * lookup).map(|o| list + o) else { return out };
    let (Some(kind), Some(n)) = (u16_at(l), u16_at(l + 4)) else { return out };
    for i in 0..n {
        let Some(mut st) = u16_at(l + 6 + 2 * i).map(|o| l + o) else { continue };
        let mut kind = kind;
        if kind == 9 {
            let (Some(k), Some(o)) = (u16_at(st + 2), u32_at(st + 4)) else { continue };
            kind = k;
            st += o;
        }
        if kind != 2 || u16_at(st) != Some(1) {
            continue;
        }
        let (Some(vf1), Some(vf2), Some(sets)) = (u16_at(st + 4), u16_at(st + 6), u16_at(st + 8)) else { continue };
        let record = 2 + 2 * ((vf1 as u16).count_ones() as usize + (vf2 as u16).count_ones() as usize);
        for k in 0..sets {
            let Some(set) = u16_at(st + 10 + 2 * k).map(|o| st + o) else { continue };
            let Some(count) = u16_at(set) else { continue };
            out.extend((0..count).filter_map(|j| u16_at(set + 2 + j * record).map(|g| g as u16)));
        }
    }
    out
}

/// Whether the lookup would substitute these glyphs, taken alone when
/// `zero_context` says so (`hb_ot_layout_lookup_would_substitute`)
#[no_mangle]
pub unsafe extern "C" fn hb_ot_layout_lookup_would_substitute(face: *mut hb_face_t, lookup_index: c_uint, glyphs: *const u32,
                                                              len: c_uint, zero_context: hb_bool_t) -> hb_bool_t {
    let Some(f) = parsed(face) else { return 0 };
    let Some(table) = f.tables().gsub else { return 0 };
    let Some(lookup) = table.lookups.get(lookup_index as u16) else { return 0 };
    let gs: Vec<GlyphId> = (0..len as usize).map(|i| GlyphId(*glyphs.add(i) as u16)).collect();
    let Some(&first) = gs.first() else { return 0 };
    use gsub::SubstitutionSubtable as S;
    for i in 0..lookup.subtables.len() {
        let Some(st) = lookup.subtables.get::<S>(i) else { continue };
        let hit = match st {
            S::Single(s) => gs.len() == 1 && s.coverage().contains(first),
            S::Multiple(m) => gs.len() == 1 && m.coverage.contains(first),
            S::Alternate(a) => gs.len() == 1 && a.coverage.contains(first),
            S::Ligature(l) => l.coverage.get(first).and_then(|at| l.ligature_sets.get(at)).is_some_and(|set| {
                (0..set.len()).filter_map(|k| set.get(k)).any(|lig| {
                    lig.components.len() as usize + 1 == gs.len() && lig.components.into_iter().zip(&gs[1..]).all(|(a, b)| a == *b)
                })
            }),
            S::Context(c) => context_would(&c, &gs),
            S::ChainContext(c) => chained_would(&c, &gs, zero_context != 0),
            S::ReverseChainSingle(r) => gs.len() == 1 && r.coverage.contains(first)
                && (zero_context == 0 || (r.backtrack_coverages.is_empty() && r.lookahead_coverages.is_empty())),
        };
        if hit {
            return 1;
        }
    }
    0
}

fn context_would(c: &ggg::ContextLookup<'_>, gs: &[GlyphId]) -> bool {
    use ggg::ContextLookup as C;
    match c {
        C::Format1 { coverage, sets } => coverage.get(gs[0]).and_then(|at| sets.get(at)).is_some_and(|set| {
            (0..set.len()).filter_map(|k| set.get(k)).any(|r| {
                r.input.len() as usize + 1 == gs.len() && r.input.into_iter().zip(&gs[1..]).all(|(a, b)| a == b.0)
            })
        }),
        C::Format2 { coverage, classes, sets } => coverage.contains(gs[0]) && sets.get(classes.get(gs[0])).is_some_and(|set| {
            (0..set.len()).filter_map(|k| set.get(k)).any(|r| {
                r.input.len() as usize + 1 == gs.len() && r.input.into_iter().zip(&gs[1..]).all(|(a, b)| a == classes.get(*b))
            })
        }),
        C::Format3 { coverage, coverages, .. } => coverage.contains(gs[0]) && coverages.len() as usize == gs.len()
            && (0..coverages.len()).all(|i| coverages.get(i).is_some_and(|c| c.contains(gs[i as usize]))),
    }
}

fn chained_would(c: &ggg::ChainedContextLookup<'_>, gs: &[GlyphId], zero_context: bool) -> bool {
    use ggg::ChainedContextLookup as C;
    match c {
        C::Format1 { coverage, sets } => coverage.get(gs[0]).and_then(|at| sets.get(at)).is_some_and(|set| {
            (0..set.len()).filter_map(|k| set.get(k)).any(|r| {
                (!zero_context || (r.backtrack.is_empty() && r.lookahead.is_empty()))
                    && r.input.len() as usize + 1 == gs.len() && r.input.into_iter().zip(&gs[1..]).all(|(a, b)| a == b.0)
            })
        }),
        C::Format2 { coverage, input_classes, sets, .. } => coverage.contains(gs[0]) && sets.get(input_classes.get(gs[0])).is_some_and(|set| {
            (0..set.len()).filter_map(|k| set.get(k)).any(|r| {
                (!zero_context || (r.backtrack.is_empty() && r.lookahead.is_empty()))
                    && r.input.len() as usize + 1 == gs.len() && r.input.into_iter().zip(&gs[1..]).all(|(a, b)| a == input_classes.get(*b))
            })
        }),
        C::Format3 { coverage, backtrack_coverages, input_coverages, lookahead_coverages, .. } => {
            coverage.contains(gs[0]) && (!zero_context || (backtrack_coverages.is_empty() && lookahead_coverages.is_empty()))
                && input_coverages.len() as usize == gs.len()
                && (0..input_coverages.len()).all(|i| input_coverages.get(i).is_some_and(|c| c.contains(gs[i as usize])))
        }
    }
}
