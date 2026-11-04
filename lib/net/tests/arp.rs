// The ARP table with a controlled clock

use mikunet::stack::arp::{self, ArpTable, ARP_TABLE_SIZE, ENTRY_TTL_MS};
use mikunet::stack::eth::{self, EthFrame};

const ME: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const GW: [u8; 6] = [0x52, 0x55, 0x0A, 0x00, 0x02, 0x02];
const OTHER: [u8; 6] = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
const MY_IP: [u8; 4] = [10, 0, 2, 15];
const GW_IP: [u8; 4] = [10, 0, 2, 2];

/// One ARP frame as it would arrive off the wire
fn frame(payload: &[u8], from: [u8; 6], buf: &mut [u8]) -> usize {
    EthFrame::build(&eth::BROADCAST_MAC, &from, eth::ETHERTYPE_ARP, payload, buf)
}

#[test]
fn a_mapping_is_remembered_and_then_forgotten() {
    let mut t = ArpTable::new();
    t.insert(GW_IP, GW, 1_000);

    assert_eq!(t.lookup(&GW_IP, 1_000), Some(GW));
    assert_eq!(t.lookup(&GW_IP, 1_000 + ENTRY_TTL_MS - 1), Some(GW), "still inside the lifetime");
    assert_eq!(
        t.lookup(&GW_IP, 1_000 + ENTRY_TTL_MS),
        None,
        "a gateway that changed its card must not stay unreachable until reboot"
    );

    // Expired but still known, which decides whether an overheard ARP is worth
    // learning from
    assert!(t.knows(&GW_IP));
}

#[test]
fn a_refresh_moves_the_deadline_and_can_change_the_card() {
    let mut t = ArpTable::new();
    t.insert(GW_IP, GW, 0);
    t.insert(GW_IP, OTHER, 30_000);

    assert_eq!(t.lookup(&GW_IP, 30_000), Some(OTHER), "the newer answer wins");
    assert_eq!(
        t.lookup(&GW_IP, ENTRY_TTL_MS + 1),
        Some(OTHER),
        "the clock restarts from the refresh, not from the first sighting"
    );

    // Reuses the slot
    assert_eq!(t.entries.iter().filter(|e| e.valid).count(), 1);
}

#[test]
fn the_unspecified_address_is_not_a_mapping() {
    // A host with no address sends ARP probes from 0.0.0.0; that is not a
    // mapping
    let mut t = ArpTable::new();
    t.insert([0, 0, 0, 0], OTHER, 0);
    assert!(!t.knows(&[0, 0, 0, 0]));
    assert_eq!(t.lookup(&[0, 0, 0, 0], 0), None);
}

#[test]
fn a_full_table_evicts_something_expired_before_something_live() {
    let mut t = ArpTable::new();
    // Fill it, all learned at time zero
    for i in 0..ARP_TABLE_SIZE {
        t.insert([10, 0, 1, i as u8], [i as u8; 6], 0);
    }
    assert_eq!(t.entries.iter().filter(|e| e.valid).count(), ARP_TABLE_SIZE);

    // One live entry learned much later, then a new address when everything
    // from time zero has expired
    t.insert(GW_IP, GW, ENTRY_TTL_MS);
    let now = ENTRY_TTL_MS + 1;
    t.insert([10, 0, 9, 9], OTHER, now);

    assert_eq!(t.lookup(&GW_IP, now), Some(GW), "the live entry survived");
    assert_eq!(t.lookup(&[10, 0, 9, 9], now), Some(OTHER), "the new one landed");
}

#[test]
fn a_question_addressed_to_us_gets_an_answer() {
    let mut t = ArpTable::new();
    let ask = arp::build_request(&GW, &GW_IP, &MY_IP);
    let mut wire = [0u8; 64];
    let n = frame(&ask, GW, &mut wire);
    let f = EthFrame::parse(&wire[..n]).unwrap();

    let mut out = [0u8; 64];
    let len = arp::handle(&f, &ME, &MY_IP, &mut t, 5_000, &mut out);
    assert!(len > 0, "a request for our own address must be answered");

    let reply = EthFrame::parse(&out[..len]).unwrap();
    assert_eq!(reply.dst, GW, "answered to the asker, not broadcast");
    assert_eq!(reply.src, ME);
    assert_eq!(reply.ethertype, eth::ETHERTYPE_ARP);

    let (sender_ip, sender_mac, op) = arp::parse(reply.payload).unwrap();
    assert_eq!(op, 2, "an ARP reply");
    assert_eq!(sender_ip, MY_IP);
    assert_eq!(sender_mac, ME);

    // Learned the asker's mapping as well
    assert_eq!(t.lookup(&GW_IP, 5_000), Some(GW));
}

#[test]
fn a_conversation_between_two_strangers_teaches_us_nothing() {
    // Broadcast requests between other machines must not fill the table and
    // evict the gateway
    let mut t = ArpTable::new();
    t.insert(GW_IP, GW, 0);

    let stranger_ip = [10, 0, 2, 77];
    let ask = arp::build_request(&OTHER, &stranger_ip, &[10, 0, 2, 88]);
    let mut wire = [0u8; 64];
    let n = frame(&ask, OTHER, &mut wire);
    let f = EthFrame::parse(&wire[..n]).unwrap();

    let mut out = [0u8; 64];
    let len = arp::handle(&f, &ME, &MY_IP, &mut t, 1_000, &mut out);

    assert_eq!(len, 0, "not addressed to us, so nothing to say");
    assert!(!t.knows(&stranger_ip), "and nothing to learn");
    assert_eq!(t.lookup(&GW_IP, 1_000), Some(GW), "the gateway is still there");
}

#[test]
fn a_reply_is_learned_from_even_unasked() {
    // A reply is directed traffic and the answer to our question
    let mut t = ArpTable::new();
    let reply = arp::build_reply(&GW, &GW_IP, &ME, &MY_IP);
    let mut wire = [0u8; 64];
    let n = frame(&reply, GW, &mut wire);
    let f = EthFrame::parse(&wire[..n]).unwrap();

    let mut out = [0u8; 64];
    let len = arp::handle(&f, &ME, &MY_IP, &mut t, 2_000, &mut out);
    assert_eq!(len, 0, "a reply needs no reply");
    assert_eq!(t.lookup(&GW_IP, 2_000), Some(GW));
}

#[test]
fn a_refresh_of_something_we_already_track_is_taken() {
    let mut t = ArpTable::new();
    t.insert(GW_IP, OTHER, 0);

    // A request from the gateway to a third party: not to us, but it updates a
    // held mapping
    let ask = arp::build_request(&GW, &GW_IP, &[10, 0, 2, 88]);
    let mut wire = [0u8; 64];
    let n = frame(&ask, GW, &mut wire);
    let f = EthFrame::parse(&wire[..n]).unwrap();

    let mut out = [0u8; 64];
    arp::handle(&f, &ME, &MY_IP, &mut t, 9_000, &mut out);
    assert_eq!(t.lookup(&GW_IP, 9_000), Some(GW), "the card behind a known address changed");
}

#[test]
fn a_malformed_packet_is_dropped_rather_than_read() {
    let good = arp::build_request(&GW, &GW_IP, &MY_IP);

    assert!(arp::parse(&good[..27]).is_none(), "shorter than an ARP packet");

    let mut wrong_hw = good;
    wrong_hw[1] = 6; // hardware type 6, not Ethernet
    assert!(arp::parse(&wrong_hw).is_none());

    let mut wrong_proto = good;
    wrong_proto[2] = 0x86; // IPv6
    wrong_proto[3] = 0xDD;
    assert!(arp::parse(&wrong_proto).is_none());

    let mut wrong_lens = good;
    wrong_lens[4] = 8; // eight-byte hardware addresses
    assert!(
        arp::parse(&wrong_lens).is_none(),
        "the address lengths decide where every later field is - believing \
         them without checking reads the wrong bytes as a MAC"
    );
}

#[test]
fn a_request_frame_goes_out_broadcast_and_parses_back() {
    let mut out = [0u8; 64];
    let n = arp::request_frame(&ME, &MY_IP, &GW_IP, &mut out);
    let f = EthFrame::parse(&out[..n]).unwrap();

    assert_eq!(f.dst, eth::BROADCAST_MAC, "nobody knows who to ask yet");
    assert_eq!(f.src, ME);
    let (sender_ip, sender_mac, op) = arp::parse(f.payload).unwrap();
    assert_eq!(op, 1);
    assert_eq!(sender_ip, MY_IP);
    assert_eq!(sender_mac, ME);
    assert_eq!(arp::target_ip(f.payload), Some(GW_IP));
}
