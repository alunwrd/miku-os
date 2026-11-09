//! The real transfer: HTTPS through `mikunet`. The rest of the logic is
//! `super::object`

use mikunet::http::wire::BodySink;
use mikunet::http::{Client, GetOpts};

use super::{Answer, Transfer};

pub struct Http<'a, 'c> {
    pub client: &'a Client<'c>,
    /// Longest stall. Restarts on every byte, so it bounds the gap, not the
    /// transfer
    pub idle_ms: u64,
}

impl<'a, 'c> Http<'a, 'c> {
    pub fn new(client: &'a Client<'c>) -> Http<'a, 'c> {
        Http { client, idle_ms: mikunet::http::wire::RECV_IDLE_MS }
    }
}

impl Transfer for Http<'_, '_> {
    fn get(
        &self,
        url: &str,
        from: Option<u64>,
        max_bytes: u64,
        sink: &mut dyn BodySink,
    ) -> Option<Answer> {
        let opts = GetOpts { range_from: from, max_bytes, idle_ms: self.idle_ms };
        let info = mikunet::http::get_to(url, self.client, &opts, sink)?;
        Some(Answer {
            status: info.status,
            partial: info.partial,
            truncated: info.truncated,
        })
    }
}
