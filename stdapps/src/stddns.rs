//! Name resolution through `std`, tokio, and `libc`'s `getaddrinfo`, all
//! `libc_miku.so`'s (`lib/runtime/libmiku/libc/netdb.rs`)
//!
//! Order: literal, `localhost`, `/etc/hosts`, DNS to the lease's resolver
//! (`10.0.2.3` in QEMU). Also `AI_NUMERICHOST`, `AF_INET6`, a named service,
//! `AI_CANONNAME`, and telling a missing name from an unanswered query.

use std::ffi::{CStr, CString};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

fn check(name: &str, ok: bool, detail: String, failed: &mut i32) {
    if ok {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: {detail}");
        *failed += 1;
    }
}

/// `getaddrinfo`: error code, first address, and port
fn gai(node: Option<&str>, service: Option<&str>, family: i32, flags: i32) -> (i32, Option<SocketAddr>, Option<String>) {
    let node = node.map(|n| CString::new(n).unwrap());
    let service = service.map(|s| CString::new(s).unwrap());
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = family;
    hints.ai_socktype = libc::SOCK_STREAM;
    hints.ai_flags = flags;
    let mut res: *mut libc::addrinfo = std::ptr::null_mut();
    let rc = unsafe {
        libc::getaddrinfo(
            node.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            service.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            &hints,
            &mut res,
        )
    };
    if rc != 0 {
        return (rc, None, None);
    }
    let ai = unsafe { &*res };
    let sin = unsafe { &*(ai.ai_addr as *const libc::sockaddr_in) };
    let addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes())),
        u16::from_be(sin.sin_port),
    );
    let canon = (!ai.ai_canonname.is_null())
        .then(|| unsafe { CStr::from_ptr(ai.ai_canonname) }.to_string_lossy().into_owned());
    unsafe { libc::freeaddrinfo(res) };
    (0, Some(addr), canon)
}

fn main() {
    println!("stddns: names through std, tokio and getaddrinfo");
    let mut failed = 0;

    let lit: Vec<_> = "10.0.2.2:8000".to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    check("an address literal needs no resolver",
        lit == vec!["10.0.2.2:8000".parse::<SocketAddr>().unwrap()], format!("{lit:?}"), &mut failed);

    let lo: Vec<_> = ("localhost", 8000).to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
    check("localhost is the loopback address, through std",
        lo == vec!["127.0.0.1:8000".parse::<SocketAddr>().unwrap()], format!("{lo:?}"), &mut failed);

    // Waits for the DHCP lease; before it a name gives EAI_AGAIN
    let up = (0..300).any(|_| {
        let net = std::fs::read_to_string("/proc/net").unwrap_or_default();
        let ok = net.lines().any(|l| l.starts_with("ip: ") && l != "ip: 0.0.0.0");
        if !ok { std::thread::sleep(std::time::Duration::from_millis(100)); }
        ok
    });
    check("the network came up (a DHCP lease)", up, String::new(), &mut failed);

    let dns = ("example.com", 443).to_socket_addrs().map(|a| a.collect::<Vec<_>>());
    check("example.com through std: a DNS answer with the port kept",
        matches!(&dns, Ok(v) if !v.is_empty() && v.iter().all(|a| a.port() == 443 && !a.ip().is_loopback() && !a.ip().is_unspecified())),
        format!("{dns:?}"), &mut failed);

    let missing = ("no-such-host.invalid", 80).to_socket_addrs().map(|a| a.count());
    let msg = missing.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
    check("a name that does not exist is an error that says so",
        missing.is_err() && msg.contains("not known"), msg, &mut failed);

    // tokio's lookup_host is std's, on a blocking thread
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let t: Vec<_> = rt
        .block_on(async { tokio::net::lookup_host("localhost:1").await.map(|a| a.collect::<Vec<_>>()) })
        .unwrap_or_default();
    check("tokio::net::lookup_host", t == vec!["127.0.0.1:1".parse::<SocketAddr>().unwrap()],
        format!("{t:?}"), &mut failed);

    // /etc/hosts, when there is one to write
    {
        let before = std::fs::read("/etc/hosts").ok();
        let wrote = std::fs::write("/etc/hosts", b"# stddns\n10.1.2.3   mikutest  mikutest.local # an alias\n").is_ok();
        let (rc, a, _) = gai(Some("MikuTest.Local"), Some("22"), libc::AF_INET, 0);
        match before {
            Some(b) => { let _ = std::fs::write("/etc/hosts", b); }
            None => { let _ = std::fs::remove_file("/etc/hosts"); }
        }
        if wrote {
            check("/etc/hosts, an alias matched without regard to case",
                rc == 0 && a == Some("10.1.2.3:22".parse().unwrap()),
                format!("rc {rc} {a:?}"), &mut failed);
        } else {
            println!("  skip  /etc/hosts: /etc is not writable on this image");
        }
    }

    let (rc, _, _) = gai(Some("example.com"), None, libc::AF_INET, libc::AI_NUMERICHOST);
    check("AI_NUMERICHOST with a name: EAI_NONAME", rc == libc::EAI_NONAME, format!("{rc}"), &mut failed);

    let (rc, _, _) = gai(Some("localhost"), None, libc::AF_INET6, 0);
    check("AF_INET6: EAI_FAMILY - there is no IPv6", rc == libc::EAI_FAMILY, format!("{rc}"), &mut failed);

    let (rc, a, canon) = gai(Some("localhost"), Some("https"), libc::AF_UNSPEC, libc::AI_CANONNAME);
    check("a named service, and AI_CANONNAME",
        rc == 0 && a.map(|a| a.port()) == Some(443) && canon.as_deref() == Some("localhost"),
        format!("rc {rc} {a:?} {canon:?}"), &mut failed);

    let (rc, a, _) = gai(None, Some("8080"), libc::AF_INET, libc::AI_PASSIVE);
    check("no node with AI_PASSIVE is the wildcard address",
        rc == 0 && a == Some("0.0.0.0:8080".parse().unwrap()), format!("rc {rc} {a:?}"), &mut failed);

    let s = unsafe { CStr::from_ptr(libc::gai_strerror(libc::EAI_AGAIN)) }.to_string_lossy().into_owned();
    check("gai_strerror", s.contains("Temporary failure"), s, &mut failed);

    println!("stddns: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
