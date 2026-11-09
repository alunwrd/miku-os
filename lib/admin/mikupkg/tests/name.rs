// What a package may be called; checked once, at the only way in

use mikupkg::name::{Name, NameError, MAX_NAME};

#[test]
fn the_names_this_tree_will_actually_use() {
    for s in [
        "miku-base",
        "miku-utils",
        "miku-system",
        "mayterm",
        "libmay",
        "ca-certificates",
        "gcc-14",
        "lib32-x264",
        "a",
        "7zip",
    ] {
        let n = Name::parse(s).unwrap_or_else(|e| panic!("{s:?} should be a name: {e}"));
        assert_eq!(n.as_str(), s);
        assert_eq!(n.to_string(), s);
    }
}

#[test]
fn what_is_refused_and_why() {
    let cases: &[(&str, NameError)] = &[
        ("", NameError::Empty),
        ("Miku", NameError::UpperCase { at: 0, byte: b'M' }),
        ("miku base", NameError::BadByte { at: 4, byte: b' ' }),
        ("../etc/passwd", NameError::BadByte { at: 2, byte: b'/' }),
        ("miku*", NameError::BadByte { at: 4, byte: b'*' }),
        ("-miku", NameError::BadEdge { at: 0, byte: b'-' }),
        ("miku-", NameError::BadEdge { at: 4, byte: b'-' }),
        (".hidden", NameError::BadEdge { at: 0, byte: b'.' }),
    ];
    for (text, want) in cases {
        assert_eq!(
            Name::parse(text),
            Err(*want),
            "{text:?} should be refused as {want:?}"
        );
    }
}

/// The error states the limit
#[test]
fn a_name_stops_at_the_limit() {
    let ok = "a".repeat(MAX_NAME);
    assert!(Name::parse(&ok).is_ok());

    let too_long = "a".repeat(MAX_NAME + 1);
    assert_eq!(
        Name::parse(&too_long),
        Err(NameError::TooLong { len: MAX_NAME + 1 })
    );
    let said = std::format!("{}", Name::parse(&too_long).unwrap_err());
    assert!(
        said.contains(&std::format!("{MAX_NAME} bytes")),
        "the refusal should state the limit: {said:?}"
    );
}

#[test]
fn names_order_and_compare_by_their_text() {
    let a = Name::parse("miku-base").unwrap();
    let b = Name::parse("miku-utils").unwrap();
    assert!(a < b);
    assert_eq!(a, Name::parse("miku-base").unwrap());
    assert_ne!(a, b);

    use std::collections::HashSet;
    let mut set = HashSet::new();
    assert!(set.insert(a));
    assert!(!set.insert(Name::parse("miku-base").unwrap()), "one name, one entry");
    assert!(set.insert(b));
}
