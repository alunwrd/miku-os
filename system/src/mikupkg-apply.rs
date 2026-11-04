#![no_std]
#![no_main]
#![allow(unsafe_code)]

//! mikupkg-apply: the root half of the package manager
//!
//! `/bin/mikupkg`, running as the user, handles DNS, TLS, HTTP, container
//! framing, the repository index, configuration, and version solving. It stages
//! bytes and writes a plan.
//!
//! The plan is not trusted: the signature over each package's metadata and file
//! table is verified against a keyring read here, and every staged file is
//! hashed against that table.
//!
//! Parsed here:
//! * the plan blob: fixed-width lengths and blobs (`plan/encode.rs`)
//! * package metadata and file table, bounds-checked by `format/`
//! * the keyring: a directory of fixed-size records
//! * staged files by number, hashed first
//!
//! Paths are compiled in, with no configuration language.
//!
//! Usage: mikupkg-apply <serial> [--allow-unsigned] [--root <path>]

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mikupkg::apply::miku::MikuRoot;
use mikupkg::apply::root::Root;
use mikupkg::apply::{staged, Paths};
use mikupkg::meta::Trigger;
use mikupkg::plan::encode::StagedPlan;
use mikupkg::trust::keyring::{Key, Keyring, RECORD_LEN};

/// Compiled in
const KEYS: &str = "etc/mikupkg/keys";

fn out(s: &str) {
    miku::println(s);
}

fn die(s: &str) -> ! {
    miku::eprintln(&format!("mikupkg-apply: {s}"));
    miku::exit(1)
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut serial: Option<u64> = None;
    let mut allow_unsigned = false;
    let mut base = String::new();
    let mut i = 1usize;
    while i < args.len() {
        match args[i] {
            "--allow-unsigned" => allow_unsigned = true,
            "--root" => {
                i += 1;
                match args.get(i) {
                    Some(p) => base = String::from(*p),
                    None => die("--root wants a path after it"),
                }
            }
            other if other.starts_with('-') => die(&format!("no option {other:?}")),
            other => match parse_u64(other) {
                Some(n) => serial = Some(n),
                None => die(&format!("{other:?} is not a transaction number")),
            },
        }
        i += 1;
    }
    let serial = serial.unwrap_or_else(|| die("which transaction? (a number)"));

    let paths = Paths::default();
    let mut root = if base.is_empty() { MikuRoot::new() } else { MikuRoot::under(&base) };
    let reader = if base.is_empty() { MikuRoot::new() } else { MikuRoot::under(&base) };

    let plan_path = format!("{}/{serial}/plan", paths.stage);
    let raw = root
        .read(&plan_path)
        .unwrap_or_else(|e| die(&format!("{plan_path}: {e}")));
    let plan = StagedPlan::parse(&raw).unwrap_or_else(|e| die(&format!("{plan_path}: {e}")));
    if plan.serial != serial {
        die("the plan is for a different transaction than the one asked for");
    }

    let keyring = load_keyring(&root, &base);

    // Only installs need a key; a removal takes its authority from the local
    // database
    let installing = plan
        .items
        .iter()
        .any(|i| i.verb == mikupkg::plan::encode::Verb::Install);
    if installing && keyring.is_empty() && !allow_unsigned {
        die(
            "there are no keys in /etc/mikupkg/keys, so nothing can be shown to be \
             genuine. Add one, or say --allow-unsigned and mean it",
        );
    }

    let now = now_secs();
    let done = staged::run(
        &plan,
        &mut root,
        &reader,
        &keyring,
        &paths,
        &paths.stage,
        allow_unsigned,
        now,
    )
    .unwrap_or_else(|e| die(&format!("{e}")));

    for name in &done.installed {
        out(&format!("installed {name}"));
    }
    for name in &done.removed {
        out(&format!("removed {name}"));
    }

    // Triggers from this program's compiled-in table, so only shipped code runs
    // with privilege after an install
    for t in &done.triggers {
        match run_trigger(*t) {
            Ok(()) => out(&format!("ran {t}")),
            Err(rc) => miku::eprintln(&format!("mikupkg-apply: {t} failed ({rc})")),
        }
    }

    sweep(&mut root, &paths.stage, serial);
    out(&format!(
        "{} file(s) placed, transaction {serial}",
        done.placed
    ));
    miku::exit(0)
}

/// Loads every key. A non-key file is named and skipped
fn load_keyring(root: &MikuRoot, base: &str) -> Keyring {
    let dir = if base.is_empty() {
        format!("/{KEYS}")
    } else {
        format!("{base}/{KEYS}")
    };
    let mut ring = Keyring::new();
    for (name, kind) in read_dir(&dir) {
        if kind == 1 {
            continue;
        }
        let path = format!("{KEYS}/{name}");
        let raw = match root.read(&path) {
            Ok(b) => b,
            Err(e) => {
                miku::eprintln(&format!("mikupkg-apply: {path}: {e}"));
                continue;
            }
        };
        if raw.len() != RECORD_LEN {
            miku::eprintln(&format!(
                "mikupkg-apply: {path}: a key is {RECORD_LEN} bytes and this is {}",
                raw.len()
            ));
            continue;
        }
        match Key::parse(&raw) {
            Ok(k) => ring.add(k),
            Err(e) => miku::eprintln(&format!("mikupkg-apply: {path}: {e}")),
        }
    }
    ring
}

/// Runs a trigger from the compiled-in table
fn run_trigger(t: Trigger) -> Result<(), i64> {
    let text: &str = match t {
        Trigger::Ldconfig => "ldconfig",
        Trigger::Services => "sv scan",
    };
    let rc = unsafe { miku::miku_kcmd(text.as_ptr(), text.len()) };
    if rc < 0 {
        Err(rc)
    } else {
        Ok(())
    }
}

/// Removes staged copies of files already placed
fn sweep(root: &mut MikuRoot, stage: &str, serial: u64) {
    let base = format!("{stage}/{serial}");
    for (item, kind) in read_dir(&format!("/{base}")) {
        if kind != 1 {
            let _ = root.remove_file(&format!("{base}/{item}"));
            continue;
        }
        let inner = format!("{base}/{item}");
        for (file, _) in read_dir(&format!("/{inner}")) {
            let _ = root.remove_file(&format!("{inner}/{file}"));
        }
        let _ = root.remove_dir(&inner);
    }
    let _ = root.remove_dir(&base);
}

/// Directory names; `Root` has no listing
fn read_dir(path: &str) -> Vec<(String, u8)> {
    let mut c = Vec::with_capacity(path.len() + 1);
    c.extend_from_slice(path.as_bytes());
    c.push(0);

    const BATCH: usize = 64;
    let mut entries: [miku::MikuDirent; BATCH] = unsafe { core::mem::zeroed() };
    let n = unsafe { miku::miku_readdir(c.as_ptr(), entries.as_mut_ptr(), BATCH) };
    let mut out = Vec::new();
    if n <= 0 {
        return out;
    }
    for e in entries.iter().take(n as usize) {
        let len = (e.name_len as usize).min(e.name.len());
        if let Ok(name) = core::str::from_utf8(&e.name[..len]) {
            if name == "." || name == ".." {
                continue;
            }
            out.push((String::from(name), e.kind));
        }
    }
    out
}

fn parse_u64(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut acc: u64 = 0;
    for c in s.bytes() {
        acc = acc.checked_mul(10)?.checked_add((c - b'0') as u64)?;
    }
    Some(acc)
}

/// Zero when the time is unknown; `trust::policy` treats it as "cannot tell"
fn now_secs() -> u64 {
    miku::realtime().unwrap_or(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    miku::eprintln(&format!("mikupkg-apply: panic: {info}"));
    miku::exit(127)
}
