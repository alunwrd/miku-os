// Process-management syscalls: exit, sleep, uptime, fork, wait4, kill, exec

extern crate alloc;

use super::errno::{err, ECHILD, EINVAL, ENOENT, ENOMEM, EPERM, ESRCH};
use super::user_mem::{current_cr3, current_pid, read_user_path, user_ptr_mapped};
use crate::vmm::AddressSpace;

const WNOHANG: u64 = 1;

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

    // The syscall_handler pushes (in order): rcx, r11, rbp, rbx, r12, r13,
    // r14, r15, r10, r9, r8.  Read them back from the per-CPU kernel stack
    // and gs:[8] (saved user RSP)
    let kernel_stack_top: u64;
    let user_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) kernel_stack_top);
        core::arch::asm!("mov {}, gs:[8]", out(reg) user_rsp);
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
                if status_ptr != 0 && user_ptr_mapped(cr3, status_ptr, 8) {
                    unsafe { *(status_ptr as *mut u64) = exit_code; }
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
pub fn sys_exec(path_ptr: u64, path_len: u64, _argv_ptr: u64, _argc: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    let cr3 = current_cr3();
    let pid = current_pid();

    let file_data = match crate::vfs_read::read_file(path) {
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

    let image = match crate::elf_loader::load(&file_data, &new_aspace, &[path], Some(&read_file)) {
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
        x86_64::registers::model_specific::FsBase::write(
            x86_64::VirtAddr::new(image.tls_base),
        );
    }

    // Patch sysret-state so we return to the new entry/stack
    let kernel_stack_top: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) kernel_stack_top);
        // saved RCX (= user RIP) is at [top-8]
        *((kernel_stack_top - 8) as *mut u64) = image.entry;
        // user RSP lives at gs:[8]
        core::arch::asm!("mov gs:[8], {}", in(reg) image.stack_top, options(nostack, preserves_flags));
    }

    crate::serial_println!(
        "[exec] pid={} replaced with '{}': entry={:#x} sp={:#x}",
        pid, path, image.entry, image.stack_top
    );

    0
}
