# lib

| Directory | What it is | Runs in |
|---|---|---|
| `abi/` | `miku-abi`: syscall numbers, errno, flags, structures, ELF, auxv, vvar, the TCB. What the kernel and everything above it must agree on | everywhere |
| `base/` | `miku-base`: pure algorithms, no OS calls and no allocation: crc32, crc32c, fnv1a, calendar dates, base64, hex | everywhere |
| `crypto/` | `mikucrypto`: SHA-1, SHA-2, HMAC, HKDF, AES-GCM, bignum, EC, RSA, BLAKE2b, Argon2id, constant-time compare | ring 3, host |
| `runtime/ld-miku/` | the dynamic linker `/lib/ld-miku.so` | every process |
| `runtime/vdso/` | the vDSO image | every process |
| `runtime/libmiku/` | the ten `*_miku.so`, one directory per library; `libs.list` names them | every process |
| `sdk/miku/` | the crate no_std programs depend on: declarations, safe wrappers, `_start`, the allocator, link stubs | build, ring 3 |
| `sdk/c/` | C headers, `miku-cc`, the layout check | build |
| `sdk/rust-std/` | the `x86_64-unknown-miku` target and the `std` patch | build |
| `sdk/rust-libc/` | the `libc` crate's `miku` module | build |
| `fs/fshost/` | the block-device trait and test double filesystems are written against | kernel, host |
| `fs/extfs/` | the ext2, ext3, ext4 on-disk format, journal, mkfs | kernel, host |
| `fs/btrfs/` | the btrfs on-disk format, read-only | kernel, host |
| `net/` | `mikunet`: the L2-L4 stack the kernel links; DNS, NTP, TLS, HTTP in ring 3 | kernel, ring 3, host |
| `gfx/mayland/` | `libmay`: the display protocol, client connection, pools, shapes, keymaps | kernel, ring 3, host |
| `gfx/freetype/` | vendored FreeType with a shim, for aosh's text | ring 3, host |
| `admin/mikuauth/` | passwords, `/etc/passwd`, `/etc/group`, `/etc/shadow`, the `mikudo` grammar | ring 3, host |
| `admin/mikupkg/` | the package format, trust, install logic | ring 3, builder, host |
| `buildgen/` | code build scripts share: the library list, the export scanner, stubs, shims, declarations, image and shader generators | build |

## Where the next thing goes

| It is | Goes in |
|---|---|
| a number or layout both sides of a boundary must agree on | `abi` |
| a pure function needed in two places | `base` |
| a cryptographic primitive | `crypto` |
| code in every process, over syscalls | `runtime/libmiku/<library>` |
| what a program compiles against | `sdk` |
| an on-disk format | `fs` |
| a protocol above IP | `net` |
| drawing, fonts, the display protocol | `gfx` |
| accounts, packages | `admin` |
| code a `build.rs` runs | `buildgen` |

Dependencies point down: `abi` and `base` depend on nothing, and the kernel
never takes `runtime` or `sdk`. How to write a program is in
[docs/wiki/sdk.md](../docs/wiki/sdk.md), and the kernel interface is in
[docs/MikuOS_ABI.md](../docs/MikuOS_ABI.md).
