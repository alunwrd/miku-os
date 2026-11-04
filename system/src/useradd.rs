#![no_std]
#![no_main]

//! useradd: adds an account. Run under mikudo, since it writes /etc/passwd and
//! /etc/group. No `-p` option: create the account, then `passwd` it
//!
//! Usage: useradd [-u uid] [-g gid] [-G group,group] [-s shell] [-d home] name
//!        useradd -D name                 (delete)

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use mikuauth::db;

/// First ordinary uid
const FIRST_USER_UID: u16 = 1000;
/// Highest generated uid; 65534 is nobody and 65535 is chown's sentinel
const LAST_USER_UID: u16 = 60000;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);

    let mut uid: Option<u16> = None;
    let mut gid: Option<u16> = None;
    let mut extra_groups: Vec<&str> = Vec::new();
    let mut shell = mikuauth::db::DEFAULT_SESSION;
    let mut home: Option<String> = None;
    let mut delete = false;
    let mut name: Option<&str> = None;

    let mut i = 1usize;
    while i < args.len() {
        match args[i] {
            "-u" => { i += 1; uid = args.get(i).and_then(|s| parse_id(s)); if uid.is_none() { die("useradd: -u needs a number below 65535"); } }
            "-g" => { i += 1; gid = args.get(i).and_then(|s| parse_id(s)); if gid.is_none() { die("useradd: -g needs a number below 65535"); } }
            "-G" => {
                i += 1;
                match args.get(i) {
                    Some(list) => extra_groups = list.split(',').filter(|s| !s.is_empty()).collect(),
                    None => die("useradd: -G needs a group list"),
                }
            }
            "-s" => { i += 1; shell = args.get(i).copied().unwrap_or_else(|| die("useradd: -s needs a path")); }
            "-d" => { i += 1; home = args.get(i).map(|s| s.to_string()); }
            "-D" | "--delete" => delete = true,
            "-h" | "--help" => {
                println("usage: useradd [-u uid] [-g gid] [-G group,...] [-s shell] [-d home] name");
                println("       useradd -D name");
                miku::exit(0);
            }
            other if other.starts_with('-') => die("useradd: unknown option"),
            other => name = Some(other),
        }
        i += 1;
    }

    let name = match name {
        Some(n) => n,
        None => die("useradd: no name given"),
    };
    if !valid_name(name) {
        die("useradd: a name must be letters, digits, '_', '-' or '.', and must not start with '-' or '.'");
    }

    // Checks privilege before writing anything
    if miku::cred::geteuid() != 0 {
        die("useradd: this needs root - try `mikudo useradd ...`");
    }

    let passwd = miku::read_file_string(mikuauth::PASSWD_PATH).unwrap_or_default();
    let group = miku::read_file_string(mikuauth::GROUP_PATH).unwrap_or_default();
    let shadow = miku::read_file_string(mikuauth::SHADOW_PATH).unwrap_or_default();
    if passwd.is_empty() {
        die("useradd: /etc/passwd cannot be read");
    }

    if delete {
        do_delete(name, &passwd, &group, &shadow);
    }

    if db::find_user(&passwd, name).is_some() {
        die(&format!("useradd: {name} already exists"));
    }

    let uid = match uid {
        Some(u) => {
            if db::find_uid(&passwd, u).is_some() {
                die(&format!("useradd: uid {u} is already taken"));
            }
            u
        }
        None => next_free_uid(&passwd),
    };
    let gid = gid.unwrap_or(uid);
    let home = home.unwrap_or_else(|| format!("/home/{name}"));

    // Every -G group must exist
    for g in &extra_groups {
        if db::find_group(&group, g).is_none() {
            die(&format!("useradd: no such group: {g}"));
        }
    }

    // A per-user group of the same name
    let mut new_group = group.clone();
    if db::find_gid(&group, gid).is_none() {
        ensure_newline(&mut new_group);
        new_group.push_str(&format!("{name}:x:{gid}:\n"));
    }
    for g in &extra_groups {
        new_group = add_member(&new_group, g, name);
    }

    let mut new_passwd = passwd.clone();
    ensure_newline(&mut new_passwd);
    new_passwd.push_str(&format!("{name}:x:{uid}:{gid}:{name}:{home}:{shell}\n"));

    // The account starts locked, not passwordless
    let today = miku::time::realtime_secs() / 86400;
    let mut new_shadow = shadow.clone();
    ensure_newline(&mut new_shadow);
    new_shadow.push_str(&format!("{name}:!:{today}:0:99999:7:::\n"));

    write_or_die(mikuauth::PASSWD_PATH, &new_passwd, 0o644);
    write_or_die(mikuauth::GROUP_PATH, &new_group, 0o644);
    write_or_die(mikuauth::SHADOW_PATH, &new_shadow, 0o640);

    let h = cstr(&home);
    unsafe {
        miku::miku_mkdir(h.as_ptr(), 0o700);
        miku::miku_chown(h.as_ptr(), uid as u32, gid as u32);
    }

    miku::klog(&format!("[useradd] created {name} (uid {uid}, gid {gid})"));
    println(&format!("useradd: created {name} (uid {uid}, gid {gid}), home {home}"));
    println(&format!("useradd: the account is locked until you run `passwd {name}`"));
    miku::exit(0);
}

fn do_delete(name: &str, passwd: &str, group: &str, shadow: &str) -> ! {
    if db::find_user(passwd, name).is_none() {
        die(&format!("useradd: no such user: {name}"));
    }
    if name == "root" {
        die("useradd: root cannot be removed");
    }

    let new_passwd = drop_line(passwd, |l| db::Passwd::parse(l).map(|p| p.name == name).unwrap_or(false));
    let new_shadow = drop_line(shadow, |l| db::Shadow::parse(l).map(|s| s.name == name).unwrap_or(false));
    let new_group = drop_line(group, |l| db::Group::parse(l).map(|g| g.name == name).unwrap_or(false));
    let new_group = remove_member(&new_group, name);

    write_or_die(mikuauth::PASSWD_PATH, &new_passwd, 0o644);
    write_or_die(mikuauth::GROUP_PATH, &new_group, 0o644);
    write_or_die(mikuauth::SHADOW_PATH, &new_shadow, 0o640);

    // The home directory stays when an account is removed
    miku::klog(&format!("[useradd] removed {name}"));
    println(&format!("useradd: removed {name}. Their home directory was left alone"));
    miku::exit(0);
}

fn next_free_uid(passwd: &str) -> u16 {
    let mut u = FIRST_USER_UID;
    while u <= LAST_USER_UID {
        if db::find_uid(passwd, u).is_none() {
            return u;
        }
        u += 1;
    }
    die("useradd: no free uid");
}

fn add_member(group: &str, gname: &str, user: &str) -> String {
    let mut out = String::new();
    for line in group.lines() {
        let t = line.trim_start();
        if !t.starts_with('#') && !t.is_empty() {
            if let Ok(g) = db::Group::parse(line) {
                if g.name == gname && !g.has_member(user) {
                    let sep = if g.member_list.is_empty() { "" } else { "," };
                    out.push_str(&format!("{}:x:{}:{}{}{}\n", g.name, g.gid, g.member_list, sep, user));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn remove_member(group: &str, user: &str) -> String {
    let mut out = String::new();
    for line in group.lines() {
        let t = line.trim_start();
        if !t.starts_with('#') && !t.is_empty() {
            if let Ok(g) = db::Group::parse(line) {
                if g.has_member(user) {
                    let kept: Vec<&str> = g.members().filter(|m| *m != user).collect();
                    out.push_str(&format!("{}:x:{}:{}\n", g.name, g.gid, kept.join(",")));
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn drop_line<F: Fn(&str) -> bool>(text: &str, drop: F) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        if !t.starts_with('#') && !t.is_empty() && drop(line) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Writes through a temporary and rename, which is atomic
fn write_or_die(path: &str, text: &str, mode: u32) {
    let tmp = format!("{path}.new");
    if !miku::write_file(&tmp, text.as_bytes()) {
        die(&format!(
            "useradd: cannot write {tmp}. The root filesystem is read-only - boot with \
             'root=<drive>:<lba> rootrw' for a persistent /etc, or run 'fs.rw' at the console"
        ));
    }
    let t = cstr(&tmp);
    let d = cstr(path);
    // Sets the mode before the rename
    unsafe { miku::miku_chmod(t.as_ptr(), mode) };
    if unsafe { miku::miku_rename(t.as_ptr(), d.as_ptr()) } < 0 {
        unsafe { miku::miku_unlink(t.as_ptr()) };
        die(&format!("useradd: cannot replace {path}"));
    }
}

fn ensure_newline(s: &mut String) {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
}

fn parse_id(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        n = n * 10 + (b - b'0') as u32;
        if n > 0xFFFE { return None; }
    }
    Some(n as u16)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && !s.starts_with('-')
        && !s.starts_with('.')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn println(s: &str) { miku::write(1, s.as_bytes()); miku::write(1, b"\n"); }

fn die(msg: &str) -> ! {
    miku::write(2, msg.as_bytes());
    miku::write(2, b"\n");
    miku::exit(1);
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
