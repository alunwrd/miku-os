// /etc/passwd, /etc/group, and /etc/shadow against malformed input

use mikuauth::db::{self, DbError, Group, Passwd, Shadow};

// passwd

#[test]
fn an_ordinary_line_parses_into_its_fields() {
    let p = Passwd::parse("miku:x:1000:1000:MikuOS user:/home/miku:/bin/mayterm").unwrap();
    assert_eq!(p.name, "miku");
    assert_eq!(p.uid, 1000);
    assert_eq!(p.gid, 1000);
    assert_eq!(p.gecos, "MikuOS user");
    assert_eq!(p.home, "/home/miku");
    assert_eq!(p.shell, "/bin/mayterm");
}

#[test]
fn a_short_line_is_refused_rather_than_padded() {
    // Too few fields is refused. An empty gid read as zero would put the
    // account in group root
    assert_eq!(Passwd::parse("miku:x:1000"), Err(DbError::TooFewFields));
    assert_eq!(Passwd::parse("miku:x:1000:1000:gecos:/home/miku"), Err(DbError::TooFewFields));
    assert_eq!(Passwd::parse(""), Err(DbError::TooFewFields));
}

#[test]
fn a_colon_in_the_gecos_does_not_become_a_shell() {
    // Too many fields is refused. `splitn(7)` would read the shell from the
    // gecos
    assert_eq!(
        Passwd::parse("ada:x:1001:1001:Ada:Lovelace:/home/ada:/bin/mayterm"),
        Err(DbError::TooManyFields)
    );
}

#[test]
fn a_uid_that_does_not_fit_is_refused_not_wrapped() {
    // 65536 truncated to 16 bits is uid 0
    assert_eq!(Passwd::parse("big:x:65536:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    assert_eq!(Passwd::parse("big:x:4294967296:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    // 65535 is chown's sentinel, not a user
    assert_eq!(Passwd::parse("s:x:65535:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    // 65534 is nobody and is perfectly legal
    assert!(Passwd::parse("nobody:x:65534:65534:nobody:/:/bin/false").is_ok());
}

#[test]
fn a_negative_or_empty_uid_is_refused() {
    // CVE-2019-14287. Refused as a non-digit; there is no signed parse
    assert_eq!(Passwd::parse("x:x:-1:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    assert_eq!(Passwd::parse("x:x::100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    assert_eq!(Passwd::parse("x:x:+0:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    assert_eq!(Passwd::parse("x:x:0x10:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
    assert_eq!(Passwd::parse("x:x: 100:100:g:/h:/bin/mayterm"), Err(DbError::BadId));
}

#[test]
fn a_name_that_could_be_read_as_something_else_is_refused() {
    // ':' ends a field, ',' separates members, '$' opens a hash, a leading '-'
    // reads as an option
    for bad in [
        "ro:ot:x:0:0:g:/h:/bin/mayterm",
        "ro,ot:x:0:0:g:/h:/bin/mayterm",
        "-r:x:0:0:g:/h:/bin/mayterm",
        ".hidden:x:0:0:g:/h:/bin/mayterm",
        "ro ot:x:0:0:g:/h:/bin/mayterm",
        "ro/ot:x:0:0:g:/h:/bin/mayterm",
        ":x:0:0:g:/h:/bin/mayterm",
    ] {
        assert!(Passwd::parse(bad).is_err(), "should have refused {bad:?}");
    }
}

#[test]
fn an_empty_home_or_shell_gets_a_defined_default() {
    // Empty is legal here and means the default; an empty string must not reach
    // exec
    let p = Passwd::parse("svc:x:990:990:::").unwrap();
    assert_eq!(p.home, "/");
    assert_eq!(p.shell, mikuauth::db::DEFAULT_SESSION);
}

#[test]
fn a_line_longer_than_the_cap_is_skipped_by_the_iterator() {
    let long = format!("miku:x:1000:1000:{}:/h:/bin/mayterm", "A".repeat(100_000));
    let text = format!("root:x:0:0:root:/root:/bin/mayterm\n{long}\n");
    let names: Vec<&str> = db::Lines::new(&text)
        .filter_map(|(_, l)| Passwd::parse(l).ok())
        .map(|p| p.name)
        .collect();
    // Oversized line dropped, good line kept; truncating it would give a
    // different account
    assert_eq!(names, vec!["root"]);
}

#[test]
fn a_field_longer_than_the_cap_is_refused() {
    let line = format!("miku:x:1000:1000:{}:/h:/bin/mayterm", "A".repeat(300));
    assert_eq!(Passwd::parse(&line), Err(DbError::TooLong));
}

// The file as bytes

#[test]
fn a_nul_byte_makes_the_whole_file_unreadable() {
    // A NUL splits what a C reader and this parser see
    assert_eq!(db::as_text(b"root:x:0:0:r:/root:/bin/mayterm\n\0evil:x:0:0:e:/:/bin/mayterm\n"),
               Err(DbError::BadBytes));
}

#[test]
fn invalid_utf8_makes_the_whole_file_unreadable() {
    assert_eq!(db::as_text(&[0xff, 0xfe, 0x00]), Err(DbError::BadBytes));
    assert!(db::as_text(b"root:x:0:0:r:/root:/bin/mayterm\n").is_ok());
}

#[test]
fn comments_blank_lines_and_crlf_are_handled() {
    // The CR of CRLF would otherwise end the shell path
    let text = "# accounts\n\nroot:x:0:0:root:/root:/bin/mayterm\r\n\n#miku:x:1:1:x:/:/x\n";
    let users: Vec<_> = db::Lines::new(text)
        .filter_map(|(_, l)| Passwd::parse(l).ok())
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].shell, "/bin/mayterm", "a stray CR must not reach exec");
}

#[test]
fn a_bad_line_does_not_hide_the_good_ones_after_it() {
    let text = "root:x:0:0:root:/root:/bin/mayterm\nbroken\nmiku:x:1000:1000:m:/home/miku:/bin/mayterm\n";
    let users: Vec<&str> = db::Lines::new(text)
        .filter_map(|(_, l)| Passwd::parse(l).ok())
        .map(|p| p.name)
        .collect();
    assert_eq!(users, vec!["root", "miku"]);
}

// group

#[test]
fn group_membership_is_read_from_the_comma_list() {
    let g = Group::parse("wheel:x:10:miku,ada").unwrap();
    assert_eq!(g.gid, 10);
    assert!(g.has_member("miku"));
    assert!(g.has_member("ada"));
    assert!(!g.has_member("mik"), "a prefix is not a member");
    assert!(!g.has_member("miku,ada"), "the whole list is not one member");
    assert!(!g.has_member(""), "an empty name matches nothing");
}

#[test]
fn a_group_with_no_members_has_none() {
    let g = Group::parse("shadow:x:42:").unwrap();
    assert_eq!(g.members().count(), 0);
    assert!(!g.has_member(""));
}

#[test]
fn trailing_and_doubled_commas_do_not_create_an_empty_member() {
    // An empty member name must not match an empty user name
    let g = Group::parse("wheel:x:10:miku,,ada,").unwrap();
    let members: Vec<&str> = g.members().collect();
    assert_eq!(members, vec!["miku", "ada"]);
}

#[test]
fn groups_of_collects_the_primary_and_every_secondary() {
    let groups = "root:x:0:\nwheel:x:10:miku\nusers:x:100:miku,ada\nvideo:x:44:ada\n";
    let mut out = [0u16; 8];
    let n = db::groups_of(groups, "miku", 1000, &mut out);
    assert_eq!(&out[..n], &[1000, 10, 100]);

    // A user in no group line gets only the primary group
    let n = db::groups_of(groups, "nobody", 65534, &mut out);
    assert_eq!(&out[..n], &[65534]);
}

#[test]
fn groups_of_does_not_write_past_the_buffer() {
    // Limited to MAX_GROUPS, which is the array's size
    let mut text = String::new();
    for i in 0..64 {
        text.push_str(&format!("g{i}:x:{}:miku\n", 200 + i));
    }
    let mut out = [0u16; 4];
    let n = db::groups_of(&text, "miku", 1000, &mut out);
    assert_eq!(n, 4);
}

#[test]
fn the_primary_group_is_not_listed_twice() {
    let groups = "wheel:x:10:miku\n";
    let mut out = [0u16; 8];
    let n = db::groups_of(groups, "miku", 10, &mut out);
    assert_eq!(&out[..n], &[10]);
}

// shadow

#[test]
fn a_shadow_line_parses_and_keeps_its_hash_intact() {
    let line = "miku:$argon2id$v=19$m=65536,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$dGFn:19000:0:99999:7:::";
    let s = Shadow::parse(line).unwrap();
    assert_eq!(s.name, "miku");
    assert!(s.hash.starts_with("$argon2id$"));
    assert_eq!(s.last_change, Some(19000));
    assert_eq!(s.max_days, Some(99999));
    // Empty trailing fields mean not set, not zero
    assert_eq!(s.inactive_days, None);
    assert_eq!(s.expires, None);
}

#[test]
fn an_empty_date_is_not_day_zero() {
    // Day 0 is 1970-01-01; an empty expiry read as zero would refuse every
    // login
    let s = Shadow::parse("miku:!:::::::").unwrap();
    assert_eq!(s.expires, None);
    assert!(s.usable_on(20000));
}

#[test]
fn an_expiry_in_the_past_makes_the_account_unusable() {
    let s = Shadow::parse("old:!:100:0:99999:7::19000:").unwrap();
    assert!(s.usable_on(18999));
    assert!(!s.usable_on(19000), "the expiry day itself is too late");
    assert!(!s.usable_on(19001));
}

#[test]
fn a_locked_account_has_no_password_that_matches() {
    use mikuauth::hash::{verify, Verdict};
    // root's line: a hash field no password can produce
    assert_eq!(verify(b"", "!"), Verdict::Locked);
    assert_eq!(verify(b"anything", "!"), Verdict::Locked);
    assert_eq!(verify(b"anything", "*"), Verdict::Locked);
    // A locked account whose old hash is kept after the '!' is still locked
    assert_eq!(verify(b"anything", "!$argon2id$v=19$m=64,t=1,p=1$c2FsdHNhbHQ$dGFn"), Verdict::Locked);
}

#[test]
fn an_empty_hash_field_is_no_password_and_not_a_match() {
    use mikuauth::hash::{verify, Verdict};
    // "No password set" and "the empty password was correct" must differ
    assert_eq!(verify(b"", ""), Verdict::NoPassword);
    assert_eq!(verify(b"guess", ""), Verdict::NoPassword);
}

#[test]
fn a_hash_field_that_is_not_a_hash_never_matches() {
    use mikuauth::hash::{verify, Verdict};
    for junk in [
        "notahash",
        "$",
        "$argon2id",
        "$argon2id$v=19$m=65536,t=3,p=1$",
        "$argon2id$v=19$m=65536,t=3,p=1$salt",
        "$argon2id$v=19$m=65536,t=3,p=1$salt$tag$extra",
        "$argon2i$v=19$m=65536,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$dGFn",
        "$argon2id$v=16$m=65536,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$dGFn",
        "$argon2id$v=19$m=65536,t=3,p=1,k=9$c2FsdHNhbHRzYWx0c2FsdA$dGFn",
        "$argon2id$v=19$m=,t=3,p=1$c2FsdHNhbHRzYWx0c2FsdA$dGFn",
        "$argon2id$v=19$m=65536,t=3,p=1$!!!!$dGFn",
    ] {
        assert_eq!(verify(b"password", junk), Verdict::Wrong, "junk: {junk:?}");
    }
}

#[test]
fn a_round_trip_verifies_and_a_near_miss_does_not() {
    use mikuauth::argon2::Params;
    use mikuauth::hash::{encode_into, verify, Verdict};
    let params = Params { m_cost: 64, t_cost: 1, p_cost: 1 };
    let mut buf = [0u8; 256];
    let field = encode_into(b"correct horse", b"saltsaltsaltsalt", params, &mut buf)
        .unwrap()
        .to_owned();

    assert_eq!(verify(b"correct horse", &field), Verdict::Ok);
    assert_eq!(verify(b"correct hors", &field), Verdict::Wrong);
    assert_eq!(verify(b"correct horse ", &field), Verdict::Wrong);
    assert_eq!(verify(b"Correct horse", &field), Verdict::Wrong);
    assert_eq!(verify(b"", &field), Verdict::Wrong);

    // A stored field survives a round trip through a shadow line
    let line = format!("miku:{field}:19000:0:99999:7:::");
    let s = Shadow::parse(&line).unwrap();
    assert_eq!(s.hash, field);
    assert_eq!(verify(b"correct horse", s.hash), Verdict::Ok);
}

#[test]
fn a_password_may_contain_anything_a_user_can_type() {
    use mikuauth::argon2::Params;
    use mikuauth::hash::{encode_into, verify, Verdict};
    let params = Params { m_cost: 64, t_cost: 1, p_cost: 1 };
    for pw in [
        "with:colons".as_bytes(),
        "with$dollars".as_bytes(),
        "with spaces and\ttabs".as_bytes(),
        "пароль".as_bytes(),
        "🔐".as_bytes(),
        &[0xffu8, 0x00, 0x41][..],
    ] {
        let mut buf = [0u8; 256];
        let field = encode_into(pw, b"saltsaltsaltsalt", params, &mut buf).unwrap().to_owned();
        assert_eq!(verify(pw, &field), Verdict::Ok, "{pw:?}");
        assert_eq!(verify(b"other", &field), Verdict::Wrong);
    }
}

#[test]
fn lookups_find_the_right_record_and_no_other() {
    let passwd = "root:x:0:0:root:/root:/bin/mayterm\nmiku:x:1000:1000:m:/home/miku:/bin/mayterm\n";
    assert_eq!(db::find_user(passwd, "miku").unwrap().uid, 1000);
    assert_eq!(db::find_uid(passwd, 0).unwrap().name, "root");
    assert!(db::find_user(passwd, "mik").is_none());
    assert!(db::find_user(passwd, "").is_none());
    assert!(db::find_uid(passwd, 999).is_none());
}
