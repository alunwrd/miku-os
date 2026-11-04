// `session::authenticate`, the one place a password is checked against an
// account

use mikuauth::argon2::Params;
use mikuauth::hash;
use mikuauth::session::{authenticate, in_group, identity_of_uid, Refusal};

/// A real Argon2id hash of `password` with cheap parameters; the code reads
/// them from the string
fn hash_of(password: &str) -> String {
    let params = Params { m_cost: 64, t_cost: 1, p_cost: 1 };
    let mut out = [0u8; 256];
    hash::encode_into(password.as_bytes(), b"0123456789abcdef", params, &mut out)
        .expect("the test's own hash must encode")
        .to_string()
}

fn files() -> (String, String, String) {
    let passwd = "\
root:x:0:0:root:/root:/bin/maysession\n\
miku:x:1000:1000:Miku:/home/miku:/bin/maysession\n\
ghost:x:1001:1001:no shadow line:/home/ghost:/bin/maysession\n";
    let shadow = format!("\
root:{}:20000:0:99999:7:::\n\
miku:{}:20000:0:99999:7:::\n\
locked:!:20000:0:99999:7:::\n\
empty::20000:0:99999:7:::\n\
old:{}:1:0:10:7::20010:\n",
        hash_of("rootpw"), hash_of("mikupw"), hash_of("oldpw"));
    let group = "\
root:x:0:\n\
miku:x:1000:\n\
wheel:x:10:miku\n\
session:x:11:greeter\n";
    (passwd.to_string(), shadow, group.to_string())
}

const TODAY: u32 = 20_000;

#[test]
fn the_right_password_names_the_account_it_belongs_to() {
    let (p, s, g) = files();
    let id = authenticate(&p, &s, &g, "miku", b"mikupw", TODAY, false).expect("must be let in");
    assert_eq!(id.name, "miku");
    assert_eq!(id.uid, 1000);
    assert_eq!(id.gid, 1000);
    assert_eq!(id.home, "/home/miku");
    // Field seven is what a session starts
    assert_eq!(id.program, "/bin/maysession");
    // The primary group and the secondary one it is named in
    assert!(id.group_slice().contains(&1000));
    assert!(id.group_slice().contains(&10), "wheel membership must be collected");
}

#[test]
fn the_wrong_password_is_refused_and_says_nothing_about_the_account() {
    let (p, s, g) = files();
    let e = authenticate(&p, &s, &g, "miku", b"not it", TODAY, false).unwrap_err();
    assert_eq!(e, Refusal::WrongPassword);
    // The prompt shows the same sentence as for a missing name
    let missing = authenticate(&p, &s, &g, "nobody-here", b"not it", TODAY, false).unwrap_err();
    assert_eq!(missing, Refusal::NoSuchUser);
    assert_eq!(e.as_str(), missing.as_str());
    // The journal is allowed to know
    assert_ne!(e.detail(), missing.detail());
}

#[test]
fn one_account_cannot_be_entered_with_anothers_password() {
    let (p, s, g) = files();
    assert_eq!(
        authenticate(&p, &s, &g, "root", b"mikupw", TODAY, false).unwrap_err(),
        Refusal::WrongPassword
    );
}

#[test]
fn a_locked_account_is_locked_whatever_is_typed() {
    let (p, s, g) = files();
    for pw in [b"".as_slice(), b"!", b"anything at all"] {
        assert_eq!(
            authenticate(&p, &s, &g, "locked", pw, TODAY, true).unwrap_err(),
            Refusal::Locked,
            "a '!' field must never match"
        );
    }
}

#[test]
fn an_empty_password_field_is_the_callers_decision_and_only_the_callers() {
    let (p, s, g) = files();
    // `allow_empty`: login accepts an empty field, mikudod does not
    assert_eq!(
        authenticate(&p, &s, &g, "empty", b"", TODAY, false).unwrap_err(),
        Refusal::NoPassword,
        "a broker must never accept an empty password field"
    );
    // And even where it is accepted, there has to be an account to be
    assert_eq!(
        authenticate(&p, &s, &g, "empty", b"", TODAY, true).unwrap_err(),
        Refusal::NoSuchUser,
        "'empty' has a shadow line and no passwd line"
    );
}

#[test]
fn a_shadow_line_with_no_passwd_line_is_not_a_login() {
    // Right password, no passwd line: a refusal, so a caller cannot get uid 0
    // by default
    let (p, s, g) = files();
    assert_eq!(
        authenticate(&p, &s, &g, "old", b"oldpw", 5, false).unwrap_err(),
        Refusal::NoSuchUser
    );
}

#[test]
fn a_passwd_line_with_no_shadow_line_is_not_a_login_either() {
    // Refused as a wrong password, since the account exists. `NoSuchUser` would
    // reveal the shadow file's contents to a prompt
    let (p, s, g) = files();
    for pw in [b"".as_slice(), b"anything"] {
        assert_eq!(
            authenticate(&p, &s, &g, "ghost", pw, TODAY, true).unwrap_err(),
            Refusal::WrongPassword,
            "no shadow line means no password can be right"
        );
    }
}

#[test]
fn an_expired_account_is_refused_even_with_the_right_password() {
    let (p, s, g) = files();
    // Checked after the password: an expiry message for a wrong password would
    // reveal the account
    let mut shadow = s.clone();
    shadow.push_str(&format!("expired:{}:1:0:99999:7::10:\n", hash_of("pw")));
    let mut passwd = p.clone();
    passwd.push_str("expired:x:1002:1002::/home/expired:/bin/maysession\n");

    assert_eq!(
        authenticate(&passwd, &shadow, &g, "expired", b"pw", TODAY, false).unwrap_err(),
        Refusal::Expired
    );
    assert_eq!(
        authenticate(&passwd, &shadow, &g, "expired", b"wrong", TODAY, false).unwrap_err(),
        Refusal::WrongPassword,
        "a wrong password must not learn that the account expired"
    );
}

#[test]
fn a_uid_becomes_the_same_identity_the_password_path_produces() {
    // mikudod gets the caller from peercred and the target from a password;
    // both must produce the same shape
    let (p, s, g) = files();
    let typed = authenticate(&p, &s, &g, "miku", b"mikupw", TODAY, false).unwrap();
    let peer = identity_of_uid(&p, &g, 1000).expect("uid 1000 has an account");
    assert_eq!(typed.name, peer.name);
    assert_eq!(typed.uid, peer.uid);
    assert_eq!(typed.gid, peer.gid);
    assert_eq!(typed.group_slice(), peer.group_slice());
    assert_eq!(typed.program, peer.program);
}

#[test]
fn group_membership_counts_the_primary_group_and_the_member_list() {
    let (p, s, g) = files();
    let miku = authenticate(&p, &s, &g, "miku", b"mikupw", TODAY, false).unwrap();
    assert!(in_group(&miku, &g, "wheel"), "named in the member list");
    assert!(in_group(&miku, &g, "miku"), "the primary group counts");
    assert!(!in_group(&miku, &g, "session"), "not a member of session");
    assert!(!in_group(&miku, &g, "no-such-group"));
}

#[test]
fn the_shipped_password_is_recognised_and_nothing_else_is() {
    assert!(mikuauth::is_shipped_password(b"miku"));
    for other in [b"".as_slice(), b"mik", b"mikuu", b"Miku", b"miku "] {
        assert!(!mikuauth::is_shipped_password(other));
    }
}
