// Process-management syscalls: exit, sleep, uptime, fork, wait4, kill, exec

extern crate alloc;

use super::errno::{err, ECHILD, EINVAL, ENOENT, ENOMEM, EPERM, ESRCH};
use super::user_mem::{current_cr3, current_pid, read_user_argv, read_user_path, user_ptr_writable};
use crate::vmm::AddressSpace;

const WNOHANG: u64 = 1;

// 47  umask(mask) -> previous_mask
pub fn sys_umask(mask: u64) -> u64 {
    let new_mask = (mask as u16) & 0o777;
    let pid = current_pid();
    let old = crate::scheduler::umask_of(pid);
    crate::scheduler::set_current_umask(new_mask);
    old as u64
}

// 48  getuid() -> uid
pub fn sys_getuid() -> u64 { crate::scheduler::uid_of(current_pid()) as u64 }

// 49  getgid() -> gid
pub fn sys_getgid() -> u64 { crate::scheduler::gid_of(current_pid()) as u64 }

// 50  geteuid() -> euid
pub fn sys_geteuid() -> u64 { crate::scheduler::euid_of(current_pid()) as u64 }

// 51  getegid() -> egid
pub fn sys_getegid() -> u64 { crate::scheduler::egid_of(current_pid()) as u64 }

// Identity-setting syscalls. POSIX allows non-root processes to switch
// effective IDs back to their real/saved values; the simpler rule we
// enforce here is "only root may change identity", deferring the full
// saved-set-uid mechanism to when MikuOS actually has multiple users
fn set_identity_atomic<F: FnOnce(&crate::process::Process)>(setter: F) -> u64 {
    use core::sync::atomic::Ordering;
    let pid = current_pid();
    let ptr = unsafe { crate::scheduler::proc_index_raw(pid) };
    if ptr.is_null() { return err(super::errno::ESRCH); }
    let p = unsafe { &*ptr };
    // Root check: only root (euid == 0) can change identity
    if p.euid.load(Ordering::Relaxed) != 0 {
        return err(EPERM);
    }
    setter(p);
    0
}

// 52  setuid(uid) -> 0/-EPERM
pub fn sys_setuid(uid: u64) -> u64 {
    use core::sync::atomic::Ordering;
    set_identity_atomic(|p| {
        let u = uid as u16;
        p.uid.store(u, Ordering::Relaxed);
        p.euid.store(u, Ordering::Relaxed);
    })
}

// 53  setgid(gid) -> 0/-EPERM
pub fn sys_setgid(gid: u64) -> u64 {
    use core::sync::atomic::Ordering;
    set_identity_atomic(|p| {
        let g = gid as u16;
        p.gid.store(g, Ordering::Relaxed);
        p.egid.store(g, Ordering::Relaxed);
    })
}

// 54  seteuid(euid) -> 0/-EPERM
pub fn sys_seteuid(euid: u64) -> u64 {
    use core::sync::atomic::Ordering;
    set_identity_atomic(|p| p.euid.store(euid as u16, Ordering::Relaxed))
}

// 55  setegid(egid) -> 0/-EPERM
pub fn sys_setegid(egid: u64) -> u64 {
    use core::sync::atomic::Ordering;
    set_identity_atomic(|p| p.egid.store(egid as u16, Ordering::Relaxed))
}

// 0  exit(code) -> never returns to caller
pub fn sys_exit(code: u64) -> u64 {
    let pid = current_pid();
    crate::serial_println!("[syscall] exit pid={} code={}", pid, code);
    crate::scheduler::kill_with_code(pid, code);
    crate::signal::send_sigchld(pid);
    crate::scheduler::yield_now();
    0
}

// 16  sleep(ticks) -> 0
pub fn sys_sleep(ticks: u64) -> u64 {
    if ticks == 0 {
        crate::scheduler::yield_now();
        return 0;
    }
    let clamped = ticks.min(100_000);
    crate::scheduler::sleep(clamped);
    0
}

// 17  uptime() -> ticks
pub fn sys_uptime() -> u64 {
    crate::interrupts::get_tick()
}

// 43  fork() -> child_pid (parent) / 0 (child)
pub fn sys_fork() -> u64 {
    let cr3 = current_cr3();
    let pid = current_pid();

    // Cannot fork kernel threads
    if cr3 == crate::vmm::kernel_cr3() {
        return err(EPERM);
    }

    // The syscall_handler prologue pushes (in order from the kernel
    // stack top): rcx, r11, rbp, rbx, r12, r13, r14, r15, r10, r9, r8.
    // The kernel stack top lives at gs:[0x10] (percpu::Cpu::kernel_rsp)
    // and the saved user RSP at gs:[0x18] (percpu::Cpu::user_rsp). See
    // `syscall_handler` in src/syscall/mod.rs and `Cpu` in src/percpu.rs
    // - keep those three call sites in lock-step
    let kernel_stack_top: u64;
    let user_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) kernel_stack_top);
        core::arch::asm!("mov {}, gs:[0x18]", out(reg) user_rsp);
    }
    let user_rip    = unsafe { *((kernel_stack_top - 8)  as *const u64) }; // saved rcx
    let user_rflags = unsafe { *((kernel_stack_top - 16) as *const u64) }; // saved r11

    let saved = crate::process::SavedSyscallRegs {
        rbp: unsafe { *((kernel_stack_top - 24) as *const u64) },
        rbx: unsafe { *((kernel_stack_top - 32) as *const u64) },
        r12: unsafe { *((kernel_stack_top - 40) as *const u64) },
        r13: unsafe { *((kernel_stack_top - 48) as *const u64) },
        r14: unsafe { *((kernel_stack_top - 56) as *const u64) },
        r15: unsafe { *((kernel_stack_top - 64) as *const u64) },
        r10: unsafe { *((kernel_stack_top - 72) as *const u64) },
        r9:  unsafe { *((kernel_stack_top - 80) as *const u64) },
        r8:  unsafe { *((kernel_stack_top - 88) as *const u64) },
    };

    // Clone address space with COW
    let parent_aspace = AddressSpace::from_raw(cr3);
    let child_aspace = match parent_aspace.clone_cow() {
        Some(a) => a,
        None => {
            let _ = parent_aspace.into_raw();
            return err(ENOMEM);
        }
    };
    let _ = parent_aspace.into_raw();

    let child_cr3 = child_aspace.into_raw();

    crate::mmap::vma_clone(cr3, child_cr3);
    let parent_brk = crate::mmap::sys_brk(cr3, 0);

    let child = crate::process::Process::new_fork(
        pid,
        child_cr3,
        None, // user_stack_phys is COW-shared, not separately tracked
        parent_brk,
        user_rip,
        user_rsp,
        user_rflags,
        &saved,
    );
    let child_pid = child.pid;

    // Per-process FD table: clone parent's so the child inherits the
    // same descriptors at the same numeric slots
    crate::vfs::core::with_vfs(|vfs| vfs.fork_fds(pid, child_pid));

    crate::serial_println!("[fork] parent={} child={} cr3={:#x}", pid, child_pid, child_cr3);

    crate::scheduler::add_user_process(child);
    child_pid
}

// 44  wait4(pid, status_ptr, options) -> child_pid / -errno
pub fn sys_wait4(target_pid: u64, status_ptr: u64, options: u64) -> u64 {
    let my_pid = current_pid();
    let cr3 = current_cr3();

    loop {
        let found = crate::scheduler::find_zombie_child(my_pid, target_pid);
        match found {
            Some((child_pid, exit_code)) => {
                if status_ptr != 0 && user_ptr_writable(cr3, status_ptr, 8) {
                    unsafe {
                        // write_unaligned - status_ptr is user-supplied
                        // and need not be aligned
                        (status_ptr as *mut u64).write_unaligned(exit_code);
                    }
                }
                crate::scheduler::reap_zombie(child_pid);
                return child_pid;
            }
            None => {
                if !crate::scheduler::has_children(my_pid) {
                    return err(ECHILD);
                }
                if options & WNOHANG != 0 {
                    return 0; // non-blocking, no zombie yet
                }
                crate::scheduler::block_current("wait4");
            }
        }
    }
}

// 45  kill(pid, sig) -> 0 / -errno
pub fn sys_kill(target_pid: u64, sig: u64) -> u64 {
    if target_pid == 0 { return err(EINVAL); }

    match sig {
        9 | 15 => {
            // SIGKILL / SIGTERM
            crate::scheduler::kill(target_pid);
            crate::signal::send_sigchld(target_pid);
            0
        }
        0 => {
            // probe whether the process exists
            if crate::scheduler::process_exists(target_pid) { 0 } else { err(ESRCH) }
        }
        _ => {
            crate::signal::send_signal(target_pid, sig as u32);
            0
        }
    }
}

// 46  exec(path_ptr, path_len, argv_ptr, argc) -> never on success
//
// argv_ptr is an array of `argc` u64 user pointers, each pointing to a
// NUL-terminated string. argv[0] conventionally repeats the program
// name. If userspace passes argc=0 or argv_ptr=0 we fall back to a
// single-entry argv of [path] so the loader always has a valid argv[0]
pub fn sys_exec(path_ptr: u64, path_len: u64, argv_ptr: u64, argc: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    // Copy argv strings into kernel memory BEFORE swapping address
    // spaces; user pages disappear once the new cr3 is loaded
    let user_args = match read_user_argv(argv_ptr, argc) {
        Ok(v)  => v,
        Err(e) => return e,
    };

    let cr3 = current_cr3();
    let pid = current_pid();

    // POSIX: descriptors marked O_CLOEXEC are closed before the new
    // image starts. Run this *before* we even attempt to load the ELF
    // so that a failed exec() doesn't leave a half-closed table
    let cloexec_victims = crate::vfs::core::with_vfs(|vfs| vfs.close_cloexec_fds());
    if !cloexec_victims.is_empty() {
        crate::vfs::core::with_vfs(|vfs| {
            for vid in cloexec_victims {
                let idx = vid as usize;
                if vfs.valid_vnode(idx) {
                    vfs.nodes[idx].dec_ref();
                }
            }
        });
    }

    let file_data = match crate::vfs_read::read_file(&path) {
        Some(data) => data,
        None       => return err(ENOENT),
    };

    let new_aspace = match AddressSpace::new_user() {
        Some(a) => a,
        None    => return err(ENOMEM),
    };

    // ELF loader needs to be able to fault in the dynamic linker
    let read_file = |interp_path: &str| -> Option<alloc::vec::Vec<u8>> {
        if interp_path.contains("ld-miku") || interp_path.contains("ld.so") {
            return Some(crate::ldso::LDSO_BYTES.to_vec());
        }
        crate::vfs_read::read_file(interp_path)
    };

    // Build &[&str] view: argv[0]=path when userspace didn't supply one
    let arg_refs: alloc::vec::Vec<&str> = if user_args.is_empty() {
        alloc::vec![path.as_str()]
    } else {
        user_args.iter().map(|s| s.as_str()).collect()
    };

    let image = match crate::elf_loader::load(&file_data, &new_aspace, &arg_refs, Some(&read_file)) {
        Ok(img) => img,
        Err(e) => {
            crate::serial_println!("[exec] ELF load failed: {}", e.as_str());
            // new_aspace dropped -> freed
            return err(ENOENT);
        }
    };

    let new_cr3 = new_aspace.into_raw();

    crate::mmap::vma_cleanup(cr3);
    crate::mmap::vma_set_brk(new_cr3, image.brk);

    crate::scheduler::update_process_cr3(pid, new_cr3);

    // Switch to the new address space before freeing the old one so an
    // interrupt firing between free and switch cannot use-after-free
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) new_cr3, options(nostack, preserves_flags));
    }

    if cr3 != 0 && cr3 != crate::vmm::kernel_cr3() {
        let mut old = AddressSpace::from_raw(cr3);
        old.free_address_space();
    }

    if image.tls_base != 0 {
        // try_new - tls_base derives from the loaded ELF and must stay
        // canonical user-half. Silently skip if the loader produced an
        // unusable value rather than wrmsr-#GP the kernel
        if let Ok(va) = x86_64::VirtAddr::try_new(image.tls_base) {
            if image.tls_base <= super::user_mem::USER_MAX {
                x86_64::registers::model_specific::FsBase::write(va);
            }
        }
    }

    // Patch sysret-state so we return to the new entry/stack. Per-cpu
    // layout: gs:[0x10]=kernel_rsp (saved-reg stack top, see Cpu in
    // src/percpu.rs and the syscall prologue in src/syscall/mod.rs),
    // gs:[0x18]=user_rsp (where the prologue stashed userspace RSP)
    let kernel_stack_top: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) kernel_stack_top);
        // saved RCX (= user RIP) is at [top-8]
        *((kernel_stack_top - 8) as *mut u64) = image.entry;
        core::arch::asm!("mov gs:[0x18], {}", in(reg) image.stack_top, options(nostack, preserves_flags));
    }

    crate::serial_println!(
        "[exec] pid={} replaced with '{}': entry={:#x} sp={:#x}",
        pid, path, image.entry, image.stack_top
    );

    0
}
