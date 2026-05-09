use super::eth::{EthFrame, BROADCAST_MAC, ETHERTYPE_IP};
use super::ipv4;
use super::udp;

const DHCP_DISCOVER: u8 = 1;
const DHCP_OFFER: u8 = 2;
const DHCP_REQUEST: u8 = 3;
const DHCP_ACK: u8 = 5;

const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99];

const OPT_REQUESTED_IP: u8 = 50; // RFC 2131: MUST be present in REQUEST (SELECTING state)
const OPT_SUBNET: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_LEASE: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_SERVER: u8 = 54;
const OPT_PARAM_REQ: u8 = 55;
const OPT_END: u8 = 255;

pub struct DhcpResult {
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub gw: [u8; 4],
    pub dns: [u8; 4],
    pub server: [u8; 4],
}

fn build_packet(
    msg_type: u8,
    xid: u32,
    mac: &[u8; 6],
    ciaddr: &[u8; 4],
    server_ip: Option<&[u8; 4]>,
    requested_ip: Option<&[u8; 4]>,
    out: &mut [u8; 548],
) -> usize {
    out[0] = 1;
    out[1] = 1;
    out[2] = 6;
    out[3] = 0;
    out[4] = (xid >> 24) as u8;
    out[5] = (xid >> 16) as u8;
    out[6] = (xid >> 8) as u8;
    out[7] = xid as u8;
    // secs=0; flags bit15=1 (broadcast) so server broadcasts all replies -
    // required when ciaddr=0 and our IP stack cannot yet accept unicast to yiaddr
    out[8] = 0; out[9] = 0;   // secs
    out[10] = 0x80; out[11] = 0x00; // flags: broadcast bit set
    out[12..16].copy_from_slice(ciaddr);
    out[16..20].fill(0);
    out[20..24].fill(0);
    out[24..28].fill(0);
    out[28..34].copy_from_slice(mac);
    out[34..236].fill(0);
    out[236..240].copy_from_slice(&DHCP_MAGIC);

    let mut pos = 240usize;

    out[pos] = OPT_MSG_TYPE;
    out[pos + 1] = 1;
    out[pos + 2] = msg_type;
    pos += 3;

    // RFC 2131 §4.3.2: DHCP REQUEST in SELECTING state MUST include option 54
    // (server identifier) and option 50 (requested IP address = yiaddr from OFFER)
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

    while pos < 300 {
        out[pos] = 0;
        pos += 1;
    }

    300
}

fn parse_offer(buf: &[u8], xid: u32) -> Option<DhcpResult> {
    if buf.len() < 240 {
        return None;
    }
    let rx_xid = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if rx_xid != xid {
        return None;
    }
    if buf[236..240] != DHCP_MAGIC {
        return None;
    }

    let mut yiaddr = [0u8; 4];
    yiaddr.copy_from_slice(&buf[16..20]);

    let mut mask = [255u8, 255, 255, 0];
    let mut gw = [0u8; 4];
    let mut dns = [8u8, 8, 8, 8];
    let mut server = [0u8; 4];
    let mut msg_type = 0u8;

    let mut i = 240;
    while i < buf.len() {
        let opt = buf[i];
        if opt == OPT_END {
            break;
        }
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
            _ => {}
        }
        i += 2 + len;
    }

    if msg_type != DHCP_OFFER && msg_type != DHCP_ACK {
        return None;
    }

    Some(DhcpResult { ip: yiaddr, mask, gw, dns, server })
}

pub fn do_dhcp() -> Option<DhcpResult> {
    let xid: u32 = 0xDEAD_BEEF;
    let mac = super::get_mac();
    let zero_ip = [0u8; 4];
    let bcast_ip = [255u8; 4];
    let src_ip = [0u8; 4];

    let mut dhcp_buf = [0u8; 548];
    let dhcp_len = build_packet(DHCP_DISCOVER, xid, &mac, &zero_ip, None, None, &mut dhcp_buf);

    // Retry DISCOVER up to 4 times. On real hardware the PHY may have only
    // just finished auto-negotiation when the user types dhcp, and the
    // first frame can race the link state and hit the floor. Each attempt
    // waits +_4s (1000 ticks at 250Hz) which is enough headroom for a typical
    // home router with ARP+OFFER latency
    crate::serial_println!(
        "[dhcp] start xid={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        xid, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    let mut offer: Option<DhcpResult> = None;
    for attempt in 1..=4u32 {
        crate::serial_println!("[dhcp] DISCOVER {}/4", attempt);
        if let Some(o) = send_and_recv(
            &src_ip, &bcast_ip, &mac, &BROADCAST_MAC,
            &dhcp_buf[..dhcp_len], xid, 1000,
        ) {
            offer = Some(o);
            break;
        }
        crate::serial_println!("[dhcp] discover {}/4 timeout", attempt);
    }
    let offer = offer?;

    let server = offer.server;
    crate::serial_println!(
        "[dhcp] offer ip={}.{}.{}.{} server={}.{}.{}.{}",
        offer.ip[0], offer.ip[1], offer.ip[2], offer.ip[3],
        server[0], server[1], server[2], server[3]
    );

    let offered_ip = offer.ip;
    let dhcp_len = build_packet(DHCP_REQUEST, xid, &mac, &zero_ip, Some(&server), Some(&offered_ip), &mut dhcp_buf);

    let mut ack: Option<DhcpResult> = None;
    for attempt in 1..=4u32 {
        crate::serial_println!("[dhcp] request {}/4", attempt);
        if let Some(a) = send_and_recv(
            &src_ip, &bcast_ip, &mac, &BROADCAST_MAC,
            &dhcp_buf[..dhcp_len], xid, 1000,
        ) {
            ack = Some(a);
            break;
        }
    }
    let ack = ack?;
    crate::serial_println!(
        "[dhcp] ACK ip={}.{}.{}.{}",
        ack.ip[0], ack.ip[1], ack.ip[2], ack.ip[3]
    );

    Some(ack)
}

fn send_and_recv(
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    src_mac: &[u8; 6],
    dst_mac: &[u8; 6],
    dhcp_payload: &[u8],
    xid: u32,
    timeout_ticks: u64,
) -> Option<DhcpResult> {
    let mut udp_buf = [0u8; 600];
    let udp_len = udp::build(68, 67, dhcp_payload, src_ip, dst_ip, &mut udp_buf);
    if udp_len == 0 {
        return None;
    }

    let mut ip_buf = [0u8; 650];
    let ip_len = ipv4::build(src_ip, dst_ip, ipv4::PROTO_UDP, &udp_buf[..udp_len], &mut ip_buf);
    if ip_len == 0 {
        return None;
    }

    let mut eth_buf = [0u8; 700];
    let eth_len = EthFrame::build(dst_mac, src_mac, ETHERTYPE_IP, &ip_buf[..ip_len], &mut eth_buf);
    if eth_len == 0 {
        return None;
    }

    {
        let mut state = super::NET.lock();
        let mut sent_ok = false;
        if let Some(drv) = state.driver.as_mut() {
            sent_ok = drv.send(&eth_buf[..eth_len]);
        }
        if sent_ok {
            state.tx_count += 1;
        } else {
            crate::serial_println!("[dhcp] driver send returned false (TX queue busy?)");
        }
    }

    let start = crate::vfs::procfs::uptime_ticks();
    x86_64::instructions::interrupts::enable();
    loop {
        let now = crate::vfs::procfs::uptime_ticks();
        if now.wrapping_sub(start) > timeout_ticks {
            return None;
        }

        x86_64::instructions::hlt();

        let mut result: Option<DhcpResult> = None;
        {
            let mut state = super::NET.lock();
            let mut found: Option<DhcpResult> = None;
            if let Some(drv) = state.driver.as_mut() {
                drv.recv(&mut |raw| {
                    if found.is_some() { return; }
                    if let Some(frame) = EthFrame::parse(raw) {
                        if frame.ethertype != ETHERTYPE_IP { return; }
                        if let Some(ip) = ipv4::Ipv4Header::parse(frame.payload) {
                            if ip.proto != ipv4::PROTO_UDP { return; }
                            let payload = ip.payload(frame.payload);
                            if payload.len() < 8 { return; }
                            let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
                            if dst_port != 68 { return; }
                            let udp_payload = &payload[8..];
                            if let Some(r) = parse_offer(udp_payload, xid) {
                                found = Some(r);
                            }
                        }
                    }
                });
            }
            result = found;
        }

        if let Some(r) = result {
            return Some(r);
        }
    }
}
