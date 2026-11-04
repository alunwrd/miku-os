// libc's memory and string functions against byte loops: every size to 512 at
// every alignment of both ends, and for `memmove` every overlap in both
// directions

use crate::mem::{miku_memchr, miku_memcmp, miku_memcpy, miku_memmove, miku_memset};
use crate::string::{miku_strlen, miku_strnlen};

const MOST: usize = 512;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add((i as u8).wrapping_mul(29))).collect()
}

#[test]
fn memcpy_and_memset_touch_exactly_their_bytes() {
    for len in 0..=MOST {
        for a in 0..8 {
            for b in 0..8 {
                let src = pattern(len + 16, 7);
                let mut dst = vec![0xEEu8; len + 16];
                miku_memcpy(dst[a..].as_mut_ptr(), src[b..].as_ptr(), len);
                assert_eq!(&dst[a..a + len], &src[b..b + len], "memcpy {len} at {a}<-{b}");
                assert!(dst[..a].iter().chain(&dst[a + len..]).all(|&x| x == 0xEE), "memcpy {len} at {a} wrote outside");

                let mut buf = vec![0x11u8; len + 16];
                miku_memset(buf[a..].as_mut_ptr(), 0x1C3, len);
                assert!(buf[a..a + len].iter().all(|&x| x == 0xC3), "memset {len} at {a}");
                assert!(buf[..a].iter().chain(&buf[a + len..]).all(|&x| x == 0x11), "memset {len} at {a} wrote outside");
            }
        }
    }
}

#[test]
fn memmove_is_right_for_every_overlap() {
    for len in 0..=200 {
        for shift in 0..24usize {
            for forward in [false, true] {
                let mut buf = pattern(len + 64, 3);
                let mut want = buf.clone();
                let (from, to) = if forward { (8, 8 + shift) } else { (8 + shift, 8) };
                want.copy_within(from..from + len, to);
                miku_memmove(buf[to..].as_mut_ptr(), buf[from..].as_ptr(), len);
                assert_eq!(buf, want, "memmove {len} from {from} to {to}");
            }
        }
    }
}

#[test]
fn memcmp_finds_the_first_difference() {
    for len in 0..=MOST.min(300) {
        let a = pattern(len, 9);
        assert_eq!(miku_memcmp(a.as_ptr(), a.as_ptr(), len), 0);
        for at in 0..len {
            for (x, y) in [(0x10u8, 0x90u8), (0x90, 0x10)] {
                let mut p = a.clone();
                let mut q = a.clone();
                p[at] = x;
                q[at] = y;
                // A later difference the other way must not count
                if at + 1 < len {
                    p[len - 1] = 0xFF;
                    q[len - 1] = 0x00;
                }
                let r = miku_memcmp(p.as_ptr(), q.as_ptr(), len);
                assert_eq!(r.signum(), (x as i32 - y as i32).signum(), "memcmp {len}, first difference at {at}");
            }
        }
    }
}

#[test]
fn memchr_strlen_and_strnlen_stop_where_they_should() {
    for len in 0..=MOST.min(300) {
        for a in 0..8 {
            let mut buf = vec![b'x'; len + 24];
            let s = &mut buf[a..];
            s[len] = 0;
            assert_eq!(miku_strlen(s.as_ptr()), len, "strlen {len} at {a}");
            assert_eq!(miku_strnlen(s.as_ptr(), len + 5), len, "strnlen {len} past it");
            assert_eq!(miku_strnlen(s.as_ptr(), len / 2), len / 2, "strnlen {len} short of it");
            if len > 0 {
                s[len - 1] = b'q';
                let found = miku_memchr(s.as_ptr(), b'q' as i32, len);
                assert_eq!(found as usize - s.as_ptr() as usize, len - 1, "memchr {len} at {a}");
                assert!(miku_memchr(s.as_ptr(), b'q' as i32, len - 1).is_null(), "memchr {len} past its end");
            }
        }
    }
}
