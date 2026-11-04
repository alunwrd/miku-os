// x86_64 - boot entry, GDT, interrupts/IDT, APIC, ACPI, per-CPU state, SMP
// bring-up, GRUB/multiboot handoff, ring3 transition, and the serial console

pub mod acpi;
pub mod apic;
pub mod boot_entry;
pub mod cpu;
pub mod earlycon;
pub mod fpu;
pub mod gdt;
pub mod grub;
pub mod interrupts;
pub mod percpu;
pub mod ring3;
pub mod rtc;
pub mod serial;
pub mod smp;
pub mod trap;

// Memory a device reads on its own is `mm::dma::DmaBox` or `DmaBuffer`, and its
// address comes from there (`dev/kernel-core.md`, "Devices get device memory")
pub use grub::phys_to_virt;
pub use grub::HHDM as HHDM_OFFSET;
