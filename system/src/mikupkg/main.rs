#![no_std]
#![no_main]
#![allow(unsafe_code)]

//! mikupkg: the package manager's command line
//!
//! `list`, `info`, `files`, `owner`, `why`, `verify`, and `search` are
//! read-only and need no privilege or network, since the image ships its
//! database. `add`, `remove`, `tx recover`, and `clean` stage what they have
//! checked and ask `/bin/mikupkg-apply` through `mikudo`.

extern crate alloc;

pub use miku;

#[path = "sys.rs"]
mod sys;
#[path = "change.rs"]
mod change;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use mikupkg::apply::miku::MikuRoot;
use mikupkg::apply::root::Root;
use mikupkg::conf::{self, Config};
use mikupkg::db::{Db, OwnerIndex};
use mikupkg::format::db::{Installed, Reason};
use mikupkg::format::meta as meta_codec;
use mikupkg::format::sha256;
use mikupkg::meta::PackageMeta;
use mikupkg::name::Name;

const USAGE: &str = "\
usage: mikupkg <verb> [arguments]

  list [-e|-d]        what is installed; asked for, or pulled in
  info <package>      everything it says about itself
  files <package>     what it owns
  owner <path>        which package owns a file
  why <package>       what keeps it on this machine
  verify [package]    every installed file against its recorded hash
  search <text>       packages whose name or description mentions it

  add <file.mpk>...   install packages given as files
  remove <package>... take packages away
  tx recover          put back a transaction that was interrupted
  clean               empty the download cache

  --dry-run           say what would happen and write nothing
  --allow-unsigned    install a package nobody signed, and mean it

The verbs that change the machine need a writable root (root=<drive>:<lba>
rootrw on the boot line) and ask mikudo to run /bin/mikupkg-apply.";

pub fn out(s: &str) {
    miku::println(s);
}

pub fn die(s: &str) -> ! {
    miku::eprintln(&format!("mikupkg: {s}"));
    miku::exit(1)
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let verb = args.get(1).copied().unwrap_or("");
    // Two global flags, accepted anywhere
    let mut dry_run = false;
    let mut allow_unsigned = false;
    let rest: Vec<&str> = args
        .iter()
        .skip(2)
        .copied()
        .filter(|a| match *a {
            "--dry-run" | "-n" => {
                dry_run = true;
                false
            }
            "--allow-unsigned" => {
                allow_unsigned = true;
                false
            }
            _ => true,
        })
        .collect();

    // Configuration first: every verb needs the database path, and an untrusted
    // configuration stops the run
    let cfg = configuration();
    let root = MikuRoot::new();

    match verb {
        "list" => list(&cfg, &root, &rest),
        "info" => info(&cfg, &root, &rest),
        "files" => files(&cfg, &root, &rest),
        "owner" => owner(&cfg, &root, &rest),
        "why" => why(&cfg, &root, &rest),
        "verify" => verify(&cfg, &root, &rest),
        "search" => search(&cfg, &root, &rest),
        "add" => change::add(&cfg, &root, &rest, dry_run, allow_unsigned),
        "remove" => change::remove(&cfg, &root, &rest, dry_run),
        "clean" => change::clean(&cfg),
        "tx" => match rest.first().copied() {
            Some("recover") => change::recover(&cfg, dry_run),
            Some("status") => tx_status(&cfg),
            _ => {
                miku::eprintln("mikupkg: tx recover, or tx status");
                miku::exit(2)
            }
        },
        "help" | "-h" | "--help" => {
            out(USAGE);
            miku::exit(0)
        }
        "" => {
            miku::eprintln(USAGE);
            miku::exit(2)
        }
        other => {
            miku::eprintln(&format!("mikupkg: no verb called {other:?}"));
            miku::eprintln(USAGE);
            miku::exit(2)
        }
    }
}

/// Reads `/etc/mikupkg`. Non-fatal notes print and continue; a fatal one, an
/// unknown setting deciding trust, stops
fn configuration() -> Config {
    let (cfg, notes) = conf::load(&sys::SysFiles, conf::ETC);
    let mut stop = false;
    for note in &notes {
        miku::eprintln(&format!("mikupkg: {note}"));
        stop = stop || note.fatal;
    }
    if stop {
        miku::eprintln("mikupkg: the configuration is not one this machine can act on");
        miku::exit(1);
    }
    cfg
}

/// Every package in the database
pub fn installed(cfg: &Config, root: &MikuRoot) -> Vec<(Name, Installed)> {
    let mut names: Vec<Name> = Vec::new();
    for (file, kind) in sys::read_dir(&cfg.db) {
        if kind == sys::KIND_DIR {
            continue;
        }
        if let Some(stem) = file.strip_suffix(".mpkr") {
            match Name::parse(stem) {
                Ok(n) => names.push(n),
                // Reports a file in the database that is not a record
                Err(e) => miku::eprintln(&format!("mikupkg: {}/{file}: {e}", cfg.db)),
            }
        }
    }
    names.sort();

    // Database through the `Root`, read as the applier reads it
    let d = Db::open(root, &cfg.paths().db);
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match d.get(&name) {
            Ok(rec) => out.push((name, rec)),
            Err(e) => miku::eprintln(&format!("mikupkg: {name}: {e}")),
        }
    }
    out
}

fn meta_of(rec: &Installed) -> Option<PackageMeta> {
    meta_codec::decode(&rec.meta, |tag, at| {
        miku::eprintln(&format!(
            "mikupkg: byte {at}: metadata tag {tag:#06x} is not one this build knows; ignoring"
        ))
    })
    .ok()
}

fn find<'a>(
    set: &'a [(Name, Installed)],
    want: &str,
) -> (&'a Name, &'a Installed) {
    let name = match Name::parse(want) {
        Ok(n) => n,
        Err(e) => die(&format!("{want:?}: {e}")),
    };
    match set.iter().find(|(n, _)| *n == name) {
        Some((n, rec)) => (n, rec),
        None => die(&format!("{name} is not installed")),
    }
}

// Verbs

fn list(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let mut only_explicit = false;
    let mut only_deps = false;
    for a in args {
        match *a {
            "-e" | "--explicit" => only_explicit = true,
            "-d" | "--deps" => only_deps = true,
            other => die(&format!("list: no option {other:?}")),
        }
    }

    let set = installed(cfg, root);
    if set.is_empty() {
        out("nothing is installed, and this machine has no database to say otherwise");
        miku::exit(0);
    }

    for (name, rec) in &set {
        let explicit = rec.reason == Reason::Explicit;
        if (only_explicit && !explicit) || (only_deps && explicit) {
            continue;
        }
        let version = meta_of(rec)
            .map(|m| m.version.to_string())
            .unwrap_or_else(|| "?".to_string());
        let why = if explicit { "" } else { "  (a dependency)" };
        out(&format!("{name} {version}{why}"));
    }
    miku::exit(0)
}

fn info(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let want = args.first().unwrap_or_else(|| die("info: which package?"));
    let set = installed(cfg, root);
    let (name, rec) = find(&set, want);
    let meta = meta_of(rec).unwrap_or_else(|| die(&format!("{name}: its record will not read")));

    let say = |k: &str, v: &str| {
        if !v.is_empty() {
            out(&format!("{:<14}{v}", format!("{k}:")));
        }
    };
    say("name", meta.name.as_str());
    say("version", &meta.version.to_string());
    say("arch", meta.arch.as_str());
    say("description", &meta.desc);
    say("url", &meta.url);
    say("license", &meta.license);
    say(
        "installed",
        if rec.reason == Reason::Explicit { "because you asked" } else { "as a dependency" },
    );
    say("files", &format!("{}", rec.files.entries.len()));
    say("size", &format!("{} bytes", meta.installed_size));

    fn joined<T: core::fmt::Display>(what: &[T]) -> String {
        what.iter().map(|d| format!("{d}")).collect::<Vec<_>>().join(", ")
    }
    say("depends", &joined(&meta.depends));
    say("provides", &joined(&meta.provides));
    say("conflicts", &joined(&meta.conflicts));
    say("replaces", &joined(&meta.replaces));
    say("triggers", &joined(&meta.triggers));
    // Suggestions are printed and never acted on
    for o in &meta.optdepends {
        out(&format!("{:<14}{o}", "suggests:"));
    }
    miku::exit(0)
}

fn files(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let want = args.first().unwrap_or_else(|| die("files: which package?"));
    let set = installed(cfg, root);
    let (_, rec) = find(&set, want);
    for e in &rec.files.entries {
        out(&format!("/{}", e.path));
    }
    miku::exit(0)
}

fn owner(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let path = args.first().unwrap_or_else(|| die("owner: which file?"));
    // Package paths are root-relative; users type absolute ones
    let wanted = path.trim_start_matches('/');

    let set = installed(cfg, root);
    let index = OwnerIndex::build(&set);
    match mikupkg::plan::conflict::Owners::owner_of(&index, wanted) {
        Some(name) => {
            out(&format!("/{wanted} belongs to {name}"));
            miku::exit(0)
        }
        None => {
            out(&format!("/{wanted} belongs to no package"));
            miku::exit(1)
        }
    }
}

fn why(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let want = args.first().unwrap_or_else(|| die("why: which package?"));
    let set = installed(cfg, root);
    let (name, rec) = find(&set, want);

    if rec.reason == Reason::Explicit {
        out(&format!("{name} is here because you asked for it"));
    }

    let mut needed_by = Vec::new();
    for (other, other_rec) in &set {
        if other == name {
            continue;
        }
        if let Some(meta) = meta_of(other_rec) {
            for d in &meta.depends {
                if d.name == *name {
                    needed_by.push(format!("{other} needs {d}"));
                }
            }
        }
    }

    if needed_by.is_empty() {
        if rec.reason != Reason::Explicit {
            out(&format!(
                "{name} is here as a dependency and nothing needs it any more"
            ));
        }
    } else {
        for line in needed_by {
            out(&line);
        }
    }
    miku::exit(0)
}

/// Checks every installed file against its recorded hash
fn verify(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let set = installed(cfg, root);
    let chosen: Vec<&(Name, Installed)> = match args.first() {
        None => set.iter().collect(),
        Some(want) => {
            let (name, _) = find(&set, want);
            set.iter().filter(|(n, _)| n == name).collect()
        }
    };

    let mut checked = 0usize;
    let mut wrong = 0usize;
    for (name, rec) in chosen {
        for e in &rec.files.entries {
            use mikupkg::format::FileKind;
            let here = match e.kind {
                FileKind::Directory => continue,
                FileKind::Symlink => match root.read_link(&e.path) {
                    Ok(t) => t.into_bytes(),
                    Err(err) => {
                        miku::eprintln(&format!("{name}: /{}: {err}", e.path));
                        wrong += 1;
                        continue;
                    }
                },
                FileKind::File => match root.read(&e.path) {
                    Ok(b) => b,
                    Err(err) => {
                        miku::eprintln(&format!("{name}: /{}: {err}", e.path));
                        wrong += 1;
                        continue;
                    }
                },
            };
            checked += 1;
            if sha256(&here) != e.sha256 {
                // A changed editable default is "changed"; anything else is
                // damaged
                let what = if e.is_default_config() { "changed" } else { "does not match" };
                miku::eprintln(&format!("{name}: /{} {what}", e.path));
                wrong += 1;
            }
        }
    }

    out(&format!("{checked} file(s) checked, {wrong} not as recorded"));
    miku::exit(if wrong == 0 { 0 } else { 1 })
}

fn search(cfg: &Config, root: &MikuRoot, args: &[&str]) -> ! {
    let text = args.first().unwrap_or_else(|| die("search: for what?"));
    let needle = text.to_lowercase();

    let mut found = 0usize;
    for (name, rec) in installed(cfg, root) {
        let meta = match meta_of(&rec) {
            Some(m) => m,
            None => continue,
        };
        if name.as_str().contains(&needle) || meta.desc.to_lowercase().contains(&needle) {
            out(&format!("{name} {}  {}", meta.version, meta.desc));
            found += 1;
        }
    }
    if found == 0 {
        // Reports that nothing is installed here; no index exists yet
        out(&format!(
            "nothing installed mentions {text:?}. This build searches what is \
             installed; there is no repository index on this machine yet"
        ));
    }
    miku::exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    miku::eprintln(&format!("mikupkg: panic: {info}"));
    miku::exit(127)
}

/// Reports an unfinished transaction, which blocks other changes
fn tx_status(cfg: &Config) -> ! {
    use mikupkg::format::journal::{Journal, Record};

    let paths = cfg.paths();
    let root = MikuRoot::new();
    let raw = match root.read(&paths.journal) {
        Ok(b) => b,
        Err(_) => {
            out("no transaction has ever run on this machine");
            miku::exit(0)
        }
    };
    let journal = Journal::parse(&raw);
    if journal.records.is_empty() {
        out("the log is empty");
        miku::exit(0);
    }
    let last = journal.last_transaction();
    let serial = match last.first() {
        Some(Record::Begin { serial, .. }) => *serial,
        _ => 0,
    };
    if journal.committed() {
        out(&format!("the last transaction ({serial}) finished"));
        if journal.torn {
            out("...and the log after it is torn, which is what an interrupted write looks like");
        }
        miku::exit(0);
    }
    out(&format!(
        "transaction {serial} did not finish. `mikupkg tx recover` puts it back"
    ));
    miku::exit(1)
}
