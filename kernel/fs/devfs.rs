use crate::fs::vfs::types::*;

pub struct DevFs;

impl DevFs {
    pub const fn new() -> Self {
        Self
    }
}

/// Major 8 = raw block devices (Linux's sd major), minor = dev * 16 + part
pub const BLOCK_MAJOR: u8 = 8;

/// Per-terminal master minors start here, clear of `tty` (0), `console` (1) and
/// `ptmx` (2)
pub const PTY_MASTER_MINOR: u8 = 16;

/// Slave minors, clear of the masters while `MAX_TTY` is below 48
pub const PTY_SLAVE_MINOR: u8 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevType {
    Null,
    Zero,
    Random,
    /// `/dev/console`: the console, tty0, the panel this machine boots on. Not
    /// routed by the writer's controlling terminal, as `print!` and `/dev/tty`
    /// are
    Console,
    /// `/dev/tty`: the caller's controlling terminal. How a program with
    /// redirected stdout still talks to the person (`less`, `ssh`, password
    /// prompts).
    ///
    /// A process with no controlling terminal cannot open it: `ENXIO`, as on
    /// Linux
    Tty,
    /// `/dev/ptmx`: opening it makes a terminal and returns its master end. A
    /// descriptor never points at this node: `open` substitutes the
    /// per-terminal master node below, as `pipe()` creates a nameless node
    Ptmx,
    /// The master end of pty `slot`. One node per slot, made at boot, reached
    /// only through `/dev/ptmx`. Opening it by name is refused: a master is
    /// given, not taken from its holder
    PtyMaster { slot: u8 },
    /// The slave end of pty `slot`, `/dev/ptsN`: what a program running in the
    /// terminal reads and writes. Opened by name (`openpty`, `ptsname`), by a
    /// process the terminal's family rule admits (`io::tty::may_use`, as for
    /// `tty(ATTACH)`). A descriptor on it is a terminal descriptor like
    /// `/dev/tty`'s (`FdKind::Tty`), naming this terminal, not the caller's
    /// own. Flat `ptsN`, as `blkN`
    PtySlave { slot: u8 },
    /// Raw block device node /dev/blkN (part 0) or /dev/blkNpM (part 1-15),
    /// byte-addressable on top of the block layer
    Block { dev: u8, part: u8 },
    /// /dev/net0: whole ethernet frames, for a program acting as the network
    /// stack. One frame per read, one per write, no header touched. Fed only
    /// when the boot line says `net=raw`.
    ///
    /// Flat, as devfs has no subdirectories, following `blkN`; a second card
    /// would be `net1`
    NetRaw,
}

impl DevType {
    /// The mode this node ships with
    ///
    /// 0666 suits the nodes that hand out nothing: null, zero, the random pool.
    /// Others:
    ///
    ///   * `/dev/blkN` is the disk, byte-addressable and writable; world access
    ///     would bypass every file permission above it (`/etc/shadow` from the
    ///     raw device).
    ///   * `/dev/net0` is the card; world access would allow forged frames and
    ///     reading others' traffic.
    ///
    /// Both are root's, and the network one also needs `CAP_NET_RAW`
    /// (`MikuVFS::open`). Root, since there is no `disk` group
    pub const fn default_mode(self) -> u16 {
        match self {
            DevType::Null | DevType::Zero | DevType::Random => 0o666,
            // Everybody's own terminal. Reading it still needs the foreground
            // job's turn, which no mode bit expresses
            DevType::Tty => 0o666,
            // Anyone may make a terminal, as a terminal emulator does. The
            // master node itself cannot be opened by name
            DevType::Ptmx => 0o666,
            DevType::PtyMaster { .. } => 0o000,
            // Anyone may try; `open` asks the family rule
            DevType::PtySlave { .. } => 0o666,
            // Root's, as on Linux. It names the machine's terminal; a program
            // wanting its own has `/dev/tty`
            DevType::Console => 0o620,
            DevType::NetRaw => 0o600,
            DevType::Block { .. } => 0o600,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "null" => Some(Self::Null),
            "zero" => Some(Self::Zero),
            "random" | "urandom" => Some(Self::Random),
            "console" => Some(Self::Console),
            "tty" => Some(Self::Tty),
            "ptmx" => Some(Self::Ptmx),
            _ if name.starts_with("pts") => {
                let n: u8 = name[3..].parse().ok()?;
                if (n as usize) < crate::io::tty::MAX_TTY { Some(Self::PtySlave { slot: n }) } else { None }
            }
            _ if name.starts_with("ptm") => {
                let n: u8 = name[3..].parse().ok()?;
                if (n as usize) < crate::io::tty::MAX_TTY { Some(Self::PtyMaster { slot: n }) } else { None }
            }
            "net0" => Some(Self::NetRaw),
            _ => Self::parse_block_name(name),
        }
    }

    /// "blkN" or "blkNpM" with N in 0..=7, M in 1..=15
    fn parse_block_name(name: &str) -> Option<Self> {
        let rest = name.strip_prefix("blk")?;
        let (dev_s, part_s) = match rest.find('p') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let dev: u8 = dev_s.parse().ok()?;
        if dev as usize >= crate::fs::vfs::types::MAX_BLOCK_DEVICES {
            return None;
        }
        let part: u8 = match part_s {
            Some(s) => {
                let p = s.parse().ok()?;
                if p == 0 || p > 15 { return None; }
                p
            }
            None => 0,
        };
        Some(Self::Block { dev, part })
    }

    pub fn major(&self) -> u8 {
        match self {
            Self::Null | Self::Zero | Self::Random => 1,
            Self::Console | Self::Tty | Self::Ptmx | Self::PtyMaster { .. }
                | Self::PtySlave { .. } => 5,
            // Linux's misc major, where /dev/net/tun also lives
            Self::NetRaw => 10,
            Self::Block { .. } => BLOCK_MAJOR,
        }
    }

    pub fn minor(&self) -> u8 {
        match self {
            Self::Null => 3,
            Self::Zero => 5,
            Self::Random => 8,
            // Linux's numbers: 5:0 is /dev/tty, 5:1 is /dev/console
            Self::Tty => 0,
            Self::Console => 1,
            Self::Ptmx => 2,
            // The slot, offset clear of the three above
            Self::PtyMaster { slot } => PTY_MASTER_MINOR + slot,
            Self::PtySlave { slot } => PTY_SLAVE_MINOR + slot,
            Self::NetRaw => 200,
            Self::Block { dev, part } => dev * 16 + part,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Null => "null device (discards all)",
            Self::Zero => "zero device (reads zeros)",
            Self::Random => "pseudo-random generator",
            Self::Console => "system console",
            Self::Tty => "controlling terminal",
            Self::Ptmx => "pseudo-terminal multiplexer",
            Self::PtyMaster { .. } => "pseudo-terminal master",
            Self::PtySlave { .. } => "pseudo-terminal slave",
            Self::NetRaw => "raw ethernet frames",
            Self::Block { .. } => "raw block device",
        }
    }
}

pub fn dev_read(dev_type: DevType, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
    match dev_type {
        DevType::Null => Ok(0),
        DevType::Zero => {
            let len = buf.len();
            for b in buf.iter_mut() {
                *b = 0;
            }
            Ok(len)
        }
        // `kcore::random`, the machine's one source of unpredictable bytes:
        // RDRAND where the processor has it. The same source as syscall 78
        DevType::Random => {
            let len = buf.len();
            let mut chunks = buf.chunks_mut(8);
            for chunk in &mut chunks {
                let word = crate::kcore::random::random_u64().to_ne_bytes();
                chunk.copy_from_slice(&word[..chunk.len()]);
            }
            Ok(len)
        }
        // Neither reaches here: `syscall::adapters::io::classify` routes a
        // descriptor on either node to the terminal it names, since a terminal
        // read waits, and waiting here would hold VFS locks. Anything else
        // arriving here (a kernel-internal read, a `stat`-driven copy) gets end
        // of file: terminal input needs to know who is asking. A master read is
        // routed the same way
        DevType::Console | DevType::Tty | DevType::Ptmx
            | DevType::PtyMaster { .. } | DevType::PtySlave { .. } => Ok(0),
        // One frame, or nothing. Never a partial frame and never a wait: a
        // reader that wants to block asks `poll`, which also waits on its
        // clients
        DevType::NetRaw => Ok(crate::net::raw::take(buf)),
        DevType::Block { dev, part } => block_read(dev, part, buf, offset),
    }
}

/// Byte-granular read from a raw block node, clamped at the end of the
/// disk/partition (so reads return 0 at EOF like a regular file)
fn block_read(dev: u8, part: u8, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
    let Some((base, sectors)) = crate::io::block::node_range(dev, part) else {
        return Err(VfsError::NotFound);
    };
    let size = sectors * 512;
    if offset >= size {
        return Ok(0);
    }
    let want = (buf.len() as u64).min(size - offset) as usize;

    let mut tmp = [0u8; 4096];
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let sector = base + pos / 512;
        let in_off = (pos % 512) as usize;
        let chunk = (want - done).min(tmp.len() - in_off);
        let nsec = ((in_off + chunk + 511) / 512) as u32;
        crate::io::block::read(dev, sector, nsec, &mut tmp[..nsec as usize * 512])
            .map_err(|_| VfsError::IoError)?;
        buf[done..done + chunk].copy_from_slice(&tmp[in_off..in_off + chunk]);
        done += chunk;
    }
    Ok(want)
}

/// Byte-granular write to a raw block node; unaligned edges read-modify-
/// write their sectors. Writing past the end reports 'NoSpace'
fn block_write(dev: u8, part: u8, data: &[u8], offset: u64) -> VfsResult<usize> {
    let Some((base, sectors)) = crate::io::block::node_range(dev, part) else {
        return Err(VfsError::NotFound);
    };
    let size = sectors * 512;
    if offset >= size {
        return Err(VfsError::NoSpace);
    }
    let want = (data.len() as u64).min(size - offset) as usize;

    let mut tmp = [0u8; 4096];
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let sector = base + pos / 512;
        let in_off = (pos % 512) as usize;
        let chunk = (want - done).min(tmp.len() - in_off);
        let nsec = ((in_off + chunk + 511) / 512) as u32;

        if in_off == 0 && chunk % 512 == 0 {
            crate::io::block::write(dev, sector, nsec, &data[done..done + chunk])
                .map_err(|_| VfsError::IoError)?;
        } else {
            crate::io::block::read(dev, sector, nsec, &mut tmp[..nsec as usize * 512])
                .map_err(|_| VfsError::IoError)?;
            tmp[in_off..in_off + chunk].copy_from_slice(&data[done..done + chunk]);
            crate::io::block::write(dev, sector, nsec, &tmp[..nsec as usize * 512])
                .map_err(|_| VfsError::IoError)?;
        }
        done += chunk;
    }
    Ok(want)
}

pub fn dev_write(dev_type: DevType, buf: &[u8], offset: u64) -> VfsResult<usize> {
    match dev_type {
        DevType::Null | DevType::Zero | DevType::Random => Ok(buf.len()),
        DevType::NetRaw => {
            if crate::net::raw::send(buf) { Ok(buf.len()) } else { Err(VfsError::IoError) }
        }
        DevType::Block { dev, part } => block_write(dev, part, buf, offset),
        // As above: the syscall path does not arrive here, and what does is not
        // a terminal write. Counted as written: the device discards
        DevType::Console | DevType::Tty | DevType::Ptmx
            | DevType::PtyMaster { .. } | DevType::PtySlave { .. } => Ok(buf.len()),
    }
}

pub fn dev_type_from_node(major: u8, minor: u8) -> Option<DevType> {
    match (major, minor) {
        (1, 3) => Some(DevType::Null),
        (1, 5) => Some(DevType::Zero),
        (1, 8) => Some(DevType::Random),
        (5, 0) => Some(DevType::Tty),
        (5, 1) => Some(DevType::Console),
        (5, 2) => Some(DevType::Ptmx),
        (5, m)
            if m >= PTY_SLAVE_MINOR
                && ((m - PTY_SLAVE_MINOR) as usize) < crate::io::tty::MAX_TTY =>
        {
            Some(DevType::PtySlave { slot: m - PTY_SLAVE_MINOR })
        }
        (5, m)
            if m >= PTY_MASTER_MINOR
                && ((m - PTY_MASTER_MINOR) as usize) < crate::io::tty::MAX_TTY =>
        {
            Some(DevType::PtyMaster { slot: m - PTY_MASTER_MINOR })
        }
        (10, 200) => Some(DevType::NetRaw),
        (BLOCK_MAJOR, m) => Some(DevType::Block { dev: m / 16, part: m % 16 }),
        _ => None,
    }
}

pub const DEV_ENTRIES: &[(&str, DevType)] = &[
    ("null", DevType::Null),
    ("zero", DevType::Zero),
    ("random", DevType::Random),
    ("urandom", DevType::Random),
    ("console", DevType::Console),
    ("tty", DevType::Tty),
    ("ptmx", DevType::Ptmx),
    ("net0", DevType::NetRaw),
];
