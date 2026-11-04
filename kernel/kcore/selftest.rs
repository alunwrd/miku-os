// The shared harness behind every in-kernel selftest
//
// A selftest is a list of named cases, each passed, failed, or not run.
// `mayland::selftest` and `net::selftest` collect their cases through this
// `Runner` and return this `Report`.
//
// Anything subsystem-specific stays there: mayland adds `Runner::needs`, which
// knows leased channels, and net adds a gate for no link on this machine.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

pub struct Case {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
    /// Not run, and not a failure: something the case needs is in use by the
    /// machine, or absent
    pub skipped: bool,
}

pub struct Report {
    pub cases: Vec<Case>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.cases.iter().filter(|c| c.ok && !c.skipped).count()
    }
    pub fn failed(&self) -> usize {
        self.cases.iter().filter(|c| !c.ok && !c.skipped).count()
    }
    pub fn skipped(&self) -> usize { self.cases.iter().filter(|c| c.skipped).count() }
}

pub struct Runner {
    pub cases: Vec<Case>,
}

impl Runner {
    pub fn new() -> Self {
        Runner { cases: Vec::new() }
    }

    pub fn check(&mut self, name: &'static str, ok: bool, detail: String) {
        self.cases.push(Case { name, ok, detail, skipped: false });
    }

    /// Record a case that was not run, with the reason. Green, not a failure
    pub fn skip(&mut self, name: &'static str, why: String) {
        self.cases.push(Case { name, ok: true, detail: why, skipped: true });
    }

    pub fn report(self) -> Report {
        Report { cases: self.cases }
    }
}
