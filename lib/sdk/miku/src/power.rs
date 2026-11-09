//! Power control, under `CAP_SYS_BOOT`, which sessions hold and `libmay::spawn`
//! drops for launched programs. mikuD stops services and syncs first. Nothing
//! returns on success

pub use miku_abi::kproc::POWER_OFF as OFF;
pub use miku_abi::kproc::POWER_REBOOT as REBOOT;

/// Stop the machine, or stop it and start it again
pub fn request(action: u64) -> Result<(), i64> {
    let r = unsafe { super::miku_power(action) };
    if r < 0 { Err(r) } else { Ok(()) }
}
