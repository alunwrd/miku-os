//! `<netinet/in.h>`, `<netinet/tcp.h>`

#![allow(unused_imports)]

use super::prelude::*;

pub const IPPROTO_IGMP: c_int = 2;
pub const IPPROTO_IPIP: c_int = 4;
pub const IPPROTO_EGP: c_int = 8;
pub const IPPROTO_PUP: c_int = 12;
pub const IPPROTO_IDP: c_int = 22;
pub const IPPROTO_TP: c_int = 29;
pub const IPPROTO_DCCP: c_int = 33;
pub const IPPROTO_ROUTING: c_int = 43;
pub const IPPROTO_FRAGMENT: c_int = 44;
pub const IPPROTO_RSVP: c_int = 46;
pub const IPPROTO_GRE: c_int = 47;
pub const IPPROTO_ESP: c_int = 50;
pub const IPPROTO_AH: c_int = 51;
pub const IPPROTO_MTP: c_int = 92;
pub const IPPROTO_BEETPH: c_int = 94;
pub const IPPROTO_ENCAP: c_int = 98;
pub const IPPROTO_PIM: c_int = 103;
pub const IPPROTO_COMP: c_int = 108;
pub const IPPROTO_SCTP: c_int = 132;
pub const IPPROTO_MH: c_int = 135;
pub const IPPROTO_UDPLITE: c_int = 136;
pub const IPPROTO_MPLS: c_int = 137;
pub const IPPROTO_RAW: c_int = 255;
pub const IPPROTO_MPTCP: c_int = 262;
/// `IP_TTL` is the one implemented option (`setsockopt`(80), for `traceroute`);
/// the rest are names
pub const IP_TOS: c_int = 1;
pub const IP_TTL: c_int = 2;
pub const IP_HDRINCL: c_int = 3;
pub const IP_MULTICAST_IF: c_int = 32;
pub const IP_MULTICAST_TTL: c_int = 33;
pub const IP_MULTICAST_LOOP: c_int = 34;
pub const IP_ADD_MEMBERSHIP: c_int = 35;
pub const IP_DROP_MEMBERSHIP: c_int = 36;
pub const IPV6_UNICAST_HOPS: c_int = 16;
pub const IPV6_MULTICAST_IF: c_int = 17;
pub const IPV6_MULTICAST_HOPS: c_int = 18;
pub const IPV6_MULTICAST_LOOP: c_int = 19;
pub const IPV6_ADD_MEMBERSHIP: c_int = 20;
pub const IPV6_DROP_MEMBERSHIP: c_int = 21;
pub const IPV6_V6ONLY: c_int = 26;
pub const IPV6_TCLASS: c_int = 67;
pub const TCP_NODELAY: c_int = 1;
pub const TCP_MAXSEG: c_int = 2;
pub const TCP_KEEPIDLE: c_int = 4;
pub const TCP_KEEPINTVL: c_int = 5;
pub const TCP_KEEPCNT: c_int = 6;
// Below: for Rust only, not in the C headers
pub type in_port_t = u16;
pub type in_addr_t = u32;
c_structs! {
    pub struct in_addr {
        pub s_addr: in_addr_t,
    }

    pub struct sockaddr_in {
        pub sin_family: sa_family_t,
        pub sin_port: in_port_t,
        pub sin_addr: in_addr,
        pub sin_zero: [u8; 8],
    }

    /// Declared for naming only; the kernel has no IPv6 and answers
    /// `EAFNOSUPPORT`
    pub struct sockaddr_in6 {
        pub sin6_family: sa_family_t,
        pub sin6_port: in_port_t,
        pub sin6_flowinfo: u32,
        pub sin6_addr: in6_addr,
        pub sin6_scope_id: u32,
    }

    pub struct ip_mreq {
        pub imr_multiaddr: in_addr,
        pub imr_interface: in_addr,
    }

    pub struct ip_mreqn {
        pub imr_multiaddr: in_addr,
        pub imr_address: in_addr,
        pub imr_ifindex: c_int,
    }
}
