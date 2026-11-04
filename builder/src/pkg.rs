//! Builds the image by installing packages
//!
//! Creates `.mpk` containers from build output and installs them through
//! `mikupkg`'s applier over a `std::fs` backend, the same applier, transaction,
//! and journal a running machine uses.
//!
//! - One install path: a program in a package is in the image, otherwise it is
//!   not.
//! - Modes and owners come from the packages.
//! - `/pkg/db` is written as a side effect, so the first `mikupkg upgrade` has
//!   a base.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use mikupkg::apply::host::{HostRoot, Ownership};
use mikupkg::apply::{tx, Paths, Root};
use mikupkg::db;
use mikupkg::format::db::{Installed, Reason};
use mikupkg::format::writer::Builder;
use mikupkg::meta::path::USER_CONFIG_DIR;
use mikupkg::format::{FileKind, Package};
use mikupkg::meta::dep::Dep;
use mikupkg::meta::{Arch, PackageMeta};
use mikupkg::name::Name;
use mikupkg::plan::{ContentKey, Op, Plan, Source};
use mikupkg::version::Version;

/// Package groups. Grouped rather than per binary, since `/pkg/db` is read
/// whole by every query
pub const PACKAGES: &[(&str, &str, &[&str])] = &[
    (
        "miku-base",
        "the commands, and the shell to type them at",
        &[],
    ),
    (
        "miku-system",
        "accounts, privilege, and the broker that grants it",
        &["miku-base"],
    ),
    (
        "miku-desktop",
        "the window manager, the terminal, the bar, and what they draw",
        &["miku-base", "miku-system"],
    ),
    ("miku-net", "the network tools, and the certificates they trust", &["miku-base"]),
    ("miku-demos", "display clients and ABI smoke tests", &["miku-base"]),
    // Zed: 200 MB, built separately (`EXTERNAL_BINS`)
    ("zed", "the Zed editor", &["miku-desktop"]),
    ("miku-firmware", "vendor microcode for the GPU", &[]),
    // The owner's desktop from `personal/`, which git ignores, so a fresh clone
    // builds without it (`personal_into`)
    (
        "miku-personal",
        "the owner's own desktop: their pictures, fonts, notes and settings",
        &["miku-desktop"],
    ),
];

/// Config a bare `aosh` runs when `personal/` is in the image
/// (`aosh/src/shell.rs`, `default_config`)
const NEGI_DEFAULT: &str = "negi\n";

/// negi keybinds. The default launcher and previous-workspace chords are
/// unbound first, since a key goes to the first binding naming it
const NEGI_KEYS: &str = "\
# negi's keys, shipped by miku-personal with the desktop they open. The search
# and the overview are the shell's own (`IpcHandler { target: \"search\" }` in
# /etc/aosh/negi/modules/ii/overview/Overview.qml), said with `signal`. On the
# owner's Arch these are Super and Super+Tab: Super pressed and let go alone
# opens the search, as `bindr` on a modifier key is a tap of it alone, and
# goes on to whoever has the keyboard (`n`), so Super is still a modifier to
# them
bindrn = SUPER, Super_L, signal, search toggle
bindrn = SUPER, Super_R, signal, search toggle
unbind = $mod, D
bind = $mod, D, signal, search toggle
unbind = $mod, TAB
bind = $mod, TAB, signal, search workspacesToggle
";

/// A package item: a file, or an empty directory
struct Entry {
    dest: String,
    mode: u16,
    /// `None` for a directory
    body: Option<Vec<u8>>,
}

impl Entry {
    fn is_dir(&self) -> bool {
        self.body.is_none()
    }
}

/// File mode, as a function of the destination path
fn mode_for(dest: &str) -> u16 {
    match dest {
        // Root and the `shadow` group only; only mikudod opens it
        "etc/shadow" => 0o640,
        // mikudod refuses this file if it is world-writable
        "etc/mikudo.conf" => 0o640,
        p if p.starts_with("bin/") => 0o755,
        _ => 0o644,
    }
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Everything under `dir` that a package may carry. A `conf.d` directory ships
/// empty: nothing is installed there, since an upgrade would overwrite it
fn tree_into(dir: &Path, prefix: &str, out: &mut Vec<Entry>) {
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((d, at)) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        let mut items: Vec<_> = rd.flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let dest = format!("{at}/{name}");
            if p.is_dir() {
                if name == USER_CONFIG_DIR {
                    // Only the directory
                    println!("[i] {dest} ships empty - nothing is ever installed there");
                    out.push(Entry { dest, mode: 0o755, body: None });
                    continue;
                }
                stack.push((p, dest));
                continue;
            }
            let mode = mode_for(&dest);
            out.push(Entry { dest, mode, body: Some(read(&p)) });
        }
    }
}

/// Collect what each package holds, from what the build produced.
fn collect(
    root: &Path,
    programs: &[(&'static str, &'static str, PathBuf)],
    firmware: &[(String, PathBuf)],
) -> BTreeMap<&'static str, Vec<Entry>> {
    let mut out: BTreeMap<&'static str, Vec<Entry>> = BTreeMap::new();
    for (name, _, _) in PACKAGES {
        out.insert(name, Vec::new());
    }

    // The programs, each into the package its line in the table names
    for (bin, package, path) in programs {
        let dest = format!("bin/{bin}");
        out.get_mut(package)
            .unwrap_or_else(|| panic!("{bin} names package {package}, which is not in PACKAGES"))
            .push(Entry { mode: mode_for(&dest), dest, body: Some(read(path)) });
    }

    // /etc split by package. A file with no declared package panics
    let etc = root.join("etc");
    if etc.is_dir() {
        let mut all = Vec::new();
        tree_into(&etc, "etc", &mut all);
        for e in all {
            let package = match e.dest.as_str() {
                d if d.starts_with("etc/mayland/") => "miku-desktop",
                // The shell's own files, beside the program (`aosh`)
                d if d.starts_with("etc/aosh/") => "miku-desktop",
                // The package manager's defaults
                d if d.starts_with("etc/mikupkg/") => "miku-base",
                d if d.starts_with("etc/ssl/") => "miku-net",
                // System identification for programs reading it, without a
                // version
                "etc/os-release" => "miku-base",
                "etc/passwd" | "etc/group" | "etc/shadow" | "etc/mikudo.conf" => "miku-system",
                other => panic!(
                    "{other} is in the tree's etc/ and no package claims it - say which \
                     one in builder/src/pkg.rs, or the image would ship a file nobody owns"
                ),
            };
            out.get_mut(package).unwrap().push(e);
        }
    }

    // Kernel build script output: wallpapers, icons, login portrait, and
    // compiled shaders
    for (from, to) in [
        ("target/wallpapers", "etc/mayland/wallpapers"),
        ("target/icons", "etc/mayland/icons"),
        ("target/art", "etc/mayland/art"),
        ("target/gpu", "etc/gpu"),
    ] {
        let dir = root.join(from);
        if dir.is_dir() {
            tree_into(&dir, to, out.get_mut("miku-desktop").unwrap());
        }
    }

    // Fonts aosh rasterizes at run time, the same files libmay's build uses
    for (from, to) in [
        ("assets/ui-regular.ttf", "etc/aosh/fonts/ui-regular.ttf"),
        ("assets/ui-bold.ttf", "etc/aosh/fonts/ui-bold.ttf"),
        ("assets/JetBrainsMono-Regular.ttf", "etc/aosh/fonts/JetBrainsMono-Regular.ttf"),
    ] {
        let p = root.join(from);
        if p.is_file() {
            out.get_mut("miku-desktop").unwrap()
                .push(Entry { dest: to.to_string(), mode: mode_for(to), body: Some(read(&p)) });
        }
    }

    for (dest, path) in firmware {
        out.get_mut("miku-firmware")
            .unwrap()
            .push(Entry { dest: dest.clone(), mode: 0o644, body: Some(read(path)) });
    }

    personal_into(&root.join("personal"), &mut out);

    out
}

/// Owner's files from `personal/` into miku-personal (`personal/README.md`):
/// - `assets/` to `/etc/aosh/negi-personal`;
/// - `home/` to `/etc/skel` (mikudod's `make_home`);
/// - `hostname` to `/etc/hostname`;
/// - `/etc/aosh/default` and negi's keys;
/// - `shadow`: the owner's line replaces the locked one in miku-system's
///   `/etc/shadow`.
/// Without `personal/` none of this happens
fn personal_into(dir: &Path, out: &mut BTreeMap<&'static str, Vec<Entry>>) {
    if !dir.is_dir() {
        println!("[i] no personal/ - the image carries nobody's own desktop, and alunwrd stays locked");
        return;
    }
    let mine = out.get_mut("miku-personal").unwrap();
    tree_into(&dir.join("assets"), "etc/aosh/negi-personal", mine);
    tree_into(&dir.join("home"), "etc/skel", mine);
    let host = dir.join("hostname");
    if host.is_file() {
        let name = String::from_utf8_lossy(&read(&host)).trim().to_string();
        if name.is_empty() || name.contains(char::is_whitespace) || name.len() > 64 {
            panic!("personal/hostname has to be one word, the machine's name");
        }
        mine.push(Entry { dest: "etc/hostname".into(), mode: 0o644, body: Some(format!("{name}\n").into_bytes()) });
    }
    mine.push(Entry { dest: "etc/aosh/default".into(), mode: 0o644, body: Some(NEGI_DEFAULT.as_bytes().to_vec()) });
    mine.push(Entry {
        dest: "etc/mayland/default.d/70-negi.conf".into(),
        mode: 0o644,
        body: Some(NEGI_KEYS.as_bytes().to_vec()),
    });

    let line = dir.join("shadow");
    if !line.is_file() {
        println!("[i] no personal/shadow - alunwrd is in the image, locked");
        return;
    }
    let text = String::from_utf8_lossy(&read(&line)).into_owned();
    let own = owner_line(&text).unwrap_or_else(|why| panic!("personal/shadow: {why}"));
    let shadow = out.get_mut("miku-system").unwrap().iter_mut()
        .find(|e| e.dest == "etc/shadow")
        .expect("etc/shadow is the tree's, in miku-system");
    let body = String::from_utf8_lossy(shadow.body.as_ref().unwrap()).into_owned();
    shadow.body = Some(with_line(&body, &own).unwrap_or_else(|why| panic!("etc/shadow: {why}")).into_bytes());
    println!("[ok] alunwrd's own line from personal/shadow is in /etc/shadow");
}

/// The one line in `personal/shadow`: `alunwrd` with an Argon2id hash
fn owner_line(text: &str) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let [line] = lines.as_slice() else {
        return Err(format!("{} lines; it holds one, alunwrd's", lines.len()));
    };
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() != 9 || fields[0] != "alunwrd" {
        return Err("not an /etc/shadow line for alunwrd (`mkshadow alunwrd ...` makes one)".into());
    }
    if !fields[1].starts_with("$argon2id$") {
        return Err("its hash is not Argon2id".into());
    }
    Ok(line.to_string())
}

/// `shadow` with the matching account's line replaced by `line`
fn with_line(shadow: &str, line: &str) -> Result<String, String> {
    let name = line.split(':').next().unwrap_or("");
    let prefix = format!("{name}:");
    let mut found = false;
    let mut out = String::new();
    for l in shadow.lines() {
        if l.starts_with(&prefix) {
            found = true;
            out.push_str(line);
        } else {
            out.push_str(l);
        }
        out.push('\n');
    }
    if found { Ok(out) } else { Err(format!("no line for {name} to put the owner's in place of")) }
}

/// Build one container.
fn make_container(
    name: &str,
    desc: &str,
    depends: &[&str],
    version: &str,
    entries: &mut Vec<Entry>,
) -> Vec<u8> {
    let mut meta = PackageMeta::new(
        Name::parse(name).unwrap_or_else(|e| panic!("{name}: {e}")),
        Version::parse(version).unwrap_or_else(|e| panic!("{version}: {e}")),
        // One architecture per package; the strictest file decides
        Arch::X86_64,
    );
    meta.desc = desc.to_string();
    meta.license = "MIT".to_string();
    meta.url = "https://github.com/alunwrd/miku-os".to_string();
    for d in depends {
        meta.depends.push(Dep::parse(d).unwrap_or_else(|e| panic!("{d}: {e}")));
    }

    let mut b = Builder::new(meta);

    // Declares every directory a package's files need, so modes do not depend
    // on the umask
    let mut dirs: Vec<String> = Vec::new();
    for e in entries.iter() {
        let mut at = 0usize;
        while let Some(slash) = e.dest[at..].find('/') {
            at += slash;
            let d = e.dest[..at].to_string();
            at += 1;
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        // ...and a directory that ships empty, which has no file to imply it
        if e.is_dir() && !dirs.contains(&e.dest) {
            dirs.push(e.dest.clone());
        }
    }
    dirs.sort();
    for d in &dirs {
        b.add_dir(d, 0o755, 0, 0).unwrap_or_else(|e| panic!("{name}: {e}"));
    }

    entries.sort_by(|a, b| a.dest.cmp(&b.dest));
    for e in entries.iter() {
        if let Some(body) = &e.body {
            b.add_file(&e.dest, e.mode, 0, 0, body)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
        }
    }

    // Unsigned unless `builder` is given a key. Install policy is decided
    // elsewhere
    b.finish(None).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The bytes the applier asks for, out of the containers already opened
struct Containers<'a>(Vec<Package<'a>>);

impl Source for Containers<'_> {
    fn content(&self, key: ContentKey) -> mikupkg::apply::RootResult<Vec<u8>> {
        self.0
            .get(key.package)
            .ok_or(mikupkg::apply::RootError::NotFound { path: "package".into() })?
            .content(key.file)
            .map(|b| b.to_vec())
            .map_err(|e| mikupkg::apply::RootError::Io {
                path: format!("{e}"),
                code: -1,
            })
    }
}

/// Builds the containers and installs them into a staging tree. Returns the
/// tree and whether firmware went in
pub fn build_image(
    root: &Path,
    version: &str,
    programs: &[(&'static str, &'static str, PathBuf)],
    firmware: &[(String, PathBuf)],
) -> (PathBuf, bool) {
    let staging = root.join("target/fw_root");
    if staging.exists() {
        fs::remove_dir_all(&staging).ok();
    }
    fs::create_dir_all(&staging).expect("cannot make the staging tree");

    let pkgs_dir = root.join("target/pkgs");
    fs::create_dir_all(&pkgs_dir).ok();

    let mut sets = collect(root, programs, firmware);
    let have_firmware = !sets["miku-firmware"].is_empty();

    // build the containers
    let mut built: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for (name, desc, depends) in PACKAGES {
        let entries = sets.get_mut(name).unwrap();
        if entries.is_empty() {
            println!("[i] {name}: nothing to put in it, skipped");
            continue;
        }
        let bytes = make_container(name, desc, depends, version, entries);
        let file = pkgs_dir.join(format!("{name}-{version}.mpk"));
        fs::write(&file, &bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", file.display()));
        println!(
            "[ok] {name}-{version}.mpk  {} files, {} KB",
            entries.len(),
            bytes.len() / 1024
        );
        built.push((name, bytes));
    }

    // install them
    let mut fs_root = HostRoot::new(&staging, Ownership::Record);
    db::create(&mut fs_root, &Paths::default().db).expect("cannot lay out /pkg/db");

    let opened: Vec<Package<'_>> = built
        .iter()
        .map(|(name, bytes)| {
            Package::open(bytes).unwrap_or_else(|e| panic!("{name}: the container we just built will not open: {e}"))
        })
        .collect();

    let paths = Paths::default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for (i, pkg) in opened.iter().enumerate() {
        let meta = pkg.meta(|tag, at| {
            panic!("byte {at}: tag {tag:#06x} in a package this same build wrote")
        })
        .expect("the metadata we just wrote will not read back");

        let mut plan = Plan::new((i + 1) as u64);
        for (index, e) in pkg.table().entries.iter().enumerate() {
            plan.ops.push(match e.kind {
                FileKind::Directory => Op::Dir {
                    path: e.path.clone(),
                    mode: e.mode,
                    uid: e.uid,
                    gid: e.gid,
                },
                FileKind::Symlink => Op::Symlink {
                    path: e.path.clone(),
                    target: String::from_utf8_lossy(
                        pkg.content(index).expect("a link target we wrote"),
                    )
                    .into_owned(),
                    uid: e.uid,
                    gid: e.gid,
                },
                FileKind::File => Op::File {
                    path: e.path.clone(),
                    mode: e.mode,
                    uid: e.uid,
                    gid: e.gid,
                    key: ContentKey { package: i, file: index },
                    size: e.size as u64,
                    sha256: e.sha256,
                },
            });
        }
        plan.installs.push(meta.clone());
        plan.triggers = meta.triggers.clone();
        plan.finish();

        let source = Containers(
            built
                .iter()
                .map(|(_, bytes)| Package::open(bytes).unwrap())
                .collect(),
        );
        let out = tx::run(&plan, &mut fs_root, &source, &paths, now)
            .unwrap_or_else(|e| panic!("{}: {e}", meta.name));

        db::write(
            &mut fs_root,
            &paths.db,
            &meta.name,
            &Installed {
                meta: pkg.meta_bytes().to_vec(),
                files: pkg.table().clone(),
                // Explicit, so `mikupkg remove` does not treat it as an unused
                // dependency
                reason: Reason::Explicit,
                install_time: now,
                container_sha256: mikupkg::format::sha256(&built[i].1),
            },
        )
        .unwrap_or_else(|e| panic!("{}: cannot record it as installed: {e}", meta.name));

        println!(
            "[ok] installed {} {}  ({} files, {} directories)",
            meta.name, meta.version, out.placed, out.directories
        );
    }

    // The journal records this build's transactions and does not ship. The
    // image has the database and an empty cache
    let _ = fs::remove_dir_all(staging.join(&paths.journal_dir));
    fs_root
        .make_dir(&paths.journal_dir, 0o700, 0, 0)
        .expect("cannot make /pkg/journal");
    fs_root
        .make_dir(&paths.cache, 0o755, 0, 0)
        .expect("cannot make /pkg/cache");
    // World-writable and sticky, like /tmp. Unprivileged `mikupkg` stores
    // downloads here; files are hashed against a signed table before the root
    // half places them
    fs_root
        .make_dir(&paths.stage, 0o1777, 0, 0)
        .expect("cannot make /pkg/stage");

    println!(
        "[ok] /pkg/db describes the image: {} packages",
        opened.len()
    );

    (staging, have_firmware)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Repository-like tree for tests, without compiling a kernel
    struct Fake(PathBuf);

    impl Fake {
        fn new(tag: &str) -> Fake {
            let mut p = std::env::temp_dir();
            p.push(format!("mikupkg-builder-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            for d in ["etc/mayland", "etc/ssl/certs", "target/wallpapers", "bin-src"] {
                fs::create_dir_all(p.join(d)).unwrap();
            }
            fs::write(p.join("etc/passwd"), b"root:x:0:0::/root:/bin/msh\n").unwrap();
            fs::write(p.join("etc/shadow"), b"root:!:::::::\n").unwrap();
            fs::write(p.join("etc/mikudo.conf"), b"permit :wheel\n").unwrap();
            fs::write(p.join("etc/group"), b"wheel:x:10:miku\n").unwrap();
            fs::write(p.join("etc/mayland/may.conf"), b"# the desktop\n").unwrap();
            fs::write(p.join("etc/ssl/certs/ca-bundle.pem"), b"-----BEGIN\n").unwrap();
            fs::write(p.join("target/wallpapers/miku.qoi"), b"qoif....").unwrap();
            fs::write(p.join("bin-src/msh"), b"\x7fELF the shell").unwrap();
            fs::write(p.join("bin-src/mikudo"), b"\x7fELF the broker").unwrap();
            fs::write(p.join("bin-src/maywm"), b"\x7fELF the wm").unwrap();
            Fake(p)
        }

        fn programs(&self) -> Vec<(&'static str, &'static str, PathBuf)> {
            vec![
                ("msh", "miku-base", self.0.join("bin-src/msh")),
                ("mikudo", "miku-system", self.0.join("bin-src/mikudo")),
                ("maywm", "miku-desktop", self.0.join("bin-src/maywm")),
            ]
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(p: &Path) -> u16 {
        use std::os::unix::fs::PermissionsExt;
        (fs::metadata(p).unwrap().permissions().mode() & 0o7777) as u16
    }

    /// `personal/` makes miku-personal, and the owner's shadow line replaces
    /// the locked one
    #[test]
    fn personal_files_are_the_owners_package() {
        let fake = Fake::new("personal");
        fs::write(fake.0.join("etc/shadow"), b"root:!:::::::\nalunwrd:!:20334:0:99999:7:::\n").unwrap();
        for d in ["personal/assets", "personal/home/.config/negi"] {
            fs::create_dir_all(fake.0.join(d)).unwrap();
        }
        fs::write(fake.0.join("personal/assets/sticker.webp"), b"RIFF").unwrap();
        fs::write(fake.0.join("personal/home/.config/negi/config.json"), b"{}").unwrap();
        fs::write(fake.0.join("personal/hostname"), b"pc\n").unwrap();
        let own = "alunwrd:$argon2id$v=19$m=65536,t=3,p=1$c2FsdA$dGFn:20334:0:99999:7:::";
        fs::write(fake.0.join("personal/shadow"), format!("{own}\n")).unwrap();
        let (staging, _) = build_image(&fake.0, "0.2.10", &fake.programs(), &[]);

        assert!(fake.0.join("target/pkgs/miku-personal-0.2.10.mpk").is_file());
        assert_eq!(fs::read(staging.join("etc/aosh/negi-personal/sticker.webp")).unwrap(), b"RIFF");
        assert_eq!(fs::read(staging.join("etc/skel/.config/negi/config.json")).unwrap(), b"{}");
        assert_eq!(fs::read(staging.join("etc/hostname")).unwrap(), b"pc\n");
        assert_eq!(fs::read(staging.join("etc/aosh/default")).unwrap(), b"negi\n");
        let keys = fs::read_to_string(staging.join("etc/mayland/default.d/70-negi.conf")).unwrap();
        assert!(keys.contains("bind = $mod, D, signal, search toggle"));
        let shadow = fs::read_to_string(staging.join("etc/shadow")).unwrap();
        assert!(shadow.lines().any(|l| l == own), "{shadow}");
        assert!(!shadow.contains("alunwrd:!"));
        assert_eq!(mode_of(&staging.join("etc/shadow")), 0o640);
    }

    /// Without `personal/`, nothing of anybody's own is built
    #[test]
    fn no_personal_files_no_personal_package() {
        let fake = Fake::new("nopersonal");
        let (staging, _) = build_image(&fake.0, "0.2.10", &fake.programs(), &[]);
        assert!(!fake.0.join("target/pkgs/miku-personal-0.2.10.mpk").exists());
        assert!(!staging.join("etc/skel").exists());
        assert!(!staging.join("etc/aosh/default").exists());
        assert!(!staging.join("etc/mayland/default.d/70-negi.conf").exists());
    }

    /// `personal/shadow` holds alunwrd's line and nothing else
    #[test]
    fn only_the_owners_line_is_taken() {
        let ok = "alunwrd:$argon2id$v=19$m=65536,t=3,p=1$c2FsdA$dGFn:20334:0:99999:7:::";
        assert_eq!(owner_line(&format!("# mine\n{ok}\n")).unwrap(), ok);
        assert!(owner_line("root:$argon2id$x:20334:0:99999:7:::").is_err(), "root's is not taken");
        assert!(owner_line(&format!("{ok}\n{ok}")).is_err(), "one line");
        assert!(owner_line("alunwrd:plain:20334:0:99999:7:::").is_err(), "an Argon2id hash");
        assert!(with_line("root:!::\n", ok).is_err(), "a tree with no alunwrd line");
    }

    #[test]
    fn the_image_is_built_by_installing_packages() {
        let fake = Fake::new("image");
        let (staging, have_fw) = build_image(&fake.0, "0.2.10", &fake.programs(), &[]);
        assert!(!have_fw, "no firmware was offered");

        // The containers are real files, and they open
        for name in ["miku-base", "miku-system", "miku-desktop", "miku-net"] {
            let f = fake.0.join(format!("target/pkgs/{name}-0.2.10.mpk"));
            assert!(f.is_file(), "{name} was not built");
            let bytes = fs::read(&f).unwrap();
            let pkg = Package::open(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            pkg.verify_all().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        // An empty package is not built
        assert!(!fake.0.join("target/pkgs/miku-firmware-0.2.10.mpk").exists());

        // The programs landed, executable
        assert_eq!(fs::read(staging.join("bin/msh")).unwrap(), b"\x7fELF the shell");
        assert_eq!(mode_of(&staging.join("bin/msh")), 0o755);
        assert_eq!(mode_of(&staging.join("bin")), 0o755, "/bin's own mode is the package's");

        // Required modes of those two files
        assert_eq!(mode_of(&staging.join("etc/shadow")), 0o640);
        assert_eq!(mode_of(&staging.join("etc/mikudo.conf")), 0o640);
        assert_eq!(mode_of(&staging.join("etc/passwd")), 0o644);

        // What the build script made is there too
        assert!(staging.join("etc/mayland/wallpapers/miku.qoi").is_file());
        assert!(staging.join("etc/ssl/certs/ca-bundle.pem").is_file());
    }

    /// The image ships its package database
    #[test]
    fn the_image_carries_a_database_of_itself() {
        let fake = Fake::new("db");
        let (staging, _) = build_image(&fake.0, "0.2.10", &fake.programs(), &[]);

        assert_eq!(fs::read(staging.join("pkg/db/FORMAT")).unwrap(), b"1\n");

        let raw = fs::read(staging.join("pkg/db/miku-base.mpkr")).expect("a record for miku-base");
        let rec = Installed::parse(&raw).expect("and it is a record");
        assert_eq!(rec.reason, Reason::Explicit);
        let meta = mikupkg::format::meta::decode(&rec.meta, |_, _| {}).unwrap();
        assert_eq!(meta.name.as_str(), "miku-base");
        assert_eq!(meta.version.to_string(), "0.2.10");
        assert!(
            rec.files.entries.iter().any(|e| e.path == "bin/msh"),
            "the record names what it owns"
        );

        // And the dependency graph the tree declares is in there
        let sys = Installed::parse(&fs::read(staging.join("pkg/db/miku-system.mpkr")).unwrap()).unwrap();
        let sys = mikupkg::format::meta::decode(&sys.meta, |_, _| {}).unwrap();
        assert_eq!(sys.depends.len(), 1);
        assert_eq!(sys.depends[0].name.as_str(), "miku-base");
    }

    /// The build's own journal has no business on the machine
    #[test]
    fn the_staging_tree_ships_no_working_state() {
        let fake = Fake::new("clean");
        let (staging, _) = build_image(&fake.0, "0.2.10", &fake.programs(), &[]);

        assert!(staging.join("pkg/journal").is_dir(), "the directory ships");
        assert!(
            !staging.join("pkg/journal/tx").exists(),
            "but not the record of this build's transactions"
        );
        assert!(staging.join("pkg/cache").is_dir());

        let mut strays = Vec::new();
        let mut stack = vec![staging.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.file_name().unwrap().to_string_lossy().contains(".mikupkg-") {
                    strays.push(p.clone());
                }
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
        assert!(strays.is_empty(), "left behind: {strays:?}");
    }

    /// Every file in `etc/` must have a package
    #[test]
    fn a_file_no_package_claims_stops_the_build() {
        let fake = Fake::new("unclaimed");
        fs::write(fake.0.join("etc/stray.conf"), b"who owns me\n").unwrap();

        let programs = fake.programs();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_image(&fake.0, "0.2.10", &programs, &[]);
        }));
        assert!(caught.is_err(), "an unclaimed file must stop the build");
    }

    #[test]
    fn the_mode_of_a_path_is_decided_in_one_place() {
        assert_eq!(mode_for("bin/msh"), 0o755);
        assert_eq!(mode_for("etc/shadow"), 0o640);
        assert_eq!(mode_for("etc/mikudo.conf"), 0o640);
        assert_eq!(mode_for("etc/passwd"), 0o644);
        assert_eq!(mode_for("lib/firmware/nvidia/x.bin"), 0o644);
    }
}
