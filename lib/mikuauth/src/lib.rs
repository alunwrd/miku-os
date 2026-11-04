// mikuauth: account files, password hashing, and the mikudo policy
//
// blake2b  RFC 7693, plus Argon2's H'
// argon2   RFC 9106, id variant
// b64      crypt(3) base64
// ct       constant-time comparison and wiping
//      hash     the $argon2id$... string: parse, verify, produce
//      db       /etc/passwd, /etc/group, /etc/shadow
//      conf     /etc/mikudo.conf
// session  authenticating a user against an account
// proto    mikudo <-> mikudod wire format
// client   requesting a session (userspace only)
//
// No I/O and no policy decisions, except `client` under the `userspace`
// feature.

#![cfg_attr(not(test), no_std)]

pub mod argon2;
pub mod b64;
pub mod blake2b;
#[cfg(feature = "userspace")]
pub mod client;
pub mod conf;
pub mod ct;
pub mod db;
pub mod hash;
pub mod proto;
pub mod session;

pub use conf::{Action, Config, ConfError, Decision, Request, RuleOptions};
pub use db::{DbError, Group, Passwd, Shadow};
pub use hash::{Verdict, MAX_HASH_LEN};
pub use session::{authenticate, Identity, Refusal};

/// File paths
pub const PASSWD_PATH: &str = "/etc/passwd";
pub const GROUP_PATH:  &str = "/etc/group";
pub const SHADOW_PATH: &str = "/etc/shadow";
pub const CONF_PATH:   &str = "/etc/mikudo.conf";
pub const SOCKET_PATH: &str = "/run/mikudo.sock";

/// Group allowed to use mikudo by the shipped configuration
pub const WHEEL_GROUP: &str = "wheel";

/// The image's documented default password
pub const SHIPPED_PASSWORD: &[u8] = b"miku";

/// Whether a password is the default one, in constant time
pub fn is_shipped_password(password: &[u8]) -> bool {
    ct::eq(password, SHIPPED_PASSWORD)
}

pub const ROOT_UID: u16 = 0;
pub const NOBODY_UID: u16 = 65534;
