//! One configuration file. Line-oriented, with one block form.
//!
//! ```text
//! # a comment
//! cache = /pkg/cache
//! ignore = linux-firmware
//!
//! repo core {
//!     url      = https://repo.miku/core
//!     key      = 3f9a1c22b0d4e7f1
//!     priority = 10
//!     mirror   = https://mirror.example/core
//! }
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::note::{Complaint, Note};
use super::{Config, Repo, MAX_BYTES, MAX_IGNORES, MAX_MIRRORS, MAX_REPOS};
use crate::name::Name;
use crate::trust::keyring::KeyId;

/// Reads one file into `cfg`, appending complaints to `notes`. Later files win;
/// `layers.rs` folds over this
pub fn parse_into(file: &str, text: &[u8], cfg: &mut Config, notes: &mut Vec<Note>) {
    if text.len() > MAX_BYTES {
        notes.push(Note {
            file: file.to_string(),
            line: 0,
            what: Complaint::TooBig { limit: MAX_BYTES },
            text: String::new(),
            fatal: true,
        });
        return;
    }
    let text = match core::str::from_utf8(text) {
        Ok(t) => t,
        Err(_) => {
            notes.push(Note {
                file: file.to_string(),
                line: 0,
                what: Complaint::BadValue { wanted: "text" },
                text: String::new(),
                fatal: true,
            });
            return;
        }
    };

    let mut open: Option<Repo> = None;
    let mut open_line = 0usize;

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = strip_comment(raw).trim();
        if body.is_empty() {
            continue;
        }

        let mut say = |what: Complaint, fatal: bool| {
            notes.push(Note {
                file: file.to_string(),
                line,
                what,
                text: body.to_string(),
                fatal,
            });
        };

        // inside a repository
        if let Some(repo) = open.as_mut() {
            if body == "}" {
                let done = open.take().expect("checked");
                if done.url.is_empty() {
                    say(Complaint::RepoWithoutUrl, true);
                } else if cfg.repos.len() >= MAX_REPOS {
                    say(Complaint::TooMany { what: "repositories", limit: MAX_REPOS }, true);
                } else {
                    // The later of two same-named blocks wins, so a `conf.d`
                    // file can repoint a shipped repository
                    cfg.repos.retain(|r| r.name != done.name);
                    cfg.repos.push(done);
                }
                continue;
            }
            let (key, value) = match split(body) {
                Some(kv) => kv,
                None => {
                    // Inside a block that decides trust, an unknown line is an
                    // error
                    say(Complaint::UnknownRepoSetting, true);
                    continue;
                }
            };
            if value.is_empty() {
                say(Complaint::MissingValue, true);
                continue;
            }
            match key {
                "url" => repo.url = value.to_string(),
                "mirror" => {
                    if repo.mirrors.len() >= MAX_MIRRORS {
                        say(Complaint::TooMany { what: "mirrors", limit: MAX_MIRRORS }, true);
                    } else {
                        repo.mirrors.push(value.to_string());
                    }
                }
                "key" => match key_id(value) {
                    Some(id) => repo.key = Some(id),
                    None => say(
                        Complaint::BadValue { wanted: "sixteen hex digits naming a key" },
                        true,
                    ),
                },
                "priority" => match parse_i32(value) {
                    Some(p) => repo.priority = p,
                    None => say(Complaint::BadValue { wanted: "a number" }, true),
                },
                _ => say(Complaint::UnknownRepoSetting, true),
            }
            continue;
        }

        // a repository begins
        if let Some(rest) = body.strip_prefix("repo ") {
            let rest = rest.trim();
            let name = rest.strip_suffix('{').unwrap_or(rest).trim();
            if !rest.ends_with('{') {
                say(Complaint::Unbalanced, true);
                continue;
            }
            match Name::parse(name) {
                Ok(n) => {
                    open = Some(Repo {
                        name: n,
                        url: String::new(),
                        key: None,
                        priority: 0,
                        mirrors: Vec::new(),
                    });
                    open_line = line;
                }
                Err(_) => say(Complaint::BadRepoName, true),
            }
            continue;
        }
        if body == "}" {
            say(Complaint::Unbalanced, true);
            continue;
        }

        // an ordinary setting
        let (key, value) = match split(body) {
            Some(kv) => kv,
            None => {
                say(Complaint::UnknownSetting, false);
                continue;
            }
        };
        if value.is_empty() {
            say(Complaint::MissingValue, false);
            continue;
        }
        match key {
            "cache" => cfg.cache = value.to_string(),
            "stage" => cfg.stage = value.to_string(),
            "db" => cfg.db = value.to_string(),
            "journal" => cfg.journal = value.to_string(),
            // Which keys this machine believes. A nonsensical value stops the
            // run
            "keyring" => cfg.keyring = value.to_string(),
            "max_index_age" => match duration(value) {
                Some(d) => cfg.max_index_age = d,
                None => say(
                    Complaint::BadValue { wanted: "a length of time, like 7d or 12h" },
                    false,
                ),
            },
            "allow_downgrade" => match value {
                "true" | "yes" | "1" => cfg.allow_downgrade = true,
                "false" | "no" | "0" => cfg.allow_downgrade = false,
                _ => say(Complaint::BadValue { wanted: "true or false" }, false),
            },
            "ignore" => {
                if cfg.ignore.len() >= MAX_IGNORES {
                    say(Complaint::TooMany { what: "ignored packages", limit: MAX_IGNORES }, false);
                } else {
                    match Name::parse(value) {
                        Ok(n) => {
                            if !cfg.ignore.contains(&n) {
                                cfg.ignore.push(n);
                            }
                        }
                        Err(_) => say(Complaint::BadValue { wanted: "a package name" }, false),
                    }
                }
            }
            _ => say(Complaint::UnknownSetting, false),
        }
    }

    if open.is_some() {
        notes.push(Note {
            file: file.to_string(),
            line: open_line,
            what: Complaint::Unbalanced,
            text: String::new(),
            fatal: true,
        });
    }
}

/// `#` to the end of the line. Also cuts a `#` in a URL
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(at) => &line[..at],
        None => line,
    }
}

fn split(body: &str) -> Option<(&str, &str)> {
    let at = body.find('=')?;
    Some((body[..at].trim(), body[at + 1..].trim()))
}

fn parse_i32(s: &str) -> Option<i32> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut acc: i64 = 0;
    for c in digits.bytes() {
        acc = acc * 10 + (c - b'0') as i64;
        if acc > i32::MAX as i64 + 1 {
            return None;
        }
    }
    Some(if neg { -acc as i32 } else { acc as i32 })
}

/// `90`, `90s`, `30m`, `12h`, `7d`
fn duration(s: &str) -> Option<u64> {
    let (digits, unit) = match s.as_bytes().last() {
        Some(c) if c.is_ascii_digit() => (s, 1u64),
        Some(b's') => (&s[..s.len() - 1], 1),
        Some(b'm') => (&s[..s.len() - 1], 60),
        Some(b'h') => (&s[..s.len() - 1], 3600),
        Some(b'd') => (&s[..s.len() - 1], 86_400),
        _ => return None,
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut acc: u64 = 0;
    for c in digits.bytes() {
        acc = acc.checked_mul(10)?.checked_add((c - b'0') as u64)?;
    }
    acc.checked_mul(unit)
}

fn key_id(s: &str) -> Option<KeyId> {
    if s.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        let hi = hex(pair[0])?;
        let lo = hex(pair[1])?;
        out[i] = hi << 4 | lo;
    }
    Some(KeyId(out))
}

use miku_base::hex::value as hex;
