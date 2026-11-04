// Generates a /etc/shadow line
//
//   cargo run --release --example mkshadow -- <user> <password> [m_cost] [t_cost]
use mikuauth::argon2::Params;
use mikuauth::hash::encode_into;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: mkshadow <user> <password> [m_cost_kib] [t_cost]");
        std::process::exit(2);
    }
    let m: u32 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(65536);
    let t: u32 = a.get(4).map(|s| s.parse().unwrap()).unwrap_or(3);

    // Salt from the OS. `read_exact`, since /dev/urandom never reports end of
    // file
    let salt: [u8; 16] = {
        use std::io::Read;
        let mut b = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .expect("no entropy source")
            .read_exact(&mut b)
            .expect("short read from /dev/urandom");
        b
    };

    let mut buf = [0u8; 256];
    let start = Instant::now();
    let field = encode_into(
        a[2].as_bytes(),
        &salt,
        Params { m_cost: m, t_cost: t, p_cost: 1 },
        &mut buf,
    )
    .unwrap();
    let took = start.elapsed();

    // 20334 is 2025-09-03; last_change is days since the epoch
    println!("{}:{}:20334:0:99999:7:::", a[1], field);
    eprintln!("[mkshadow] m={m} KiB t={t} took {took:?}");
}
