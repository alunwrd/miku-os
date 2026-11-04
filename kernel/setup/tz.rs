// The time zones the installer offers
//
// The machine's clock stays UTC: `kcore::clock` anchors it to the RTC at boot
// and to NTP afterwards. A zone is only what the person in front of it can say,
// so it is asked on the first screen beside the language, and it sets the
// offset `clock::local_from_unix` reads.
//
// A dozen fixed offsets rather than the IANA database, which has no reader in
// this tree.

/// One offer in the list
pub struct Zone {
    /// What it is called, which is what the field shows
    pub name: &'static str,
    /// The offset as it is written, shown beside the name in the open list
    pub note: &'static str,
    /// Minutes ahead of UTC
    pub minutes: i32,
}

/// In order from west to east, which is the order a list of offsets has
pub const ZONES: [Zone; 13] = [
    Zone { name: "PST", note: "UTC-08:00", minutes: -480 },
    Zone { name: "MST", note: "UTC-07:00", minutes: -420 },
    Zone { name: "CST", note: "UTC-06:00", minutes: -360 },
    Zone { name: "EST", note: "UTC-05:00", minutes: -300 },
    Zone { name: "BRT", note: "UTC-03:00", minutes: -180 },
    Zone { name: "UTC", note: "UTC±00:00", minutes: 0 },
    Zone { name: "CET", note: "UTC+01:00", minutes: 60 },
    Zone { name: "EET", note: "UTC+02:00", minutes: 120 },
    Zone { name: "MSK", note: "UTC+03:00", minutes: 180 },
    Zone { name: "IST", note: "UTC+05:30", minutes: 330 },
    Zone { name: "HKT", note: "UTC+08:00", minutes: 480 },
    Zone { name: "JST", note: "UTC+09:00", minutes: 540 },
    Zone { name: "AEST", note: "UTC+10:00", minutes: 600 },
];

/// Where each list starts before anybody has said otherwise. A language is not
/// a place, but it is all the installer has been told by then, and a wrong
/// guess costs one press of a field on the same screen
pub fn default_for(lang: super::lang::Lang) -> usize {
    match lang {
        super::lang::Lang::En => index_of("UTC"),
        super::lang::Lang::Ru => index_of("MSK"),
    }
}

const fn index_of(name: &str) -> usize {
    let mut i = 0;
    while i < ZONES.len() {
        // `==` on strings is not const; the names here are short and distinct
        if eq(ZONES[i].name, name) {
            return i;
        }
        i += 1;
    }
    0
}

const fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The names, for the field and its list
pub fn names() -> [&'static str; ZONES.len()] {
    core::array::from_fn(|i| ZONES[i].name)
}

/// What each one is worth, shown beside the name in the open list
pub fn notes() -> [&'static str; ZONES.len()] {
    core::array::from_fn(|i| ZONES[i].note)
}
