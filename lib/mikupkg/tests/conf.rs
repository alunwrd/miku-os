// Machine configuration
//
// An unrecognized line is a note with file and line number. A parse error in
// anything deciding trust denies everything, since `key =` says whose
// signatures this machine believes.

use mikupkg::conf::layers::Files;
use mikupkg::conf::{load, Complaint, Config, MAX_MIRRORS, MAX_REPOS};
use mikupkg::name::Name;

/// A tree of files, written out
#[derive(Default)]
struct Tree(Vec<(String, String)>);

impl Tree {
    fn with(mut self, path: &str, body: &str) -> Tree {
        self.0.push((path.to_string(), body.to_string()));
        self
    }
}

impl Files for Tree {
    fn list(&self, dir: &str) -> Vec<String> {
        let prefix = std::format!("{dir}/");
        self.0
            .iter()
            .filter_map(|(p, _)| p.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('/'))
            .map(|s| s.to_string())
            .collect()
    }
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_bytes().to_vec())
    }
}

const ETC: &str = "etc/mikupkg";

fn read(tree: &Tree) -> (Config, Vec<mikupkg::conf::Note>) {
    load(tree, ETC)
}

fn must(tree: &Tree) -> Config {
    let (cfg, notes) = read(tree);
    let fatal: Vec<_> = notes.iter().filter(|n| n.fatal).collect();
    assert!(fatal.is_empty(), "unexpected refusal: {fatal:?}");
    cfg
}

// The layers

#[test]
fn later_wins_and_within_a_directory_it_is_name_order() {
    let tree = Tree::default()
        .with("etc/mikupkg/default.d/10-first.conf", "cache = /a\n")
        .with("etc/mikupkg/default.d/50-second.conf", "cache = /b\n")
        .with("etc/mikupkg/mikupkg.conf", "cache = /c\n")
        .with("etc/mikupkg/conf.d/99-mine.conf", "cache = /d\n");

    assert_eq!(must(&tree).cache, "/d");

    // Without the last layer, the one before it
    let tree = Tree::default()
        .with("etc/mikupkg/default.d/10-first.conf", "cache = /a\n")
        .with("etc/mikupkg/default.d/50-second.conf", "cache = /b\n");
    assert_eq!(must(&tree).cache, "/b", "50- comes after 10-");
}

/// A file in `conf.d` named like one in `default.d` replaces it, so an empty
/// file is an off switch
#[test]
fn a_shipped_default_can_be_switched_off_with_an_empty_file() {
    let shipped = "repo core {\n  url = https://repo.miku/core\n}\n";

    let on = Tree::default().with("etc/mikupkg/default.d/10-repos.conf", shipped);
    assert_eq!(must(&on).repos.len(), 1);

    let off = Tree::default()
        .with("etc/mikupkg/default.d/10-repos.conf", shipped)
        .with("etc/mikupkg/conf.d/10-repos.conf", "");
    assert_eq!(
        must(&off).repos.len(),
        0,
        "an empty file of the same name replaces the shipped one"
    );

    // One with contents replaces, not adds to it
    let replaced = Tree::default()
        .with("etc/mikupkg/default.d/10-repos.conf", shipped)
        .with(
            "etc/mikupkg/conf.d/10-repos.conf",
            "repo local {\n  url = https://elsewhere/local\n}\n",
        );
    let cfg = must(&replaced);
    assert_eq!(cfg.repos.len(), 1);
    assert_eq!(cfg.repos[0].name.as_str(), "local");
}

#[test]
fn only_conf_files_are_read() {
    let tree = Tree::default()
        .with("etc/mikupkg/default.d/10-repos.conf", "cache = /good\n")
        .with("etc/mikupkg/default.d/10-repos.conf.bak", "cache = /backup\n")
        .with("etc/mikupkg/default.d/notes.txt", "cache = /notes\n");
    assert_eq!(must(&tree).cache, "/good");
}

// The settings

#[test]
fn the_configuration_the_image_will_actually_ship() {
    let tree = Tree::default().with(
        "etc/mikupkg/default.d/10-repos.conf",
        "\
# where this machine looks for packages
cache = /pkg/cache
db    = /pkg/db
keyring = /etc/mikupkg/keys
max_index_age = 7d

repo core {
    url      = https://repo.miku/core
    key      = 3f9a1c22b0d4e7f1
    priority = 10
    mirror   = https://mirror.example/core
    mirror   = https://other.example/core
}
",
    );

    let cfg = must(&tree);
    assert_eq!(cfg.cache, "/pkg/cache");
    assert_eq!(cfg.db, "/pkg/db");
    assert_eq!(cfg.keyring, "/etc/mikupkg/keys");
    assert_eq!(cfg.max_index_age, 7 * 86_400);
    assert!(!cfg.allow_downgrade, "off unless somebody says otherwise");

    assert_eq!(cfg.repos.len(), 1);
    let core = &cfg.repos[0];
    assert_eq!(core.name.as_str(), "core");
    assert_eq!(core.url, "https://repo.miku/core");
    assert_eq!(core.priority, 10);
    assert_eq!(core.mirrors.len(), 2);
    assert_eq!(
        core.key.unwrap().to_string(),
        "3f9a1c22b0d4e7f1",
        "the key id round-trips through its text"
    );
}

#[test]
fn a_length_of_time_can_be_written_the_way_people_write_one() {
    for (text, want) in [
        ("90", 90u64),
        ("90s", 90),
        ("30m", 1800),
        ("12h", 43_200),
        ("7d", 604_800),
    ] {
        let tree = Tree::default().with(
            "etc/mikupkg/mikupkg.conf",
            &std::format!("max_index_age = {text}\n"),
        );
        assert_eq!(must(&tree).max_index_age, want, "{text}");
    }
}

#[test]
fn ignored_packages_accumulate_and_do_not_repeat() {
    let tree = Tree::default().with(
        "etc/mikupkg/mikupkg.conf",
        "ignore = miku-firmware\nignore = miku-demos\nignore = miku-firmware\n",
    );
    let cfg = must(&tree);
    assert_eq!(cfg.ignore.len(), 2);
    assert!(cfg.is_ignored(&Name::parse("miku-firmware").unwrap()));
    assert!(!cfg.is_ignored(&Name::parse("miku-base").unwrap()));
}

// Complaining

/// An unrecognized line is reported with file and line, and the run continues
#[test]
fn an_unrecognised_setting_is_named_and_not_fatal() {
    let tree = Tree::default().with(
        "etc/mikupkg/mikupkg.conf",
        "cache = /pkg/cache\nparallel_downloads = 4\ndb = /pkg/db\n",
    );
    let (cfg, notes) = read(&tree);

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].what, Complaint::UnknownSetting);
    assert_eq!(notes[0].file, "etc/mikupkg/mikupkg.conf");
    assert_eq!(notes[0].line, 2, "the line as an editor counts it");
    assert!(notes[0].text.contains("parallel_downloads"));
    assert!(!notes[0].fatal);

    // The settings around it were still read
    assert_eq!(cfg.cache, "/pkg/cache");
    assert_eq!(cfg.db, "/pkg/db");

    let said = std::format!("{}", notes[0]);
    assert!(said.starts_with("etc/mikupkg/mikupkg.conf:2:"), "{said}");
}

/// Inside a block deciding trust, it is fatal
#[test]
fn a_line_inside_a_repository_that_nobody_understands_stops_the_run() {
    let tree = Tree::default().with(
        "etc/mikupkg/mikupkg.conf",
        "repo core {\n  url = https://repo.miku/core\n  verify = never\n}\n",
    );
    let (_, notes) = read(&tree);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].what, Complaint::UnknownRepoSetting);
    assert!(notes[0].fatal, "trust is not somewhere to guess");
    assert_eq!(notes[0].line, 3);
}

#[test]
fn a_key_that_is_not_a_key_stops_the_run() {
    for bad in ["notahexstring", "3f9a1c22b0d4e7", "3f9a1c22b0d4e7fg"] {
        let tree = Tree::default().with(
            "etc/mikupkg/mikupkg.conf",
            &std::format!("repo core {{\n  url = https://x/y\n  key = {bad}\n}}\n"),
        );
        let (_, notes) = read(&tree);
        assert!(
            notes.iter().any(|n| n.fatal),
            "{bad:?} should be refused: {notes:?}"
        );
    }
}

#[test]
fn a_repository_with_nothing_to_fetch_from_stops_the_run() {
    let tree = Tree::default()
        .with("etc/mikupkg/mikupkg.conf", "repo core {\n  priority = 5\n}\n");
    let (cfg, notes) = read(&tree);
    assert!(notes.iter().any(|n| n.what == Complaint::RepoWithoutUrl && n.fatal));
    assert!(cfg.repos.is_empty());
}

#[test]
fn a_block_that_does_not_close_stops_the_run() {
    for text in [
        "repo core {\n  url = https://x/y\n",
        "repo core\n  url = https://x/y\n",
        "}\n",
    ] {
        let tree = Tree::default().with("etc/mikupkg/mikupkg.conf", text);
        let (_, notes) = read(&tree);
        assert!(
            notes.iter().any(|n| n.what == Complaint::Unbalanced && n.fatal),
            "{text:?} -> {notes:?}"
        );
    }
}

#[test]
fn a_repository_with_a_name_that_is_not_one_stops_the_run() {
    let tree = Tree::default()
        .with("etc/mikupkg/mikupkg.conf", "repo ../etc {\n  url = https://x\n}\n");
    let (_, notes) = read(&tree);
    assert!(notes.iter().any(|n| n.what == Complaint::BadRepoName && n.fatal));
}

#[test]
fn everything_is_bounded() {
    let mut text = String::new();
    for i in 0..MAX_REPOS + 2 {
        text.push_str(&std::format!("repo r{i} {{\n  url = https://x/{i}\n}}\n"));
    }
    let tree = Tree::default().with("etc/mikupkg/mikupkg.conf", &text);
    let (cfg, notes) = read(&tree);
    assert!(cfg.repos.len() <= MAX_REPOS);
    assert!(notes.iter().any(|n| matches!(n.what, Complaint::TooMany { .. })));

    let mut text = String::from("repo core {\n  url = https://x\n");
    for i in 0..MAX_MIRRORS + 2 {
        text.push_str(&std::format!("  mirror = https://m{i}/core\n"));
    }
    text.push_str("}\n");
    let tree = Tree::default().with("etc/mikupkg/mikupkg.conf", &text);
    let (_, notes) = read(&tree);
    assert!(notes.iter().any(|n| matches!(n.what, Complaint::TooMany { .. })));

    let huge = "x".repeat(mikupkg::conf::MAX_BYTES + 1);
    let tree = Tree::default().with("etc/mikupkg/mikupkg.conf", &huge);
    let (_, notes) = read(&tree);
    assert!(notes.iter().any(|n| matches!(n.what, Complaint::TooBig { .. }) && n.fatal));
}

#[test]
fn a_comment_is_not_a_setting() {
    let tree = Tree::default().with(
        "etc/mikupkg/mikupkg.conf",
        "# cache = /wrong\ncache = /right   # and this is a comment too\n",
    );
    let (cfg, notes) = read(&tree);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(cfg.cache, "/right");
}

/// Applier paths come from the configuration, so `--root` and a test tree are
/// the same mechanism
#[test]
fn the_configuration_says_where_the_state_lives() {
    let tree = Tree::default().with(
        "etc/mikupkg/mikupkg.conf",
        "db = var/db\ncache = var/cache\njournal = var/journal\n",
    );
    let paths = must(&tree).paths();
    assert_eq!(paths.db, "var/db");
    assert_eq!(paths.cache, "var/cache");
    assert_eq!(paths.journal_dir, "var/journal");
    assert_eq!(paths.journal, "var/journal/tx");
}

/// The shipped file, read by the parser used on the machine
#[test]
fn the_configuration_this_build_ships_is_one_the_parser_accepts() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../etc/mikupkg/default.d/10-paths.conf"
    );
    let bytes = std::fs::read(path).expect("the shipped default should be in the tree");

    let mut cfg = Config::default();
    let mut notes = Vec::new();
    mikupkg::conf::grammar::parse_into("10-paths.conf", &bytes, &mut cfg, &mut notes);

    assert!(notes.is_empty(), "the file we ship must parse cleanly: {notes:?}");
    assert_eq!(cfg.db, "/pkg/db");
    assert_eq!(cfg.cache, "/pkg/cache");
    assert_eq!(cfg.keyring, "/etc/mikupkg/keys");
    assert_eq!(cfg.max_index_age, 7 * 86_400);
    assert!(!cfg.allow_downgrade);
    assert!(
        cfg.repos.is_empty(),
        "there is no repository yet, and the shipped file should not pretend otherwise"
    );

    // ...and what it says turns into paths the applier can use
    let paths = cfg.paths();
    assert_eq!(paths.db, "pkg/db", "relative, for a Root");
    assert_eq!(paths.journal, "pkg/journal/tx");
}
