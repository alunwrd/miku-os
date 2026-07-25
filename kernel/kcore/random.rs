use core::arch::asm;

fn has_rdrand() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
        );
    }
    ecx & (1 << 30) != 0
}

fn rdrand64() -> Option<u64> {
    let val: u64;
    let ok: u8;
    unsafe {
        asm!(
            "rdrand {val}",
            "setc {ok}",
            val = out(reg) val,
            ok = out(reg_byte) ok,
        );
    }
    if ok != 0 { Some(val) } else { None }
}

fn rdrand64_retry() -> Option<u64> {
    for _ in 0..10 {
        if let Some(v) = rdrand64() {
            return Some(v);
        }
    }
    None
}

fn tsc_fallback() -> u64 {
    let tsc = crate::kcore::time::rdtsc();
    let tick = crate::arch::x86_64::interrupts::get_tick();
    let a = tsc.wrapping_mul(0x9E3779B97F4A7C15);
    let b = tick.wrapping_mul(0x517CC1B727220A95);
    let mix = a ^ b;
    mix ^ (mix >> 17) ^ (mix >> 31)
}

pub fn random_u64() -> u64 {
    if has_rdrand() {
        if let Some(v) = rdrand64_retry() {
            return v;
        }
    }
    tsc_fallback()
}

pub fn aslr_offset(max_bits: u32, step: u64) -> u64 {
    let mask = (1u64 << max_bits) - 1;
    let raw = random_u64();
    let slot = (raw >> 12) & mask;
    slot * step
}

pub fn random_bytes_16() -> [u8; 16] {
    let a = random_u64();
    let b = random_u64();
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&a.to_le_bytes());
    out[8..16].copy_from_slice(&b.to_le_bytes());
    out
}
