// SysV initial stack: [rsp]=argc, [rsp+8]=argv[0], ..., argv NULL, envp[0],
// ..., envp NULL, auxv. Passes argc/argv/envp to _start_main in rdi/rsi/rdx, so
// any of these signatures work:
//   fn _start_main() -> !
//   fn _start_main(argc: i32, argv: *const *const u8) -> !
//   fn _start_main(argc: i32, argv: *const *const u8, envp: *const *const u8) -> !
// envp goes to miku_env_init() first, since the environment is a table in
// `sys_miku.so`.
//
// ld-miku reads auxv and passes it to libmiku (`lib/runtime/libmiku/core/auxv.rs`).
//
// argc/argv/envp stay in the callee-saved rbx/r12/r13 across the call
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, [rsp]",
    "lea r12, [rsp + 8]",
    "lea r13, [r12 + rbx*8 + 8]",
    "and rsp, -16",
    "mov rdi, r13",
    "call miku_env_init",
    "mov rdi, rbx",
    "mov rsi, r12",
    "mov rdx, r13",
    "call _start_main",
    "ud2",
);
