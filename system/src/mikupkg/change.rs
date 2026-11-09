//! Verbs that change the machine
//!
//! Holds no authority. Reads and checks containers, works out the transaction,
//! unpacks into a world-writable staging directory, writes a plan, and asks
//! `mikudo` to run `/bin/mikupkg-apply`, which checks everything again against
//! the keyring and signatures.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mikupkg::apply::miku::MikuRoot;
use mikupkg::apply::root::Root;
use mikupkg::conf::Config;
use mikupkg::db::OwnerIndex;
use mikupkg::format::db::Reason;
use mikupkg::format::{meta as meta_codec, FileKind, Package};
use mikupkg::meta::dep::Dep;
use mikupkg::name::Name;
use mikupkg::plan::conflict;
use mikupkg::plan::encode::{Item, StagedPlan, Verb};
use mikupkg::plan::Plan;
use mikupkg::resolve::{self, Catalogue};

use super::{die, installed, out};

/// Checks that the root is writable, before opening a container or fetching. A
/// default boot has `/bin` and `/etc` on a read-only module
fn require_writable(cfg: &Config) {
    let probe = format!("{}/.mikupkg-probe", cfg.paths().stage);
    let mut c = Vec::with_capacity(probe.len() + 2);
    c.push(b'/');
    c.extend_from_slice(probe.as_bytes());
    c.push(0);

    let dir = format!("/{}", cfg.paths().stage);
    let mut d = Vec::with_capacity(dir.len() + 1);
    d.extend_from_slice(dir.as_bytes());
    d.push(0);
    unsafe { miku::miku_mkdir_p(d.as_ptr(), 0o1777) };

    let full = format!("/{probe}");
    let fd = unsafe { miku::miku_open(full.as_ptr(), full.len(), 0x0002 | 0x0008 | 0x0020, 0o600) };
    if fd >= 0 {
        unsafe { miku::miku_close(fd) };
        unsafe { miku::miku_unlink(c.as_ptr()) };
        return;
    }

    miku::eprintln(
        "mikupkg: nothing can be installed on this machine, because its filesystem \
         cannot be written to.",
    );
    miku::eprintln("");
    miku::eprintln(
        "  A boot from the ISO takes /bin, /etc and /pkg from a copy in memory, and \
         that copy is",
    );
    miku::eprintln(
        "  thrown away at reboot - so nothing installed on it would survive, and \
         nothing is allowed",
    );
    miku::eprintln("  to pretend otherwise.");
    miku::eprintln("");
    miku::eprintln("  To install onto a disk, add this to the GRUB line and boot again:");
    miku::eprintln("");
    miku::eprintln("      root=<drive>:<lba> rootrw");
    miku::eprintln("");
    miku::eprintln(
        "  `blkstat` lists the drives this machine can see, and `gpt <drive>` their \
         partitions.",
    );
    miku::eprintln(
        "  Nothing here mounts a disk on its own - which disk to write to is not a \
         decision a",
    );
    miku::eprintln("  package manager gets to make.");
    miku::exit(1);
}

/// Transaction number: seconds since the epoch
fn serial() -> u64 {
    match miku::realtime() {
        Some(t) if t > 0 => t,
        // No clock: uses uptime
        _ => miku::uptime() as u64 + 1,
    }
}

/// Installs packages from files
pub fn add(cfg: &Config, root: &MikuRoot, files: &[&str], dry_run: bool, allow_unsigned: bool) -> ! {
    if files.is_empty() {
        die("add: which file?");
    }
    if !dry_run {
        require_writable(cfg);
    }

    // Reads all first, so a failure on one stages nothing
    let mut bytes: Vec<Vec<u8>> = Vec::new();
    for f in files {
        match read_whole(f) {
            Some(b) => bytes.push(b),
            None => die(&format!("{f}: cannot read it")),
        }
    }

    let mut packages = Vec::new();
    for (i, raw) in bytes.iter().enumerate() {
        let pkg = Package::open(raw).unwrap_or_else(|e| die(&format!("{}: {e}", files[i])));
        pkg.verify_all()
            .unwrap_or_else(|e| die(&format!("{}: {e}", files[i])));
        packages.push(pkg);
    }

    // Local packages' dependencies solve against the machine like fetched ones
    let set = installed(cfg, root);
    let mut catalogue = Catalogue::new();
    for (name, rec) in &set {
        if let Ok(meta) = meta_codec::decode(&rec.meta, |_, _| {}) {
            catalogue.add_installed(*name, meta.version);
            catalogue.add_local(meta, "", rec.container_sha256, 0);
        }
    }
    let mut request: Vec<Dep> = Vec::new();
    for (i, pkg) in packages.iter().enumerate() {
        let meta = pkg
            .meta(|tag, at| {
                miku::eprintln(&format!(
                    "mikupkg: {}: byte {at}: tag {tag:#06x} is not one this build knows",
                    files[i]
                ))
            })
            .unwrap_or_else(|e| die(&format!("{}: {e}", files[i])));
        request.push(Dep::exactly(meta.name, meta.version));
        catalogue.add_local(meta, files[i], mikupkg::format::sha256(&bytes[i]), bytes[i].len() as u64);
    }

    let solution = match resolve::solve(&catalogue, &request) {
        Ok(s) => s,
        Err(f) => {
            miku::eprintln(&resolve::report(&f));
            miku::exit(1)
        }
    };

    // Only given files can be fetched; other solved packages are reported as
    // unavailable
    let mut to_install: Vec<usize> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for (name, version) in &solution.selected {
        if set.iter().any(|(n, _)| n == name) && !request.iter().any(|d| d.name == *name) {
            continue; // Already present and not replaced
        }
        match packages.iter().position(|p| {
            p.meta(|_, _| {}).map(|m| m.name == *name && m.version == *version).unwrap_or(false)
        }) {
            Some(i) => to_install.push(i),
            None => missing.push(format!("{name} {version}")),
        }
    }
    if !missing.is_empty() {
        die(&format!(
            "these are needed and were not given: {}. There is no repository on \
             this machine to fetch them from - hand them over as files too",
            missing.join(", ")
        ));
    }

    // Two packages claiming one path, or a path somebody else already owns
    let mut check = Plan::new(0);
    for i in &to_install {
        for e in packages[*i].table().entries.iter() {
            if e.kind != FileKind::Directory {
                check.ops.push(mikupkg::plan::Op::File {
                    path: e.path.clone(),
                    mode: e.mode,
                    uid: e.uid,
                    gid: e.gid,
                    key: mikupkg::plan::ContentKey { package: *i, file: 0 },
                    size: e.size as u64,
                    sha256: e.sha256,
                });
            }
        }
        if let Ok(m) = packages[*i].meta(|_, _| {}) {
            check.installs.push(m);
        }
    }
    let owners = OwnerIndex::build(&set);
    let clashes = conflict::check(&check, &owners, |p| root.stat(p).is_some());
    if !clashes.is_empty() {
        for c in &clashes {
            miku::eprintln(&format!("mikupkg: {c}"));
        }
        miku::exit(1);
    }

    for i in &to_install {
        let meta = packages[*i].meta(|_, _| {}).unwrap();
        out(&format!(
            "{} {} {}",
            if set.iter().any(|(n, _)| *n == meta.name) { "upgrade" } else { "install" },
            meta.name,
            meta.version
        ));
    }
    if dry_run {
        out("--dry-run: nothing was written");
        miku::exit(0);
    }

    // Stages files in numbered slots, so the root half reads no archive paths
    let serial = serial();
    let mut items = Vec::new();
    for (slot, i) in to_install.iter().enumerate() {
        let pkg = &packages[*i];
        for (file, entry) in pkg.table().entries.iter().enumerate() {
            if entry.kind == FileKind::Directory {
                continue;
            }
            let at = StagedPlan::staged_at(&cfg.paths().stage, serial, slot, file);
            let body = pkg
                .content(file)
                .unwrap_or_else(|e| die(&format!("{}: {e}", files[*i])));
            write_staged(&at, body);
        }
        let meta = pkg.meta(|_, _| {}).unwrap();
        items.push(Item {
            verb: Verb::Install,
            reason: Reason::Explicit,
            name: meta.name,
            meta: pkg.meta_bytes().to_vec(),
            table: pkg.table_bytes().to_vec(),
            signature: pkg.signature().map(|s| s.to_vec()).unwrap_or_default(),
            container_sha256: mikupkg::format::sha256(&bytes[*i]),
        });
    }

    hand_over(cfg, serial, items, allow_unsigned)
}

/// Take packages away.
pub fn remove(cfg: &Config, root: &MikuRoot, names: &[&str], dry_run: bool) -> ! {
    if names.is_empty() {
        die("remove: which package?");
    }
    if !dry_run {
        require_writable(cfg);
    }
    let set = installed(cfg, root);

    let mut going: Vec<Name> = Vec::new();
    for want in names {
        let name = Name::parse(want).unwrap_or_else(|e| die(&format!("{want}: {e}")));
        if !set.iter().any(|(n, _)| *n == name) {
            die(&format!("{name} is not installed"));
        }
        going.push(name);
    }

    // Refuses to remove something still needed
    let mut orphaned: Vec<String> = Vec::new();
    for (other, rec) in &set {
        if going.contains(other) {
            continue;
        }
        if let Ok(meta) = meta_codec::decode(&rec.meta, |_, _| {}) {
            for d in &meta.depends {
                if going.contains(&d.name) {
                    orphaned.push(format!("{other} needs {d}"));
                }
            }
        }
    }
    if !orphaned.is_empty() {
        for line in &orphaned {
            miku::eprintln(&format!("mikupkg: {line}"));
        }
        die("removing these would leave the machine needing what is gone");
    }

    for name in &going {
        out(&format!("remove {name}"));
    }
    if dry_run {
        out("--dry-run: nothing was written");
        miku::exit(0);
    }

    let serial = serial();
    let items = going
        .iter()
        .map(|name| Item {
            verb: Verb::Remove,
            reason: Reason::Explicit,
            name: *name,
            meta: Vec::new(),
            table: Vec::new(),
            signature: Vec::new(),
            container_sha256: [0u8; 32],
        })
        .collect();
    hand_over(cfg, serial, items, false)
}

/// Write the plan and ask `mikudo` to run the half that may act on it.
fn hand_over(cfg: &Config, serial: u64, items: Vec<Item>, allow_unsigned: bool) -> ! {
    let plan = StagedPlan { serial, time: serial, items };
    write_staged(&format!("{}/{serial}/plan", cfg.paths().stage), &plan.encode());

    let number = format!("{serial}");
    let mut argv: Vec<&str> = Vec::new();
    let broker = !already_allowed();
    if broker {
        // `mikudo` asks `mikudod`, which names the caller by `peercred`
        argv.push("/bin/mikudo");
    }
    argv.push("/bin/mikupkg-apply");
    argv.push(&number);
    if allow_unsigned {
        argv.push("--allow-unsigned");
    }

    match run(&argv) {
        Some(code) => miku::exit(code),
        None => die(if broker {
            "could not run /bin/mikudo"
        } else {
            "could not run /bin/mikupkg-apply"
        }),
    }
}

/// Whether this process already has the required capability. Root is not in
/// `wheel`, so `mikudo` would refuse it
fn already_allowed() -> bool {
    /// `kernel/process/cred.rs`
    const CAP_DAC_OVERRIDE: u64 = miku_abi::cred::CAP_DAC_OVERRIDE as u64;
    const CAP_CHOWN: u64 = miku_abi::cred::CAP_CHOWN as u64;

    let mut sets = [0u64; 3];
    let rc = unsafe { miku::miku_capget(sets.as_mut_ptr()) };
    if rc < 0 {
        return false;
    }
    // Effective set
    let effective = sets[1];
    let has = |bit: u64| effective & (1 << bit) != 0;
    has(CAP_CHOWN) && has(CAP_DAC_OVERRIDE)
}

/// Runs `argv[0]` and waits, with the caller's terminal and identity
fn run(argv: &[&str]) -> Option<i64> {
    let mut store: Vec<Vec<u8>> = Vec::with_capacity(argv.len());
    for a in argv {
        let mut c = Vec::with_capacity(a.len() + 1);
        c.extend_from_slice(a.as_bytes());
        c.push(0);
        store.push(c);
    }
    let pointers: Vec<*const u8> = store.iter().map(|c| c.as_ptr()).collect();

    let pid = unsafe { miku::miku_fork() };
    if pid < 0 {
        return None;
    }
    if pid == 0 {
        unsafe { miku::miku_exec(store[0].as_ptr(), pointers.as_ptr(), pointers.len()) };
        miku::eprintln(&format!("mikupkg: {} did not start", argv[0]));
        miku::exit(127);
    }
    let (_, status) = miku::proc::wait_job(pid as u64, false)?;
    Some(miku::proc::wait_status_code(status))
}

/// Files under staging are created by the caller and not trusted by the root
/// half
fn write_staged(path: &str, body: &[u8]) {
    let dir = match path.rfind('/') {
        Some(at) => &path[..at],
        None => "",
    };
    if !dir.is_empty() {
        let mut c = Vec::with_capacity(dir.len() + 2);
        c.push(b'/');
        c.extend_from_slice(dir.as_bytes());
        c.push(0);
        unsafe { miku::miku_mkdir_p(c.as_ptr(), 0o755) };
    }
    let full = format!("/{path}");
    let fd = unsafe { miku::miku_open(full.as_ptr(), full.len(), 0x0002 | 0x0008 | 0x0020, 0o600) };
    if fd < 0 {
        // `require_writable` passed, so something changed underneath
        die(&format!("{full}: cannot write there ({fd})"));
    }
    let mut at = 0usize;
    while at < body.len() {
        let n = unsafe { miku::miku_write_fd(fd, body[at..].as_ptr(), body.len() - at) };
        if n <= 0 {
            unsafe { miku::miku_close(fd) };
            die(&format!("{full}: the write stopped short"));
        }
        at += n as usize;
    }
    unsafe { miku::miku_fsync(fd) };
    unsafe { miku::miku_close(fd) };
}

fn read_whole(path: &str) -> Option<Vec<u8>> {
    let mut c = Vec::with_capacity(path.len() + 1);
    c.extend_from_slice(path.as_bytes());
    c.push(0);
    let mut size: usize = 0;
    let ptr = unsafe { miku::miku_read_file(c.as_ptr(), &mut size) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, size) }.to_vec())
}

/// Recovers an interrupted transaction
pub fn recover(cfg: &Config, dry_run: bool) -> ! {
    use mikupkg::apply::recover as rec;

    let paths = cfg.paths();
    let mut root = MikuRoot::new();
    if dry_run {
        out("--dry-run: recovery is not something to rehearse; run it or do not");
        miku::exit(0);
    }
    match rec::recover(&mut root, &paths) {
        Ok(what) => {
            out(&format!("{what}"));
            miku::exit(0)
        }
        Err(e) => die(&format!("{e}")),
    }
}

/// Empties the cache
pub fn clean(cfg: &Config) -> ! {
    let dir = format!("/{}", cfg.paths().cache);
    let mut gone = 0usize;
    let mut root = MikuRoot::new();
    for (name, kind) in super::sys::read_dir(&dir) {
        if kind == super::sys::KIND_DIR {
            continue;
        }
        let at = format!("{}/{name}", cfg.paths().cache);
        match root.remove_file(&at) {
            Ok(()) => gone += 1,
            Err(e) => miku::eprintln(&format!("mikupkg: /{at}: {e}")),
        }
    }
    out(&format!("{gone} file(s) removed from the cache"));
    miku::exit(0)
}
