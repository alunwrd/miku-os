#![no_std]
#![no_main]

//! id: prints identity and capability set
//!
//! Usage: id [-u] [-g] [-G] [-n] [user]

extern crate alloc;

pub use miku;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

use mikuauth::db;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut only_uid = false;
    let mut only_gid = false;
    let mut only_groups = false;
    let mut names = false;
    let mut who: Option<&str> = None;

    for a in args.iter().skip(1) {
        match *a {
            "-u" => only_uid = true,
            "-g" => only_gid = true,
            "-G" => only_groups = true,
            "-n" => names = true,
            "-h" | "--help" => {
                println("usage: id [-u] [-g] [-G] [-n] [user]");
                miku::exit(0);
            }
            other if other.starts_with('-') => {
                eprintln(&format!("id: unknown option {other}"));
                miku::exit(2);
            }
            other => who = Some(other),
        }
    }

    let passwd = miku::read_file_string(mikuauth::PASSWD_PATH).unwrap_or_default();
    let group = miku::read_file_string(mikuauth::GROUP_PATH).unwrap_or_default();

    // Another user comes from the files; yourself from the kernel, which knows
    // the current identity
    let (uid, gid, name) = match who {
        Some(n) => match db::find_user(&passwd, n) {
            Some(p) => (p.uid, p.gid, p.name.to_string()),
            None => {
                eprintln(&format!("id: no such user: {n}"));
                miku::exit(1);
            }
        },
        None => {
            let uid = miku::cred::getuid();
            let gid = miku::cred::getgid();
            let name = db::find_uid(&passwd, uid)
                .map(|p| p.name.to_string())
                .unwrap_or_else(|| uid.to_string());
            (uid, gid, name)
        }
    };

    let gids: Vec<u16> = if who.is_some() {
        let mut buf = [0u16; miku::cred::MAX_GROUPS];
        let n = db::groups_of(&group, &name, gid, &mut buf);
        buf[..n].to_vec()
    } else {
        let mut buf = [0u16; miku::cred::MAX_GROUPS];
        match miku::cred::getgroups(&mut buf) {
            Ok(n) if n > 0 => buf[..n].to_vec(),
            _ => alloc::vec![gid],
        }
    };

    if only_uid {
        println(&if names { name.clone() } else { uid.to_string() });
        miku::exit(0);
    }
    if only_gid {
        println(&if names { gname(&group, gid) } else { gid.to_string() });
        miku::exit(0);
    }
    if only_groups {
        let mut out = String::new();
        for (i, g) in gids.iter().enumerate() {
            if i > 0 { out.push(' '); }
            out.push_str(&if names { gname(&group, *g) } else { g.to_string() });
        }
        println(&out);
        miku::exit(0);
    }

    let mut line = format!("uid={uid}({name}) gid={gid}({})", gname(&group, gid));
    if !gids.is_empty() {
        line.push_str(" groups=");
        for (i, g) in gids.iter().enumerate() {
            if i > 0 { line.push(','); }
            line.push_str(&format!("{g}({})", gname(&group, *g)));
        }
    }
    println(&line);

    // Session and capabilities belong to a running process, so only for
    // yourself
    if who.is_none() {
        // Session of this process, found by the kernel through the parent chain
        let session = miku::session::current();
        if session != 0 {
            println(&format!("session={session}"));
        } else {
            // Outside any session: a service, or started before login
            println("session=none");
        }
        if let Ok((permitted, effective, bounding)) = miku::cred::capget() {
            if effective == 0 && permitted == 0 {
                println("caps=none");
            } else {
                println(&format!("caps={}", cap_names(effective)));
                if permitted != effective {
                    println(&format!("  held but not in force: {}", cap_names(permitted & !effective)));
                }
            }
            if bounding != ALL_CAPS {
                println(&format!("  ceiling: {}", cap_names(bounding)));
            }
        }
    }
    miku::exit(0);
}

const ALL_CAPS: u64 = (1u64 << 16) - 1;

const CAP_NAMES: [&str; 16] = [
    "setuid", "setgid", "kill", "dac_override", "dac_read", "fowner", "chown",
    "sys_time", "sys_admin", "sys_boot", "net_raw", "net_bind", "mknod",
    "may_priv", "audit", "gpu",
];

fn cap_names(set: u64) -> String {
    if set == ALL_CAPS {
        return "all".to_string();
    }
    let mut out = String::new();
    for (i, n) in CAP_NAMES.iter().enumerate() {
        if set & (1u64 << i) != 0 {
            if !out.is_empty() { out.push(','); }
            out.push_str(n);
        }
    }
    if out.is_empty() { "none".to_string() } else { out }
}

fn gname(group: &str, gid: u16) -> String {
    db::find_gid(group, gid)
        .map(|g| g.name.to_string())
        .unwrap_or_else(|| gid.to_string())
}

fn println(s: &str) {
    miku::write(1, s.as_bytes());
    miku::write(1, b"\n");
}

fn eprintln(s: &str) {
    miku::write(2, s.as_bytes());
    miku::write(2, b"\n");
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
