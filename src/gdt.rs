// Per-CPU GDT/TSS. Each CPU has its own GDT (because its TSS descriptor
// encodes a different base) and its own IST stacks for DF/PF/syscall
//
// The GDT layout is identical on every CPU, so the selector indices are
// constant and safe to reference from anywhere after BSP init

use core::cell::UnsafeCell;
use core::sync::atomic::Ordering;

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

use crate::percpu;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST_INDEX:   u16 = 1;
pub const SYSCALL_IST_INDEX:      u16 = 2;

const IST_BYTES: usize = 8 * 1024;

#[repr(C, align(16))]
struct Stack([u8; IST_BYTES]);

#[derive(Copy, Clone)]
#[repr(C)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_compat: SegmentSelector,
    pub user_data:   SegmentSelector,
    pub user_code:   SegmentSelector,
    pub tss:         SegmentSelector,
}

impl Selectors {
    const fn zero() -> Self {
        Self {
            kernel_code: SegmentSelector(0),
            kernel_data: SegmentSelector(0),
            user_compat: SegmentSelector(0),
            user_data:   SegmentSelector(0),
            user_code:   SegmentSelector(0),
            tss:         SegmentSelector(0),
        }
    }
}

#[repr(C)]
struct CpuGdt {
    gdt:  GlobalDescriptorTable,
    tss:  TaskStateSegment,
    sels: Selectors,
    df:   Stack,
    pf:   Stack,
    sysc: Stack,
}

struct CpuGdtCell(UnsafeCell<CpuGdt>);
unsafe impl Sync for CpuGdtCell {}

static CPU_GDTS: [CpuGdtCell; percpu::MAX_CPUS] = {
    const ZERO: CpuGdtCell = CpuGdtCell(UnsafeCell::new(CpuGdt {
        gdt:  GlobalDescriptorTable::new(),
        tss:  TaskStateSegment::new(),
        sels: Selectors::zero(),
        df:   Stack([0; IST_BYTES]),
        pf:   Stack([0; IST_BYTES]),
        sysc: Stack([0; IST_BYTES]),
    }));
    [ZERO; percpu::MAX_CPUS]
};

#[inline]
fn cpu_gdt_mut(idx: usize) -> *mut CpuGdt {
    CPU_GDTS[idx.min(percpu::MAX_CPUS - 1)].0.get()
}

// Selector accessors return BSP's copy. The layout is identical on every CPU,
// so the index/RPL bits are the same - callers just need valid values
pub fn kernel_code_selector() -> SegmentSelector {
    unsafe { (*cpu_gdt_mut(0)).sels.kernel_code }
}
pub fn kernel_data_selector() -> SegmentSelector {
    unsafe { (*cpu_gdt_mut(0)).sels.kernel_data }
}
pub fn user_code_selector() -> SegmentSelector {
    unsafe { (*cpu_gdt_mut(0)).sels.user_code }
}
pub fn user_data_selector() -> SegmentSelector {
    unsafe { (*cpu_gdt_mut(0)).sels.user_data }
}
pub fn user_compat_selector() -> SegmentSelector {
    unsafe { (*cpu_gdt_mut(0)).sels.user_compat }
}

/// Pointer to the current CPU's TSS. Interrupt handlers use this for diagnostics
pub fn tss_ptr() -> *mut TaskStateSegment {
    let idx = percpu::current_index();
    unsafe { &mut (*cpu_gdt_mut(idx)).tss as *mut TaskStateSegment }
}

/// BSP init - must run before anything else touches segments. Sets up percpu
/// infrastructure for CPU 0 and loads its GDT/TSS
pub fn init() {
    percpu::init_array();
    percpu::register(0);
    unsafe {
        percpu::install_gs_base(0);
        init_cpu(0);
    }
}

/// Per-CPU GDT/TSS init. Call from each AP after install_gs_base
pub unsafe fn init_cpu(idx: usize) {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
    use x86_64::instructions::tables::load_tss;

    let cg = &mut *cpu_gdt_mut(idx);

    let df_top   = VirtAddr::from_ptr(&cg.df.0)   + IST_BYTES as u64;
    let pf_top   = VirtAddr::from_ptr(&cg.pf.0)   + IST_BYTES as u64;
    let sysc_top = VirtAddr::from_ptr(&cg.sysc.0) + IST_BYTES as u64;

    cg.tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = df_top;
    cg.tss.interrupt_stack_table[PAGE_FAULT_IST_INDEX   as usize] = pf_top;
    cg.tss.interrupt_stack_table[SYSCALL_IST_INDEX      as usize] = sysc_top;
    cg.tss.privilege_stack_table[0] = sysc_top;

    let kernel_code = cg.gdt.add_entry(Descriptor::kernel_code_segment());
    let kernel_data = cg.gdt.add_entry(Descriptor::kernel_data_segment());
    let user_compat = cg.gdt.add_entry(Descriptor::user_data_segment());
    let user_data   = cg.gdt.add_entry(Descriptor::user_data_segment());
    let user_code   = cg.gdt.add_entry(Descriptor::user_code_segment());
    let tss_sel     = cg.gdt.add_entry(Descriptor::tss_segment_unchecked(&cg.tss as *const _));

    cg.sels = Selectors {
        kernel_code, kernel_data, user_compat, user_data, user_code, tss: tss_sel,
    };

    cg.gdt.load_unsafe();
    CS::set_reg(kernel_code);
    DS::set_reg(kernel_data);
    SS::set_reg(kernel_data);
    ES::set_reg(kernel_data);
    load_tss(tss_sel);

    let cpu = percpu::get(idx);
    cpu.tss_ptr  .store(&cg.tss as *const _ as u64, Ordering::Relaxed);
    cpu.df_stack .store(df_top.as_u64(),   Ordering::Relaxed);
    cpu.pf_stack .store(pf_top.as_u64(),   Ordering::Relaxed);
    cpu.sysc_stack.store(sysc_top.as_u64(),Ordering::Relaxed);
    cpu.kernel_rsp.store(sysc_top.as_u64(),Ordering::Relaxed);

    if idx == 0 {
        crate::serial_println!(
            "[gdt] cpu0: kcs={:#x} ucs={:#x} uds={:#x} tss={:#x}",
            kernel_code.0, user_code.0, user_data.0, tss_sel.0,
        );
        crate::serial_println!(
            "[gdt] cpu0: df={:#x} pf={:#x} sysc={:#x}",
            df_top.as_u64(), pf_top.as_u64(), sysc_top.as_u64(),
        );
    } else {
        crate::serial_println!("[gdt] cpu{} gdt/tss loaded", idx);
    }
}

/// Update the current CPU's TSS.rsp0 (used on every context switch so that a
/// CPL3 -> CPL0 transition lands on the incoming task's kernel stack)
pub fn set_kernel_stack(stack_top: u64) {
    let idx = percpu::current_index();
    unsafe {
        (*cpu_gdt_mut(idx)).tss.privilege_stack_table[0] = VirtAddr::new(stack_top);
    }
    percpu::current().kernel_rsp.store(stack_top, Ordering::Relaxed);
}
