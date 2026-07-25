// x86_64 - boot entry, GDT, interrupts/IDT, APIC, ACPI, per-CPU state, SMP
// bring-up, GRUB/multiboot handoff, ring3 transition, and the serial console

pub mod acpi;
pub mod apic;
pub mod boot_entry;
pub mod gdt;
pub mod grub;
pub mod interrupts;
pub mod percpu;
pub mod ring3;
pub mod rtc;
pub mod serial;
pub mod smp;
