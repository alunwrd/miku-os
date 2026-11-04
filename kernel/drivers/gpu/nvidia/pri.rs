// Reading the PRI ring: telling an error code from register data
//
// A read the GPU's PRI hub cannot service does not fault and does not return
// all-ones. It returns an error code that looks like data. RM's own test is
// `GPU_READ_PRI_ERROR_CODE`: 0xBAD00000 with the class in bits 31:8.
//
// This driver checks three widths, which nest:
//
//     is_error          0xbad0_0000 / 0xfff0_0000   every PRI error class
//       is_falcon       0xBADF_0000 / 0xFFFF_0000   the falcon/FECS family
//         is_target_locked  0xBADF_4100 / 0xFFFF_FF00   one code in it
//
// Too wide and a gated engine reads as alive; too narrow and 0xbad0fb00 passes
// as a flush token and the poll spins out its timeout. So each width has a name
// and the caller says which it means.

/// Any PRI error code at all.
///
/// The widest test: both the falcon/FECS family (`0xbadf____`) and the
/// host-side one (`0xbad0____`). For "is this register data"
#[inline(always)]
pub fn is_error(v: u32) -> bool {
    v & 0xfff0_0000 == 0xbad0_0000
}

/// The falcon/FECS sentinel, `0xBADF_xxxx`.
///
/// When the host reads a register window of a gated or floor-swept engine, the
/// PRI hub returns this for every register in the window, with the source class
/// in the low 16 bits. A `hw != 0 && hw != 0xFFFFFFFF` check would report that
/// engine as alive
#[inline(always)]
pub fn is_falcon(v: u32) -> bool {
    v & 0xffff_0000 == 0xbadf_0000
}

/// The BAR0 decoupler, `0xBADF_41xx`.
///
/// While the GSP TARGET_MASK is still applied by the bootrom or ACR, CPU reads
/// of the whole NV_PGSP block return 0 or this, never a real register value
/// (OpenRM `_kfspIsGspTargetMaskReleased`). A poll has to check the 0 alongside
/// this
#[inline(always)]
pub fn is_target_locked(v: u32) -> bool {
    v & 0xffff_ff00 == 0xbadf_4100
}
