use core::sync::atomic::Ordering;
use super::CTRL_C;
use super::tcp::TcpSocket;
use super::tls_crypto::{
    cbc_encrypt, cbc_decrypt, tls_pad, tls_unpad,
    Sha256State, hmac_sha1, hmac_sha256, prf_sha256,
};
use super::tls_rsa::{parse_rsa_public_key, rsa_pkcs1_encrypt};
use super::tls_ecdh::{ecdh_keypair, ecdh_shared};
use super::tls_gcm::{aes128gcm_seal, aes128gcm_open, hkdf_extract, hkdf_expand_label, derive_secret};
use super::tls_crypto::sha256;

const RT_CHANGE_CIPHER_SPEC: u8 = 20;
const RT_ALERT:              u8 = 21;
const RT_HANDSHAKE:          u8 = 22;
const RT_APP_DATA:           u8 = 23;

const HT_CLIENT_HELLO:        u8 = 1;
const HT_SERVER_HELLO:        u8 = 2;
const HT_CERTIFICATE:         u8 = 11;
const HT_SERVER_HELLO_DONE:   u8 = 14;
const HT_CLIENT_KEY_EXCHANGE: u8 = 16;
const HT_FINISHED:            u8 = 20;

const TLS12: [u8; 2] = [0x03, 0x03];

const CS_RSA_AES128_SHA:          [u8; 2] = [0x00, 0x2F];
const CS_RSA_AES128_SHA256:       [u8; 2] = [0x00, 0x3C];
const CS_ECDHE_RSA_AES128_SHA256: [u8; 2] = [0xC0, 0x27];
const CS_ECDHE_RSA_AES128_SHA:    [u8; 2] = [0xC0, 0x13];
const HT_SERVER_KEY_EXCHANGE:     u8      = 12;

const CS_AES128_GCM_SHA256:     [u8; 2] = [0x13, 0x01];
const HT_ENCRYPTED_EXTENSIONS:  u8      = 8;
const HT_CERTIFICATE_VERIFY:    u8      = 15;
const TLS13:                    [u8; 2] = [0x03, 0x04];

const ALERT_CLOSE_NOTIFY:          u8 = 0;
const ALERT_HANDSHAKE_FAILURE:     u8 = 40;
const ALERT_PROTOCOL_VERSION:      u8 = 70;
const ALERT_INSUFFICIENT_SECURITY: u8 = 71;
const ALERT_INTERNAL_ERROR:        u8 = 80;

fn fill_random(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let v = crate::random::random_u64().to_le_bytes();
        let take = (buf.len() - i).min(8);
        buf[i..i + take].copy_from_slice(&v[..take]);
        i += take;
    }
}

fn make_record(rtype: u8, body: &[u8], out: &mut [u8]) -> usize {
    let len = body.len();
    out[0] = rtype;
    out[1] = 0x03;
    out[2] = 0x03;
    out[3] = (len >> 8) as u8;
    out[4] = len as u8;
    out[5..5 + len].copy_from_slice(body);
    5 + len
}

fn make_handshake(htype: u8, body: &[u8], out: &mut [u8]) -> usize {
    let len = body.len();
    out[0] = htype;
    out[1] = (len >> 16) as u8;
    out[2] = (len >> 8) as u8;
    out[3] = len as u8;
    out[4..4 + len].copy_from_slice(body);
    4 + len
}

fn alert_desc(code: u8) -> &'static str {
    match code {
        ALERT_CLOSE_NOTIFY           => "close_notify",
        40                           => "handshake_failure",
        ALERT_PROTOCOL_VERSION       => "protocol_version",
        ALERT_INSUFFICIENT_SECURITY  => "insufficient_security",
        ALERT_INTERNAL_ERROR         => "internal_error",
        20                           => "bad_record_mac",
        22                           => "record_overflow",
        42                           => "bad_certificate",
        43                           => "unsupported_certificate",
        44                           => "certificate_revoked",
        45                           => "certificate_expired",
        46                           => "certificate_unknown",
        47                           => "illegal_parameter",
        48                           => "unknown_ca",
        50                           => "decode_error",
        51                           => "decrypt_error",
        _                            => "unknown",
    }
}

fn compute_mac(mac_len: usize, key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    if mac_len == 20 {
        let m = hmac_sha1(&key[..20], data);
        out[..20].copy_from_slice(&m);
    } else {
        let m = hmac_sha256(key, data);
        out[..32].copy_from_slice(&m);
    }
    out
}

pub struct TlsStream {
    tcp:              TcpSocket,
    client_random:    [u8; 32],
    server_random:    [u8; 32],
    master_secret:    [u8; 48],
    mac_len:          usize,
    selected_cipher:  u16,
    client_mac_key:   [u8; 32],
    server_mac_key:   [u8; 32],
    client_key:       [u8; 16],
    server_key:       [u8; 16],
    client_seq:       u64,
    server_seq:       u64,
    cipher_active:    bool,
    raw:              [u8; 17408],
    raw_len:          usize,
    hs_buf:           [u8; 16384],
    hs_len:           usize,
    ecdh_priv:        [u8; 32],
    server_ec_pub:    [u8; 65],
    use_ecdhe:        bool,
    tls13:            bool,
    tls13_client_key: [u8; 16],
    tls13_server_key: [u8; 16],
    tls13_client_iv:  [u8; 12],
    tls13_server_iv:  [u8; 12],
    tls13_client_seq: u64,
    tls13_server_seq: u64,
    alpn_h2:          bool,
    pub rx_buf:       [u8; 8192],
    pub rx_len:       usize,
}

impl TlsStream {
    fn tcp_fill(&mut self, need: usize, timeout: usize) -> bool {
        for _ in 0..timeout {
            if CTRL_C.load(Ordering::SeqCst) { return false; }
            if self.raw_len >= need { return true; }
            if self.tcp.peer_closed { return false; }
            let mut temp = [0u8; 4096];
            let mut tlen = 0usize;
            self.tcp.recv_one_into(&mut temp, &mut tlen);
            if tlen > 0 {
                let copy = tlen.min(self.raw.len() - self.raw_len);
                self.raw[self.raw_len..self.raw_len + copy].copy_from_slice(&temp[..copy]);
                self.raw_len += copy;
            }
            if self.raw_len >= need { return true; }
            core::hint::spin_loop();
        }
        self.raw_len >= need
    }

    fn consume(&mut self, n: usize) {
        if n >= self.raw_len {
            self.raw_len = 0;
        } else {
            self.raw.copy_within(n..self.raw_len, 0);
            self.raw_len -= n;
        }
    }

    fn send_record(&mut self, rtype: u8, data: &[u8]) {
        if !self.cipher_active {
            let mut rec = [0u8; 4096];
            let n = make_record(rtype, data, &mut rec);
            self.tcp.send(&rec[..n]);
            return;
        }
        let mut mac_input = [0u8; 2048];
        mac_input[0..8].copy_from_slice(&self.client_seq.to_be_bytes());
        mac_input[8]  = rtype;
        mac_input[9]  = 0x03;
        mac_input[10] = 0x03;
        mac_input[11] = (data.len() >> 8) as u8;
        mac_input[12] = data.len() as u8;
        mac_input[13..13 + data.len()].copy_from_slice(data);
        let mac = compute_mac(self.mac_len, &self.client_mac_key, &mac_input[..13 + data.len()]);
        let ml = self.mac_len;
        let mut plain = [0u8; 2048];
        plain[..data.len()].copy_from_slice(data);
        plain[data.len()..data.len() + ml].copy_from_slice(&mac[..ml]);
        let plain_len = data.len() + ml;
        let mut padded = [0u8; 2048];
        let padded_len = tls_pad(&plain[..plain_len], &mut padded);
        let mut iv = [0u8; 16];
        fill_random(&mut iv);
        let mut enc = [0u8; 2048];
        cbc_encrypt(&self.client_key, &iv, &padded[..padded_len], &mut enc);
        let total = 16 + padded_len;
        let mut rec = [0u8; 2100];
        rec[0] = rtype;
        rec[1] = 0x03;
        rec[2] = 0x03;
        rec[3] = (total >> 8) as u8;
        rec[4] = total as u8;
        rec[5..21].copy_from_slice(&iv);
        rec[21..21 + padded_len].copy_from_slice(&enc[..padded_len]);
        self.tcp.send(&rec[..5 + total]);
        self.client_seq += 1;
    }

    fn decrypt_record(&mut self, data_off: usize, data_len: usize) -> Option<usize> {
        let ml = self.mac_len;
        if data_len < 16 + ml { return None; }
        let rtype = self.raw[0];
        let iv: [u8; 16] = self.raw[data_off..data_off + 16].try_into().ok()?;
        let cipher = &self.raw[data_off + 16..data_off + data_len];
        let mut plain = [0u8; 17408];
        cbc_decrypt(&self.server_key, &iv, cipher, &mut plain);
        let unpadded = tls_unpad(&plain[..cipher.len()])?;
        if unpadded.len() < ml { return None; }
        let plain_data = &unpadded[..unpadded.len() - ml];
        let mac_got    = &unpadded[unpadded.len() - ml..];
        let mut mac_input = [0u8; 17408];
        mac_input[0..8].copy_from_slice(&self.server_seq.to_be_bytes());
        mac_input[8]  = rtype;
        mac_input[9]  = 0x03;
        mac_input[10] = 0x03;
        mac_input[11] = (plain_data.len() >> 8) as u8;
        mac_input[12] = plain_data.len() as u8;
        mac_input[13..13 + plain_data.len()].copy_from_slice(plain_data);
        let mac_exp = compute_mac(ml, &self.server_mac_key, &mac_input[..13 + plain_data.len()]);
        let mut diff = 0u8;
        for i in 0..ml {
            diff |= mac_got[i] ^ mac_exp[i];
        }
        if diff != 0 {
            crate::log_err!("tls: bad MAC seq={} cipher=0x{:04X}", self.server_seq, self.selected_cipher);
            return None;
        }
        self.server_seq += 1;
        let copy = plain_data.len().min(self.rx_buf.len() - self.rx_len);
        self.rx_buf[self.rx_len..self.rx_len + copy].copy_from_slice(&plain_data[..copy]);
        self.rx_len += copy;
        Some(copy)
    }

    fn derive_keys(&mut self, premaster: &[u8; 48]) {
        let mut seed = [0u8; 64];
        seed[..32].copy_from_slice(&self.client_random);
        seed[32..].copy_from_slice(&self.server_random);
        prf_sha256(premaster, b"master secret", &seed, &mut self.master_secret);
        let mut seed2 = [0u8; 64];
        seed2[..32].copy_from_slice(&self.server_random);
        seed2[32..].copy_from_slice(&self.client_random);
        let ml = self.mac_len;
        let kb_need = ml * 2 + 32;
        let mut kb = [0u8; 128];
        prf_sha256(&self.master_secret, b"key expansion", &seed2, &mut kb[..kb_need]);
        self.client_mac_key[..ml].copy_from_slice(&kb[..ml]);
        self.server_mac_key[..ml].copy_from_slice(&kb[ml..ml * 2]);
        self.client_key.copy_from_slice(&kb[ml * 2..ml * 2 + 16]);
        self.server_key.copy_from_slice(&kb[ml * 2 + 16..ml * 2 + 32]);
    }

    fn derive_keys_ecdhe(&mut self, shared: &[u8; 32]) {
        let mut seed = [0u8; 64];
        seed[..32].copy_from_slice(&self.client_random);
        seed[32..].copy_from_slice(&self.server_random);
        prf_sha256(shared, b"master secret", &seed, &mut self.master_secret);
        let mut seed2 = [0u8; 64];
        seed2[..32].copy_from_slice(&self.server_random);
        seed2[32..].copy_from_slice(&self.client_random);
        let ml = self.mac_len;
        let kb_need = ml * 2 + 32;
        let mut kb = [0u8; 128];
        prf_sha256(&self.master_secret, b"key expansion", &seed2, &mut kb[..kb_need]);
        self.client_mac_key[..ml].copy_from_slice(&kb[..ml]);
        self.server_mac_key[..ml].copy_from_slice(&kb[ml..ml * 2]);
        self.client_key.copy_from_slice(&kb[ml * 2..ml * 2 + 16]);
        self.server_key.copy_from_slice(&kb[ml * 2 + 16..ml * 2 + 32]);
    }

    fn finished_verify(master: &[u8; 48], label: &[u8], hs_hash: &[u8; 32]) -> [u8; 12] {
        let mut out = [0u8; 12];
        prf_sha256(master, label, hs_hash, &mut out);
        out
    }

    pub fn connect(host: &str, ip: [u8; 4], port: u16) -> Option<Self> {
        let tcp = TcpSocket::connect(ip, port)?;
        let mut stream = TlsStream {
            tcp,
            client_random:    [0u8; 32],
            server_random:    [0u8; 32],
            master_secret:    [0u8; 48],
            mac_len:          20,
            selected_cipher:  0x002F,
            client_mac_key:   [0u8; 32],
            server_mac_key:   [0u8; 32],
            client_key:       [0u8; 16],
            server_key:       [0u8; 16],
            client_seq:       0,
            server_seq:       0,
            cipher_active:    false,
            raw:              [0u8; 17408],
            raw_len:          0,
            hs_buf:           [0u8; 16384],
            hs_len:           0,
            ecdh_priv:        [0u8; 32],
            server_ec_pub:    [0u8; 65],
            use_ecdhe:        false,
            tls13:            false,
            tls13_client_key: [0u8; 16],
            tls13_server_key: [0u8; 16],
            tls13_client_iv:  [0u8; 12],
            tls13_server_iv:  [0u8; 12],
            tls13_client_seq: 0,
            tls13_server_seq: 0,
            alpn_h2:          false,
            rx_buf:           [0u8; 8192],
            rx_len:           0,
        };
        stream.do_handshake(host)?;
        Some(stream)
    }

    fn tls13_send(&mut self, content_type: u8, data: &[u8]) {
        let mut plain = [0u8; 17408];
        let n = data.len();
        plain[..n].copy_from_slice(data);
        plain[n] = content_type;
        let plain_len = n + 1;
        let mut nonce = self.tls13_client_iv;
        let seq_be = self.tls13_client_seq.to_be_bytes();
        for i in 0..8 { nonce[4 + i] ^= seq_be[i]; }
        self.tls13_client_seq += 1;
        let enc_len = plain_len + 16;
        let aad: [u8; 5] = [RT_APP_DATA, 0x03, 0x03, (enc_len >> 8) as u8, enc_len as u8];
        let mut enc = [0u8; 17448];
        let enc_n = aes128gcm_seal(&self.tls13_client_key, &nonce, &aad, &plain[..plain_len], &mut enc);
        let mut rec = [0u8; 17460];
        rec[0] = RT_APP_DATA;
        rec[1..3].copy_from_slice(&[0x03, 0x03]);
        rec[3] = (enc_n >> 8) as u8;
        rec[4] = enc_n as u8;
        rec[5..5 + enc_n].copy_from_slice(&enc[..enc_n]);
        self.tcp.send(&rec[..5 + enc_n]);
    }

    fn do_handshake(&mut self, host: &str) -> Option<()> {
        let mut hs_hash = Sha256State::new();

        let unix_time = (crate::vfs::procfs::uptime_ticks() / 18) as u32;
        self.client_random[0..4].copy_from_slice(&unix_time.to_be_bytes());
        fill_random(&mut self.client_random[4..]);

        let mut ecdh_rand = [0u8; 32];
        fill_random(&mut ecdh_rand);
        let (ecdh_priv, ecdh_pub) = ecdh_keypair(&ecdh_rand);
        self.ecdh_priv = ecdh_priv;

        let sni_bytes = host.as_bytes();
        let mut ch_body = [0u8; 800];
        let mut p = 0usize;

        ch_body[p..p+2].copy_from_slice(&[0x03, 0x03]); p += 2;
        ch_body[p..p+32].copy_from_slice(&self.client_random); p += 32;
        ch_body[p] = 32; p += 1;
        fill_random(&mut ch_body[p..p+32]); p += 32;

        ch_body[p..p+2].copy_from_slice(&[0, 12]); p += 2;
        ch_body[p..p+2].copy_from_slice(&CS_AES128_GCM_SHA256);       p += 2;
        ch_body[p..p+2].copy_from_slice(&CS_ECDHE_RSA_AES128_SHA256); p += 2;
        ch_body[p..p+2].copy_from_slice(&CS_ECDHE_RSA_AES128_SHA);    p += 2;
        ch_body[p..p+2].copy_from_slice(&CS_RSA_AES128_SHA256);       p += 2;
        ch_body[p..p+2].copy_from_slice(&CS_RSA_AES128_SHA);          p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x00, 0xFF]);               p += 2;

        ch_body[p] = 1; p += 1;
        ch_body[p] = 0; p += 1;

        let ext_start = p;
        p += 2;

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x2B]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 5]);        p += 2;
        ch_body[p] = 4; p += 1;
        ch_body[p..p+2].copy_from_slice(&TLS13); p += 2;
        ch_body[p..p+2].copy_from_slice(&TLS12); p += 2;

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x33]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 71]);       p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 69]);       p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x00, 0x17]);  p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 65]);        p += 2;
        ch_body[p..p+65].copy_from_slice(&ecdh_pub);      p += 65;

        ch_body[p..p+2].copy_from_slice(&[0, 0]);                                                       p += 2;
        ch_body[p..p+2].copy_from_slice(&((sni_bytes.len() + 5) as u16).to_be_bytes()); p += 2;
        ch_body[p..p+2].copy_from_slice(&((sni_bytes.len() + 3) as u16).to_be_bytes()); p += 2;
        ch_body[p] = 0; p += 1;
        ch_body[p..p+2].copy_from_slice(&(sni_bytes.len() as u16).to_be_bytes());       p += 2;
        ch_body[p..p+sni_bytes.len()].copy_from_slice(sni_bytes); p += sni_bytes.len();

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x0D]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 14]);       p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 12]);       p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x08, 0x04]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x08, 0x05]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x08, 0x06]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x04, 0x01]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x05, 0x01]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x04, 0x03]); p += 2;

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x0A]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 4]);        p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 2]);        p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x00, 0x17]);  p += 2;

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x0B]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0, 2]);        p += 2;
        ch_body[p] = 1; p += 1;
        ch_body[p] = 0; p += 1;

        ch_body[p..p+2].copy_from_slice(&[0x00, 0x10]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x00, 0x0E]); p += 2;
        ch_body[p..p+2].copy_from_slice(&[0x00, 0x0C]); p += 2;
        ch_body[p] = 2; p += 1;
        ch_body[p..p+2].copy_from_slice(b"h2"); p += 2;
        ch_body[p] = 8; p += 1;
        ch_body[p..p+8].copy_from_slice(b"http/1.1"); p += 8;

        let ext_len = p - ext_start - 2;
        ch_body[ext_start..ext_start+2].copy_from_slice(&(ext_len as u16).to_be_bytes());

        let mut hs_msg = [0u8; 900];
        let hs_len = make_handshake(HT_CLIENT_HELLO, &ch_body[..p], &mut hs_msg);
        hs_hash.update(&hs_msg[..hs_len]);
        self.send_record(RT_HANDSHAKE, &hs_msg[..hs_len]);
        crate::log!("tls: ClientHello sent (SNI={}, TLS1.3+ECDHE+RSA)", host);

        let mut server_cert      = [0u8; 8192];
        let mut cert_len         = 0usize;
        let mut got_shd          = false;
        let mut tls13_agreed     = false;
        let mut handshake_done   = false;
        let mut server_hs_key    = [0u8; 16];
        let mut server_hs_iv     = [0u8; 12];
        let mut client_hs_key    = [0u8; 16];
        let mut client_hs_iv     = [0u8; 12];
        let mut hs_secret        = [0u8; 32];
        let mut hs_hash_after_sh = [0u8; 32];

        'recv: for _ in 0..100_000 {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            if handshake_done { break; }

            if !self.tcp_fill(5, 20000) {
                if self.tcp.peer_closed {
                    crate::log!("tls13: peer_closed, breaking");
                    break 'recv;
                }
                continue;
            }

            let rtype   = self.raw[0];
            let rec_len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
            crate::log!("tls13: recv rtype={} len={} seq={}", rtype, rec_len, self.tls13_server_seq);

            if !self.tcp_fill(5 + rec_len, 20000) {
                if self.tcp.peer_closed {
                    crate::log!("tls13: peer_closed, breaking");
                    break 'recv;
                }
                continue;
            }

            if rtype == RT_ALERT {
                crate::log_err!("tls: Alert {}", alert_desc(self.raw[6]));
                return None;
            }

            if rtype == RT_HANDSHAKE {
                let payload = &self.raw[5..5+rec_len];
                if self.hs_len + rec_len <= self.hs_buf.len() {
                    self.hs_buf[self.hs_len..self.hs_len+rec_len].copy_from_slice(payload);
                    self.hs_len += rec_len;
                }
                let mut hp = 0;
                while hp + 4 <= self.hs_len {
                    let htype = self.hs_buf[hp];
                    let hlen  = ((self.hs_buf[hp+1] as usize) << 16)
                              | ((self.hs_buf[hp+2] as usize) << 8)
                              |  (self.hs_buf[hp+3] as usize);
                    if hp + 4 + hlen > self.hs_len { break; }
                    let hmsg = &self.hs_buf[hp..hp+4+hlen];
                    hs_hash.update(hmsg);
                    match htype {
                        HT_SERVER_HELLO => {
                            if hlen >= 38 {
                                self.server_random.copy_from_slice(&hmsg[6..38]);
                                let sid_len = hmsg[38] as usize;
                                let cs_off  = 38 + 1 + sid_len;
                                if cs_off + 2 <= hmsg.len() {
                                    let cs = u16::from_be_bytes([hmsg[cs_off], hmsg[cs_off+1]]);
                                    self.selected_cipher = cs;
                                    crate::log!("tls: ServerHello cipher=0x{:04X}", cs);
                                    let ext_off = cs_off + 3;
                                    if ext_off + 2 <= hmsg.len() {
                                        let ext_total = u16::from_be_bytes([hmsg[ext_off], hmsg[ext_off+1]]) as usize;
                                        let mut ep    = ext_off + 2;
                                        let ext_end   = (ext_off + 2 + ext_total).min(hmsg.len());
                                        while ep + 4 <= ext_end {
                                            let etype = u16::from_be_bytes([hmsg[ep], hmsg[ep+1]]);
                                            let elen  = u16::from_be_bytes([hmsg[ep+2], hmsg[ep+3]]) as usize;
                                            ep += 4;
                                            if etype == 0x002B && elen >= 2 && ep + elen <= ext_end {
                                                let ver = u16::from_be_bytes([hmsg[ep], hmsg[ep+1]]);
                                                if ver == 0x0304 {
                                                    tls13_agreed = true;
                                                    crate::log!("tls: TLS 1.3 negotiated");
                                                }
                                            }
                                            if etype == 0x0033 && elen >= 4 && ep + elen <= ext_end {
                                                let key_len = u16::from_be_bytes([hmsg[ep+2], hmsg[ep+3]]) as usize;
                                                if ep + 4 + key_len <= ext_end && key_len == 65 {
                                                    self.server_ec_pub.copy_from_slice(&hmsg[ep+4..ep+4+65]);
                                                    crate::log!("tls: ServerHello key_share received");
                                                }
                                            }
                                            if etype == 0x0010 && elen >= 3 && ep + elen <= ext_end {
                                                let proto_len = hmsg[ep+2] as usize;
                                                if ep + 3 + proto_len <= ext_end {
                                                    let proto = &hmsg[ep+3..ep+3+proto_len];
                                                    self.alpn_h2 = proto == b"h2";
                                                    crate::log!("tls: ALPN='{}'",
                                                        core::str::from_utf8(proto).unwrap_or("?"));
                                                }
                                            }
                                            ep += elen;
                                        }
                                    }
                                    if !tls13_agreed {
                                        self.mac_len  = match cs { 0x003C | 0xC027 => 32, _ => 20 };
                                        self.use_ecdhe = cs == 0xC027 || cs == 0xC013;
                                    }
                                    if tls13_agreed {
                                        let shared = ecdh_shared(&self.ecdh_priv, &self.server_ec_pub)?;
                                        let hash_after_sh = hs_hash.clone_finalize();
                                        hs_hash_after_sh  = hash_after_sh;
                                        let zeros32      = [0u8; 32];
                                        let early_secret = hkdf_extract(&zeros32, &zeros32);
                                        let derived_salt = derive_secret(&early_secret, b"derived", &sha256(b""));
                                        hs_secret        = hkdf_extract(&derived_salt, &shared);
                                        let c_hs = derive_secret(&hs_secret, b"c hs traffic", &hash_after_sh);
                                        let s_hs = derive_secret(&hs_secret, b"s hs traffic", &hash_after_sh);
                                        hkdf_expand_label(&c_hs, b"key", b"", &mut client_hs_key);
                                        hkdf_expand_label(&c_hs, b"iv",  b"", &mut client_hs_iv);
                                        hkdf_expand_label(&s_hs, b"key", b"", &mut server_hs_key);
                                        hkdf_expand_label(&s_hs, b"iv",  b"", &mut server_hs_iv);
                                        self.tls13 = true;
                                        crate::log!("tls: handshake keys derived");
                                    }
                                }
                            }
                        }
                        HT_CERTIFICATE => {
                            if hlen > 6 {
                                let ctx_len  = hmsg[4] as usize;
                                let list_off = 5 + ctx_len;
                                if list_off + 6 <= hmsg.len() {
                                    let first_cert_len = ((hmsg[list_off+3] as usize) << 16)
                                                       | ((hmsg[list_off+4] as usize) << 8)
                                                       |  (hmsg[list_off+5] as usize);
                                    cert_len = first_cert_len.min(8192);
                                    if list_off + 6 + cert_len <= hmsg.len() {
                                        server_cert[..cert_len].copy_from_slice(&hmsg[list_off+6..list_off+6+cert_len]);
                                    }
                                    crate::log!("tls: Certificate received ({} bytes)", cert_len);
                                }
                            }
                        }
                        HT_SERVER_HELLO_DONE => { got_shd = true; }
                        HT_SERVER_KEY_EXCHANGE => {
                            let body = &hmsg[4..];
                            if body.len() >= 69 && body[0] == 3 {
                                let curve  = u16::from_be_bytes([body[1], body[2]]);
                                let pt_len = body[3] as usize;
                                if curve == 0x0017 && pt_len == 65 {
                                    self.server_ec_pub.copy_from_slice(&body[4..69]);
                                    crate::log!("tls: ServerKeyExchange EC secp256r1");
                                }
                            }
                        }
                        _ => {}
                    }
                    hp += 4 + hlen;
                }
                if hp > 0 {
                    self.hs_buf.copy_within(hp..self.hs_len, 0);
                    self.hs_len -= hp;
                }
            }

            if rtype == RT_APP_DATA && tls13_agreed {
                let payload = self.raw[5..5+rec_len].to_vec();
                let mut nonce = server_hs_iv;
                let seq_be = self.tls13_server_seq.to_be_bytes();
                for i in 0..8 { nonce[4+i] ^= seq_be[i]; }
                self.tls13_server_seq += 1;
                let aad: [u8; 5] = [RT_APP_DATA, 0x03, 0x03,
                    (rec_len >> 8) as u8, rec_len as u8];
                let mut plain = [0u8; 17408];
                if let Some(n) = aes128gcm_open(&server_hs_key, &nonce, &aad, &payload, &mut plain) {
                    if n > 0 {
                        let content_type = plain[n-1];
                        if content_type == 21 {
                            crate::log!("tls13: ALERT level={} desc={}", plain[0], plain[1]);
                        }
                        crate::log!("tls13: decrypted n={} ctype={}", n, content_type);
                        let data         = &plain[..n-1];
                        if content_type == RT_HANDSHAKE {
                            if self.hs_len + data.len() <= self.hs_buf.len() {
                                self.hs_buf[self.hs_len..self.hs_len+data.len()].copy_from_slice(data);
                                self.hs_len += data.len();
                            }
                            let mut hp = 0;
                            while hp + 4 <= self.hs_len {
                                let htype = self.hs_buf[hp];
                                let hlen  = ((self.hs_buf[hp+1] as usize) << 16)
                                          | ((self.hs_buf[hp+2] as usize) << 8)
                                          |  (self.hs_buf[hp+3] as usize);
                                if hp + 4 + hlen > self.hs_len {
                                    crate::log!("tls13: break hp={} hlen={} hs_len={}", hp, hlen, self.hs_len);
                                    break;
                                }
                                let hmsg = &self.hs_buf[hp..hp+4+hlen];
                                hs_hash.update(hmsg);
                                crate::log!("tls13: inner htype={} hlen={} hp={} hs_len={}", htype, hlen, hp, self.hs_len);
                                match htype {
                                    HT_ENCRYPTED_EXTENSIONS => {
                                        crate::log!("tls13: EncryptedExtensions");
                                        let body = &hmsg[4..];
                                        if body.len() >= 2 {
                                            let ext_total = u16::from_be_bytes([body[0], body[1]]) as usize;
                                            let mut ep = 2;
                                            let ext_end = (2 + ext_total).min(body.len());
                                            while ep + 4 <= ext_end {
                                                let etype = u16::from_be_bytes([body[ep], body[ep+1]]);
                                                let elen  = u16::from_be_bytes([body[ep+2], body[ep+3]]) as usize;
                                                ep += 4;
                                                if etype == 0x0010 && elen >= 3 && ep + elen <= ext_end {
                                                    let proto_len = body[ep+2] as usize;
                                                    if ep + 3 + proto_len <= ext_end {
                                                        let proto = &body[ep+3..ep+3+proto_len];
                                                        self.alpn_h2 = proto == b"h2";
                                                        crate::log!("tls13: EE ALPN='{}'",
                                                            core::str::from_utf8(proto).unwrap_or("?"));
                                                    }
                                                }
                                                ep += elen;
                                            }
                                        }
                                    }
                                    HT_CERTIFICATE => {
                                        let ctx_len  = if hmsg.len() > 4 { hmsg[4] as usize } else { 0 };
                                        let list_off = 5 + ctx_len;
                                        if list_off + 6 <= hmsg.len() {
                                            let first_cert_len = ((hmsg[list_off+3] as usize) << 16)
                                                               | ((hmsg[list_off+4] as usize) << 8)
                                                               |  (hmsg[list_off+5] as usize);
                                            cert_len = first_cert_len.min(8192);
                                            if list_off + 6 + cert_len <= hmsg.len() {
                                                server_cert[..cert_len].copy_from_slice(
                                                    &hmsg[list_off+6..list_off+6+cert_len]);
                                            }
                                            crate::log!("tls13: Certificate {} bytes", cert_len);
                                        }
                                    }
                                    HT_CERTIFICATE_VERIFY => {
                                        crate::log!("tls13: CertificateVerify");
                                    }
                                    HT_FINISHED => {
                                        crate::log!("tls13: ServerFinished received");
                                        let hash_after_sf = hs_hash.clone_finalize();
                                        let zeros32    = [0u8; 32];
                                        let derived2   = derive_secret(&hs_secret, b"derived", &sha256(b""));
                                        let app_secret = hkdf_extract(&derived2, &zeros32);
                                        let c_app      = derive_secret(&app_secret, b"c ap traffic", &hash_after_sf);
                                        let s_app      = derive_secret(&app_secret, b"s ap traffic", &hash_after_sf);
                                        hkdf_expand_label(&s_app, b"key", b"", &mut self.tls13_server_key);
                                        hkdf_expand_label(&s_app, b"iv",  b"", &mut self.tls13_server_iv);
                                        self.tls13_server_seq = 0;

                                        let hash_for_fin = hs_hash.clone_finalize();
                                        let c_hs = derive_secret(&hs_secret, b"c hs traffic", &hs_hash_after_sh);
                                        let mut fin_key = [0u8; 32];
                                        hkdf_expand_label(&c_hs, b"finished", b"", &mut fin_key);
                                        let verify = hmac_sha256(&fin_key, &hash_for_fin);
                                        let mut fin_body = [0u8; 40];
                                        let fin_len = make_handshake(HT_FINISHED, &verify, &mut fin_body);

                                        self.tls13_client_key = client_hs_key;
                                        self.tls13_client_iv  = client_hs_iv;
                                        self.tls13_client_seq = 0;
                                        self.tls13_send(RT_HANDSHAKE, &fin_body[..fin_len]);

                                        hkdf_expand_label(&c_app, b"key", b"", &mut self.tls13_client_key);
                                        hkdf_expand_label(&c_app, b"iv",  b"", &mut self.tls13_client_iv);
                                        self.tls13_client_seq = 0;
                                        crate::log!("tls13: ClientFinished sent");
                                        handshake_done = true;
                                    }
                                    _ => {}
                                }
                                hp += 4 + hlen;
                            }
                            if hp > 0 {
                                self.hs_buf.copy_within(hp..self.hs_len, 0);
                                self.hs_len -= hp;
                            }
                        }
                    }
                } else {
                    crate::log_err!("tls13: decrypt failed seq={}", self.tls13_server_seq - 1);
                }
                self.consume(5+rec_len);
                continue;
            }

            if rtype == RT_CHANGE_CIPHER_SPEC { self.consume(5+rec_len); continue; }

            if rtype == RT_HANDSHAKE && !tls13_agreed && got_shd {
                self.consume(5+rec_len);
                break 'recv;
            }

            self.consume(5+rec_len);
        }

        if tls13_agreed {
            if !handshake_done {
                crate::log_err!("tls13: handshake incomplete");
                return None;
            }
            self.cipher_active = true;
            crate::log!("tls: handshake complete ({}{})", self.cipher_name(),
                if self.alpn_h2 { " h2" } else { "" });
            return Some(());
        }

        if cert_len == 0 {
            crate::log_err!("tls: no certificate received");
            return None;
        }

        if self.use_ecdhe {
            let shared = match ecdh_shared(&self.ecdh_priv, &self.server_ec_pub) {
                Some(s) => s,
                None => { crate::log_err!("tls: ECDH failed"); return None; }
            };
            self.derive_keys_ecdhe(&shared);
            let mut cke_body = [0u8; 66];
            cke_body[0] = 65;
            cke_body[1..66].copy_from_slice(&ecdh_pub);
            let mut hs_cke = [0u8; 80];
            let hs_cke_len = make_handshake(HT_CLIENT_KEY_EXCHANGE, &cke_body, &mut hs_cke);
            hs_hash.update(&hs_cke[..hs_cke_len]);
            self.send_record(RT_HANDSHAKE, &hs_cke[..hs_cke_len]);
        } else {
            let rsa_key = match parse_rsa_public_key(&server_cert[..cert_len]) {
                Some(k) => k,
                None => { crate::log_err!("tls: RSA key parse failed"); return None; }
            };
            let mut premaster = [0u8; 48];
            premaster[0..2].copy_from_slice(&TLS12);
            fill_random(&mut premaster[2..]);
            self.derive_keys(&premaster);
            let mut enc_pm = [0u8; 256];
            let enc_len    = rsa_pkcs1_encrypt(&rsa_key, &premaster, &mut enc_pm);
            if enc_len == 0 { return None; }
            let mut cke_body = [0u8; 260];
            cke_body[0..2].copy_from_slice(&(enc_len as u16).to_be_bytes());
            cke_body[2..2+enc_len].copy_from_slice(&enc_pm[..enc_len]);
            let mut hs_cke = [0u8; 300];
            let hs_cke_len = make_handshake(HT_CLIENT_KEY_EXCHANGE, &cke_body[..2+enc_len], &mut hs_cke);
            hs_hash.update(&hs_cke[..hs_cke_len]);
            self.send_record(RT_HANDSHAKE, &hs_cke[..hs_cke_len]);
        }

        self.send_record(RT_CHANGE_CIPHER_SPEC, &[1]);
        self.cipher_active = true;

        let hs_digest = hs_hash.clone_finalize();
        let vd        = Self::finished_verify(&self.master_secret, b"client finished", &hs_digest);
        let mut fin_body = [0u8; 20];
        let fin_len      = make_handshake(HT_FINISHED, &vd, &mut fin_body);
        self.send_record(RT_HANDSHAKE, &fin_body[..fin_len]);
        hs_hash.update(&fin_body[..fin_len]);
        let sf_digest     = hs_hash.clone_finalize();
        let sf_expected   = Self::finished_verify(&self.master_secret, b"server finished", &sf_digest);
        crate::log!("tls: ClientFinished sent (TLS1.2)");

        let mut got_ccs = false;
        let mut got_fin = false;
        'server_fin: for _ in 0..50_000 {
            if CTRL_C.load(Ordering::SeqCst) { return None; }
            if !self.tcp_fill(5, 20000) {
                if self.tcp.peer_closed { break 'server_fin; }
                continue;
            }
            let rtype   = self.raw[0];
            let rec_len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
            if !self.tcp_fill(5+rec_len, 2000) {
                if self.tcp.peer_closed { break 'server_fin; }
                continue;
            }
            if rtype == RT_ALERT {
                if got_ccs {
                    if let Some(dec_len) = self.decrypt_record(5, rec_len) {
                        let off = self.rx_len - dec_len;
                        crate::log_err!("tls: Alert (enc) {}", alert_desc(self.rx_buf[off+1]));
                        self.rx_len -= dec_len;
                    }
                } else {
                    crate::log_err!("tls: Alert {}", alert_desc(self.raw[6]));
                }
                self.consume(5+rec_len);
                break;
            }
            if rtype == RT_CHANGE_CIPHER_SPEC { got_ccs = true; }
            if rtype == RT_HANDSHAKE && got_ccs {
                if let Some(dec_len) = self.decrypt_record(5, rec_len) {
                    let off = self.rx_len - dec_len;
                    if dec_len >= 16 && self.rx_buf[off] == HT_FINISHED {
                        let mut diff = 0u8;
                        for i in 0..12 {
                            diff |= self.rx_buf[off + 4 + i] ^ sf_expected[i];
                        }
                        self.rx_len -= dec_len;
                        if diff == 0 {
                            got_fin = true;
                        } else {
                            crate::log_err!("tls: server Finished verify_data mismatch");
                            self.consume(5+rec_len);
                            return None;
                        }
                    } else {
                        self.rx_len -= dec_len;
                    }
                }
            }
            self.consume(5+rec_len);
            if got_fin { break 'server_fin; }
        }

        if got_fin {
            crate::log!("tls: handshake complete ({})", self.cipher_name());
            Some(())
        } else {
            crate::log_err!("tls: handshake failed");
            None
        }
    }

    pub fn send(&mut self, data: &[u8]) -> bool {
        const CHUNK: usize = 1024;
        let mut off = 0;
        while off < data.len() {
            if CTRL_C.load(Ordering::SeqCst) { return false; }
            let end = (off + CHUNK).min(data.len());
            if self.tls13 {
                self.tls13_send(RT_APP_DATA, &data[off..end]);
            } else {
                self.send_record(RT_APP_DATA, &data[off..end]);
            }
            off = end;
        }
        true
    }

    pub fn recv_all(&mut self, timeout_iters: usize) -> &[u8] {
        let mut idle = 0usize;
        loop {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            if !self.tcp_fill(5, 100) {
                if self.tcp.peer_closed { break; }
                idle += 1;
                if idle > timeout_iters { break; }
                continue;
            }
            let rtype   = self.raw[0];
            let rec_len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
            if !self.tcp_fill(5+rec_len, 1000) { break; }
            idle = 0;
            if self.tls13 {
                crate::log!("tls13: recv_all rtype={} len={} seq={}", rtype, rec_len, self.tls13_server_seq);
                if rtype == RT_APP_DATA {
                    let payload = self.raw[5..5+rec_len].to_vec();
                    let mut nonce = self.tls13_server_iv;
                    let seq_be    = self.tls13_server_seq.to_be_bytes();
                    for i in 0..8 { nonce[4+i] ^= seq_be[i]; }
                    self.tls13_server_seq += 1;
                    let aad: [u8; 5] = [RT_APP_DATA, 0x03, 0x03,
                        (rec_len >> 8) as u8, rec_len as u8];
                    let mut plain = [0u8; 17408];
                    if let Some(n) = aes128gcm_open(&self.tls13_server_key, &nonce, &aad, &payload, &mut plain) {
                        if n > 0 {
                            let content_type = plain[n-1];
                            if content_type == 21 && n >= 3 {
                                crate::log!("tls13: recv_all ALERT level={} desc={}", plain[0], plain[1]);
                            }
                            crate::log!("tls13: recv_all decrypted n={} ctype={}", n, content_type);
                            if content_type == RT_ALERT { break; }
                            let data_len = n - 1;
                            let copy = data_len.min(self.rx_buf.len() - self.rx_len);
                            self.rx_buf[self.rx_len..self.rx_len+copy].copy_from_slice(&plain[..copy]);
                            self.rx_len += copy;
                        }
                    }
                }
                self.consume(5+rec_len);
            } else {
                match rtype {
                    RT_APP_DATA | RT_ALERT => {
                        if let Some(dec_len) = self.decrypt_record(5, rec_len) {
                            if rtype == RT_ALERT {
                                let off = self.rx_len - dec_len;
                                self.rx_len -= dec_len;
                                if self.rx_buf[off+1] != ALERT_CLOSE_NOTIFY { break; }
                                break;
                            }
                        } else { break; }
                    }
                    _ => {}
                }
                self.consume(5+rec_len);
            }
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn close(&mut self) {
        self.send_record(RT_ALERT, &[1, ALERT_CLOSE_NOTIFY]);
        self.tcp.close();
    }

    pub fn clear_rx(&mut self) {
        self.rx_len = 0;
    }

    pub fn is_closed(&self) -> bool {
        self.tcp.peer_closed
    }

    pub fn recv_chunk(&mut self, timeout: usize) -> &[u8] {
        self.rx_len = 0;
        for _ in 0..timeout {
            if CTRL_C.load(Ordering::SeqCst) { break; }
            if self.tcp.peer_closed { break; }
            if !self.tcp_fill(5, 100) { continue; }
            let rtype   = self.raw[0];
            let rec_len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
            if !self.tcp_fill(5+rec_len, 500) { break; }
            if self.tls13 {
                if rtype == RT_APP_DATA {
                    let payload = self.raw[5..5+rec_len].to_vec();
                    let mut nonce = self.tls13_server_iv;
                    let seq_be    = self.tls13_server_seq.to_be_bytes();
                    for i in 0..8 { nonce[4+i] ^= seq_be[i]; }
                    self.tls13_server_seq += 1;
                    let aad: [u8; 5] = [RT_APP_DATA, 0x03, 0x03,
                        (rec_len >> 8) as u8, rec_len as u8];
                    let mut plain = [0u8; 17408];
                    if let Some(n) = aes128gcm_open(&self.tls13_server_key, &nonce, &aad, &payload, &mut plain) {
                        if n > 0 {
                            let content_type = plain[n-1];
                            if content_type == RT_APP_DATA {
                                let copy = (n-1).min(self.rx_buf.len());
                                self.rx_buf[..copy].copy_from_slice(&plain[..copy]);
                                self.rx_len = copy;
                                self.consume(5+rec_len);
                                break;
                            }
                        }
                    }
                    self.consume(5+rec_len);
                } else {
                    self.consume(5+rec_len);
                }
            } else {
                match rtype {
                    23 => {
                        if self.decrypt_record(5, rec_len).is_some() {
                            self.consume(5+rec_len);
                            if self.rx_len > 0 { break; }
                        } else {
                            self.consume(5+rec_len);
                        }
                    }
                    21 => { self.consume(5+rec_len); break; }
                    _  => { self.consume(5+rec_len); }
                }
            }
        }
        &self.rx_buf[..self.rx_len]
    }

    pub fn cipher_name(&self) -> &'static str {
        match self.selected_cipher {
            0x1301 => "TLS13_AES_128_GCM_SHA256",
            0x003C => "TLS_RSA_WITH_AES_128_CBC_SHA256",
            0x002F => "TLS_RSA_WITH_AES_128_CBC_SHA",
            0xC027 => "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256",
            0xC013 => "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
            _      => "unknown",
        }
    }

    pub fn is_tls13(&self) -> bool { self.tls13 }

    pub fn is_h2(&self) -> bool { self.alpn_h2 }

    pub fn try_recv_more(&mut self) -> bool {
        if !self.tcp_fill(5, 200) { return false; }
        let rtype   = self.raw[0];
        let rec_len = u16::from_be_bytes([self.raw[3], self.raw[4]]) as usize;
        if !self.tcp_fill(5 + rec_len, 500) { return false; }
        match rtype {
            23 => {
                if self.decrypt_record(5, rec_len).is_some() {
                    self.consume(5 + rec_len);
                    return true;
                }
            }
            _ => { self.consume(5 + rec_len); }
        }
        false
    }
}
