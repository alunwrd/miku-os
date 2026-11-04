// Process and file tests on the host, with real programs and a manual poll

use std::path::Path;
use std::time::{Duration, Instant};

use crate::object::engine::Engine;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

fn ev(e: &Engine, root: u32, expr: &str) -> String {
    e.eval(root, expr).unwrap_or_else(|err| panic!("{expr}: {err}"))
}

/// Runs the shell loop's steps until `done` or 2 s
fn pump(e: &Engine, done: impl Fn(&Engine) -> bool) {
    let start = Instant::now();
    while !done(e) && start.elapsed() < Duration::from_secs(2) {
        let fds = e.io_fds();
        let mut p: Vec<libc::pollfd> = fds.iter().map(|&(fd, _)| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
        unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, 10) };
        for (pf, &(_, f)) in p.iter().zip(fds.iter()) {
            if pf.revents != 0 {
                e.io_ready(f);
            }
        }
        e.advance(start.elapsed().as_millis() as u64);
    }
}

#[test]
fn a_process_runs_and_its_output_is_collected() {
    let (e, r) = load(r#"
        Item {
            id: root
            property string out: ""
            property int code: -1
            property string who: "aosh"
            Process {
                id: p
                command: ["echo", "hello", root.who]
                running: true
                stdout: StdioCollector { onStreamFinished: root.out = this.text }
                onExited: (exitCode, exitStatus) => root.code = exitCode
            }
        }
    "#);
    pump(&e, |e| ev(e, r, "code") != "-1");
    assert_eq!(ev(&e, r, "out"), "hello aosh\n");
    assert_eq!(ev(&e, r, "code"), "0");
    assert_eq!(ev(&e, r, "p.running"), "false");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn a_split_parser_says_each_line_and_the_exit_code_comes_back() {
    let (e, r) = load(r#"
        Item {
            id: root
            property var lines: []
            property int code: -1
            Process {
                command: ["sh", "-c", "printf 'a\nb\nc'; exit 3"]
                running: true
                stdout: SplitParser { onRead: (data) => root.lines = root.lines.concat([data]) }
                onExited: (exitCode) => root.code = exitCode
            }
        }
    "#);
    pump(&e, |e| ev(e, r, "code") != "-1");
    assert_eq!(ev(&e, r, "lines.join(',')"), "a,b,c");
    assert_eq!(ev(&e, r, "code"), "3");
}

#[test]
fn running_false_then_true_restarts_once_the_old_one_is_gone() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int runs: 0
            property int exits: 0
            Process {
                id: p
                command: ["sleep", "5"]
                running: true
                onStarted: root.runs++
                onExited: root.exits++
            }
        }
    "#);
    pump(&e, |e| ev(e, r, "runs") == "1");
    e.eval(r, "(p.running = false, p.running = true)").unwrap();
    pump(&e, |e| ev(e, r, "runs") == "2");
    assert_eq!(ev(&e, r, "exits"), "1", "the first one exited (SIGTERM) before the second began");
    assert_eq!(ev(&e, r, "p.running"), "true");
    // And it goes with its object
    e.destroy(r);
}

#[test]
fn a_command_that_arrives_late_still_starts() {
    let (e, r) = load(r#"
        Item {
            id: root
            property var cmd: []
            property string out: ""
            Process {
                command: root.cmd
                running: true
                stdout: StdioCollector { onStreamFinished: root.out = text }
            }
        }
    "#);
    e.eval(r, "root.cmd = ['echo', 'late']").unwrap();
    pump(&e, |e| ev(e, r, "out") != "");
    assert_eq!(ev(&e, r, "out"), "late\n");
}

#[test]
fn a_file_view_reads_follows_and_writes() {
    let dir = std::env::temp_dir().join(format!("aosh-fv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, "first").unwrap();
    let src = format!(r#"
        Item {{
            id: root
            property int changes: 0
            property string shown: view.text()
            FileView {{ id: view; path: "{}"; watchChanges: true; onFileChanged: root.changes++ }}
        }}
    "#, file.display());
    let (e, r) = load(&src);
    assert_eq!(ev(&e, r, "shown"), "first");
    assert_eq!(ev(&e, r, "view.loaded"), "true");
    std::fs::write(&file, "second").unwrap();
    pump(&e, |e| ev(e, r, "changes") != "0");
    assert_eq!(ev(&e, r, "shown"), "second", "text() in a binding follows the file");
    e.eval(r, "view.setText('third')").unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "third");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn quickshell_env_reads_the_environment() {
    let (e, r) = load("Item { property string home: Quickshell.env('HOME') || ''; property var none: Quickshell.env('AOSH_NOT_SET_ANYWHERE') }");
    assert_eq!(ev(&e, r, "home"), std::env::var("HOME").unwrap_or_default());
    assert_eq!(ev(&e, r, "none === null"), "true");
}

/// Creating the user directory after startup (as `mkdir` and
/// `cp -r /etc/aosh ~/.config/aosh` do) triggers a reload, as does a later save
#[test]
fn the_users_directory_is_followed_before_it_exists() {
    let base = std::env::temp_dir().join(format!("aosh-user-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let user = base.join(".config/aosh");
    let (e, _) = load("Item {}");
    crate::io::watch_user(&e.core, user.clone());
    let due = |e: &Engine| e.core.0.borrow().io.reload_due.is_some();

    std::fs::create_dir(base.join(".config")).unwrap();
    pump(&e, |e| e.core.0.borrow().io.dirs.values().any(|d| *d == base.join(".config")));
    assert!(!due(&e), "a directory on the way is not a reload");

    std::fs::create_dir(&user).unwrap();
    std::fs::write(user.join("shell.qml"), "Item {}").unwrap();
    pump(&e, due);
    assert!(due(&e), "the directory appearing is a reload");
    assert!(e.core.0.borrow().io.config_dirs.contains(&user));

    e.core.0.borrow_mut().io.reload_due = None;
    std::fs::write(user.join("Bar.qml"), "Item {}").unwrap();
    pump(&e, due);
    assert!(due(&e), "and so is a save in it");
    let _ = std::fs::remove_dir_all(&base);
}

/// Reduced form of end4's `Config.qml`: a missing file is written from
/// defaults, a file fills the properties it names, and a script change emits
/// `adapterUpdated` and saves
#[test]
fn a_json_adapter_is_its_file() {
    let dir = std::env::temp_dir().join(format!("aosh-json-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config.json");
    let src = format!(r#"
        Item {{
            id: root
            property bool ready: false
            property int updates: 0
            property int failed: -1
            property alias options: adapter
            FileView {{
                id: view
                path: "{}"
                onLoaded: root.ready = true
                onAdapterUpdated: {{ root.updates++; writeAdapter() }}
                onLoadFailed: error => {{
                    root.failed = error
                    if (error == FileViewError.FileNotFound) writeAdapter();
                }}
                JsonAdapter {{
                    id: adapter
                    property string panelFamily: "ii"
                    property list<var> models: [{{ "name": "a" }}]
                    property JsonObject time: JsonObject {{
                        property string format: "hh:mm"
                        property bool secondPrecision: false
                        property JsonObject pomodoro: JsonObject {{ property int focus: 1500 }}
                    }}
                    property var extra: {{ "k": [1, 2] }}
                }}
            }}
        }}
    "#, file.display());
    let (e, r) = load(&src);
    assert_eq!(ev(&e, r, "failed"), "2", "FileViewError.FileNotFound");
    assert_eq!(ev(&e, r, "updates"), "0", "writing the defaults is not an update");
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written, r#"{
    "extra": {
        "k": [
            1,
            2
        ]
    },
    "models": [
        {
            "name": "a"
        }
    ],
    "panelFamily": "ii",
    "time": {
        "format": "hh:mm",
        "pomodoro": {
            "focus": 1500
        },
        "secondPrecision": false
    }
}
"#, "sorted keys, four spaces, a newline at the end - QJsonDocument::Indented");

    // Named keys are set, others keep defaults, unknown keys are ignored
    std::fs::write(&file, r#"{"panelFamily": "waffle", "time": {"format": "HH:mm", "pomodoro": {"focus": 60}}, "unknown": 1}"#).unwrap();
    e.eval(r, "view.reload()").unwrap();
    assert_eq!(ev(&e, r, "ready"), "true");
    assert_eq!(ev(&e, r, "options.panelFamily"), "waffle");
    assert_eq!(ev(&e, r, "options.time.format"), "HH:mm");
    assert_eq!(ev(&e, r, "options.time.pomodoro.focus"), "60");
    assert_eq!(ev(&e, r, "options.time.secondPrecision"), "false");
    assert_eq!(ev(&e, r, "options.extra.k.length"), "2");
    assert_eq!(ev(&e, r, "updates"), "0", "a load is not an update");

    // A nested script change updates and saves
    e.eval(r, "options.time.pomodoro.focus = 25").unwrap();
    assert_eq!(ev(&e, r, "updates"), "1");
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(saved.contains("\"focus\": 25") && saved.contains("\"panelFamily\": \"waffle\""), "{saved}");
    assert!(!saved.contains("unknown"), "only the properties are written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn standard_paths_are_qts_under_home() {
    let (e, r) = load(r#"
        Item {
            property string home: StandardPaths.standardLocations(StandardPaths.HomeLocation)[0]
            property string state: StandardPaths.standardLocations(StandardPaths.StateLocation)[0]
            property string config: StandardPaths.writableLocation(StandardPaths.ConfigLocation)
        }
    "#);
    let home = std::env::var("HOME").unwrap();
    assert_eq!(ev(&e, r, "home"), format!("file://{home}"));
    let state = std::env::var("XDG_STATE_HOME").ok().filter(|s| s.starts_with('/'))
        .unwrap_or_else(|| format!("{home}/.local/state"));
    assert_eq!(ev(&e, r, "state"), format!("file://{state}/quickshell"));
    assert!(ev(&e, r, "config").starts_with("file:///"));
    assert_eq!(ev(&e, r, "FileViewError.toString(FileViewError.NotAFile)"), "The specified path was not a file");
}
