// /etc/mikudo.conf: who may become whom, to run what. doas.conf's grammar
//
//     rule    = ("permit" | "deny") option* ident ["as" target]
//                                             ["cmd" path ["args" arg...]]
//     ident   = username | ":" groupname | uid | ":" gid
//     option  = "nopass" | "nolog" | "persist" | "keepenv"
//             | "setenv" "{" (KEY | KEY "=" value)* "}"
//
// 1. The last matching rule wins.
// 2. No match is a denial.
// 3. A parse error denies everything. mikudod logs the line number and refuses
//    requests until fixed.
//
// No "any command except": `permit :wheel cmd !/bin/sh` is defeated by any
// other shell.

extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;

/// Maximum rules per file
pub const MAX_RULES: usize = 256;
/// The largest file that will be parsed at all
pub const MAX_CONF_BYTES: usize = 64 * 1024;
/// Arguments a single rule may pin
pub const MAX_RULE_ARGS: usize = 32;
/// Environment assignments a single rule may carry
pub const MAX_SETENV: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Permit,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfError {
    /// The file is larger than MAX_CONF_BYTES
    TooLarge,
    /// Not valid UTF-8, or contains a NUL
    BadBytes,
    /// More than MAX_RULES rules
    TooManyRules,
    /// Syntax error, with its line and kind
    Syntax { line: usize, what: SyntaxError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxError {
    /// The line does not begin with permit or deny
    ExpectedAction,
    /// The line ended before the identity
    MissingIdentity,
    /// `as`, `cmd` or `args` with nothing after it
    MissingArgument,
    /// Not a keyword where one was required
    UnknownKeyword,
    /// `setenv` without `{`, or an unclosed `{`
    UnterminatedSetenv,
    /// `cmd` with a non-absolute path
    CommandNotAbsolute,
    /// More arguments, or more setenv entries, than a rule may carry
    TooMany,
    /// An identity or target that cannot be a name or a number
    BadIdentity,
    /// A setenv key that is not a valid variable name
    BadEnvName,
}

/// The identity a rule applies to
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ident {
    User(String),
    Uid(u16),
    Group(String),
    Gid(u16),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleOptions {
    /// No password required; mikudod logs its use at a higher level
    pub nopass: bool,
    /// Do not journal a successful use; denials are always journaled
    pub nolog: bool,
    /// Remembers authentication for a few minutes, in the daemon's memory
    pub persist: bool,
    /// Passes the caller's environment, minus variables mikudod always removes
    pub keepenv: bool,
    /// Variables by name: `KEY` copies the caller's, `KEY=value` sets it
    pub setenv: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub action: Action,
    pub ident: Ident,
    pub options: RuleOptions,
    /// Who the caller may become. None means "anybody"
    pub target: Option<String>,
    /// The command this rule is about. None means "any command"
    pub cmd: Option<String>,
    /// Exact required arguments. `Some(vec![])` is the bare `args` keyword: no
    /// arguments at all
    pub args: Option<Vec<String>>,
    /// Which line of the file this came from, for the journal
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub rules: Vec<Rule>,
}

/// What mikudod is being asked to allow
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// Caller, from peercred and /etc/passwd
    pub user: &'a str,
    pub uid: u16,
    pub gid: u16,
    /// Every group the caller is in, primary included
    pub groups: &'a [u16],
    /// Group names in the same order, since a rule may name a group either way
    pub group_names: &'a [&'a str],
    /// Who they want to become
    pub target: &'a str,
    /// The resolved absolute path of the command
    pub cmd: &'a str,
    /// Arguments, excluding argv[0]
    pub args: &'a [&'a str],
}

/// What the configuration says about a request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    pub options: RuleOptions,
    /// Deciding line, for the journal; 0 for the default denial
    pub line: usize,
}

impl Decision {
    /// The answer when no rule matches
    pub fn default_deny() -> Self {
        Decision { action: Action::Deny, options: RuleOptions::default(), line: 0 }
    }

    pub fn permitted(&self) -> bool {
        self.action == Action::Permit
    }
}

impl Config {
    /// Parses a whole file; one bad line returns an error with its number
    pub fn parse(text: &str) -> Result<Config, ConfError> {
        if text.len() > MAX_CONF_BYTES {
            return Err(ConfError::TooLarge);
        }
        if text.contains('\0') {
            return Err(ConfError::BadBytes);
        }
        let mut rules = Vec::new();
        for (line_no, raw) in text.lines().enumerate() {
            let line_no = line_no + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if rules.len() >= MAX_RULES {
                return Err(ConfError::TooManyRules);
            }
            match parse_rule(line, line_no) {
                Ok(r) => rules.push(r),
                Err(what) => return Err(ConfError::Syntax { line: line_no, what }),
            }
        }
        Ok(Config { rules })
    }

    /// Decides one request: last match wins, no match denies
    pub fn decide(&self, req: &Request<'_>) -> Decision {
        let mut out = Decision::default_deny();
        for rule in &self.rules {
            if rule_matches(rule, req) {
                out = Decision {
                    action: rule.action,
                    options: rule.options.clone(),
                    line: rule.line,
                };
            }
        }
        out
    }

    /// Every rule that could apply to this user, for `mikudo -l`
    pub fn rules_for(&self, user: &str, uid: u16, group_names: &[&str], groups: &[u16])
        -> Vec<&Rule>
    {
        self.rules
            .iter()
            .filter(|r| ident_matches(&r.ident, user, uid, group_names, groups))
            .collect()
    }
}

/// `#` begins a comment, except inside a setenv value
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_braces = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'{' => in_braces = true,
            b'}' => in_braces = false,
            b'#' if !in_braces => return &line[..i],
            _ => {}
        }
    }
    line
}

fn rule_matches(rule: &Rule, req: &Request<'_>) -> bool {
    if !ident_matches(&rule.ident, req.user, req.uid, req.group_names, req.groups) {
        return false;
    }
    if let Some(t) = &rule.target {
        if t != req.target {
            return false;
        }
    }
    if let Some(c) = &rule.cmd {
        if c != req.cmd {
            return false;
        }
    }
    if let Some(want) = &rule.args {
        // Exact match in order, including the count, so a pinned prefix cannot
        // allow extra arguments
        if want.len() != req.args.len() {
            return false;
        }
        if want.iter().zip(req.args.iter()).any(|(a, b)| a != b) {
            return false;
        }
    }
    true
}

fn ident_matches(
    ident: &Ident,
    user: &str,
    uid: u16,
    group_names: &[&str],
    groups: &[u16],
) -> bool {
    match ident {
        Ident::User(n) => n == user,
        Ident::Uid(u) => *u == uid,
        Ident::Group(n) => group_names.iter().any(|g| g == n),
        Ident::Gid(g) => groups.contains(g),
    }
}

fn parse_rule(line: &str, line_no: usize) -> Result<Rule, SyntaxError> {
    let mut words = Words::new(line);

    let action = match words.next() {
        Some("permit") => Action::Permit,
        Some("deny") => Action::Deny,
        _ => return Err(SyntaxError::ExpectedAction),
    };

    let mut options = RuleOptions::default();
    // Options come before the identity, whose position disambiguates it
    let ident_word = loop {
        let w = match words.next() {
            Some(w) => w,
            None => return Err(SyntaxError::MissingIdentity),
        };
        match w {
            "nopass" => options.nopass = true,
            "nolog" => options.nolog = true,
            "persist" => options.persist = true,
            "keepenv" => options.keepenv = true,
            "setenv" => parse_setenv(&mut words, &mut options)?,
            other => break other,
        }
    };

    let ident = parse_ident(ident_word)?;

    let mut target = None;
    let mut cmd = None;
    let mut args = None;

    while let Some(w) = words.next() {
        match w {
            "as" => {
                let t = words.next().ok_or(SyntaxError::MissingArgument)?;
                if !valid_name(t) {
                    return Err(SyntaxError::BadIdentity);
                }
                target = Some(t.to_owned());
            }
            "cmd" => {
                let c = words.next().ok_or(SyntaxError::MissingArgument)?;
                // Absolute path only, since a bare name would need a PATH
                if !c.starts_with('/') {
                    return Err(SyntaxError::CommandNotAbsolute);
                }
                cmd = Some(c.to_owned());
            }
            "args" => {
                // The bare keyword means no arguments, distinct from leaving
                // `args` out
                let mut list = Vec::new();
                for a in words.by_ref() {
                    if list.len() >= MAX_RULE_ARGS {
                        return Err(SyntaxError::TooMany);
                    }
                    list.push(a.to_owned());
                }
                args = Some(list);
            }
            _ => return Err(SyntaxError::UnknownKeyword),
        }
    }

    if args.is_some() && cmd.is_none() {
        // `args` requires a command
        return Err(SyntaxError::MissingArgument);
    }

    Ok(Rule { action, ident, options, target, cmd, args, line: line_no })
}

fn parse_setenv(words: &mut Words<'_>, options: &mut RuleOptions) -> Result<(), SyntaxError> {
    match words.next() {
        Some("{") => {}
        _ => return Err(SyntaxError::UnterminatedSetenv),
    }
    loop {
        let w = words.next().ok_or(SyntaxError::UnterminatedSetenv)?;
        if w == "}" {
            return Ok(());
        }
        if options.setenv.len() >= MAX_SETENV {
            return Err(SyntaxError::TooMany);
        }
        match w.split_once('=') {
            Some((k, v)) => {
                if !valid_env_name(k) {
                    return Err(SyntaxError::BadEnvName);
                }
                options.setenv.push((k.to_owned(), Some(v.to_owned())));
            }
            None => {
                if !valid_env_name(w) {
                    return Err(SyntaxError::BadEnvName);
                }
                options.setenv.push((w.to_owned(), None));
            }
        }
    }
}

fn parse_ident(w: &str) -> Result<Ident, SyntaxError> {
    if let Some(rest) = w.strip_prefix(':') {
        if rest.is_empty() {
            return Err(SyntaxError::BadIdentity);
        }
        return parse_ident_body(rest, true);
    }
    parse_ident_body(w, false)
}

/// All digits is an id, otherwise a name. An out-of-range all-digit word is an
/// error, not a name
fn parse_ident_body(w: &str, group: bool) -> Result<Ident, SyntaxError> {
    let all_digits = !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit());
    if all_digits {
        let n = parse_num(w).ok_or(SyntaxError::BadIdentity)?;
        return Ok(if group { Ident::Gid(n) } else { Ident::Uid(n) });
    }
    if !valid_name(w) {
        return Err(SyntaxError::BadIdentity);
    }
    Ok(if group { Ident::Group(w.to_owned()) } else { Ident::User(w.to_owned()) })
}

/// All digits and fits an id. "0x10" is a name
fn parse_num(s: &str) -> Option<u16> {
    if s.is_empty() || s.len() > 5 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: u32 = 0;
    for b in s.bytes() {
        n = n * 10 + (b - b'0') as u32;
        if n > crate::db::ID_MAX {
            return None;
        }
    }
    Some(n as u16)
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && !s.starts_with('-')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// Valid variable name, without `=` or NUL
fn valid_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.as_bytes()[0].is_ascii_digit()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whitespace-separated words, with `{` and `}` always separate
struct Words<'a> {
    rest: &'a str,
}

impl<'a> Words<'a> {
    fn new(s: &'a str) -> Self {
        Words { rest: s }
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        self.rest = self.rest.trim_start();
        if self.rest.is_empty() {
            return None;
        }
        let b = self.rest.as_bytes();
        if b[0] == b'{' || b[0] == b'}' {
            let (w, rest) = self.rest.split_at(1);
            self.rest = rest;
            return Some(w);
        }
        let end = self.rest
            .find(|c: char| c.is_ascii_whitespace() || c == '{' || c == '}')
            .unwrap_or(self.rest.len());
        let (w, rest) = self.rest.split_at(end);
        self.rest = rest;
        Some(w)
    }
}
