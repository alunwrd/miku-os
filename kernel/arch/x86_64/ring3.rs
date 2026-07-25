use crate::arch::x86_64::gdt;

pub fn jump_to_userspace(rip: u64, rsp: u64) -> ! {
    let cs = gdt::user_code_selector().0 as u64;
    let ss = gdt::user_data_selector().0 as u64;

    unsafe {
        core::arch::asm!(
            "push {ss}",
            "push {rsp}",
            "push 0x202",
            "push {cs}",
            "push {rip}",
            "iretq",
            ss  = in(reg) ss,
            rsp = in(reg) rsp,
            cs  = in(reg) cs,
            rip = in(reg) rip,
            options(noreturn)
        );
    }
}
