# Certificates used by the crate's tests

Binary test data needs a provenance note, so here it is.

These are real certificates, chosen because their verdict cannot go stale:

| file | what it is | why it is here |
|---|---|---|
| `good-leaf.der`, `good-ca.der` | `www.google.com` and the intermediate that signed it, captured 2026-08-23 | the positive control - a chain that must be accepted. Checked against a pinned timestamp inside the leaf's window, because a leaf is good for about three months and a test that starts failing on a calendar date teaches nothing |
| `expired-leaf.der`, `expired-ca.der` | `expired.badssl.com`, valid 9-12 April 2015 | expired then, expired forever |
| `self-signed.der` | `self-signed.badssl.com` | signs itself, so it chains to nothing and never will |
| `names.der` | generated here: `CN=test.miku.local`, SAN `test.miku.local`, `*.wild.local` | the hostname rules - one wildcard label, never a dot, case-insensitive |
| `ipsan.der` | generated here: SAN `IP:10.0.2.2`, `DNS:bench.local` | an address certificate matches the address it names and nothing else |

None of them carry a private key, and none is trusted by anything here - they
are inputs to a parser and a verifier.

The trust anchors are somewhere else entirely: `etc/ssl/certs/ca-bundle.pem`,
which is both what the tests read and what lands on a running machine as
`/etc/ssl/certs/ca-bundle.pem`. One file, so the tests cannot pass against a
set of roots the system does not have.
