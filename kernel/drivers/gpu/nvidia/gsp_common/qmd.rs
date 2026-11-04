// The launch descriptor
//
// A copy is a stream of methods in the ring. For a compute dispatch the ring
// carries the address of a structure in memory and an instruction to schedule
// it, and the structure says where the program is, how many registers a thread
// wants, how large a block is, how many blocks there are, and where the
// constant bank sits.
//
// A QMD, 96 words long on this architecture, with fields as bit ranges across
// the whole array: a field can straddle a word boundary. Version 5.0, as
// Blackwell numbers it.
//
// The field positions are the hardware's, from `clcec0qmd.h`. What is written
// here: a bit-range writer, and the smallest set of fields making a dispatch
// legal.
//
// Two methods launch it, from `clcec0.h`: `SEND_PCAS_A` takes the QMD's address
// shifted right by eight, and `SEND_SIGNALING_PCAS2_B` with the
// INVALIDATE_COPY_SCHEDULE action tells the front end to invalidate its caches,
// copy the descriptor in, and run it.

/// Words in one descriptor
pub const QMD_WORDS: usize = 96;
pub const QMD_BYTES: usize = QMD_WORDS * 4;
/// The descriptor is addressed shifted right by eight, so it starts on a
/// 256-byte boundary
#[allow(dead_code)] // the class's method map; the launch path writes the QMD and nothing else
pub const QMD_ALIGN: u64 = 256;

/// `NVCEC0_SET_OBJECT`, and the two methods that launch
#[allow(dead_code)] // the class's method map; the launch path writes the QMD and nothing else
pub const M_SET_OBJECT: u32 = 0x0000;
#[allow(dead_code)] // the class's method map; the launch path writes the QMD and nothing else
pub const M_SEND_PCAS_A: u32 = 0x02b4;
#[allow(dead_code)] // the class's method map; the launch path writes the QMD and nothing else
pub const M_SEND_SIGNALING_PCAS2_B: u32 = 0x02c0;
/// `PCAS_ACTION_INVALIDATE_COPY_SCHEDULE`
#[allow(dead_code)] // the class's method map; the launch path writes the QMD and nothing else
pub const PCAS_INVALIDATE_COPY_SCHEDULE: u32 = 3;

/// What a dispatch needs to know: from the compiled program (registers, shared
/// memory, the size of constant bank 0) or the caller's decision about how much
/// work there is
pub struct Launch {
    /// Address of the program in the channel's address space
    pub program: u64,
    /// How long the program is, which the front end wants in order to prefetch
    /// it
    pub code_bytes: u32,
    /// Address of constant bank 0, which holds the block dimensions the program
    /// reads and, above `PARAM_BASE`, the arguments
    pub const0: u64,
    pub const0_size: u32,
    /// Threads per block
    pub block: (u32, u32, u32),
    /// Blocks
    pub grid: (u32, u32, u32),
    /// Registers a thread uses, as the compiler reported them
    pub regs: u32,
    /// Shared memory a block asks for
    pub shared: u32,
    /// The shader instruction set this chip speaks, folded from the
    /// `SM_VERSION` RM reports. The machine checks it against itself
    pub sass_version: u32,
}

pub struct Qmd {
    words: [u32; QMD_WORDS],
}

impl Qmd {
    /// Write `value` into bits `lo..=hi` of the descriptor.
    ///
    /// The header's ranges are absolute bit positions across all 96 words, and
    /// several fields cross a word boundary, hence the split write
    fn set(&mut self, lo: u32, hi: u32, value: u32) {
        let width = hi - lo + 1;
        let masked = if width >= 32 { value } else { value & ((1u32 << width) - 1) };
        let word = (lo / 32) as usize;
        let shift = lo % 32;
        if word >= QMD_WORDS {
            return;
        }
        self.words[word] |= masked << shift;
        // The part that did not fit continues in the next word
        if shift + width > 32 && word + 1 < QMD_WORDS {
            self.words[word + 1] |= masked >> (32 - shift);
        }
    }

    pub fn as_words(&self) -> &[u32; QMD_WORDS] {
        &self.words
    }

    /// Build a descriptor for one dispatch.
    ///
    /// The version, the type and the group id are constants for this
    /// architecture: type GRID_CTA is an ordinary compute grid, and the group
    /// id is what the front end expects outside a dependent chain
    pub fn new(l: &Launch) -> Self {
        let mut q = Qmd { words: [0; QMD_WORDS] };

        q.set(468, 471, 5); // QMD_MAJOR_VERSION
        q.set(464, 467, 0); // QMD_MINOR_VERSION
        q.set(151, 153, 2); // QMD_TYPE = GRID_CTA
        q.set(144, 149, 0x3f); // QMD_GROUP_ID
        q.set(448, 455, l.sass_version);

        // The four caches a launch drops, the call-depth limit and the sampler
        // bank. Zero here differs from every working descriptor
        q.set(456, 456, 1); // API_VISIBLE_CALL_LIMIT
        q.set(457, 457, 1); // SAMPLER_INDEX
        q.set(472, 472, 1); // INVALIDATE_TEXTURE_HEADER_CACHE
        q.set(473, 473, 1); // INVALIDATE_TEXTURE_SAMPLER_CACHE
        q.set(474, 474, 1); // INVALIDATE_TEXTURE_DATA_CACHE
        q.set(475, 475, 1); // INVALIDATE_SHADER_DATA_CACHE
        q.set(624, 625, 1); // CWD_MEMBAR_TYPE = L1_SYSMEMBAR

        // The program. Shifted by four in both halves: instructions are
        // sixteen-byte aligned and the descriptor stores the address that way
        let prog = l.program >> 4;
        q.set(1024, 1055, prog as u32);
        q.set(1056, 1076, (prog >> 32) as u32);

        // The same address again, shifted by eight, with a length in 256-byte
        // units: what the front end prefetches, a separate field from the one
        // the machine jumps to
        q.set(1077, 1085, (l.code_bytes >> 8).min(0x1ff));
        q.set(1888, 1919, (l.program >> 8) as u32);
        q.set(1920, 1936, (l.program >> 40) as u32);

        q.set(1088, 1103, l.block.0);
        q.set(1104, 1119, l.block.1);
        q.set(1120, 1127, l.block.2);

        q.set(1248, 1279, l.grid.0);
        q.set(1280, 1295, l.grid.1);
        q.set(1312, 1327, l.grid.2);

        q.set(1128, 1136, l.regs);
        q.set(1137, 1141, 1); // BARRIER_COUNT

        // Shared memory the program asks for, in units of 128 bytes
        q.set(1152, 1162, (l.shared + 127) / 128);

        // What the multiprocessor is configured with, in another unit: the
        // smallest of the three hardware shared-memory carves holding the
        // request, in 4 KiB units plus one. The maximum is the hardware's own
        // ceiling
        let carve = [32u32, 64, 100].iter().copied().find(|k| k * 1024 >= l.shared).unwrap_or(100);
        let cfg = carve * 1024 / 4096 + 1;
        q.set(1163, 1168, cfg); // MIN_SM_CONFIG_SHARED_MEM_SIZE
        q.set(1169, 1174, 0x1a); // MAX_SM_CONFIG_SHARED_MEM_SIZE
        q.set(1175, 1180, cfg); // TARGET_SM_CONFIG_SHARED_MEM_SIZE

        // No local memory: nothing here spills or takes an address of a local
        q.set(1184, 1199, 0);
        q.set(1200, 1215, 0);

        // Constant bank 0. The address is shifted by six, the size by four, and
        // the bank has to be marked valid or the program reads zeroes
        let cb = l.const0 >> 6;
        q.set(1344, 1375, cb as u32);
        q.set(1376, 1394, (cb >> 32) as u32);
        q.set(1395, 1407, (l.const0_size + 15) / 16);
        q.set(1856, 1856, 1); // CONSTANT_BUFFER_VALID_0
        q.set(1859, 1859, 1); // CONSTANT_BUFFER_INVALIDATE_0

        q
    }
}
