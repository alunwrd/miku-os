//! Metadata section bytes
//!
//! A list of `u16 tag, u32 len, value`, with non-decreasing tags. A reader can
//! stop past the tag it wants, and one metadata has one encoding, since the
//! signature covers these bytes and a rebuild must give the same digest.
//!
//! The high bit of a tag means critical; see `meta/tag.rs`.

use alloc::string::String;
use alloc::vec::Vec;

use super::{put_u16, put_u32, u16_at, u32_at, FormatError};
use crate::meta::dep::{Dep, OptDep, Provide};
use crate::meta::tag;
use crate::meta::{Arch, PackageMeta, Trigger};
use crate::name::Name;
use crate::version::Version;

/// A record's fixed part
const REC_HEAD: usize = 6;
/// More than this is somebody using a field as storage
pub const MAX_VALUE: u32 = 4096;
/// Enough for a name, a paragraph and a few dozen dependencies
pub const MAX_RECORDS: usize = 512;

/// Reads a metadata section. A non-critical unknown tag is reported to `note`
/// and skipped; a critical one refuses the package by number
pub fn decode(
    b: &[u8],
    mut note: impl FnMut(u16, usize),
) -> Result<PackageMeta, FormatError> {
    let mut name: Option<Name> = None;
    let mut version: Option<Version> = None;
    let mut arch: Option<Arch> = None;
    let mut desc = String::new();
    let mut url = String::new();
    let mut license = String::new();
    let mut build_time = 0u64;
    let mut installed_size = 0u64;
    let mut depends = Vec::new();
    let mut provides = Vec::new();
    let mut conflicts = Vec::new();
    let mut replaces = Vec::new();
    let mut optdepends = Vec::new();
    let mut triggers = Vec::new();

    let mut at = 0usize;
    let mut last_tag = 0u16;
    let mut records = 0usize;

    while at < b.len() {
        if at + REC_HEAD > b.len() {
            return Err(FormatError::BadRecord { at });
        }
        let t = u16_at(b, at).unwrap();
        let len = u32_at(b, at + 2).unwrap();
        if len > MAX_VALUE {
            return Err(FormatError::TooLarge {
                what: "bytes in one metadata field",
                count: len as u64,
                limit: MAX_VALUE as u64,
            });
        }
        let start = at + REC_HEAD;
        let end = start
            .checked_add(len as usize)
            .filter(|e| *e <= b.len())
            .ok_or(FormatError::BadRecord { at })?;
        let value = &b[start..end];

        records += 1;
        if records > MAX_RECORDS {
            return Err(FormatError::TooLarge {
                what: "metadata records",
                count: records as u64,
                limit: MAX_RECORDS as u64,
            });
        }
        // Non-decreasing. Out of order is refused, not sorted, since the
        // signature covers the bytes
        if t < last_tag {
            return Err(FormatError::BadRecord { at });
        }
        last_tag = t;

        let text = || core::str::from_utf8(value).map_err(|_| FormatError::BadValue { tag: t, at });
        // A field that may appear once. Takes the answer, since three types
        // share this rule
        let once = |already: bool| -> Result<(), FormatError> {
            if already {
                Err(FormatError::RepeatedTag { tag: t, at })
            } else {
                Ok(())
            }
        };

        match t {
            tag::NAME => {
                once(name.is_some())?;
                name = Some(Name::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?);
            }
            tag::VERSION => {
                once(version.is_some())?;
                version =
                    Some(Version::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?);
            }
            tag::ARCH => {
                once(arch.is_some())?;
                arch = Some(Arch::parse(text()?).ok_or(FormatError::BadValue { tag: t, at })?);
            }
            tag::DEPENDS => depends
                .push(Dep::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?),
            tag::CONFLICTS => conflicts
                .push(Dep::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?),
            tag::REPLACES => replaces
                .push(Dep::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?),
            tag::PROVIDES => provides
                .push(Provide::parse(text()?).map_err(|_| FormatError::BadValue { tag: t, at })?),
            tag::TRIGGER => {
                let code = u16_at(value, 0).ok_or(FormatError::BadValue { tag: t, at })?;
                // An unknown trigger is refused by name; the package expected
                // something to happen
                let trig = Trigger::from_code(code)
                    .ok_or(FormatError::UnknownCriticalTag { tag: t, at })?;
                triggers.push(trig);
            }
            tag::OPTDEPENDS => {
                // `dep\0reason`
                let mut parts = value.splitn(2, |&c| c == 0);
                let d = parts.next().unwrap_or(&[]);
                let why = parts.next().unwrap_or(&[]);
                let d = core::str::from_utf8(d).map_err(|_| FormatError::BadValue { tag: t, at })?;
                let why =
                    core::str::from_utf8(why).map_err(|_| FormatError::BadValue { tag: t, at })?;
                optdepends.push(OptDep {
                    dep: Dep::parse(d).map_err(|_| FormatError::BadValue { tag: t, at })?,
                    why: String::from(why),
                });
            }
            tag::DESC => desc = String::from(text()?),
            tag::URL => url = String::from(text()?),
            tag::LICENSE => license = String::from(text()?),
            tag::BUILD_TIME => {
                build_time = super::u64_at(value, 0).ok_or(FormatError::BadValue { tag: t, at })?
            }
            tag::INSTALLED_SIZE => {
                installed_size =
                    super::u64_at(value, 0).ok_or(FormatError::BadValue { tag: t, at })?
            }
            unknown => {
                if tag::is_critical(unknown) {
                    return Err(FormatError::UnknownCriticalTag { tag: unknown, at });
                }
                note(unknown, at);
            }
        }

        at = end;
    }

    let name = name.ok_or(FormatError::MissingTag { tag: tag::NAME })?;
    let version = version.ok_or(FormatError::MissingTag { tag: tag::VERSION })?;
    let arch = arch.ok_or(FormatError::MissingTag { tag: tag::ARCH })?;

    Ok(PackageMeta {
        name,
        version,
        arch,
        depends,
        provides,
        conflicts,
        replaces,
        optdepends,
        triggers,
        desc,
        url,
        license,
        build_time,
        installed_size,
    })
}

/// Writes the model in tag order. Deterministic, so a rebuilt package verifies
/// against the first signature
pub fn encode(m: &PackageMeta) -> Vec<u8> {
    use alloc::format;

    let mut recs: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut push = |t: u16, v: Vec<u8>| recs.push((t, v));

    push(tag::NAME, m.name.as_bytes().to_vec());
    push(tag::VERSION, format!("{}", m.version).into_bytes());
    push(tag::ARCH, m.arch.as_str().as_bytes().to_vec());
    for d in &m.depends {
        push(tag::DEPENDS, d.to_wire().into_bytes());
    }
    for p in &m.provides {
        push(tag::PROVIDES, format!("{p}").into_bytes());
    }
    for d in &m.conflicts {
        push(tag::CONFLICTS, d.to_wire().into_bytes());
    }
    for d in &m.replaces {
        push(tag::REPLACES, d.to_wire().into_bytes());
    }
    for t in &m.triggers {
        push(tag::TRIGGER, t.code().to_le_bytes().to_vec());
    }
    if !m.desc.is_empty() {
        push(tag::DESC, m.desc.as_bytes().to_vec());
    }
    if !m.url.is_empty() {
        push(tag::URL, m.url.as_bytes().to_vec());
    }
    if !m.license.is_empty() {
        push(tag::LICENSE, m.license.as_bytes().to_vec());
    }
    if m.build_time != 0 {
        push(tag::BUILD_TIME, m.build_time.to_le_bytes().to_vec());
    }
    if m.installed_size != 0 {
        push(tag::INSTALLED_SIZE, m.installed_size.to_le_bytes().to_vec());
    }
    for o in &m.optdepends {
        let mut v = o.dep.to_wire().into_bytes();
        v.push(0);
        v.extend_from_slice(o.why.as_bytes());
        push(tag::OPTDEPENDS, v);
    }

    // Stable sort by tag: repeats keep their order, so a dependency list reads
    // back as written
    recs.sort_by_key(|(t, _)| *t);

    let mut out = Vec::new();
    for (t, v) in recs {
        let mut head = [0u8; REC_HEAD];
        put_u16(&mut head, 0, t);
        put_u32(&mut head, 2, v.len() as u32);
        out.extend_from_slice(&head);
        out.extend_from_slice(&v);
    }
    out
}
