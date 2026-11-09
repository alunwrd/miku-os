// /etc/mikudo.conf: the grammar and invalid input. Valid-rule cases follow
// doas.conf(5); a file with an error anywhere must be refused whole

use mikuauth::conf::{Action, Config, ConfError, Ident, Request, SyntaxError};

fn req<'a>(
    user: &'a str,
    uid: u16,
    group_names: &'a [&'a str],
    groups: &'a [u16],
    target: &'a str,
    cmd: &'a str,
    args: &'a [&'a str],
) -> Request<'a> {
    Request { user, uid, gid: groups[0], groups, group_names, target, cmd, args }
}

// The grammar

#[test]
fn the_shipped_default_lets_wheel_become_anybody_after_a_password() {
    let c = Config::parse("# the default\npermit persist :wheel\n").unwrap();
    assert_eq!(c.rules.len(), 1);
    let r = &c.rules[0];
    assert_eq!(r.action, Action::Permit);
    assert_eq!(r.ident, Ident::Group("wheel".into()));
    assert!(r.options.persist);
    assert!(!r.options.nopass, "the default must still ask for a password");
    assert_eq!(r.target, None);
    assert_eq!(r.cmd, None);

    let d = c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/id", &[]));
    assert!(d.permitted());
    assert_eq!(d.line, 2);
}

#[test]
fn no_rule_at_all_denies() {
    // An empty file grants nothing
    let c = Config::parse("").unwrap();
    let d = c.decide(&req("miku", 1000, &["miku"], &[1000], "root", "/bin/id", &[]));
    assert!(!d.permitted());
    assert_eq!(d.line, 0, "line 0 marks the default denial rather than a rule");
}

#[test]
fn a_user_who_matches_no_rule_is_denied() {
    let c = Config::parse("permit :wheel\n").unwrap();
    let d = c.decide(&req("ada", 1001, &["ada"], &[1001], "root", "/bin/id", &[]));
    assert!(!d.permitted());
}

#[test]
fn the_last_matching_rule_wins_not_the_most_specific() {
    // Last match wins
    let c = Config::parse("permit :wheel\ndeny miku cmd /bin/mkfs\n").unwrap();
    let allowed = c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/id", &[]));
    assert!(allowed.permitted());
    let denied = c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/mkfs", &[]));
    assert!(!denied.permitted());
    assert_eq!(denied.line, 2);

    // ...and the reverse order really does reverse the answer
    let c = Config::parse("deny miku cmd /bin/mkfs\npermit :wheel\n").unwrap();
    let d = c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/mkfs", &[]));
    assert!(d.permitted(), "the later blanket permit wins");
}

#[test]
fn every_identity_form_is_understood() {
    let c = Config::parse(
        "permit miku\npermit 1001\npermit :wheel\npermit :10\n",
    ).unwrap();
    assert_eq!(c.rules[0].ident, Ident::User("miku".into()));
    assert_eq!(c.rules[1].ident, Ident::Uid(1001));
    assert_eq!(c.rules[2].ident, Ident::Group("wheel".into()));
    assert_eq!(c.rules[3].ident, Ident::Gid(10));

    // A uid rule matches by number, not name
    let c = Config::parse("permit 1001\n").unwrap();
    assert!(c.decide(&req("ada", 1001, &["ada"], &[1001], "root", "/bin/id", &[])).permitted());
    assert!(!c.decide(&req("ada", 1002, &["ada"], &[1002], "root", "/bin/id", &[])).permitted());
}

#[test]
fn a_gid_rule_matches_any_of_the_callers_groups() {
    let c = Config::parse("permit :10\n").unwrap();
    // Secondary groups count too
    assert!(c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/id", &[])).permitted());
    assert!(!c.decide(&req("ada", 1001, &["ada"], &[1001], "root", "/bin/id", &[])).permitted());
}

#[test]
fn as_restricts_who_may_be_become() {
    let c = Config::parse("permit :wheel as operator\n").unwrap();
    let g = ["miku", "wheel"];
    let ids = [1000u16, 10];
    assert!(c.decide(&req("miku", 1000, &g, &ids, "operator", "/bin/id", &[])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/id", &[])).permitted());
}

#[test]
fn cmd_restricts_which_command_and_matches_the_whole_path() {
    let c = Config::parse("permit nopass :wheel cmd /sbin/fstrim\n").unwrap();
    let g = ["miku", "wheel"];
    let ids = [1000u16, 10];
    assert!(c.decide(&req("miku", 1000, &g, &ids, "root", "/sbin/fstrim", &[])).permitted());
    // Exact path only
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/sbin/fstrim2", &[])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/fstrim", &[])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/home/miku/fstrim", &[])).permitted());
}

#[test]
fn args_pins_every_argument_and_not_a_prefix() {
    // A prefix match would let "mount /mnt/usb" allow `mount /mnt/usb --bind /`
    let c = Config::parse("permit :wheel cmd /bin/mount args /mnt/usb\n").unwrap();
    let g = ["miku", "wheel"];
    let ids = [1000u16, 10];
    assert!(c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/mount", &["/mnt/usb"])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/mount", &["/mnt/usb", "--bind", "/"])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/mount", &[])).permitted());
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/mount", &["/mnt/other"])).permitted());
    // Order matters too
    let c = Config::parse("permit :wheel cmd /bin/x args a b\n").unwrap();
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/x", &["b", "a"])).permitted());
}

#[test]
fn bare_args_means_with_no_arguments_at_all() {
    // A different statement from leaving `args` out, which means "any"
    let pinned = Config::parse("permit :wheel cmd /bin/reboot args\n").unwrap();
    let any = Config::parse("permit :wheel cmd /bin/reboot\n").unwrap();
    let g = ["miku", "wheel"];
    let ids = [1000u16, 10];

    assert!(pinned.decide(&req("miku", 1000, &g, &ids, "root", "/bin/reboot", &[])).permitted());
    assert!(!pinned.decide(&req("miku", 1000, &g, &ids, "root", "/bin/reboot", &["-f"])).permitted());
    assert!(any.decide(&req("miku", 1000, &g, &ids, "root", "/bin/reboot", &["-f"])).permitted());
}

#[test]
fn every_option_is_parsed_and_carried_to_the_decision() {
    let c = Config::parse("permit nopass nolog persist keepenv :wheel\n").unwrap();
    let d = c.decide(&req("miku", 1000, &["miku", "wheel"], &[1000, 10], "root", "/bin/id", &[]));
    assert!(d.options.nopass && d.options.nolog && d.options.persist && d.options.keepenv);
}

#[test]
fn setenv_accepts_both_forms_and_either_spacing() {
    for text in [
        "permit setenv { EDITOR PAGER=less } :wheel\n",
        "permit setenv {EDITOR PAGER=less} :wheel\n",
    ] {
        let c = Config::parse(text).unwrap();
        let e = &c.rules[0].options.setenv;
        assert_eq!(e.len(), 2, "{text:?}");
        assert_eq!(e[0], ("EDITOR".into(), None), "a bare name copies the caller's value");
        assert_eq!(e[1], ("PAGER".into(), Some("less".into())));
    }
}

#[test]
fn comments_and_blank_lines_are_ignored_but_a_hash_in_setenv_is_not() {
    let c = Config::parse(
        "# a comment\n\npermit :wheel   # trailing\npermit setenv { PS1=#> } miku\n",
    ).unwrap();
    assert_eq!(c.rules.len(), 2);
    assert_eq!(c.rules[1].options.setenv[0], ("PS1".into(), Some("#>".into())));
}

#[test]
fn a_rule_can_deny_as_well_as_permit() {
    let c = Config::parse("permit :wheel\ndeny :wheel cmd /bin/rm args -rf /\n").unwrap();
    let g = ["miku", "wheel"];
    let ids = [1000u16, 10];
    assert!(!c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/rm", &["-rf", "/"])).permitted());
    assert!(c.decide(&req("miku", 1000, &g, &ids, "root", "/bin/rm", &["-rf", "/tmp/x"])).permitted());
}

// Invalid input

#[test]
fn a_syntax_error_refuses_the_whole_file_with_a_line_number() {
    // One bad line refuses the file, including rules before it
    let e = Config::parse("permit :wheel\nnonsense here\npermit ada\n").unwrap_err();
    match e {
        ConfError::Syntax { line, what } => {
            assert_eq!(line, 2);
            assert_eq!(what, SyntaxError::ExpectedAction);
        }
        other => panic!("expected a syntax error, got {other:?}"),
    }
}

#[test]
fn a_rule_with_no_identity_is_refused() {
    for text in ["permit\n", "deny\n", "permit persist\n", "permit setenv { A }\n"] {
        assert!(Config::parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn a_keyword_with_nothing_after_it_is_refused() {
    for text in [
        "permit :wheel as\n",
        "permit :wheel cmd\n",
    ] {
        match Config::parse(text).unwrap_err() {
            ConfError::Syntax { what, .. } => assert_eq!(what, SyntaxError::MissingArgument, "{text:?}"),
            o => panic!("{text:?}: {o:?}"),
        }
    }
}

#[test]
fn a_relative_command_is_refused() {
    // A bare name would depend on PATH
    for text in [
        "permit :wheel cmd mkfs\n",
        "permit :wheel cmd ./mkfs\n",
        "permit :wheel cmd ../bin/mkfs\n",
    ] {
        match Config::parse(text).unwrap_err() {
            ConfError::Syntax { what, .. } => assert_eq!(what, SyntaxError::CommandNotAbsolute, "{text:?}"),
            o => panic!("{text:?}: {o:?}"),
        }
    }
}

#[test]
fn args_without_a_command_is_refused() {
    // `args` without a command would apply to every command taking those
    // arguments
    assert!(Config::parse("permit :wheel args -rf\n").is_err());
}

#[test]
fn an_unterminated_setenv_is_refused() {
    for text in [
        "permit setenv { A B :wheel\n",
        "permit setenv A } :wheel\n",
        "permit setenv :wheel\n",
    ] {
        assert!(Config::parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn a_setenv_name_that_could_smuggle_a_second_assignment_is_refused() {
    for text in [
        "permit setenv { A=1=2 } :wheel\n",   // legal: value is "1=2"
    ] {
        assert!(Config::parse(text).is_ok(), "{text:?}");
    }
    for text in [
        "permit setenv { 1BAD=x } :wheel\n",  // a name may not start with a digit
        "permit setenv { A-B=x } :wheel\n",   // nor contain a dash
        "permit setenv { =x } :wheel\n",      // nor be empty
        "permit setenv { A.B } :wheel\n",
    ] {
        assert!(Config::parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn an_unknown_keyword_is_refused_rather_than_ignored() {
    // An unknown keyword is refused, since it may have been a restriction
    assert!(Config::parse("permit :wheel notafter 5pm\n").is_err());
    assert!(Config::parse("permit :wheel cmd /bin/id extra\n").is_err());
}

#[test]
fn an_identity_that_cannot_be_a_name_is_refused() {
    for text in [
        "permit :\n",
        "permit ro:ot\n",
        "permit -r\n",
        "permit 70000\n",     // above the id ceiling; would truncate to 4464
        "permit 65535\n",     // the chown sentinel, not a user
        "permit :70000\n",
    ] {
        assert!(Config::parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn a_numeric_looking_name_is_a_name_not_a_number() {
    // "0x10" is a legal user name, not 16. An all-digit word is always an id,
    // and one that does not fit is an error, see parse_ident_body
    let c = Config::parse("permit 0x10\n").unwrap();
    assert_eq!(c.rules[0].ident, Ident::User("0x10".into()));
    assert!(!c.decide(&req("other", 16, &["other"], &[16], "root", "/bin/id", &[])).permitted());
    assert!(c.decide(&req("0x10", 999, &["g"], &[999], "root", "/bin/id", &[])).permitted());
}

#[test]
fn a_file_larger_than_the_cap_is_refused() {
    let big = "permit :wheel\n".repeat(10_000);
    assert!(matches!(Config::parse(&big), Err(ConfError::TooLarge) | Err(ConfError::TooManyRules)));
}

#[test]
fn too_many_rules_are_refused() {
    let many = "permit :wheel\n".repeat(300);
    assert_eq!(Config::parse(&many), Err(ConfError::TooManyRules));
}

#[test]
fn a_nul_byte_refuses_the_file() {
    assert_eq!(Config::parse("permit :wheel\n\0deny miku\n"), Err(ConfError::BadBytes));
}

#[test]
fn absurd_whitespace_does_not_change_the_meaning() {
    let c = Config::parse("   \t permit   persist   :wheel   as   root  \n").unwrap();
    assert_eq!(c.rules[0].ident, Ident::Group("wheel".into()));
    assert_eq!(c.rules[0].target.as_deref(), Some("root"));
    assert!(c.rules[0].options.persist);
}

#[test]
fn rules_for_lists_what_a_user_could_ever_run() {
    // Output of `mikudo -l`, including rules that deny
    let c = Config::parse(
        "permit :wheel\ndeny :wheel cmd /bin/mkfs\npermit ada cmd /bin/id\n",
    ).unwrap();
    let mine = c.rules_for("miku", 1000, &["miku", "wheel"], &[1000, 10]);
    assert_eq!(mine.len(), 2);
    assert_eq!(mine[1].action, Action::Deny);

    let theirs = c.rules_for("ada", 1001, &["ada"], &[1001]);
    assert_eq!(theirs.len(), 1);
    assert_eq!(theirs[0].cmd.as_deref(), Some("/bin/id"));
}
