// Authentication shared by login, mikudod, and the greeter's broker. Pure and
// host-tested; the caller provides the bytes of the three account files

use crate::db::{self, Passwd};
use crate::hash::{self, Verdict};

/// Most groups an identity carries. Same value as `MAX_GROUPS` in
/// kernel/fs/vfs/types.rs and `miku::cred`; this crate builds on the host,
/// where neither exists. tests/shipped.rs checks the shipped files against it
pub use miku_abi::cred::MAX_GROUPS;

/// Reason a login failed. `NoSuchUser` and `WrongPassword` print the same
/// sentence so a prompt cannot enumerate accounts; the journal keeps the
/// difference
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No line in /etc/passwd, or none in /etc/shadow
    NoSuchUser,
    /// Account exists; the password is wrong
    WrongPassword,
    /// Locked: no password matches
    Locked,
    /// Password or account aged out, see `Shadow::usable_on`
    Expired,
    /// No password set, and this caller does not accept that
    NoPassword,
}

impl Refusal {
    /// Text shown at the prompt; the same sentence for the two cases that could
    /// enumerate accounts
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::NoSuchUser | Refusal::WrongPassword | Refusal::NoPassword => "incorrect",
            Refusal::Locked => "that account is locked",
            Refusal::Expired => "that account has expired",
        }
    }

    /// What to write in the journal, where the distinction is safe to make
    pub fn detail(self) -> &'static str {
        match self {
            Refusal::NoSuchUser => "no such account",
            Refusal::WrongPassword => "wrong password",
            Refusal::Locked => "the account is locked",
            Refusal::Expired => "the account has expired",
            Refusal::NoPassword => "the account has no password and this caller requires one",
        }
    }
}

/// Authenticated identity, borrowing from the passwd text. `groups` is a filled
/// slice, ready for `setgroups`
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    pub name: &'a str,
    pub uid: u16,
    pub gid: u16,
    pub groups: [u16; MAX_GROUPS],
    pub ngroups: usize,
    pub home: &'a str,
    /// Field seven of /etc/passwd: what a session starts
    pub program: &'a str,
}

impl<'a> Identity<'a> {
    /// The groups, as the slice `setgroups` wants
    pub fn group_slice(&self) -> &[u16] {
        &self.groups[..self.ngroups]
    }

    fn of(account: &Passwd<'a>, group_file: &str) -> Identity<'a> {
        let mut groups = [0u16; MAX_GROUPS];
        let ngroups = db::groups_of(group_file, account.name, account.gid, &mut groups);
        Identity {
            name: account.name,
            uid: account.uid,
            gid: account.gid,
            groups,
            ngroups,
            home: account.home,
            program: account.shell,
        }
    }
}

/// Checks a password and returns its account.
///
/// `today` is days since the epoch (`realtime_secs() / 86400`). `allow_empty`
/// decides what an empty shadow field means: console login accepts it, mikudod
/// does not.
///
/// A missing user still costs a full Argon2id verification against
/// `hash::DUMMY`, so timing does not reveal which accounts exist.
pub fn authenticate<'a>(
    passwd: &'a str,
    shadow: &str,
    group: &str,
    user: &str,
    password: &[u8],
    today: u32,
    allow_empty: bool,
) -> Result<Identity<'a>, Refusal> {
    let entry = db::find_shadow(shadow, user);
    let verdict = match entry {
        Some(e) => hash::verify(password, e.hash),
        None => {
            // Takes the same time as a real check; the result is discarded
            let _ = hash::verify(password, hash::DUMMY);
            Verdict::Wrong
        }
    };

    // Resolved before acting on the verdict, so an account missing from one of
    // the two files shows no timing difference
    let account = db::find_user(passwd, user);

    match verdict {
        Verdict::Locked => return Err(Refusal::Locked),
        Verdict::Wrong => {
            return Err(if account.is_none() { Refusal::NoSuchUser } else { Refusal::WrongPassword })
        }
        Verdict::NoPassword if !allow_empty => return Err(Refusal::NoPassword),
        Verdict::NoPassword | Verdict::Ok => {}
    }

    let account = match account {
        Some(a) => a,
        // Shadow line with no passwd line: correct password, no account
        None => return Err(Refusal::NoSuchUser),
    };

    if let Some(e) = entry {
        if !e.usable_on(today) {
            return Err(Refusal::Expired);
        }
    }

    Ok(Identity::of(&account, group))
}

/// Identity of a uid without a password, for callers who got the uid from
/// `peercred`. Same `Identity` as the authenticated path
pub fn identity_of_uid<'a>(passwd: &'a str, group: &str, uid: u16) -> Option<Identity<'a>> {
    db::find_uid(passwd, uid).map(|a| Identity::of(&a, group))
}

/// The same, by name
pub fn identity_of_name<'a>(passwd: &'a str, group: &str, name: &str) -> Option<Identity<'a>> {
    db::find_user(passwd, name).map(|a| Identity::of(&a, group))
}

/// Whether an identity is in a named group: the primary group in /etc/passwd or
/// a line in /etc/group
pub fn in_group(id: &Identity<'_>, group_file: &str, name: &str) -> bool {
    match db::find_group(group_file, name) {
        Some(g) => id.group_slice().contains(&g.gid) || g.has_member(id.name),
        None => false,
    }
}
