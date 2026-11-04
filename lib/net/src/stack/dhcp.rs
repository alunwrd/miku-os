// DHCP packets. The exchange (four messages, retries, timeouts) stays with the
// caller, which has a socket and a clock. A reply comes from whoever answers
// first, before anything is authenticated, so every field is untrusted

use crate::ipv4;

pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;

/// The client and server ports, fixed by RFC 2131
pub const CLIENT_PORT: u16 = 68;
pub const SERVER_PORT: u16 = 67;

const MAGIC: [u8; 4] = [99, 130, 83, 99];

/// Where the fixed header ends and the options begin
const OPTIONS_AT: usize = 240;

/// Packet build buffer. A BOOTP message is 236 bytes plus the magic cookie and
/// options; 548 is the most a client must accept
pub const PACKET_BUF: usize = 548;

const OPT_SUBNET: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_SERVER: u8 = 54;
const OPT_PARAM_REQ: u8 = 55;
const OPT_END: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub gw: [u8; 4],
    pub dns: [u8; 4],
    pub server: [u8; 4],
    /// Option 51, in seconds. Zero when the server sent none
    pub lease_secs: u32,
    /// The server refused: start over from DISCOVER and ignore the other fields
    pub nak: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn build_packet(
    msg_type: u8,
    xid: u32,
    mac: &[u8; 6],
    ciaddr: &[u8; 4],
    server_ip: Option<&[u8; 4]>,
    requested_ip: Option<&[u8; 4]>,
    out: &mut [u8; PACKET_BUF],
) -> usize {
    out[0] = 1; // BOOTREQUEST
    out[1] = 1; // ethernet
    out[2] = 6; // six bytes of hardware address
    out[3] = 0; // hops
    out[4..8].copy_from_slice(&xid.to_be_bytes());
    out[8] = 0;
    out[9] = 0; // secs
    // Broadcast bit, required while ciaddr is zero: there is no address yet for
    // a unicast reply
    out[10] = 0x80;
    out[11] = 0x00;
    out[12..16].copy_from_slice(ciaddr);
    out[16..28].fill(0); // yiaddr, siaddr, giaddr
    out[28..34].copy_from_slice(mac);
    out[34..236].fill(0);
    out[236..240].copy_from_slice(&MAGIC);

    let mut pos = OPTIONS_AT;

    out[pos] = OPT_MSG_TYPE;
    out[pos + 1] = 1;
    out[pos + 2] = msg_type;
    pos += 3;

    // RFC 2131 4.3.2: a REQUEST in SELECTING carries option 54 (accepted
    // server) and option 50 (offered address), which tell other servers their
    // offers were declined
    if let Some(srv) = server_ip {
        out[pos] = OPT_SERVER;
        out[pos + 1] = 4;
        out[pos + 2..pos + 6].copy_from_slice(srv);
        pos += 6;
    }
    if let Some(req) = requested_ip {
        out[pos] = OPT_REQUESTED_IP;
        out[pos + 1] = 4;
        out[pos + 2..pos + 6].copy_from_slice(req);
        pos += 6;
    }

    out[pos] = OPT_PARAM_REQ;
    out[pos + 1] = 3;
    out[pos + 2] = OPT_SUBNET;
    out[pos + 3] = OPT_ROUTER;
    out[pos + 4] = OPT_DNS;
    pos += 5;

    out[pos] = OPT_END;
    pos += 1;

    // Padded to 300 bytes: some servers and relays drop a shorter BOOTP message
    while pos < 300 {
        out[pos] = 0;
        pos += 1;
    }
    300
}

/// Reads a server reply, accepted only if addressed to this client and the
/// awaited message. An OFFER is not accepted as an ACK, and the transaction id
/// alone is not enough
pub fn parse_reply(buf: &[u8], xid: u32, our_mac: &[u8; 6], want: u8) -> Option<Lease> {
    if buf.len() < OPTIONS_AT {
        return None;
    }
    // BOOTREPLY, not another client's broadcast request
    if buf[0] != 2 {
        return None;
    }
    if u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) != xid {
        return None;
    }
    if &buf[28..34] != our_mac.as_slice() {
        return None;
    }
    if buf[236..240] != MAGIC {
        return None;
    }

    let mut yiaddr = [0u8; 4];
    yiaddr.copy_from_slice(&buf[16..20]);

    let mut mask = [255u8, 255, 255, 0];
    let mut gw = [0u8; 4];
    let mut dns = [8u8, 8, 8, 8];
    let mut server = [0u8; 4];
    let mut msg_type = 0u8;
    let mut lease_secs = 0u32;

    let mut i = OPTIONS_AT;
    while i < buf.len() {
        let opt = buf[i];
        if opt == OPT_END {
            break;
        }
        // Pad, the only option besides END with no length byte
        if opt == 0 {
            i += 1;
            continue;
        }
        if i + 1 >= buf.len() {
            break;
        }
        let len = buf[i + 1] as usize;
        if i + 2 + len > buf.len() {
            break;
        }
        let data = &buf[i + 2..i + 2 + len];
        match opt {
            OPT_MSG_TYPE if len >= 1 => msg_type = data[0],
            OPT_SUBNET if len >= 4 => mask.copy_from_slice(&data[..4]),
            OPT_ROUTER if len >= 4 => gw.copy_from_slice(&data[..4]),
            OPT_DNS if len >= 4 => dns.copy_from_slice(&data[..4]),
            OPT_SERVER if len >= 4 => server.copy_from_slice(&data[..4]),
            OPT_LEASE if len >= 4 => {
                lease_secs = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            }
            _ => {}
        }
        // A zero-length option still costs two header bytes, so the walk
        // terminates
        i += 2 + len;
    }

    if msg_type != want {
        // A NAK tells the caller its REQUEST was refused, so it does not wait
        // out the timeout
        if msg_type == NAK {
            return Some(Lease {
                ip: [0; 4], mask, gw, dns, server, lease_secs: 0, nak: true,
            });
        }
        return None;
    }

    Some(Lease { ip: yiaddr, mask, gw, dns, server, lease_secs, nak: false })
}

/// Wrap a built DHCP message in UDP, IP and ethernet, ready for the wire
pub fn frame(
    payload: &[u8],
    mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    dst_mac: &[u8; 6],
    out: &mut [u8],
) -> usize {
    let mut udp_buf = [0u8; 512];
    let udp_len = super::udp::build(
        CLIENT_PORT, SERVER_PORT, payload, src_ip, dst_ip, &mut udp_buf,
    );
    if udp_len == 0 {
        return 0;
    }
    let mut ip_buf = [0u8; 600];
    let ip_len = ipv4::build(src_ip, dst_ip, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 {
        return 0;
    }
    super::eth::EthFrame::build(
        dst_mac, mac, super::eth::ETHERTYPE_IP, &ip_buf[..ip_len], out,
    )
}
