// Which SOR, which tiles and which physical windows a head gets
//
// NVKMS hands these out in its check step, from masks of what is free, so two
// heads never own the same one (`dev/display-nvkms.md`, decision 3):
//
//   `AssignSor`                            `nvkms-modeset.c:2267`
//   `EvoInitHwHeadMultiTileConfigCA`       `nvkms-evo4.c:2817`
//   `UnassignExtraOrIncompatibleTiles`     `:1949`
//   `GetFreeTiles`                         `:1999`
//   `AssignNewPhywinsIfNeeded`             `:2054`
//   `AssignNewTilesToHeadIfNeeded`         `:2088`
//   `EvoAssignHwHeadMultiTileConfigDispOutputCA`  `:2266`
//
// Pure functions over masks: no lock, no RM, no register. The caller reads the
// armed state and the caps; the selftest drives it with either.
//
// Each head has two windows (`2h`, `2h + 1`, NVKMS's mapping when the
// window-to-head assignment is flexible, `ProbeHeadCountAndWindowAssignment`),
// and each window, a "layer", needs one physical window per tile.

use super::super::dispcaps::{Caps, TILE_TYPE_0, TILE_TYPE_1};

/// Windows per head
pub const LAYERS: usize = 2;

/// What one head composites into
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeadTiles {
    pub tiles: u32,
    /// Per layer, the physical windows it fetches through
    pub phywins: [u32; LAYERS],
}

/// The SORs a new assignment must not take: every one a lit output holds, less
/// this connector's own (`AssignSor`: two streams on one connector share its
/// primary SOR)
pub fn sor_exclude_mask(lit_sors: &[u32], own: Option<u32>) -> u8 {
    let taken = lit_sors.iter().fold(0u32, |m, &s| m | 1 << (s & 7));
    (taken & !own.map_or(0, |s| 1 << (s & 7))) as u8
}

/// `GetRequiredTileType`: scaling or YUV 4:2:0 needs the scaler tiles. Neither
/// is done here, so every head asks for the trimmed kind first
pub fn required_tile_type(scaling: bool) -> u8 {
    if scaling { TILE_TYPE_0 } else { TILE_TYPE_1 }
}

fn lowest_bits(mask: u32, n: u32) -> u32 {
    let mut out = 0u32;
    let mut m = mask;
    while out.count_ones() < n && m != 0 {
        let b = m & m.wrapping_neg();
        out |= b;
        m &= !b;
    }
    out
}

/// `UnassignExtraOrIncompatibleTiles`: keep what the head has, as long as the
/// tile is not a trimmed variant it cannot use, up to the number it needs; and
/// as many of each layer's physical windows as tiles were kept
fn reuse(caps: &Caps, current: HeadTiles, required: u32, ty: u8) -> HeadTiles {
    let mut out = HeadTiles::default();
    let mut kept = 0;
    for t in 0..32 {
        if current.tiles >> t & 1 == 0 {
            continue;
        }
        let tile_type = caps.tile_type.get(t as usize).copied().unwrap_or(TILE_TYPE_1);
        if tile_type > ty || kept >= required {
            continue;
        }
        out.tiles |= 1 << t;
        kept += 1;
    }
    for l in 0..LAYERS {
        out.phywins[l] = lowest_bits(current.phywins[l], kept);
    }
    out
}

/// `GetFreeTiles`: `n` free tiles of type `ty`, and failing that of the top
/// variant; `None` when there are not enough
fn free_tiles(caps: &Caps, n: u32, ty: u8, free: &mut u32) -> Option<u32> {
    let mut want = ty;
    let mut got = 0u32;
    loop {
        for t in 0..caps.num_tiles() {
            if got.count_ones() >= n {
                break;
            }
            if *free >> t & 1 == 0 || got >> t & 1 != 0 {
                continue;
            }
            if caps.tile_type[t as usize] != want {
                continue;
            }
            got |= 1 << t;
        }
        if got.count_ones() >= n || want == TILE_TYPE_0 {
            break;
        }
        want = TILE_TYPE_0;
    }
    if got.count_ones() < n {
        return None;
    }
    *free &= !got;
    Some(got)
}

/// Tiles and physical windows for `head`, needing `required` tiles of type
/// `ty`, given what it has and what every other lit head holds
/// (`EvoAssignHwHeadMultiTileConfigDispOutputCA` for one changed head).
///
/// Unlike NVKMS, which stops when a head's reused tiles are enough
/// (`AssignNewTilesToHeadIfNeeded`, "Return early"): the core channel's init
/// has cleared every window's physical assignment before the first modeset, so
/// a head can keep its tile with no physical windows, and they are topped up to
/// one per tile per layer either way
pub fn assign(
    caps: &Caps,
    others: &[HeadTiles],
    current: HeadTiles,
    required: u32,
    ty: u8,
) -> Option<HeadTiles> {
    if required == 0 {
        return None;
    }
    let all = |n: u32| if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
    let mut free_tiles_mask = all(caps.num_tiles());
    let mut free_phywins = all(caps.num_phywins());
    for o in others {
        free_tiles_mask &= !o.tiles;
        for l in 0..LAYERS {
            free_phywins &= !o.phywins[l];
        }
    }
    // What this head has and nobody else claims
    let mine = HeadTiles {
        tiles: current.tiles & free_tiles_mask,
        phywins: [current.phywins[0] & free_phywins, current.phywins[1] & free_phywins],
    };
    let mut out = reuse(caps, mine, required, ty);
    free_tiles_mask &= !out.tiles;
    for l in 0..LAYERS {
        free_phywins &= !out.phywins[l];
    }

    let short = required - out.tiles.count_ones();
    if short > 0 {
        out.tiles |= free_tiles(caps, short, ty, &mut free_tiles_mask)?;
    }
    // `AssignNewPhywinsIfNeeded`: lowest free first, one per tile, per layer
    let need = out.tiles.count_ones();
    for l in 0..LAYERS {
        let more = need.saturating_sub(out.phywins[l].count_ones());
        let add = lowest_bits(free_phywins, more);
        if add.count_ones() < more {
            return None;
        }
        out.phywins[l] |= add;
        free_phywins &= !add;
    }
    Some(out)
}
