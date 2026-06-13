## Userspace SDK

MikuOS provides a Rust SDK for building userspace programs. C is also supported.

### SDK Structure

```
src/lib/userspace/
├── Cargo.toml              crate configuration
├── build.rs                auto-generates stub libmiku.so
├── build.sh                build + deploy script
├── x86_64-miku-app.json    target specification
└── src/
    ├── miku.rs             extern bindings + safe Rust wrappers
    ├── hello.rs            Hello World example
    └── test_full.rs        1617 tests
```

### Rust Program Template

```rust
#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello MikuOS!");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

Entry point is `_start_main` (not `_start`). `miku.rs` defines an asm trampoline `_start` that aligns the stack to 16 bytes before calling `_start_main`.

### Building

```bash
cd src/lib/userspace
./build.sh hello          # build + copy to data.img
./build.sh test_full      # test suite (1617 tests)
./build.sh                # all binaries
```

### C Programs

```bash
gcc -shared -nostdlib -fPIC -Wl,-soname,libmiku.so -o libmiku.so miku_stub.c
gcc -nostdlib -nostdinc -fno-builtin -fno-stack-protector \
    -fno-pie -no-pie -ffreestanding -mno-red-zone \
    -c app.c -o app.o
ld app.o -o app --dynamic-linker=/lib/ld-miku.so libmiku.so --no-as-needed -e _start
e2cp app ~/miku-os/miku-os/data.img:/
```

### Disk Setup for Userspace

```bash
# Create ext4 without 64bit (important for MikuOS driver compatibility)
mkfs.ext4 -O ^64bit,^metadata_csum ~/miku-os/miku-os/data.img

# Copy binary
e2cp binary ~/miku-os/miku-os/data.img:/

# List files
e2ls ~/miku-os/miku-os/data.img:/
```

### Full ABI Documentation

See [MIKUOS_ABI.md](docs/MIKUOS_ABI.md) for the complete syscall ABI, all 956 function signatures, examples, and troubleshooting.

---