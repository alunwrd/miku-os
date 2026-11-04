# Shim modules

Cross-library calls between the split miku libraries go through these
shims. A library that needs, say, `crate::heap::miku_malloc` but does not
own `heap.rs` includes `shim/heap.rs` instead of the real module; the shim
forwards the call to the C symbol exported by the owning library
(`core_miku.so`), which ld-miku resolves at load time via the library's
`DT_NEEDED` entry.

## They are generated, do not edit by hand

`mikulibs/build.rs` regenerates every shim from the real module source in
`src/lib/libmiku/` on each build, so shim signatures cannot drift from the
actual exports. Everything ABOVE the marker line

```
// ===== manual additions below (preserved by the generator) =====
```

is overwritten; everything below it is preserved. Put hand-written items
(type aliases, pure helper functions that have no C export) below the
marker.

`sync.rs` is the one fully hand-written shim (the generic `SpinLock<T>` is
not expressible over the C ABI) and is on the generator's skip list.

## The state rule

A shim must never own state.

- Mutable state (`static`, `static mut`, locks around globals, heap
  metadata, errno, ...) lives in exactly one library: the one whose real
  module defines it. Shims reach it only through the exported C functions.
  This is why `shim/errno.rs` forwards `set_errno`/`get_errno` to
  `miku_set_errno`/`miku_get_errno` instead of keeping its own
  `LAST_ERRNO`: otherwise every library would see a different errno.
- Duplicating pure code by value is fine: generic types (`SpinLock<T>`),
  `const` values, `#[repr(C)]` struct definitions, stateless helper
  functions. Each instance carries its own caller-owned state or no state
  at all.

If a new manual addition needs a `static`, it does not belong in a shim:
export a C accessor from the owning module and forward to it.

## The library manifest

Which library owns which modules, and who depends on whom, is defined in
one place: `src/lib/mikulibs/libs.list`. `mikulibs/build.rs` validates it
on every build (missing roots, unknown deps, dependency cycles all fail
the build) and it is the single source consumed by the kernel
(`build.rs` at the repo root generates the `MIKU_LIBS` preload table),
the builder, and the userspace app stubs.
