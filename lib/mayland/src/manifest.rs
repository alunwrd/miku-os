// What files a config is made of, and in what order. `conf` handles one text's
// lines; this involves a filesystem.
//
// Layers:
//
//      /etc/mayland/default.d/*.conf   what the build ships
//      /etc/mayland/may.conf           the machine's own file
//      /etc/mayland/conf.d/*.conf      the machine's drop-ins
//
// Read in that order, and the later line wins. Inside a directory, files follow
// name order, as with `10-`, `50-`, `99-`.
//
// A file in `conf.d` named like one in `default.d` replaces it, and the shipped
// one is not read. An empty file switches a shipped file off, so `Read`
// separates "empty" from "missing": an empty drop-in is silent, a missing
// `source =` is not.
//
// The list is built up front, since variables are collected from every file
// first (`conf::collect_vars`), each `Where` needs a file number, and what was
// read is reported as the config is read. It also keeps a diamond from reading
// a file twice.
//
// A `source =` path is literal: `$` is not expanded, because the file list must
// be complete before any variable is known.

use super::conf::{Note, Text};

/// Where a desktop's config is read from, lowest authority first. One config,
/// shared by the bar and the window manager.
///
/// `default.d` is what the build ships and rewrites on every ISO. Nothing ships
/// in `conf.d`, so what a person leaves there survives an upgrade
pub const DEFAULT_D: &str = "/etc/mayland/default.d";
pub const MAIN: &str = "/etc/mayland/may.conf";
pub const CONF_D: &str = "/etc/mayland/conf.d";

/// The layers, in reading order
pub const LAYERS: [Layer<'static>; 3] =
    [Layer::Dir(DEFAULT_D), Layer::File(MAIN), Layer::Dir(CONF_D)];

/// Most files in one config: eight shipped, one main file, and as many drop-ins
/// again. `Where` carries the index as a `u8`, so this could reach 255
pub const MAX_FILES: usize = 24;

/// How many entries one directory of drop-ins may hold
pub const MAX_IN_DIR: usize = 16;

/// How deep `source =` may nest, counted from a layer file
const MAX_DEPTH: u8 = 4;

/// Result of asking for a file. `Empty` is distinct because `miku::read_file`
/// gives `None` for both a zero-length and an absent file, and here an empty
/// drop-in is a silent mask while a missing `source =` is an error
pub enum Read {
    Text(&'static str),
    Empty,
    Missing,
}

/// The filesystem, as much as a config needs. A trait, since the kernel links
/// `libmay` with no `miku` under it; the selftest supplies a table of strings
pub trait Files {
    /// The bytes of one file, which the reader keeps for good
    fn read(&mut self, path: &str) -> Read;

    /// Names, not paths, in a directory, in any order. The caller filters
    /// directories and non-`.conf` names
    fn list(&mut self, dir: &str, out: &mut [Text]) -> usize;
}

/// One place files are read from
#[derive(Clone, Copy)]
pub enum Layer<'a> {
    /// Every `*.conf` in it, in the order of their names
    Dir(&'a str),
    /// Exactly this file
    File(&'a str),
}

/// The files a config is made of, in the order they are read
pub struct Manifest {
    paths: [Text; MAX_FILES],
    texts: [&'static str; MAX_FILES],
    tiers: [u8; MAX_FILES],
    n: usize,
    /// Which entries are layer files and not pulled in by `source =`. The
    /// second pass walks these; `conf::walk` recurses into a sourced file at
    /// its line
    roots: [u8; MAX_FILES],
    n_roots: usize,
    /// Files that did not fit, and the first name too long to carry. Kept for
    /// the reader to report
    pub dropped: u16,
    pub long_name: Option<Text>,
}

impl Manifest {
    pub const fn new() -> Manifest {
        Manifest {
            paths: [Text::EMPTY; MAX_FILES],
            texts: [""; MAX_FILES],
            tiers: [0; MAX_FILES],
            n: 0,
            roots: [0; MAX_FILES],
            n_roots: 0,
            dropped: 0,
            long_name: None,
        }
    }

    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    pub fn path(&self, i: u8) -> &str {
        self.paths.get(i as usize).map(|t| t.as_str()).unwrap_or("?")
    }

    pub fn text(&self, i: u8) -> &'static str {
        *self.texts.get(i as usize).unwrap_or(&"")
    }

    /// Which layer a file came from. Two files of one tier fighting over a
    /// setting is an error; a higher tier overriding a lower one is not
    pub fn tier(&self, i: u8) -> u8 {
        *self.tiers.get(i as usize).unwrap_or(&0)
    }

    /// The layer files, in reading order
    pub fn roots(&self) -> &[u8] {
        &self.roots[..self.n_roots]
    }

    /// Every file in reading order, for the first pass over variables
    pub fn all(&self) -> impl Iterator<Item = u8> + '_ {
        (0..self.n).map(|i| i as u8)
    }

    /// Number a path was filed under, if present. `Sink::source` answers with
    /// this
    pub fn find(&self, path: &str) -> Option<(&'static str, u8)> {
        let path = path.trim();
        (0..self.n)
            .find(|&i| self.paths[i].as_str() == path)
            .map(|i| (self.texts[i], i as u8))
    }

    /// Reads the layers and all they pull in. Only discovery: no line is
    /// interpreted and no variable expanded. Both config passes run off the
    /// result
    pub fn build<F: Files>(files: &mut F, layers: &[Layer]) -> Manifest {
        let mut m = Manifest::new();

        // All directories listed first, since masking compares two: a name in a
        // later one hides it in an earlier one
        let mut names = [[Text::EMPTY; MAX_IN_DIR]; 2];
        let mut counts = [0usize; 2];
        let mut dirs = 0usize;
        let mut dir_of = [usize::MAX; 8];

        for (li, layer) in layers.iter().enumerate() {
            let Layer::Dir(path) = *layer else { continue };
            if dirs == names.len() || li >= dir_of.len() {
                continue;
            }
            counts[dirs] = m.listing(files, path, &mut names[dirs]);
            dir_of[li] = dirs;
            dirs += 1;
        }

        for (li, layer) in layers.iter().enumerate() {
            let tier = li as u8;
            match *layer {
                Layer::File(path) => m.add_root(files, path, tier),
                Layer::Dir(dir) => {
                    let Some(&d) = dir_of.get(li) else { continue };
                    if d == usize::MAX {
                        continue;
                    }
                    for k in 0..counts[d] {
                        let name = names[d][k].as_str();
                        // Hidden by the same name in a later directory. The
                        // shipped file is not read, so an empty drop-in is an
                        // off switch
                        let masked = (d + 1..dirs).any(|later| {
                            (0..counts[later]).any(|j| names[later][j].as_str() == name)
                        });
                        if masked {
                            continue;
                        }
                        let mut buf = [0u8; 128];
                        let Some(path) = join(dir, name, &mut buf) else {
                            m.note_long(names[d][k]);
                            continue;
                        };
                        m.add_root(files, path, tier);
                    }
                }
            }
        }
        m
    }

    /// One directory's `*.conf`, sorted by name
    fn listing<F: Files>(&mut self, files: &mut F, dir: &str, out: &mut [Text; MAX_IN_DIR])
        -> usize
    {
        let mut raw = [Text::EMPTY; MAX_IN_DIR];
        let got = files.list(dir, &mut raw).min(MAX_IN_DIR);
        let mut n = 0;
        for k in 0..got {
            let name = raw[k].as_str();
            // `.conf` only, excluding editor backups and upgrade leftovers
            if !name.ends_with(".conf") || name.starts_with('.') {
                continue;
            }
            out[n] = raw[k];
            n += 1;
        }
        // Insertion sort: sixteen entries at most, with no allocator
        for i in 1..n {
            let mut j = i;
            while j > 0 && out[j - 1].as_str() > out[j].as_str() {
                out.swap(j - 1, j);
                j -= 1;
            }
        }
        n
    }

    /// A layer file, and everything it sources
    fn add_root<F: Files>(&mut self, files: &mut F, path: &str, tier: u8) {
        let Some(idx) = self.add(files, path, tier) else { return };
        if self.n_roots < self.roots.len() {
            self.roots[self.n_roots] = idx;
            self.n_roots += 1;
        }
        self.follow(files, idx, tier, 1);
    }

    /// Add one file, or hand back the number it already has
    fn add<F: Files>(&mut self, files: &mut F, path: &str, tier: u8) -> Option<u8> {
        if let Some((_, i)) = self.find(path) {
            return Some(i);
        }
        if !Text::fits(path) {
            // Not shortened: a truncated path is a different file
            self.note_long(Text::from(path));
            return None;
        }
        if self.n == MAX_FILES {
            self.dropped += 1;
            return None;
        }
        let text = match files.read(path) {
            Read::Text(t) => t,
            // Present and empty: contributes nothing and says nothing, as when
            // masking a shipped file
            Read::Empty => "",
            // Absent. Normal for a layer file; the dispatching pass complains
            // about a `source =` naming it, where the line number is known
            Read::Missing => return None,
        };
        let i = self.n;
        self.paths[i] = Text::from(path);
        self.texts[i] = text;
        self.tiers[i] = tier;
        self.n += 1;
        Some(i as u8)
    }

    /// Every `source =` in one file, depth first, as written. A lexical scan:
    /// comments are stripped, no variable is expanded. Depth is bounded as in
    /// the dispatching pass, so a cycle stops here too, and `add` returns a
    /// path's existing number, so a diamond costs one read
    fn follow<F: Files>(&mut self, files: &mut F, idx: u8, tier: u8, depth: u8) {
        if depth >= MAX_DEPTH {
            return;
        }
        let text = self.texts[idx as usize];
        for raw in text.lines() {
            let line = super::conf::strip_comment(raw).trim();
            let Some((key, value)) = super::conf::split_once(line, '=') else { continue };
            if key.trim() != "source" {
                continue;
            }
            let path = value.trim();
            if path.is_empty() {
                continue;
            }
            if let Some(child) = self.add(files, path, tier) {
                self.follow(files, child, tier, depth + 1);
            }
        }
    }

    fn note_long(&mut self, name: Text) {
        if self.long_name.is_none() {
            self.long_name = Some(name);
        }
    }

    /// What the reader should say about the list, if anything
    pub fn complaint(&self) -> Option<(Note, Text)> {
        if let Some(name) = self.long_name {
            return Some((Note::PathTooLong, name));
        }
        if self.dropped > 0 {
            return Some((Note::TooManyFiles, Text::EMPTY));
        }
        None
    }
}

/// `dir` + `/` + `name`, if it fits
fn join<'a>(dir: &str, name: &str, buf: &'a mut [u8; 128]) -> Option<&'a str> {
    let need = dir.len() + 1 + name.len();
    if need > buf.len() {
        return None;
    }
    buf[..dir.len()].copy_from_slice(dir.as_bytes());
    buf[dir.len()] = b'/';
    buf[dir.len() + 1..need].copy_from_slice(name.as_bytes());
    core::str::from_utf8(&buf[..need]).ok()
}

// The filesystem, for the userspace half

/// `Files` over the real filesystem, shared by both programs reading a config.
/// Behind a feature because the kernel links `libmay` and `miku` is the ring-3
/// syscall surface
#[cfg(feature = "userspace")]
pub struct DiskFiles;

#[cfg(feature = "userspace")]
impl Files for DiskFiles {
    fn read(&mut self, path: &str) -> Read {
        match miku::read_file(path) {
            Some((ptr, len)) => {
                let bytes: &'static [u8] = unsafe { core::slice::from_raw_parts(ptr, len) };
                match core::str::from_utf8(bytes) {
                    Ok(t) => Read::Text(t),
                    // Not text; "missing" would be wrong
                    Err(_) => Read::Empty,
                }
            }
            // `read_file` gives `None` for a zero-length or an absent file,
            // with opposite meanings, so the size is asked separately
            None => {
                let mut c = [0u8; 256];
                let n = path.len().min(c.len() - 1);
                c[..n].copy_from_slice(&path.as_bytes()[..n]);
                c[n] = 0;
                if unsafe { miku::miku_filesize(c.as_ptr()) } == 0 {
                    Read::Empty
                } else {
                    Read::Missing
                }
            }
        }
    }

    fn list(&mut self, dir: &str, out: &mut [Text]) -> usize {
        let mut c = [0u8; 256];
        let n = dir.len().min(c.len() - 1);
        c[..n].copy_from_slice(&dir.as_bytes()[..n]);
        c[n] = 0;

        let mut ents: [miku::MikuDirent; MAX_IN_DIR] = unsafe { core::mem::zeroed() };
        let got = unsafe { miku::miku_readdir(c.as_ptr(), ents.as_mut_ptr(), MAX_IN_DIR) };
        // A missing directory, even on an unmounted disk, counts as empty; the
        // config falls back to what is compiled in
        if got <= 0 {
            return 0;
        }
        let mut k = 0;
        for i in 0..(got as usize).min(MAX_IN_DIR) {
            if k == out.len() {
                break;
            }
            let e = &ents[i];
            // kind 2 is a directory in the MikuOS dirent ABI
            if e.kind == 2 {
                continue;
            }
            let len = (e.name_len as usize).min(e.name.len());
            let Ok(name) = core::str::from_utf8(&e.name[..len]) else { continue };
            out[k] = Text::from(name);
            k += 1;
        }
        k
    }
}
