# Shim modules

Libraries call each other through these. A library that needs
`crate::heap::miku_malloc` and does not own `heap.rs` says `shim!(heap);` in its
`main.rs`, and gets a module of the same name forwarding to the C symbol the
owner exports. `ld-miku` resolves it at load time through the library's
`DT_NEEDED`.

They are generated into `OUT_DIR` from the real module on every build
(`lib/buildgen/src/shim.rs`), so a signature cannot diverge from the export.

`manual/<module>.rs` is appended to that module's shim: type aliases and pure
helpers with no C export. `manual/sync.rs` is a whole shim: a generic
`SpinLock<T>` is not expressible over the C ABI.

## The state rule

A shim must never own state.

- Mutable state (`static`, `static mut`, locks around globals, heap metadata,
  errno) lives in the one library whose real module defines it. Shims reach it
  through exported C functions, which is why `manual/errno.rs` forwards
  `set_errno` to `miku_set_errno`: with its own `LAST_ERRNO`, every library would
  see a different errno.
- Copying pure code is fine: generic types, `const` values, stateless helpers.

A manual addition that needs a `static` does not belong here: export a C accessor
from the owning module and forward to it.
