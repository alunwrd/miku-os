// Two TCP connections facing each other with no network. The wire is a function
// call, the clock a variable, and losing a segment a `continue`

use mikunet::stack::conn::{
    Connection, Failure, Keepalive, INIT_RTO_MS, MAX_RETRIES, MAX_RTO_MS, MIN_RTO_MS,
};
use mikunet::stack::tcp::{TcpSegment, TcpState, FLAG_ACK, FLAG_FIN, FLAG_RST, FLAG_SYN};

const CLIENT_IP: [u8; 4] = [10, 0, 2, 15];
const SERVER_IP: [u8; 4] = [10, 0, 2, 99];
const CPORT: u16 = 49152;
const SPORT: u16 = 80;

/// A segment on its way from one side to the other
struct Wire {
    raw: Vec<u8>,
}

impl Wire {
    fn seg(&self) -> TcpSegment {
        TcpSegment::parse(&self.raw).expect("we built it, it must parse")
    }
    fn payload(&self) -> Vec<u8> {
        self.seg().payload(&self.raw).to_vec()
    }
    fn opts(&self) -> Vec<u8> {
        self.seg().options(&self.raw).to_vec()
    }
}

fn drain(c: &mut Connection) -> Vec<Wire> {
    let mut out = Vec::new();
    while let Some(raw) = c.take_segment() {
        out.push(Wire { raw });
    }
    out
}

fn give(to: &mut Connection, w: &Wire, now: u64) {
    to.on_segment(&w.seg(), &w.payload(), &w.opts(), now);
}

/// Moves everything each side has to say until both are quiet. Returns the
/// segment count
fn settle(a: &mut Connection, b: &mut Connection, now: u64) -> usize {
    let mut moved = 0;
    for _ in 0..64 {
        let from_a = drain(a);
        let from_b = drain(b);
        if from_a.is_empty() && from_b.is_empty() {
            return moved;
        }
        for w in &from_a {
            give(b, w, now);
            moved += 1;
        }
        for w in &from_b {
            give(a, w, now);
            moved += 1;
        }
    }
    panic!("the two sides never stopped talking - an ACK is answering an ACK");
}

/// A connected pair, handshake done
fn connected(client_isn: u32, server_isn: u32, now: u64) -> (Connection, Connection) {
    let mut client = Connection::connect(CLIENT_IP, CPORT, SERVER_IP, SPORT, client_isn, now);

    let syn = drain(&mut client);
    assert_eq!(syn.len(), 1, "connect sends exactly one SYN");
    let s = syn[0].seg();
    assert!(s.has(FLAG_SYN) && !s.has(FLAG_ACK));
    assert_eq!(s.seq, client_isn);

    let mut server = Connection::accept(
        SERVER_IP, SPORT, CLIENT_IP, CPORT, server_isn,
        s.seq, s.window, mikunet::stack::tcp::parse_syn_options(&syn[0].opts()), now,
    );
    assert_eq!(server.state, TcpState::SynReceived);

    settle(&mut client, &mut server, now);
    assert_eq!(client.state, TcpState::Established, "client finished the handshake");
    assert_eq!(server.state, TcpState::Established, "server finished the handshake");
    (client, server)
}

/// Read everything a side has, as bytes
fn read_all(c: &mut Connection) -> Vec<u8> {
    let mut out = vec![0u8; c.rx_available()];
    let n = c.read(&mut out);
    out.truncate(n);
    out
}

// the handshake

#[test]
fn three_segments_and_both_sides_are_open() {
    let (client, server) = connected(1000, 5000, 0);
    assert_eq!(client.rcv_nxt(), 5001, "the server's SYN took one number");
    assert_eq!(server.rcv_nxt(), 1001, "and so did ours");
    assert_eq!(client.snd_nxt(), 1001);
    assert_eq!(server.snd_nxt(), 5001);
    assert!(client.is_connected() && server.is_connected());
}

#[test]
fn a_syn_ack_for_a_handshake_we_did_not_start_is_ignored() {
    let mut client = Connection::connect(CLIENT_IP, CPORT, SERVER_IP, SPORT, 1000, 0);
    let _ = drain(&mut client);

    // Right ports, wrong acknowledgement. Accepting would open a connection on
    // another handshake, enabling blind injection
    let mut raw = [0u8; 64];
    let n = mikunet::stack::tcp::build(
        SPORT, CPORT, 7777, 999, FLAG_SYN | FLAG_ACK, 8192, b"",
        &SERVER_IP, &CLIENT_IP, &mut raw,
    );
    let forged = Wire { raw: raw[..n].to_vec() };
    give(&mut client, &forged, 0);
    assert_eq!(client.state, TcpState::SynSent, "still waiting for the real one");

    // And the real one, acknowledging the SYN we actually sent, is taken
    let n = mikunet::stack::tcp::build(
        SPORT, CPORT, 7777, 1001, FLAG_SYN | FLAG_ACK, 8192, b"",
        &SERVER_IP, &CLIENT_IP, &mut raw,
    );
    give(&mut client, &Wire { raw: raw[..n].to_vec() }, 0);
    assert_eq!(client.state, TcpState::Established);
}

#[test]
fn a_handshake_nobody_answers_gives_up_after_three_tries() {
    let mut client = Connection::connect(CLIENT_IP, CPORT, SERVER_IP, SPORT, 1000, 0);
    let _ = drain(&mut client); // the first SYN

    let mut now = 0u64;
    let mut syns = 0;
    for _ in 0..40 {
        now += INIT_RTO_MS as u64;
        client.on_tick(now);
        syns += drain(&mut client).len();
        if client.state == TcpState::Closed {
            break;
        }
    }
    assert_eq!(client.state, TcpState::Closed);
    assert_eq!(client.failure, Some(Failure::SynTimeout));
    assert_eq!(syns, 3, "three retries and then the connection is called dead");
}

// carrying data

#[test]
fn bytes_written_on_one_side_are_read_on_the_other() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    let msg = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    assert_eq!(client.write(msg), msg.len());
    client.on_tick(10);
    settle(&mut client, &mut server, 10);

    assert_eq!(read_all(&mut server), msg);
    assert_eq!(client.tx_pending(), 0, "and the sender let go of them");

    let reply = b"HTTP/1.1 200 OK\r\n\r\nhello";
    server.write(reply);
    server.on_tick(20);
    settle(&mut client, &mut server, 20);
    assert_eq!(read_all(&mut client), reply);
}

#[test]
fn more_than_one_segment_worth_arrives_in_order() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    // More than one segment and not a multiple of one
    let body: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let mut written = 0;
    let mut now = 0u64;
    for _ in 0..64 {
        written += client.write(&body[written..]);
        now += 10;
        client.on_tick(now);
        settle(&mut client, &mut server, now);
        if written == body.len() && client.tx_pending() == 0 {
            break;
        }
    }
    assert_eq!(written, body.len());
    assert_eq!(read_all(&mut server), body, "every byte, in order");
}

#[test]
fn the_numbers_wrap_past_the_end_of_the_space() {
    // Starts 200 bytes short of 2^32 and sends a kilobyte across the wrap
    let isn = u32::MAX - 200;
    let (mut client, mut server) = connected(isn, isn.wrapping_sub(50), 0);

    let body: Vec<u8> = (0..1000u32).map(|i| (i % 97) as u8).collect();
    let mut written = 0;
    let mut now = 0u64;
    for _ in 0..64 {
        written += client.write(&body[written..]);
        now += 10;
        client.on_tick(now);
        settle(&mut client, &mut server, now);
        if written == body.len() && client.tx_pending() == 0 {
            break;
        }
    }

    assert_eq!(read_all(&mut server), body, "the transfer survived the wrap");
    assert!(
        client.snd_nxt() < isn,
        "and the sequence number really did come round: {} < {}",
        client.snd_nxt(),
        isn
    );
}

// loss and repair

#[test]
fn a_lost_segment_is_sent_again_when_the_timer_expires() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    client.write(b"the segment that gets lost");
    client.on_tick(0);
    let lost = drain(&mut client);
    assert_eq!(lost.len(), 1);
    // Not delivered

    // Reads the timeout: the handshake was a round trip, so the timer has a
    // measurement
    let rto = client.rto_ms() as u64;
    assert_eq!(rto, MIN_RTO_MS as u64, "one fast sample puts it on the floor");

    // Nothing before the timer is due
    client.on_tick(rto - 1);
    assert!(drain(&mut client).is_empty(), "too early to give up on it");

    client.on_tick(rto);
    let again = drain(&mut client);
    assert_eq!(again.len(), 1, "the timer expired and it went out again");
    assert_eq!(again[0].seg().seq, lost[0].seg().seq, "the same bytes, the same number");
    assert_eq!(again[0].payload(), lost[0].payload());

    // This time it arrives
    give(&mut server, &again[0], rto);
    settle(&mut client, &mut server, rto);
    assert_eq!(read_all(&mut server), b"the segment that gets lost");
}

#[test]
fn the_timer_doubles_on_every_retransmission_and_then_gives_up() {
    let (mut client, _server) = connected(1000, 5000, 0);

    client.write(b"nobody is listening any more");
    client.on_tick(0);
    let _ = drain(&mut client);

    let mut now = 0u64;
    let mut sends = 0;
    let mut rtos = vec![client.rto_ms()];
    for _ in 0..64 {
        now += client.rto_ms() as u64;
        client.on_tick(now);
        sends += drain(&mut client).len();
        if client.state == TcpState::Closed {
            break;
        }
        rtos.push(client.rto_ms());
    }

    assert_eq!(client.state, TcpState::Closed);
    assert_eq!(client.failure, Some(Failure::RetransmitLimit));
    assert_eq!(sends, MAX_RETRIES, "five tries, then the connection is dead");

    // Each wait is twice the last, RFC 6298 5.5
    for pair in rtos.windows(2) {
        assert_eq!(pair[1], pair[0] * 2, "backoff went {} -> {}", pair[0], pair[1]);
    }
}

#[test]
fn three_duplicate_acknowledgements_do_not_wait_for_the_timer() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    // Two segments; the first is lost, the second arrives
    client.write(&[b'a'; 600]);
    client.on_tick(0);
    let first = drain(&mut client);
    client.write(&[b'b'; 600]);
    client.on_tick(1);
    let second = drain(&mut client);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);

    // The server reports the gap three times
    for _ in 0..3 {
        give(&mut server, &second[0], 2);
    }
    let dups = drain(&mut server);
    assert!(dups.len() >= 3, "a gap produces a duplicate ACK per segment");
    for d in &dups {
        assert_eq!(d.seg().ack, first[0].seg().seq, "all pointing at the hole");
    }

    let before = client.rto_ms();
    for d in &dups {
        give(&mut client, d, 2);
    }
    let repair = drain(&mut client);
    assert!(!repair.is_empty(), "the hole was filled without waiting for the timer");
    assert_eq!(repair[0].seg().seq, first[0].seg().seq);
    assert_eq!(
        client.rto_ms(),
        before,
        "a fast retransmit is not a timeout, so the timer must not back off"
    );
}

#[test]
fn a_segment_that_arrives_early_is_held_until_the_gap_closes() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    client.write(&[b'1'; 500]);
    client.on_tick(0);
    let first = drain(&mut client);
    client.write(&[b'2'; 500]);
    client.on_tick(1);
    let second = drain(&mut client);

    // Delivered backwards, as if on different paths
    give(&mut server, &second[0], 2);
    assert_eq!(server.rx_available(), 0, "nothing is in order yet");
    assert_eq!(server.held_out_of_order(), 1, "but it was kept, not thrown away");

    give(&mut server, &first[0], 3);
    let got = read_all(&mut server);
    assert_eq!(got.len(), 1000, "both segments, once the gap closed");
    assert!(got[..500].iter().all(|&b| b == b'1'));
    assert!(got[500..].iter().all(|&b| b == b'2'));
    assert_eq!(server.held_out_of_order(), 0);
}

#[test]
fn a_duplicate_of_something_already_taken_is_acknowledged_and_dropped() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    client.write(b"once");
    client.on_tick(0);
    let seg = drain(&mut client);
    give(&mut server, &seg[0], 1);
    assert_eq!(read_all(&mut server), b"once");

    // Same segment again: a retransmission whose ACK was lost
    let _ = drain(&mut server);
    give(&mut server, &seg[0], 2);
    assert_eq!(server.rx_available(), 0, "the bytes must not be delivered twice");
    let acks = drain(&mut server);
    assert!(!acks.is_empty(), "but it must be acknowledged, or the peer keeps trying");
}

// closing

#[test]
fn a_fin_riding_on_the_last_data_segment_is_seen() {
    // Servers often set FIN on the last data segment. The FIN's number follows
    // the data, so it must not read as out of order
    let (mut client, mut server) = connected(1000, 5000, 0);

    server.write(b"the last of it");
    server.close(0);
    server.on_tick(0);
    let out = drain(&mut server);

    // The segment carrying both, as `close` after `write` must produce
    let combined = out.iter().find(|w| w.seg().has(FLAG_FIN) && !w.payload().is_empty());
    let separate = out.iter().any(|w| w.seg().has(FLAG_FIN) && w.payload().is_empty());
    assert!(
        combined.is_some() || separate,
        "close must produce a FIN one way or the other"
    );

    for w in &out {
        give(&mut client, w, 1);
    }
    assert_eq!(read_all(&mut client), b"the last of it", "the data arrived");
    assert!(client.peer_closed, "and so did the FIN, in the same breath");
    assert_eq!(client.state, TcpState::CloseWait);
}

#[test]
fn both_sides_close_and_end_up_closed() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    client.write(b"bye");
    client.close(0);
    client.on_tick(0);
    settle(&mut client, &mut server, 1);

    assert_eq!(read_all(&mut server), b"bye");
    assert!(server.peer_closed);
    assert_eq!(server.state, TcpState::CloseWait);
    assert_eq!(
        client.state,
        TcpState::FinWait2,
        "our FIN was acknowledged, so only theirs is outstanding"
    );

    server.close(2);
    server.on_tick(2);
    settle(&mut client, &mut server, 3);

    assert_eq!(server.state, TcpState::Closed);
    assert_eq!(client.state, TcpState::Closed);
    assert_eq!(client.failure, None, "an orderly close is not a failure");
}

#[test]
fn data_written_before_close_still_goes_out() {
    let (mut client, mut server) = connected(1000, 5000, 0);

    // More than one segment, then close at once. The FIN queues behind all of
    // it
    let body = vec![b'x'; 3000];
    let mut written = client.write(&body);
    client.close(0);

    let mut now = 0u64;
    for _ in 0..64 {
        written += client.write(&body[written..]);
        now += 10;
        client.on_tick(now);
        settle(&mut client, &mut server, now);
        if server.peer_closed {
            break;
        }
    }
    assert_eq!(read_all(&mut server).len(), body.len(), "nothing was cut off");
    assert!(server.peer_closed);
}

#[test]
fn a_reset_ends_it_wherever_it_lands() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    client.write(b"midway");
    client.on_tick(0);
    let _ = drain(&mut client);

    server.reset();
    let rst = drain(&mut server);
    assert!(rst.iter().any(|w| w.seg().has(FLAG_RST)));
    for w in &rst {
        give(&mut client, w, 1);
    }

    assert_eq!(client.state, TcpState::Closed);
    assert_eq!(client.failure, Some(Failure::Reset));
    assert!(client.peer_closed);
    assert_eq!(client.write(b"more"), 0, "a closed connection takes nothing");
}

// the round-trip timer

#[test]
fn a_retransmitted_segment_gives_no_timing_sample() {
    // Karn's algorithm: an ACK of a segment sent twice gives no RTT sample
    let (mut client, mut server) = connected(1000, 5000, 0);

    client.write(b"lost the first time");
    client.on_tick(0);
    let _lost = drain(&mut client); // not delivered

    let before = client.rto_ms();
    client.on_tick(before as u64);
    let again = drain(&mut client);
    assert_eq!(again.len(), 1);
    let backed_off = client.rto_ms();
    assert_eq!(backed_off, before * 2, "the timeout doubled on the retransmission");

    // An instant ACK would otherwise be a near-zero sample, collapsing the
    // timer to its floor
    give(&mut server, &again[0], before as u64);
    for w in drain(&mut server) {
        give(&mut client, &w, before as u64);
    }
    assert_eq!(
        client.rto_ms(),
        backed_off,
        "no sample was taken, so the timeout is untouched"
    );
    assert!(client.rto_ms() > MIN_RTO_MS);
}

#[test]
fn a_clean_round_trip_does_move_the_timer() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    assert_eq!(client.rto_ms(), MIN_RTO_MS, "the handshake was its own first sample");

    client.write(b"and back");
    client.on_tick(1000);
    let seg = drain(&mut client);
    give(&mut server, &seg[0], 1100); // 100 ms out
    for w in drain(&mut server) {
        give(&mut client, &w, 1200); // and 100 ms back
    }
    assert!(
        client.rto_ms() >= MIN_RTO_MS,
        "a real sample keeps the timeout at or above the floor"
    );
    assert!(client.rto_ms() < INIT_RTO_MS, "and below the untimed default");
}

// flow control

#[test]
fn a_peer_that_advertises_nothing_still_gets_one_segment() {
    // Zero-window deadlock: the peer closes its window and the reopening update
    // is lost. One segment is always allowed with nothing in flight, and that
    // probe breaks it
    let (mut client, mut server) = connected(1000, 5000, 0);

    // Fills the server's receive window without reading, so it advertises less
    // and less
    let mut now = 0u64;
    let chunk = vec![b'z'; 4000];
    for _ in 0..12 {
        client.write(&chunk);
        now += 10;
        client.on_tick(now);
        settle(&mut client, &mut server, now);
    }
    assert!(server.rx_available() > 0, "the server is holding data it has not read");

    // Whatever the window says, the client is not wedged with nothing on the
    // wire and nothing queued
    client.write(b"probe");
    now += 10;
    client.on_tick(now);
    assert!(
        client.has_output() || client.inflight_segments() > 0,
        "a sender with data must always have something outstanding"
    );
}

#[test]
fn a_write_bigger_than_the_buffer_is_taken_in_pieces() {
    let (mut client, _server) = connected(1000, 5000, 0);
    let huge = vec![b'q'; mikunet::stack::conn::TX_CAP + 50_000];
    let took = client.write(&huge);
    assert!(took > 0 && took < huge.len(), "took {took} of {}", huge.len());
    assert_eq!(client.write(&huge[took..]), 0, "and nothing more until it drains");
}

// keepalive

fn keepalive(idle_ms: u64, interval_ms: u64, count: u32) -> Keepalive {
    Keepalive { enabled: true, idle_ms, interval_ms, count }
}

#[test]
fn a_silent_connection_is_probed_and_a_live_peer_answers() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    client.set_keepalive(keepalive(10_000, 1_000, 3), 0);

    client.on_tick(9_999);
    assert!(drain(&mut client).is_empty(), "nothing before the idle time is up");

    client.on_tick(10_000);
    let probe = drain(&mut client);
    assert_eq!(probe.len(), 1, "one probe");
    let s = probe[0].seg();
    assert!(s.has(FLAG_ACK) && !s.has(FLAG_SYN) && !s.has(FLAG_FIN));
    assert_eq!(s.seq, client.snd_nxt().wrapping_sub(1), "one behind what was acknowledged");
    assert!(probe[0].payload().is_empty());

    // The peer takes it as a duplicate and answers
    give(&mut server, &probe[0], 10_000);
    let answer = drain(&mut server);
    assert_eq!(answer.len(), 1, "a live TCP answers a probe");
    assert_eq!(server.rx_available(), 0, "and does not take it as data");
    give(&mut client, &answer[0], 10_001);

    // Heard from: the next probe is a whole idle time away
    client.on_tick(19_000);
    assert!(drain(&mut client).is_empty());
    assert!(client.is_connected());
}

#[test]
fn a_peer_that_never_answers_ends_the_connection() {
    let (mut client, _server) = connected(1000, 5000, 0);
    client.set_keepalive(keepalive(10_000, 1_000, 3), 0);

    for (at, expect) in [(10_000, 1), (10_500, 0), (11_000, 1), (12_000, 1)] {
        client.on_tick(at);
        assert_eq!(drain(&mut client).len(), expect, "probe count at {at} ms");
        assert!(client.is_connected());
    }
    client.on_tick(13_000);
    assert_eq!(client.state, TcpState::Closed, "three unanswered probes and it is over");
    assert_eq!(client.failure, Some(Failure::KeepaliveTimeout));
    assert!(client.peer_closed, "and a reader or poller is told");
}

#[test]
fn keepalive_off_sends_nothing_however_long_it_is_quiet() {
    let (mut client, _server) = connected(1000, 5000, 0);
    client.on_tick(100_000_000);
    assert!(drain(&mut client).is_empty());
    assert!(client.is_connected());
}

#[test]
fn a_connection_given_up_on_by_retransmission_is_seen_as_ended() {
    let (mut client, _server) = connected(1000, 5000, 0);
    client.write(b"into the void");
    client.on_tick(0);
    let _ = drain(&mut client);
    let mut now = 0;
    for _ in 0..=MAX_RETRIES + 1 {
        now += MAX_RTO_MS as u64 * 2;
        client.on_tick(now);
        let _ = drain(&mut client);
    }
    assert_eq!(client.failure, Some(Failure::RetransmitLimit));
    assert!(client.peer_closed, "a poller waiting on it has to wake");
}

// Window scaling, SACK, NewReno, reassembly by bytes

use mikunet::stack::conn::{INIT_CWND_SEGS, OUR_WSCALE, RX_CAP};
use mikunet::stack::tcp::{parse_sack, parse_syn_options, MAX_SEG};

#[test]
fn both_syns_carry_a_shift_and_scaling_is_on() {
    let (client, server) = connected(1000, 5000, 0);
    assert_eq!(client.window_scales(), (OUR_WSCALE, OUR_WSCALE));
    assert_eq!(server.window_scales(), (OUR_WSCALE, OUR_WSCALE));
    assert!(client.sack_permitted() && server.sack_permitted());
    assert!(OUR_WSCALE > 0 && (RX_CAP >> OUR_WSCALE) <= u16::MAX as usize);
}

#[test]
fn a_syn_without_options_gets_none_back() {
    // An old stack's SYN, with only an MSS. RFC 7323 and 2018 allow a shift or
    // SACK in the SYN-ACK only if the SYN had one
    let mut raw = [0u8; 64];
    let opts = mikunet::stack::tcp::mss_opts();
    let n = mikunet::stack::tcp::build_opts(
        CPORT, SPORT, 1000, 0, FLAG_SYN, 8192, &opts, b"", &CLIENT_IP, &SERVER_IP, &mut raw,
    );
    let syn = Wire { raw: raw[..n].to_vec() };
    let s = syn.seg();
    let mut server = Connection::accept(
        SERVER_IP, SPORT, CLIENT_IP, CPORT, 5000, s.seq, s.window, parse_syn_options(&syn.opts()), 0,
    );
    let synack = drain(&mut server);
    let said = parse_syn_options(&synack[0].opts());
    assert_eq!(said.wscale, None, "no shift offered back");
    assert!(!said.sack_permitted, "no SACK offered back");
    assert_eq!(server.window_scales(), (0, 0));
    assert!(!server.sack_permitted());
}

#[test]
fn a_window_that_arrives_backwards_is_put_back_together() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    let data: Vec<u8> = (0..20 * MAX_SEG).map(|i| (i * 7 % 251) as u8).collect();
    assert_eq!(client.write(&data), data.len());
    client.on_tick(0);
    let segs = drain(&mut client);
    assert!(segs.len() >= 10, "the initial window sends ten at once: {}", segs.len());
    // All but the first, last to first, then the first
    for w in segs[1..].iter().rev() {
        give(&mut server, w, 0);
    }
    assert_eq!(server.held_out_of_order(), 1, "one run, merged as it grew");
    give(&mut server, &segs[0], 0);
    assert_eq!(server.held_out_of_order(), 0);
    settle(&mut client, &mut server, 0);
    let mut got = read_all(&mut server);
    for _ in 0..8 {
        client.on_tick(0);
        settle(&mut client, &mut server, 0);
        got.extend(read_all(&mut server));
    }
    assert_eq!(got, data);
}

#[test]
fn an_ack_names_what_is_held_past_the_gap() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    let data = vec![b's'; 6 * MAX_SEG];
    client.write(&data);
    client.on_tick(0);
    let segs = drain(&mut client);
    // Loses the second; the third onward arrive
    give(&mut server, &segs[0], 0);
    let _ = drain(&mut server);
    for w in &segs[2..] {
        give(&mut server, w, 0);
    }
    let acks = drain(&mut server);
    let last = acks.last().expect("an ACK for each");
    let mut blocks = [(0u32, 0u32); 4];
    let n = parse_sack(&last.opts(), &mut blocks);
    assert_eq!(n, 1, "one run held");
    assert_eq!(blocks[0].0, segs[2].seg().seq, "from the first byte after the hole");
    let end = segs[segs.len() - 1].seg().seq.wrapping_add(segs[segs.len() - 1].payload().len() as u32);
    assert_eq!(blocks[0].1, end, "to the last byte held");
}

#[test]
fn slow_start_grows_the_window_and_a_loss_halves_it() {
    let (mut client, mut server) = connected(1000, 5000, 0);
    let mss = MAX_SEG;
    assert_eq!(client.cwnd(), INIT_CWND_SEGS * mss);
    let data = vec![b'w'; 40 * mss];
    client.write(&data);
    client.on_tick(0);
    settle(&mut client, &mut server, 0);
    assert!(client.cwnd() > INIT_CWND_SEGS * mss, "grown by what was acknowledged: {}", client.cwnd());

    // Loses one and lets the rest through: three duplicates start recovery
    let _ = read_all(&mut server);
    client.write(&vec![b'x'; 20 * mss]);
    client.on_tick(1);
    let segs = drain(&mut client);
    let before = client.cwnd();
    for w in &segs[1..] {
        give(&mut server, w, 1);
    }
    for w in drain(&mut server) {
        give(&mut client, &w, 1);
    }
    assert!(client.in_recovery(), "three duplicate ACKs");
    assert!(client.cwnd() < before, "the window came down: {} from {}", client.cwnd(), before);
    // The hole is resent at once, and recovery ends when it lands
    settle(&mut client, &mut server, 1);
    assert!(!client.in_recovery());
}

/// A wire losing one data segment in `every` on its first send,
/// deterministically, with a clock moving a millisecond a round. A
/// retransmission always arrives; this measures how fast the sender finds and
/// resends losses
fn transfer_over_lossy_wire(total: usize, every: usize) -> (Vec<u8>, usize, u64) {
    let mut seen = std::collections::HashSet::new();
    let (mut client, mut server) = connected(7, 9, 0);
    let data: Vec<u8> = (0..total).map(|i| (i * 31 % 253) as u8).collect();
    let mut written = 0;
    let mut got = Vec::new();
    let mut now = 0u64;
    let mut sent = 0usize;
    let mut segments = 0usize;
    while got.len() < total {
        now += 1;
        assert!(now < 60_000, "stalled at {} of {} bytes", got.len(), total);
        written += client.write(&data[written..]);
        client.on_tick(now);
        server.on_tick(now);
        for w in drain(&mut client) {
            if !w.payload().is_empty() {
                segments += 1;
                if seen.insert(w.seg().seq) {
                    sent += 1;
                    if sent % every == 0 {
                        continue;
                    }
                }
            }
            give(&mut server, &w, now);
        }
        for w in drain(&mut server) {
            give(&mut client, &w, now);
        }
        got.extend(read_all(&mut server));
    }
    assert_eq!(got.len(), total);
    (got, segments, now)
}

#[test]
fn four_megabytes_over_a_wire_that_loses_one_in_ten_arrive_whole() {
    let total = 4 << 20;
    let (got, segments, ms) = transfer_over_lossy_wire(total, 10);
    let expect: Vec<u8> = (0..total).map(|i| (i * 31 % 253) as u8).collect();
    assert!(got == expect, "every byte, in order");
    let needed = total.div_ceil(MAX_SEG);
    println!("4 MiB, one in ten lost: {segments} segments for {needed}, {ms} ms simulated");
    // A lost segment costs one retransmission: SACK says which, and recovery
    // resends only those
    assert!(segments < needed * 13 / 10, "{segments} segments for {needed} of data");
    // And without waiting out the retransmission timer each time
    assert!(ms < 2_000, "{ms} ms of simulated time");
}
