// Kerning as HarfBuzz scales it (ValueFormat.hh, hb-font.hh)
//
// HarfBuzz adds a GPOS pair adjustment at Qt's pixel size in 64ths, scaling two
// parts separately:
// - the pair record's value by a 16.16 multiplier, rounded (`em_mult`);
// - its variation delta, unrounded from the item variation store, by a float
//   multiplier, rounded (`em_multf`).
//
// rustybuzz rounds the delta to a font unit first, which can put a width a 64th
// off. So pair records are read again here for each glyph rustybuzz moved,
// applying `kern` and `dist` pair lookups in order, first covering subtable
// wins. The result is checked against rustybuzz's value in units; any other
// adjustment (marks, contextual lookups) keeps rustybuzz's, scaled whole

use rustybuzz::ttf_parser::{self, gpos, GlyphId, Tag};

use super::shape::Glyph;

/// 26.6 adjustment per glyph at `scale` as HarfBuzz would give it; None where
/// unknown
pub fn exact(data: &[u8], index: u32, instance: &[([u8; 4], f32)], text: &str, glyphs: &[Glyph], scale: i64, upm: i64) -> Vec<Option<i64>> {
    let mut out = vec![None; glyphs.len()];
    if !glyphs.iter().any(|g| g.advance != g.base) {
        return out;
    }
    let Ok(mut face) = ttf_parser::Face::parse(data, index) else { return out };
    for (tag, v) in instance {
        face.set_variation(Tag::from_bytes(tag), *v);
    }
    let Some(table) = face.tables().gpos else { return out };
    let gdef = face.tables().gdef;
    let coords = face.variation_coordinates();
    // HarfBuzz's script for the run: Latin with a Latin letter, otherwise
    // default
    let latin = text.chars().any(|c| c.is_alphabetic() && (c as u32) < 0x0250);
    let order: [&[u8; 4]; 2] = if latin { [b"latn", b"DFLT"] } else { [b"DFLT", b"latn"] };
    let Some(script) = order.iter().find_map(|t| table.scripts.find(Tag::from_bytes(t))) else { return out };
    let mut lookups: Vec<u16> = Vec::new();
    let systems = script.default_language.into_iter().chain(script.languages);
    if let Some(lang) = systems.into_iter().next() {
        for fi in lang.feature_indices {
            let Some(feature) = table.features.get(fi) else { continue };
            if feature.tag == Tag::from_bytes(b"kern") || feature.tag == Tag::from_bytes(b"dist") {
                lookups.extend(feature.lookup_indices);
            }
        }
    }
    lookups.sort_unstable();
    lookups.dedup();
    let mult = (scale << 16) / upm;
    let em = |v: i64| (v * mult + 32768) >> 16;
    let multf = scale as f32 / upm as f32;
    let emf = |v: f32| (v * multf).round() as i64;
    for i in 0..glyphs.len().saturating_sub(1) {
        let kern = glyphs[i].advance - glyphs[i].base;
        if kern == 0 {
            continue;
        }
        let (first, second) = (GlyphId(glyphs[i].id), GlyphId(glyphs[i + 1].id));
        let (mut units, mut scaled, mut ok) = (0i32, 0i64, true);
        for &li in &lookups {
            let Some(lookup) = table.lookups.get(li) else { continue };
            for st in 0..lookup.subtables.len() {
                let Some(gpos::PositioningSubtable::Pair(pair)) = lookup.subtables.get::<gpos::PositioningSubtable>(st) else { continue };
                let Some(at) = pair.coverage().get(first) else { continue };
                let records = match pair {
                    gpos::PairAdjustment::Format1 { sets, .. } => sets.get(at).and_then(|s| s.get(second)),
                    gpos::PairAdjustment::Format2 { classes, matrix, .. } => matrix.get((classes.0.get(first), classes.1.get(second))),
                };
                // Format 1 applies only to listed pairs; format 2 to every pair
                // whose first glyph it covers
                let Some((one, two)) = records else {
                    if matches!(pair, gpos::PairAdjustment::Format2 { .. }) {
                        break;
                    }
                    continue;
                };
                if two.x_advance != 0 || two.x_placement != 0 || one.x_placement != 0 || two.x_advance_device.is_some() {
                    ok = false;
                }
                let delta = match one.x_advance_device {
                    Some(gpos::Device::Variation(d)) => gdef.and_then(|g| g.glyph_variation_delta(d.outer_index, d.inner_index, coords)).unwrap_or(0.0),
                    Some(gpos::Device::Hinting(_)) => {
                        ok = false;
                        0.0
                    }
                    None => 0.0,
                };
                units += one.x_advance as i32 + delta.round() as i32;
                scaled += em(one.x_advance as i64) + emf(delta);
                break;
            }
        }
        if ok && units == kern {
            out[i] = Some(scaled);
        }
    }
    out
}
