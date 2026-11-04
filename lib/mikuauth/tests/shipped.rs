// The shipped configuration files, parsed by the code that reads them at boot

use mikuauth::conf::Config;
use mikuauth::db::{self, Group, Passwd, Shadow};
use mikuauth::hash::{verify, Verdict};

fn etc(name: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../etc/");
    std::fs::read_to_string(format!("{path}{name}"))
        .unwrap_or_else(|e| panic!("cannot read etc/{name}: {e}"))
}

#[test]
fn the_shipped_passwd_parses_and_holds_the_five_expected_accounts() {
    let text = etc("passwd");
    let users: Vec<Passwd> = db::Lines::new(&text)
        .map(|(n, l)| Passwd::parse(l).unwrap_or_else(|e| panic!("passwd line {n}: {e:?}")))
        .collect();
    assert_eq!(users.len(), 5);
    assert_eq!(users[0].name, "root");
    assert_eq!(users[0].uid, 0);
    assert_eq!(users[1].name, "miku");
    assert_eq!(users[1].uid, 1000);
    // Field seven is the session program
    assert_eq!(users[1].shell, "/bin/maysession");

    let owner = users.iter().find(|u| u.name == "alunwrd").expect("no owner account");
    assert_eq!(owner.uid, 1001);

    let greeter = users.iter().find(|u| u.name == "greeter").expect("no greeter account");
    assert_eq!(greeter.uid, 990);
    assert_eq!(greeter.shell, "/bin/false", "the login screen must not be a session");

    let nobody = users.iter().find(|u| u.name == "nobody").expect("no nobody account");
    assert_eq!(nobody.uid, 65534);
    assert_eq!(nobody.shell, "/bin/false", "nobody must not get a shell");
}

#[test]
fn only_the_greeter_may_ask_for_a_session() {
    // The group mikudod requires of a login client; it does not grant authority
    // over other users
    let text = etc("group");
    let groups: Vec<Group> = db::Lines::new(&text).filter_map(|(_, l)| Group::parse(l).ok()).collect();
    let session = groups.iter().find(|g| g.name == "session").expect("no session group");
    assert!(session.has_member("greeter"));
    assert!(!session.has_member("miku"), "an ordinary account is not a login prompt");
    assert!(!session.has_member("nobody"));
}

#[test]
fn the_greeter_account_cannot_be_logged_into() {
    // The greeter's account cannot be logged into
    let shadow = etc("shadow");
    let greeter = db::find_shadow(&shadow, "greeter").expect("no greeter shadow line");
    for pw in [b"".as_slice(), b"!", b"greeter", b"miku"] {
        assert_eq!(verify(pw, greeter.hash), Verdict::Locked);
    }
}

#[test]
fn no_two_shipped_accounts_share_a_uid_or_a_name() {
    // Two names on one uid are never intended in a shipped file
    let text = etc("passwd");
    let users: Vec<Passwd> = db::Lines::new(&text).filter_map(|(_, l)| Passwd::parse(l).ok()).collect();
    for (i, a) in users.iter().enumerate() {
        for b in &users[i + 1..] {
            assert_ne!(a.uid, b.uid, "{} and {} share a uid", a.name, b.name);
            assert_ne!(a.name, b.name, "duplicate name {}", a.name);
        }
    }
}

#[test]
fn the_shipped_group_file_parses_and_puts_miku_in_wheel() {
    let text = etc("group");
    let groups: Vec<Group> = db::Lines::new(&text)
        .map(|(n, l)| Group::parse(l).unwrap_or_else(|e| panic!("group line {n}: {e:?}")))
        .collect();
    let wheel = groups.iter().find(|g| g.name == "wheel").expect("no wheel group");
    assert_eq!(wheel.gid, 10);
    assert!(wheel.has_member("miku"), "miku must be able to use mikudo out of the box");
    assert!(!wheel.has_member("nobody"));
}

#[test]
fn every_shipped_passwd_gid_names_a_real_group() {
    // A primary gid with no group has an unprintable owner, and a ':gid' rule
    // would never match
    let passwd = etc("passwd");
    let group = etc("group");
    for (n, line) in db::Lines::new(&passwd) {
        let p = Passwd::parse(line).unwrap();
        assert!(
            db::find_gid(&group, p.gid).is_some(),
            "passwd line {n}: {} has gid {} and no group has it",
            p.name, p.gid
        );
    }
}

#[test]
fn every_account_has_a_shadow_line_and_the_reverse() {
    // Every passwd entry has a shadow line and vice versa
    let passwd = etc("passwd");
    let shadow = etc("shadow");
    for (_, line) in db::Lines::new(&passwd) {
        let p = Passwd::parse(line).unwrap();
        assert!(db::find_shadow(&shadow, p.name).is_some(), "{} has no shadow line", p.name);
    }
    for (n, line) in db::Lines::new(&shadow) {
        let s = Shadow::parse(line).unwrap_or_else(|e| panic!("shadow line {n}: {e:?}"));
        assert!(db::find_user(&passwd, s.name).is_some(), "shadow line {n}: no account {}", s.name);
    }
}

#[test]
fn root_is_locked_in_the_shipped_shadow() {
    // root cannot log in; the way to root is mikudo, which is journaled
    let shadow = etc("shadow");
    let root = db::find_shadow(&shadow, "root").expect("no root line");
    assert_eq!(verify(b"", root.hash), Verdict::Locked);
    assert_eq!(verify(b"root", root.hash), Verdict::Locked);
    assert_eq!(verify(b"toor", root.hash), Verdict::Locked);

    let nobody = db::find_shadow(&shadow, "nobody").expect("no nobody line");
    assert_eq!(verify(b"", nobody.hash), Verdict::Locked);
}

#[test]
fn the_shipped_miku_password_is_the_documented_one() {
    // Default password is 'miku', documented in docs/MikuOS_Security.md and
    // announced by login until changed
    let shadow = etc("shadow");
    let miku = db::find_shadow(&shadow, "miku").expect("no miku line");
    assert_eq!(verify(b"miku", miku.hash), Verdict::Ok);
    assert_eq!(verify(b"Miku", miku.hash), Verdict::Wrong);
    assert_eq!(verify(b"", miku.hash), Verdict::Wrong);
    assert!(miku.hash.starts_with("$argon2id$v=19$m=19456,t=3,p=1$"));
}

#[test]
fn no_account_is_left_without_a_password_by_accident() {
    // An empty hash means no password; nothing ships that way
    let shadow = etc("shadow");
    for (n, line) in db::Lines::new(&shadow) {
        let s = Shadow::parse(line).unwrap();
        assert_ne!(
            verify(b"", s.hash),
            Verdict::NoPassword,
            "shadow line {n}: {} has no password at all",
            s.name
        );
    }
}

#[test]
fn the_shipped_mikudo_conf_parses_and_means_what_it_says() {
    let text = etc("mikudo.conf");
    let c = Config::parse(&text).unwrap_or_else(|e| panic!("mikudo.conf: {e:?}"));
    assert_eq!(c.rules.len(), 1, "only the wheel rule should be uncommented");

    let r = &c.rules[0];
    assert!(r.options.persist);
    assert!(!r.options.nopass, "the shipped rule must ask for a password");
    assert!(!r.options.keepenv, "the shipped rule must not pass the environment through");

    // And it decides the way the comment at the top of the file claims
    let groups = etc("group");
    let mut gids = [0u16; 16];
    let n = db::groups_of(&groups, "miku", 1000, &mut gids);
    let names = ["miku", "wheel", "session", "users"];
    let allowed = c.decide(&mikuauth::conf::Request {
        user: "miku", uid: 1000, gid: 1000,
        groups: &gids[..n], group_names: &names,
        target: "root", cmd: "/bin/id", args: &[],
    });
    assert!(allowed.permitted());

    let stranger = c.decide(&mikuauth::conf::Request {
        user: "nobody", uid: 65534, gid: 65534,
        groups: &[65534], group_names: &["nobody"],
        target: "root", cmd: "/bin/id", args: &[],
    });
    assert!(!stranger.permitted(), "nobody must not be able to become root");
}
