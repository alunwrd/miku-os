## ELF Loader and Dynamic Linking

### ELF Loader

| Feature | Description |
|---|---|
| **Formats** | ET_EXEC (static), ET_DYN (PIE) |
| **Segments** | PT_LOAD, PT_INTERP, PT_DYNAMIC, PT_TLS, PT_GNU_RELRO, PT_GNU_STACK |
| **Relocations** | R_X86_64_RELATIVE, R_X86_64_JUMP_SLOT, R_X86_64_GLOB_DAT, R_X86_64_64 |
| **Security** | W^X enforcement (W+X segments rejected), RELRO |
| **ASLR** | 20-bit entropy for PIE binaries (RDRAND + TSC fallback) |
| **Stack** | SysV ABI compliant: argc, argv, envp, auxv (16-byte aligned) |
| **TLS** | Thread Local Storage (via FS.base register) |

### ld-miku (Dynamic Linker)

`ld-miku` is the ELF dynamic linker for MikuOS. Written in Rust in `#![no_std]`, compiled as a static PIE binary.

```
1. Kernel loads ELF -> detects PT_INTERP
2. ld-miku.so mapped from INCLUDE_BYTES into memory
3. ld-miku starts -> parses auxv (AT_PHDR/AT_ENTRY)
4. Identifies required libraries from DT_NEEDED
5. Maps shared libraries via SYS_MAP_LIB syscall
6. Applies PLT/GOT relocations
7. Exports symbols to global table
8. Executes DT_INIT / DT_INIT_ARRAY
9. Jumps to executable entry point
```

Features:
- Global symbol table (up to 1024 symbols)
- Weak symbol resolution
- Recursive dependency loading (up to 16 libraries)
- R_X86_64_COPY relocation support
- DT_HASH / DT_GNU_HASH for accurate symbol counting

### Shared Libraries (solib)

| Parameter | Value |
|---|---|
| **Max cached** | 32 libraries |
| **Search paths** | /lib, /usr/lib |
| **Page mapping** | All segments copied per-process |
| **OOM protection** | parse_and_prepare aborts on OOM without caching broken data |

`libmiku.so` is embedded in the kernel via `include_bytes!` and registered in the cache at boot via `solib::preload`.

---