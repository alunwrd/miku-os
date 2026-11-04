// GPU selftest: everything about the driver that can be decided without the
// card
//
// QEMU does not model the GB206, so nothing below the RPC transport can run
// anywhere but the real machine. A wrong page-table index is a cleared PDE
// under a live compositor.
//
// This takes the pure functions (where a virtual address lands in six levels of
// page table, whether two physical ranges overlap, what a range check accepts)
// and decides them on the CPU, in QEMU.
//
// `run=nvidia_selftest` on the GRUB line runs it to the serial port.
//
// Anything reading or writing a page table needs a `bar0` and a page directory
// RM built, so the card: see `nvidia gmmu-test`.
extern crate alloc;
use alloc::format;

use super::gmmu;

pub use crate::kcore::selftest::{Report, Runner};

/// The 512 MiB boundary, which one level-1 table covers
const OLD_LIMIT: u64 = 512 << 20;

pub fn run() -> Report {
    let mut r = Runner::new();
    level_indices(&mut r);
    range_checks(&mut r);
    overlaps(&mut r);
    fences(&mut r);
    isr_events(&mut r);
    cubins(&mut r);
    curves(&mut r);
    head_registers(&mut r);
    engine_names(&mut r);
    rpc_transport(&mut r);
    event_dispatch(&mut r);
    nvkv_lists(&mut r);
    interrupt_tree(&mut r);
    chip_table(&mut r);
    fb_layout(&mut r);
    engine_order(&mut r);
    crash_reports(&mut r);
    nocat_records(&mut r);
    display_names(&mut r);
    display_plan(&mut r);
    display_batch(&mut r);
    program_vram(&mut r);
    page_sizes(&mut r);
    chipset_tables(&mut r);
    display_caps(&mut r);
    imp_params(&mut r);
    vrr_params(&mut r);
    resource_assignment(&mut r);
    dp_fallback(&mut r);
    va_space(&mut r);
    process_context(&mut r);
    perf_shared_data(&mut r);
    client_launches(&mut r);
    r.report()
}

/// What a launch from ring 3 is handed (`syscall/adapters/gpu.rs`, `lay_bank`).
/// A buffer can come with its size, so the kernel's own programs can run for
/// anybody without reaching past the memory they were given (`dev/aosh-gpu.md`)
fn client_launches(r: &mut Runner) {
    use crate::syscall::adapters::gpu::{lay_bank, Param, PARAM_BUFFER, PARAM_BUFFER_SIZED, PARAM_U32, PARAM_U64};
    use crate::drivers::gpu::nvidia::gsp_common::compute;
    // A pool of 0x1000 bytes bound at 0x1_0000_0000, and nothing else
    let resolve = |handle: u32, off: u64| -> Result<(u64, u64), (i64, alloc::string::String)> {
        if handle != 7 {
            return Err((-13, format!("pool {} is not this process's", handle)));
        }
        if off >= 0x1000 {
            return Err((-22, format!("offset {:#x} is past the end", off)));
        }
        Ok((0x1_0000_0000 + off, 0x1000 - off))
    };
    let p = |kind: u32, handle: u32, value: u64| Param { kind, handle, value };
    let u64_at = |b: &[u8], o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap_or([0; 8]));

    // fill(out, bytes, value, count): the address, its size, two words
    let bank = lay_bank(&[p(PARAM_BUFFER_SIZED, 7, 0x100), p(PARAM_U32, 0, 5), p(PARAM_U32, 0, 64)], resolve);
    let ok = bank.as_ref().is_ok_and(|b| b.len() == 24 && u64_at(b, 0) == 0x1_0000_0100 && u64_at(b, 8) == 0xF00
                                     && b[16..20] == 5u32.to_le_bytes() && b[20..24] == 64u32.to_le_bytes());
    r.check("gpu: a sized buffer is its address and the bytes left in its pool", ok,
            format!("{:x?}", bank.as_ref().map(|b| b.len())));

    // A plain buffer is the address alone, and a u64 after a u32 is put on its
    // own alignment, as the compiler lays the bank out
    let bank = lay_bank(&[p(PARAM_U32, 0, 1), p(PARAM_BUFFER, 7, 0), p(PARAM_U64, 0, 9)], resolve);
    let ok = bank.as_ref().is_ok_and(|b| b.len() == 24 && u64_at(b, 8) == 0x1_0000_0000 && u64_at(b, 16) == 9);
    r.check("gpu: a plain buffer is its address alone, aligned", ok, format!("{:?}", bank.as_ref().map(|b| b.len())));

    // The size is the pool's, never what was asked: the last byte has one
    let bank = lay_bank(&[p(PARAM_BUFFER_SIZED, 7, 0xFFF)], resolve);
    r.check("gpu: the last byte of a pool has a size of one", bank.as_ref().is_ok_and(|b| u64_at(b, 8) == 1),
            format!("{:?}", bank.as_ref().map(|b| u64_at(b, 8))));
    r.check("gpu: an offset past a pool is refused, sized or not",
            lay_bank(&[p(PARAM_BUFFER_SIZED, 7, 0x1000)], resolve).is_err()
                && lay_bank(&[p(PARAM_BUFFER, 7, 0x1000)], resolve).is_err(),
            format!("0x1000 into a pool of 0x1000"));
    r.check("gpu: somebody else's pool is refused", lay_bank(&[p(PARAM_BUFFER_SIZED, 8, 0)], resolve).is_err(),
            format!("handle 8"));
    r.check("gpu: a parameter kind there is none of is refused", lay_bank(&[p(4, 0, 0)], resolve).is_err(),
            format!("kind 4"));

    // A chain's launches each have a slot of their own, above the effects
    // pass's, and the pass keeps its 32
    r.check("gpu: ring 3 has sixteen launch slots above the effects pass's 32",
            compute::CLIENT_SLOTS == 16 && compute::CLIENT_SLOT == 32 && compute::CLIENT_SLOT + compute::CLIENT_SLOTS == compute::MAX_SLOTS,
            format!("slots {} from {} of {}", compute::CLIENT_SLOTS, compute::CLIENT_SLOT, compute::MAX_SLOTS));
    // Sixteen bound pools, each 64 MiB apart: the last one ends 1 GiB on
    let end = compute::client_va(compute::MAX_CLIENT_BINDS - 1) + compute::CLIENT_VA_STRIDE;
    r.check("gpu: sixteen client binds end 1 GiB above where they start",
            compute::MAX_CLIENT_BINDS == 16 && end == compute::CLIENT_VA_BASE + (1 << 30),
            format!("{:#x}", end));
}

/// Where a head's armed state is, and what the raster says about the panel.
///
/// The raster stride for class 4 is 0x800, not the c37d head stride 0x400, and
/// the pixel clock is not the refresh rate. Both are invisible on head 0 at 60
/// Hz
fn head_registers(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{
        armed_head_reg, decode_pixel_clock, RasterTiming, M_HEAD_SET_RASTER_SIZE,
    };

    r.check(
        "disp: head 0's armed RASTER_SIZE is where it always was",
        armed_head_reg(0, M_HEAD_SET_RASTER_SIZE) == 0x0068_a064,
        format!("{:#x}", armed_head_reg(0, M_HEAD_SET_RASTER_SIZE)),
    );
    r.check(
        "disp: head 1's armed RASTER_SIZE is one class-4 head stride (0x800) on",
        armed_head_reg(1, M_HEAD_SET_RASTER_SIZE) == 0x0068_a864,
        format!("{:#x}", armed_head_reg(1, M_HEAD_SET_RASTER_SIZE)),
    );

    r.check(
        "disp: a pixel clock under 2^31 Hz is the low word as it stands",
        decode_pixel_clock(148_500_000, 0) == 148_500_000,
        format!("{}", decode_pixel_clock(148_500_000, 0)),
    );
    r.check(
        "disp: the HI word carries bits 34:31 of the clock",
        decode_pixel_clock(0, 1) == 1u64 << 31,
        format!("{}", decode_pixel_clock(0, 1)),
    );
    r.check(
        "disp: ADJ1000DIV1001 makes 148.5 MHz the 59.94 Hz clock",
        decode_pixel_clock(0x8000_0000 | 148_500_000, 0) == 148_351_648,
        format!("{}", decode_pixel_clock(0x8000_0000 | 148_500_000, 0)),
    );

    let raster = |htotal: u16, vtotal: u16, pclk: u64| RasterTiming {
        vtotal,
        htotal,
        vblanke: 0,
        vblanks: 0,
        pixel_clock_hz: pclk,
    };
    let p1080 = raster(2200, 1125, 148_500_000);
    r.check(
        "disp: 1920x1080 CEA timing at 148.5 MHz is 60.000 Hz",
        p1080.refresh_mhz() == 60_000 && p1080.refresh_hz() == 60,
        format!("{} mHz", p1080.refresh_mhz()),
    );
    // The panel on the dev box: 561.6 MHz over a 2080x1125 raster
    let p240 = raster(2080, 1125, 561_600_000);
    r.check(
        "disp: the HTMS270F240's 561.6 MHz mode is 240.000 Hz",
        p240.refresh_mhz() == 240_000 && p240.refresh_hz() == 240,
        format!("{} mHz", p240.refresh_mhz()),
    );
    r.check(
        "disp: a head that is not scanning has no refresh rate, not a division by zero",
        raster(0, 0, 148_500_000).refresh_mhz() == 0,
        format!("{}", raster(0, 0, 148_500_000).refresh_mhz()),
    );
}

/// `engine_idx.h`, as the interrupt table prints it
fn engine_names(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::intr::engine_name;
    // 615 put GPIO at 176 and moved DISP_LOW to 177
    let cases: [(u16, &str); 8] = [
        (0, "NULL"),
        (2, "DISP"),
        (17, "CE"),
        (50, "GSP"),
        (84, "GR"),
        (156, "GR_FECS_LOG"),
        (176, "GPIO"),
        (177, "DISP_LOW"),
    ];
    let wrong: alloc::vec::Vec<_> = cases
        .iter()
        .filter(|(i, n)| engine_name(*i) != *n)
        .map(|(i, _)| (*i, engine_name(*i)))
        .collect();
    r.check(
        "intr: engine indices are named as engine_idx.h names them (0 is NULL)",
        wrong.is_empty(),
        format!("wrong: {:?}", wrong),
    );
}

/// One receive path, and one table of what an event means.
///
/// The poll is driven over a `Loopback` with a closure for a dispatcher, so
/// what it hands on can be looked at; the dispatcher is driven with messages
/// made here. Everything it keeps is global, so each case reads what moved
fn event_dispatch(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::{gmcapi, kgsp};
    use crate::drivers::gpu::nvidia::gsp_common::rpc::{
        self, Context, GspRpc, Loopback, RpcError, RpcMessage, NOTHING,
    };
    use crate::drivers::gpu::nvidia::le;

    const FN: u32 = 0x55;
    let msg = RpcMessage::rm;

    // Two events and then the reply: the poll hands both on and returns the
    // reply
    let mut q = Loopback::new(32);
    for f in [rpc::EV_OS_ERROR_LOG, rpc::EV_POST_EVENT, FN] {
        let _ = GspRpc::over(&mut q).send(f, &[1, 2, 3, 4], 1_000_000);
    }
    q.reflect();
    let mut handed = alloc::vec::Vec::new();
    let got = GspRpc::over(&mut q).recv_poll(FN, None, Context::Poll, 1_000_000, &mut |m, c| {
        handed.push((m.function, c));
    });
    r.check(
        "rpc: a poll hands every event it passes to the dispatcher and returns its reply",
        matches!(&got, Ok(m) if m.function == FN)
            && handed == [(rpc::EV_OS_ERROR_LOG, Context::Poll), (rpc::EV_POST_EVENT, Context::Poll)],
        format!("got {:?} handed {:?}", got.as_ref().map(|m| m.function).map_err(|e| *e), handed),
    );

    // A dispatcher that polls: refused, since the reply it would wait for is
    // behind the one the outer poll waits for
    let mut q = Loopback::new(8);
    let _ = GspRpc::over(&mut q).send(rpc::EV_OS_ERROR_LOG, &[0; 16], 1_000_000);
    let _ = GspRpc::over(&mut q).send(FN, &[0; 16], 1_000_000);
    q.reflect();
    let mut inner = None;
    let _ = GspRpc::over(&mut q).recv_poll(FN, None, Context::Poll, 1_000_000, &mut |_, _| {
        let mut other = Loopback::new(4);
        inner = Some(GspRpc::over(&mut other).recv_poll(NOTHING, None, Context::Poll, 0, &mut |_, _| {}));
    });
    r.check(
        "rpc: a poll started from inside a dispatcher is refused",
        matches!(inner, Some(Err(RpcError::NestedPoll))),
        format!("{:?}", inner.map(|x| x.map(|m| m.function))),
    );

    // Waiting for nothing handles everything and returns nothing
    let mut q = Loopback::new(16);
    for f in [rpc::EV_UCODE_LIBOS_PRINT, rpc::EV_POST_EVENT, rpc::EV_UPDATE_GSP_TRACE] {
        let _ = GspRpc::over(&mut q).send(f, &[0; 8], 1_000_000);
    }
    q.reflect();
    let mut handled = 0;
    let drained = GspRpc::over(&mut q).drain(NOTHING, None, Context::Interrupt, &mut |_, _| {}, &mut handled);
    r.check(
        "rpc: a drain for nothing handles all three events and is left with nothing",
        matches!(drained, Ok(None)) && handled == 3,
        format!("{:?} handled {}", drained.map(|m| m.map(|m| m.function)), handled),
    );

    // A whole call longer than a record: out as two records, back as a reply
    // and a CONTINUATION_RECORD, put together the size of the request
    let mut q = Loopback::answering(32);
    let big: alloc::vec::Vec<u8> = (0..70_000u32).map(|i| (i * 13 + 1) as u8).collect();
    let back = GspRpc::over(&mut q).call_with(FN, &big, 1_000_000, &mut |_, _| {});
    r.check(
        "rpc: a 70000-byte call comes back whole from a reply and its continuation",
        matches!(&back, Ok(m) if m.function == FN && m.payload == big),
        format!("{:?}", back.map(|m| m.payload.len())),
    );

    // The reply is the record with the function and the sequence of the request
    // (`_kgspRpcDrainOneEvent`): an older answer to the same function is an
    // event on the way
    let mut q = Loopback::new(16);
    let old = GspRpc::over(&mut q).send(FN, &[1; 8], 1_000_000).map(|s| s.1);
    let new = GspRpc::over(&mut q).send(FN, &[2; 8], 1_000_000).map(|s| s.1);
    q.reflect();
    let mut passed = alloc::vec::Vec::new();
    let got = match new {
        Ok(seq) => GspRpc::over(&mut q).recv_poll(FN, Some(seq), Context::Poll, 1_000_000, &mut |m, _| {
            passed.push(m.sequence);
        }),
        Err(e) => Err(e),
    };
    r.check(
        "rpc: a reply with the wrong sequence is handed on as an event, the right one is returned",
        matches!((&got, old, new), (Ok(m), Ok(o), Ok(n)) if m.sequence == n && m.payload == [2; 8] && passed == [o]),
        format!("old {:?} new {:?} got {:?} passed {:?}", old, new,
            got.as_ref().map(|m| m.sequence).map_err(|e| *e), passed),
    );

    // GMCAPI over the same rings: GSP_INIT out, its response back, matched
    // by id, sequence and the response flag
    let mut q = Loopback::answering(16);
    let data: alloc::vec::Vec<u8> = (0..300u32).map(|i| i as u8).collect();
    let back = GspRpc::over(&mut q)
        .send_gmcapi(gmcapi::GSP_INIT, &data, gmcapi::GSP_INIT_MAX_RESPONSE, 1_000_000)
        .and_then(|seq| GspRpc::over(&mut q).recv_poll(gmcapi::GSP_INIT, Some(seq), Context::PollBootup, 1_000_000, &mut |_, _| {}));
    let regs = q.regs();
    let short = back.as_ref().ok().map(|m| gmcapi::response_data(
        RpcMessage { payload: m.payload.clone(), ..RpcMessage::rm(m.function, alloc::vec::Vec::new()) }, 64,
    ));
    let back = back.map(|m| gmcapi::response_data(m, gmcapi::GSP_INIT_MAX_RESPONSE));
    r.check(
        "gmcapi: GSP_INIT goes out and its response comes back as the reply, data whole",
        matches!(&back, Ok(Ok(p)) if *p == data),
        format!("{:?} registers {:?}", back.as_ref().map(|p| p.as_ref().map(|p| p.len())), regs),
    );
    r.check(
        "gmcapi: a response longer than the request's max_response is refused",
        matches!(short, Some(Err(gmcapi::GmcError::TooLong(300)))),
        format!("{:?}", short),
    );
    // A request is not a response: one reflected without the flag is an event
    let mut q = Loopback::new(16);
    let seq = GspRpc::over(&mut q).send_gmcapi(gmcapi::GSP_INIT, &[0; 8], 64, 1_000_000);
    q.reflect();
    let mut events = 0;
    let got = match seq {
        Ok(s) => GspRpc::over(&mut q).recv_poll(gmcapi::GSP_INIT, Some(s), Context::PollBootup, 1_000_000, &mut |m, _| {
            if m.function == gmcapi::GSP_INIT && !m.response {
                events += 1;
            }
        }),
        Err(e) => Err(e),
    };
    r.check(
        "gmcapi: a GMCAPI record without the response flag is not the reply",
        matches!(got, Err(RpcError::RecvTimeout)) && events == 1,
        format!("{:?} events {}", got.map(|m| m.function), events),
    );
    // A response with a status is the status, not the data
    let mut q = Loopback::new(16);
    let seq = GspRpc::over(&mut q).send_gmcapi(gmcapi::GSP_INIT, &[0; 8], 64, 1_000_000).unwrap_or(0);
    let mut rec = q.sent();
    let c = le::u32_at(&rec, rpc::MSG_HDR_SIZE);
    le::put_u32(&mut rec, rpc::MSG_HDR_SIZE, c | gmcapi::FLAG_RESPONSE);
    le::put_u32(&mut rec, rpc::MSG_HDR_SIZE + 16, 0x1f);
    q.inject(&rec);
    let mut none = |_: &RpcMessage, _: Context| {};
    let got = GspRpc::over(&mut q).recv_poll(gmcapi::GSP_INIT, Some(seq), Context::PollBootup, 1_000_000, &mut none);
    r.check(
        "gmcapi: a response's status word is its NV_STATUS",
        matches!(&got, Ok(m) if m.response && m.rpc_result == 0x1f && m.sequence == seq),
        format!("{:?}", got.as_ref().map(|m| (m.response, m.rpc_result, m.sequence)).map_err(|e| *e)),
    );

    // The dispatcher. During boot only OpenRM's six are handled
    let (_, refused0) = kgsp::events::oddities();
    kgsp::events::dispatch(&msg(rpc::EV_POST_EVENT, alloc::vec![0; 32]), Context::PollBootup);
    let (_, refused1) = kgsp::events::oddities();
    let _ = kgsp::events::take_journal();
    // `rpc_os_error_log_v17_00` in 615: exceptType 0, chid 12, errString 16
    let mut xid = alloc::vec![0u8; 24];
    xid[0] = 79;
    xid[12] = 7;
    xid[16..20].copy_from_slice(b"test");
    kgsp::events::dispatch(&msg(rpc::EV_OS_ERROR_LOG, xid), Context::PollBootup);
    let (_, refused2) = kgsp::events::oddities();
    let journal = kgsp::events::take_journal();
    r.check(
        "kgsp: at boot a POST_EVENT is refused and an OS_ERROR_LOG is handled",
        refused1 == refused0 + 1 && refused2 == refused1
            && journal.xid.first().is_some_and(|(n, c, t)| *n == 79 && *c == 7 && t == "test"),
        format!("refused {}->{}->{} xid {:?}", refused0, refused1, refused2, journal.xid),
    );

    let was = kgsp::in_lockdown();
    kgsp::events::dispatch(&msg(rpc::EV_GSP_LOCKDOWN_NOTICE, alloc::vec![1]), Context::PollBootup);
    let engaged = kgsp::in_lockdown();
    kgsp::events::dispatch(&msg(rpc::EV_GSP_LOCKDOWN_NOTICE, alloc::vec![0]), Context::PollBootup);
    let released = !kgsp::in_lockdown();
    if was {
        kgsp::events::dispatch(&msg(rpc::EV_GSP_LOCKDOWN_NOTICE, alloc::vec![1]), Context::Poll);
    }
    r.check(
        "kgsp: LOCKDOWN_NOTICE engages the lockdown flag and releases it",
        engaged && released,
        format!("engaged {} released {}", engaged, released),
    );

    let seen0 = kgsp::events::seen(rpc::EV_RC_TRIGGERED);
    let (rc0, _) = kgsp::events::trouble_counts();
    let mut rc = alloc::vec![0u8; rpc::RcRecord::SIZE];
    rc[4] = 9;
    kgsp::events::dispatch(&msg(rpc::EV_RC_TRIGGERED, rc), Context::Interrupt);
    let (rc1, _) = kgsp::events::trouble_counts();
    let journal = kgsp::events::take_journal();
    r.check(
        "kgsp: RC_TRIGGERED is counted and its record reaches the journal",
        kgsp::events::seen(rpc::EV_RC_TRIGGERED) == seen0 + 1 && rc1 == rc0 + 1
            && journal.rc.first().is_some_and(|r| r.chid == 9),
        format!("seen {} rc {}->{} journal {:?}", seen0, rc0, rc1, journal.rc),
    );
}

/// Which GB20x a PMC_BOOT_42 value is, and where its firmware is.
///
/// Every value is built from the field layout in `nv_ref.h:149-155`, so a
/// failure means the decode disagrees with the reference
fn chip_table(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::blackwell::chips;
    use crate::drivers::gpu::nvidia::chip::ChipId;

    // ARCHITECTURE 29:24, IMPLEMENTATION 23:20, MAJOR 19:16, MINOR 15:12
    let boot42 = |arch: u32, imp: u32| (arch << 24) | (imp << 20) | (0xa << 16) | (0x1 << 12);
    let named: alloc::vec::Vec<_> = [2u32, 3, 5, 6, 7]
        .iter()
        .map(|&i| (i, chips::by_boot42(boot42(0x1b, i)).map(|c| c.codename)))
        .collect();
    r.check(
        "chips: all five GB20x are named by PMC_BOOT_42",
        named.iter().zip(["GB202", "GB203", "GB205", "GB206", "GB207"])
            .all(|((_, got), want)| *got == Some(want)),
        format!("{:?}", named),
    );
    r.check(
        "chips: a GB100 (architecture 0x1A) and an unknown GB20x number are not GB20x",
        chips::by_boot42(boot42(0x1a, 0)).is_none() && chips::by_boot42(boot42(0x1b, 4)).is_none(),
        format!("gb100 {:?} gb204 {:?}",
            chips::by_boot42(boot42(0x1a, 0)).map(|c| c.codename),
            chips::by_boot42(boot42(0x1b, 4)).map(|c| c.codename)),
    );
    let id = ChipId::from_boot42(boot42(0x1b, 6));
    r.check(
        "chips: PMC_BOOT_42 decodes to arch 0x1B, impl 6, rev a.1 - GB206",
        id.arch_byte() == 0x1b && id.implementation == 6 && id.major_rev == 0xa
            && id.minor_rev == 1 && id.codename() == "GB206",
        format!("{:?} {}", id, id.codename()),
    );
    // The paths the boot image has; the version is the GSP-RM the driver
    // speaks, and the ucodes file is new in 615
    let gb206 = chips::by_boot42(boot42(0x1b, 6));
    r.check(
        "chips: a GB206 loads nvidia/gb206/gsp/*-615.78.08.bin with its ucodes, signed in .fwsignature_gb20x",
        gb206.is_some_and(|c| c.fmc == "nvidia/gb206/gsp/fmc-615.78.08.bin"
            && c.bootloader == "nvidia/gb206/gsp/bootloader-615.78.08.bin"
            && c.gsp == "nvidia/gb206/gsp/gsp-615.78.08.bin"
            && c.ucodes == "nvidia/gb206/gsp/ucodes-615.78.08.bin"
            && c.sig_section == ".fwsignature_gb20x"),
        format!("{:?}", gb206),
    );
}

/// Engines come up in the chip's order and go down in the reverse one, and a
/// failed load does not stop the ones after it (`gpuStateLoad`/`Unload`)
fn engine_order(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::kgsp::engine::{load_all, unload_all, Engine};

    struct Step(&'static str, bool);
    impl Engine<alloc::vec::Vec<alloc::string::String>> for Step {
        fn name(&self) -> &'static str { self.0 }
        fn load(&self, log: &mut alloc::vec::Vec<alloc::string::String>) -> Result<(), &'static str> {
            log.push(format!("+{}", self.0));
            if self.1 { Ok(()) } else { Err("refused") }
        }
        fn unload(&self, log: &mut alloc::vec::Vec<alloc::string::String>) {
            log.push(format!("-{}", self.0));
        }
    }
    let (a, b, c) = (Step("memsys", true), Step("intr", false), Step("display", true));
    let engines: [&dyn Engine<alloc::vec::Vec<alloc::string::String>>; 3] = [&a, &b, &c];
    let mut log = alloc::vec::Vec::new();
    let said = core::cell::Cell::new(0u32);
    let say = |_: &str| said.set(said.get() + 1);
    let up = load_all(&engines, &mut log, &say);
    unload_all(&engines, &mut log, &say);
    r.check(
        "engines: load in order past a failure, unload every one in reverse",
        up == 2 && log == ["+memsys", "+intr", "+display", "-display", "-intr", "-memsys"],
        format!("{} up, {:?}", up, log),
    );
}

/// What GSP-RM is asked for before it boots, worked by hand from OpenRM 615
/// (`dev/gsp-615.md`, section 5)
fn fb_layout(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::blackwell::boot;
    use crate::drivers::gpu::nvidia::gsp_common::bootargs::GspBootArgs;
    use crate::drivers::gpu::nvidia::le;

    // 22 + 18 + align(96 KiB x GiB, 1 MiB) + 96, clamped to 88 at the bottom
    let heaps = [
        boot::wpr_heap_size_for(16 << 30, false) >> 20,
        boot::wpr_heap_size_for(8 << 30, false) >> 20,
        boot::wpr_heap_size_for(32 << 30, false) >> 20,
    ];
    r.check(
        "layout: the WPR heap is 138 MiB for 16 GiB, 137 for 8, 139 for 32 (_kgspCalculateFwHeapSize, 18 MiB base)",
        heaps == [138, 137, 139],
        format!("{:?}", heaps),
    );
    r.check(
        "layout: gspbase keeps boot A's 170 MiB",
        boot::wpr_heap_size_for(16 << 30, true) >> 20 == 170,
        format!("{}", boot::wpr_heap_size_for(16 << 30, true) >> 20),
    );
    // align(0x950000 + 0x1040000 + 4 KiB, 128 KiB), then
    // align(128 KiB + 4 KiB + that, 128 KiB)
    r.check(
        "layout: the PMU reserves 0x19a0000 and FRTS sits 0x19e0000 below the end of FB (kfspSendBootCommands_GH100)",
        boot::RSVD_SIZE_PMU == 0x19a_0000 && boot::frts_vidmem_offset() == 0x19e_0000,
        format!("pmu {:#x} frts {:#x}", boot::RSVD_SIZE_PMU, boot::frts_vidmem_offset()),
    );
    // _VAL is 31:4: the low nibble alone is not a region
    r.check(
        "layout: WPR2 is up only when ADDR_HI bits 31:4 are set",
        !boot::is_wpr2_up(0) && !boot::is_wpr2_up(0xf) && boot::is_wpr2_up(0x10),
        format!("{} {} {}", boot::is_wpr2_up(0), boot::is_wpr2_up(0xf), boot::is_wpr2_up(0x10)),
    );

    // GSP_FMC_BOOT_PARAMS at the offsets gcc gave the 615 header
    let mut p = [0xaau8; boot::GSP_FMC_BOOT_PARAMS_SIZE];
    boot::GspFmcBootParams::new(0x1234_5000, 0x6789_a000).write_bytes(&mut p);
    let got = (
        &p[0..4], le::u16_at(&p, 4), le::u32_at(&p, 64), le::u32_at(&p, 68), le::u64_at(&p, 72),
        p[92], le::u32_at(&p, 128), le::u64_at(&p, 136),
    );
    r.check(
        "layout: FMC boot params are 256 bytes, \"FMC \" and size, WPR metadata at 64 (coherent), libos at 128 (non-coherent)",
        got == (&b"FMC "[..], 256, 1, 256, 0x1234_5000, 1, 2, 0x6789_a000)
            && p[160..].iter().all(|&b| b == 0) && p[8..64].iter().all(|&b| b == 0),
        format!("{:x?}", got),
    );

    // The boot arguments, built in system memory and read back: MSGQ 2.0
    // headers, GSP_ARGUMENTS_CACHED, the eight libos log regions
    match GspBootArgs::self_test() {
        Ok(b) => r.check(
            "layout: boot arguments - PTEs, the 2.0 command queue header, GSP_ARGUMENTS_CACHED, nine libos regions, logs",
            b.ok,
            format!("ptes {} cmdq {} rmargs {} libos {} logs {}", b.ptes_ok, b.cmdq_ok, b.rmargs_ok, b.libos_ok, b.log_ok),
        ),
        Err(e) => r.check("layout: boot arguments can be built", false, format!("{:?}", e)),
    }
}

/// NVKV, and the two lists GSP_INIT carries: system info in, static info out
/// (`dev/gsp-615.md`, section 4)
fn nvkv_lists(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::nvkv::{self, DecodeError, Encoder, Value};
    use crate::drivers::gpu::nvidia::gsp_common::{staticinfo, sysinfo::SystemInfo};

    // Every opcode, and what comes back
    let mut e = Encoder::new();
    e.u32(0, 0x10, 7);
    e.seq32(0, 0x20, &[1, 2, 3]);
    e.seq64(2, 0x30, &[u64::MAX, 5]);
    e.bytes(0, 0x40, b"abc");
    e.array32(0, 0x50, &[9, 10, 11]);
    e.string(0, 0x60, "NVRM");
    let words = e.words().to_vec();
    let bytes = e.to_bytes();
    let mut seen = alloc::vec::Vec::new();
    let decoded = nvkv::decode(&bytes, |i, k, v| {
        seen.push(match v {
            Value::U32(x) => format!("{}:{:#x}={}", i, k, x),
            Value::U64(x) => format!("{}:{:#x}={:#x}", i, k, x),
            Value::Bytes(b) => format!("{}:{:#x}={:?}", i, k, b),
            Value::U32s(b) | Value::U64s(b) => format!("{}:{:#x}#{}", i, k, b.len()),
        });
        true
    });
    let want = [
        "0:0x10=7", "0:0x20=1", "0:0x21=2", "0:0x22=3", "2:0x30=0xffffffffffffffff", "2:0x31=0x5",
        "0:0x40=[97, 98, 99]", "0:0x50#12", "0:0x60=[78, 86, 82, 77, 0]",
    ];
    r.check(
        "nvkv: IMM32, SEQ32, SEQ64, ARRAY8, ARRAY32 and a string decode to what was encoded",
        decoded.is_ok() && seen == want,
        format!("{:?}", seen),
    );
    // The instruction word: key 15:0, index 27:16, opcode 31:28, count 63:32
    r.check(
        "nvkv: an instruction is key | index << 16 | opcode << 28 | count << 32",
        words[0] == 7 << 32 | 0x10 && words[1] == 3 << 32 | 1 << 28 | 0x20
            && words[4] == 2 << 32 | 2 << 28 | 2 << 16 | 0x30,
        format!("{:#x?}", &words[..5]),
    );
    let cut = nvkv::decode(&bytes[..bytes.len() - 8], |_, _, _| true);
    let mut bad = bytes.clone();
    bad[3] = 0x70;
    let opcode = nvkv::decode(&bad, |_, _, _| true);
    r.check(
        "nvkv: values running past the end and an unknown opcode are refused",
        matches!(cut, Err(DecodeError::OutOfRange { .. })) && opcode == Err(DecodeError::BadOpcode { at: 0, opcode: 7 }),
        format!("{:?} {:?}", cut, opcode),
    );

    // System info, as `_kgspEncodeSystemInfo` lays it out
    let info = SystemInfo {
        pci_device_id: 0x2d04_10de,
        gpu_phys: [0xf000_0000, 0xe000_0000, 0xf800_0000, 0],
        console_mem_size: 0x7f_0000,
        cl_flags: 1 << 37 | 1 << 7,
        is_primary: true,
        ..Default::default()
    };
    let list = info.encode(&[("RMSecBusResetEnable", 1), ("RMDevidCheckIgnore", 1)]);
    let mut keys = alloc::collections::BTreeMap::new();
    let mut regs = alloc::vec::Vec::new();
    let mut name = alloc::string::String::new();
    let ok = nvkv::decode(&list, |_, k, v| {
        match (k, v) {
            (0x3070, v) => name = v.as_str().unwrap_or_default(),
            (0x3071, Value::U32(x)) => regs.push((core::mem::take(&mut name), x)),
            _ => {
                keys.insert(k, v.as_u64().or(v.as_u32().map(u64::from)).or(v.as_str().map(|_| 0)).unwrap_or(u64::MAX));
            }
        }
        true
    }).is_ok();
    let mut strings = alloc::vec::Vec::new();
    let _ = nvkv::decode(&list, |_, k, v| {
        if matches!(k, 0x1100 | 0x1101) {
            strings.push(v.as_str().unwrap_or_default());
        }
        true
    });
    r.check(
        "sysinfo: ids, BARs, console size, chipset flags (bit 37) and the primary flag are their keys",
        ok && keys.get(&0x0001) == Some(&0x2d04_10de) && keys.get(&0x1010) == Some(&0xf000_0000)
            && keys.get(&0x1012) == Some(&0xf800_0000) && keys.get(&0x1031) == Some(&0x7f_0000)
            && keys.get(&0x1033) == Some(&(1 << 37 | 1 << 7)) && keys.get(&0x1001) == Some(&(1 << 2))
            && keys.get(&0x0070) == Some(&1) && keys.get(&0x0078) == Some(&4096),
        format!("{} keys", keys.len()),
    );
    r.check(
        "sysinfo: the driver is NVRM 615.78.08, and each registry key is a name and a value",
        strings == ["NVRM", crate::drivers::gpu::nvidia::gsp_common::GSP_RM_VERSION]
            && regs.iter().map(|(n, v)| (n.as_str(), *v)).eq([("RMSecBusResetEnable", 1), ("RMDevidCheckIgnore", 1)]),
        format!("{:?} {:?}", strings, regs),
    );

    // Static info, a list made by hand as GSP-RM orders it: the region count
    // before the regions
    let mut e = Encoder::new();
    e.seq32(0, 0x0FF0, &[0xc1d0_0001, 0x5c00_0002, 0x5c00_0003]);
    e.u64(0, 0x1010, 16 << 30);
    e.u32(0, 0x0010, 2);
    for (i, (base, limit, reserved, flags)) in
        [(0u64, (512u64 << 20) - 1, 0u64, 3u32), (512 << 20, (16u64 << 30) - 1, 1 << 20, 3)].iter().enumerate()
    {
        e.seq64(i as u16, 0x1011, &[*base, *limit, *reserved]);
        e.u32(i as u16, 0x0012, *flags);
    }
    // One index past the count, which must not be taken
    e.u64(2, 0x1011, 0xdead_0000);
    e.u32(0, 0x0210, 3 << 16 | 1);
    e.string(0, 0x2000, "NVIDIA GeForce RTX 5060 Ti");
    e.u32(0, 0x7777, 1);
    match staticinfo::decode(&e.to_bytes()) {
        Ok(si) => {
            r.check(
                "staticinfo: handles, FB length, VGX version and the name are read from their keys",
                si.h_internal_client == 0xc1d0_0001 && si.h_internal_subdevice == 0x5c00_0003
                    && si.fb_length == 16 << 30 && si.vgx_version == 0x30001
                    && si.gpu_name == "NVIDIA GeForce RTX 5060 Ti",
                format!("{:#x} {:#x} {} {:#x} {:?}", si.h_internal_client, si.h_internal_subdevice,
                    si.fb_length, si.vgx_version, si.gpu_name),
            );
            r.check(
                "staticinfo: two regions by index, the one with a reservation not usable; an index past the count and an unknown key skipped",
                si.regions.len() == 2 && si.regions[1].base == 512 << 20 && si.regions[1].reserved == 1 << 20
                    && si.usable_regions().count() == 1 && si.skipped == 2,
                format!("{:x?} skipped {}", si.regions, si.skipped),
            );
        }
        Err(e) => r.check("staticinfo: a list made by hand decodes", false, format!("{:?}", e)),
    }
}

fn interrupt_tree(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::intr::{
        KernelTable, CATEGORIES, CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION, PARAMS_SIZE,
    };
    use crate::drivers::gpu::nvidia::intr::Geometry;

    let gb = Geometry::GH100;
    let ga = Geometry::TU102;
    r.check(
        "intr: a GB20x tree is 12 leaves in 6 subtrees, out of 16 leaf registers",
        gb.leaves == 12 && gb.subtrees() == 6 && gb.subtree_mask() == 0x3f && gb.leaf_regs == 16,
        format!("{:?} subtrees {} mask {:#x}", gb, gb.subtrees(), gb.subtree_mask()),
    );
    r.check(
        "intr: a TU/GA/AD tree is 8 leaves in 4 subtrees",
        ga.leaves == 8 && ga.subtree_mask() == 0xf,
        format!("{:?} mask {:#x}", ga, ga.subtree_mask()),
    );
    r.check(
        "intr: vector 383 is leaf 11 bit 31 on a GB20x and past the tree on a GA102",
        gb.split(383) == Some((11, 31)) && gb.split(384).is_none()
            && ga.split(383).is_none() && ga.split(255) == Some((7, 31)),
        format!("gb {:?}/{:?} ga {:?}/{:?}",
            gb.split(383), gb.split(384), ga.split(383), ga.split(255)),
    );

    // Two engines and a subtree map: GSP on vector 300 (leaf 9, past a GA102's
    // reach), display on 5; category 0 on subtrees 0-3, the non-stalling engine
    // notifications on 4-5, the rest never initialised
    let mut reply = alloc::vec![0u8; PARAMS_SIZE];
    reply[0..4].copy_from_slice(&2u32.to_le_bytes());
    for (i, (engine, stall)) in [(50u16, 300u32), (2, 5)].iter().enumerate() {
        let e = 4 + i * 16;
        reply[e..e + 2].copy_from_slice(&engine.to_le_bytes());
        reply[e + 8..e + 12].copy_from_slice(&stall.to_le_bytes());
        reply[e + 12..e + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    // 2056: the subtree map, a u64 mask per category in 615 (570 had a start
    // and an end byte at 2052)
    for c in 0..CATEGORIES {
        let mask: u64 = match c {
            0 => 0xf,
            CATEGORY_ESCHED_DRIVEN_ENGINE_NOTIFICATION => 0x30,
            1 => 0,
            _ => u64::MAX,
        };
        reply[2056 + c * 8..2064 + c * 8].copy_from_slice(&mask.to_le_bytes());
    }
    match KernelTable::parse(&reply) {
        Ok(t) => {
            r.check(
                "intr: the table's vectors are routable on a GB20x tree and only there",
                t.entries.len() == 2
                    && t.stall_vector(50, gb) == Some(300)
                    && t.stall_vector(50, ga).is_none()
                    && t.stall_vector(2, ga) == Some(5),
                format!("{:?}", t.entries),
            );
            // GSP-RM's mask as it stands, and nothing for a category that is
            // empty or all ones (`intrGetIntrTopCategoryMask_IMPL`, 615)
            r.check(
                "intr: the non-stall mask is its category's mask; an empty or all-ones category is none",
                t.nonstall_subtrees() == 0x30 && t.category_mask(0) == 0xf
                    && t.category_mask(1) == 0 && t.category_mask(3) == 0,
                format!("map {:x?} nonstall {:#x}", t.subtrees, t.nonstall_subtrees()),
            );
        }
        Err(e) => r.check("intr: a made-up kernel table parses", false, format!("{:?}", e)),
    }
}

/// The GSP transport, over two rings in plain memory.
///
/// `Loopback` hands every record sent back as a record received, so the real
/// `send` and `recv` meet, with four fake pointer registers where the card has
/// GSP's. The framing is what these decide (MSGQ 2.0, MCTP/NVDM,
/// `dev/gsp-615.md`, sections 1-2)
fn rpc_transport(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::rpc::{
        self, GspRpc, LinkError, Loopback, RpcError, FN_CONTINUATION_RECORD, GSP_PAGE, MAX_PAYLOAD,
        MCTP_MAGIC, MCTP_MAGIC_SEEN, MSG_HDR_SIZE, RPC_HDR_SIZE, RPC_RESULT_PENDING,
    };
    use crate::drivers::gpu::nvidia::le;

    const FN: u32 = 0x55;
    let bytes = |n: usize| -> alloc::vec::Vec<u8> { (0..n).map(|i| (i * 7 + 3) as u8).collect() };
    // (function, rpc length, slots, sequence) of each record on the wire,
    // the slots from the record's own MCTP size
    let records = |wire: &[u8]| {
        let mut out = alloc::vec::Vec::new();
        let mut at = 0;
        while at + GSP_PAGE <= wire.len() {
            let pages = (le::u32_at(wire, at + 4) as usize).div_ceil(GSP_PAGE).max(1);
            out.push((
                le::u32_at(wire, at + MSG_HDR_SIZE + 12),
                le::u32_at(wire, at + MSG_HDR_SIZE + 8),
                pages as u32,
                le::u32_at(wire, at + MSG_HDR_SIZE + 24),
            ));
            at += pages * GSP_PAGE;
        }
        out
    };

    // One slot, and what is in it
    let mut q = Loopback::new(32);
    let small = bytes(100);
    let sent = GspRpc::over(&mut q).send(FN, &small, 1_000_000).is_ok();
    let w = q.sent();
    let head = (
        le::u32_at(&w, 0), le::u32_at(&w, 4), le::u32_at(&w, 8), le::u32_at(&w, 12), le::u32_at(&w, 16),
    );
    let rpc_head = (
        le::u32_at(&w, 24), le::u32_at(&w, 28), le::u32_at(&w, 32), le::u32_at(&w, 36),
        le::u32_at(&w, 40), le::u32_at(&w, 44),
    );
    r.check(
        "rpc: a record is MCTP-framed - magic, size from the start, SOM|EOM v1, NVDM 0x25 for NVIDIA, payload size",
        sent && head == (MCTP_MAGIC, (MSG_HDR_SIZE + RPC_HDR_SIZE + 100) as u32, 0xc000_0001,
            0x2510_de7e, (RPC_HDR_SIZE + 100) as u32),
        format!("{:#x?}", head),
    );
    r.check(
        "rpc: the RPC header inside it is rpc_message_header_v as before, both results pending",
        rpc_head == (0x0300_0000, 0x4350_5256, (RPC_HDR_SIZE + 100) as u32, FN,
            RPC_RESULT_PENDING, RPC_RESULT_PENDING),
        format!("{:#x?}", rpc_head),
    );
    let after_send = q.regs();
    q.reflect();
    let after_reflect = q.regs();
    let back = GspRpc::over(&mut q).recv(1_000_000);
    let after_recv = q.regs();
    r.check(
        "rpc: a one-slot message comes back exactly as it was sent",
        matches!(&back, Ok(m) if m.function == FN && m.payload == small && m.nvdm == rpc::NVDM_RM_RPC),
        format!("{:?}", back.as_ref().map(|m| (m.function, m.payload.len())).map_err(|e| *e)),
    );
    // QUEUE_HEAD, QUEUE_TAIL, MSGQ_HEAD, MSGQ_TAIL
    r.check(
        "rpc: the pointer registers count records - QUEUE_HEAD on send, MSGQ_TAIL on read",
        after_send == [1, 0, 0, 0] && after_reflect == [1, 1, 1, 0] && after_recv == [1, 1, 1, 1],
        format!("{:?} {:?} {:?}", after_send, after_reflect, after_recv),
    );
    r.check(
        "rpc: a record read is marked SEEN in the ring",
        q.magic_behind(1) == MCTP_MAGIC_SEEN,
        format!("{:#x}", q.magic_behind(1)),
    );

    // Several slots, one record
    let mut q = Loopback::new(32);
    let mid = bytes(10_000);
    let _ = GspRpc::over(&mut q).send(FN, &mid, 1_000_000);
    let wire = records(&q.sent());
    q.reflect();
    let back = GspRpc::over(&mut q).recv(1_000_000);
    let pages = (MSG_HDR_SIZE + RPC_HDR_SIZE + mid.len()).div_ceil(GSP_PAGE) as u32;
    r.check(
        "rpc: a 10000-byte message is one record of three slots, and comes back whole",
        wire.len() == 1 && wire[0].2 == pages && pages == 3
            && matches!(&back, Ok(m) if m.payload == mid),
        format!("records {:?}, want {} slots", wire, pages),
    );

    // Longer than one record: cut as `_issueRpcLarge` cuts it (`vgpu/rpc.c`):
    // every record's length is its own, the first carries the function and the
    // rest CONTINUATION_RECORD, each its own sequence
    let mut q = Loopback::new(32);
    let big = bytes(70_000);
    let first = GspRpc::over(&mut q).send(FN, &big, 1_000_000);
    let wire = records(&q.sent());
    let cap = MAX_PAYLOAD - RPC_HDR_SIZE;
    let want = [
        (FN, (RPC_HDR_SIZE + cap) as u32, 16u32),
        (FN_CONTINUATION_RECORD, (RPC_HDR_SIZE + big.len() - cap) as u32,
            (MSG_HDR_SIZE + RPC_HDR_SIZE + big.len() - cap).div_ceil(GSP_PAGE) as u32),
    ];
    let shape: alloc::vec::Vec<_> = wire.iter().map(|w| (w.0, w.1, w.2)).collect();
    r.check(
        "rpc: 70000 bytes go out as a full 16-slot record and a CONTINUATION_RECORD",
        shape == want,
        format!("got {:?} want {:?}", shape, want),
    );
    r.check(
        "rpc: each record of a split message takes the next sequence, and send says the first",
        wire.len() == 2 && wire[1].3 == wire[0].3.wrapping_add(1)
            && matches!(first, Ok((2, s)) if s == wire[0].3),
        format!("records {:?} send {:?}", wire, first),
    );

    // The counters only count up; a slot is the counter modulo the ring, so a
    // three-slot record sent into a four-slot ring wraps on every turn
    let mut q = Loopback::new(4);
    let msg = bytes(9_000);
    let mut whole = 0;
    for _ in 0..6 {
        let _ = GspRpc::over(&mut q).send(FN, &msg, 1_000_000);
        q.reflect();
        if matches!(GspRpc::over(&mut q).recv(1_000_000), Ok(m) if m.payload == msg) {
            whole += 1;
        }
    }
    r.check(
        "rpc: records that wrap the end of the ring come back whole, the registers counting past it",
        whole == 6 && q.regs() == [18, 18, 18, 18],
        format!("{} whole, registers {:?}", whole, q.regs()),
    );

    // A ring with no room says so instead of overwriting what GSP-RM has not
    // read
    let mut q = Loopback::new(4);
    let fit = (0..3).all(|_| GspRpc::over(&mut q).send(FN, &small, 1_000_000).is_ok());
    let full = GspRpc::over(&mut q).send(FN, &bytes(5_000), 1_000_000);
    r.check(
        "rpc: a two-slot record does not go into a ring with one slot free",
        fit && matches!(full, Err(RpcError::CmdqTimeout)) && q.regs()[0] == 3,
        format!("{:?} registers {:?}", full, q.regs()),
    );

    // Stale: a record already read arrives again and is skipped
    let mut q = Loopback::new(32);
    let _ = GspRpc::over(&mut q).send(FN, &[1; 16], 1_000_000);
    let _ = GspRpc::over(&mut q).send(FN, &[2; 16], 1_000_000);
    q.reflect();
    let _ = GspRpc::over(&mut q).recv(1_000_000);
    q.replay(1);
    let stale0 = rpc::stale_records();
    let next = GspRpc::over(&mut q).recv(1_000_000);
    let stale1 = rpc::stale_records();
    r.check(
        "rpc: a record still marked SEEN is stale - counted, skipped, and the next one read",
        stale1 == stale0 + 1 && matches!(&next, Ok(m) if m.payload == [2; 16]),
        format!("stale {}->{} next {:?}", stale0, stale1,
            next.as_ref().map(|m| m.payload.clone()).map_err(|e| *e)),
    );

    // The receive checks are OpenRM's: each refused record is consumed, so the
    // ring moves on
    let refused = |offset: usize| {
        let mut q = Loopback::new(32);
        let _ = GspRpc::over(&mut q).send(FN, &small, 1_000_000);
        let _ = GspRpc::over(&mut q).send(FN, &[9; 4], 1_000_000);
        q.reflect();
        q.damage(offset);
        let first = GspRpc::over(&mut q).recv(1_000_000).map(|m| m.function);
        let then = GspRpc::over(&mut q).recv(1_000_000).map(|m| m.payload);
        (first, then.ok() == Some(alloc::vec![9; 4]))
    };
    let magic = refused(0);
    let vendor = refused(13);
    let nvdm = refused(15);
    let signature = refused(MSG_HDR_SIZE + 4);
    r.check(
        "rpc: a bad magic and a foreign vendor are bad frames, an unknown NVDM type and a bad RPC signature \
         are refused, and the next record is read after each",
        matches!(magic, (Err(RpcError::BadFrame { .. }), true))
            && matches!(vendor, (Err(RpcError::BadFrame { .. }), true))
            && matches!(nvdm, (Err(RpcError::UnknownNvdm(0xda)), true))
            && matches!(signature, (Err(RpcError::BadRpcHeader { .. }), true)),
        format!("{:?} {:?} {:?} {:?}", magic, vendor, nvdm, signature),
    );
    let mut q = Loopback::new(32);
    let _ = GspRpc::over(&mut q).send(FN, &small, 1_000_000);
    q.reflect();
    q.damage(7);
    let huge = GspRpc::over(&mut q).recv(1_000_000);
    r.check(
        "rpc: a record whose size is past 64 KiB is refused before anything is copied",
        matches!(huge, Err(RpcError::BadLength(_))) && q.regs()[3] == 1,
        format!("{:?} registers {:?}", huge.map(|m| m.function), q.regs()),
    );

    // Linking the status queue (`msgqRxLink`)
    let mut q = Loopback::unlinked(8);
    let before = GspRpc::over(&mut q).rx_link();
    q.publish_header(1, 8 * GSP_PAGE as u32, GSP_PAGE as u32, 8, 0);
    let old = GspRpc::over(&mut q).rx_link();
    q.publish_header(2, 8 * GSP_PAGE as u32, GSP_PAGE as u32, 7, 0);
    let short = GspRpc::over(&mut q).rx_link();
    q.publish_header(2, 8 * GSP_PAGE as u32, GSP_PAGE as u32, 8, 0);
    let ok = GspRpc::over(&mut q).rx_link();
    r.check(
        "rpc: the status queue links only to a 2.0 header of our size whose slots fill it",
        before == Err(LinkError::NotYet)
            && matches!(old, Err(LinkError::Mismatch { major: 1, .. }))
            && matches!(short, Err(LinkError::Mismatch { msg_count: 7, .. }))
            && matches!(ok, Ok(g) if g.msg_count == 8 && g.msg_size == GSP_PAGE as u32)
            && GspRpc::over(&mut q).rx_linked(),
        format!("{:?} {:?} {:?} {:?}", before, old, short, ok),
    );
}

/// The colour curve the head's output LUT is built from.
///
/// Every value on the screen passes through this table, and there is no libm
/// here, so the exponent is fixed-point arithmetic of our own. All pure, so
/// QEMU is the instrument for it
fn curves(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset::Curve;

    // Identity is exactly what the display has been running, so the curve on
    // that path changes no pixel until somebody asks
    let id = Curve::IDENTITY;
    let exact = (0..1024u32).all(|i| id.at(i) == i);
    r.check(
        "colour: gamma 1.0 over the full range is the identity table, entry for entry",
        exact,
        format!("1024 entries, first mismatch {:?}",
            (0..1024u32).find(|&i| id.at(i) != i)),
    );

    // Both ends are exact whatever the exponent: rounding should not make black
    // grey or white dim
    let ends = [500u32, 1000, 1800, 2200, 3000].iter().all(|&g| {
        let c = Curve { gamma_milli: g, black: 0, white: 1023 };
        c.at(0) == 0 && c.at(1023) == 1023
    });
    r.check("colour: every gamma keeps black at 0 and white at 1023", ends,
            format!("gammas 0.5 to 3.0"));

    // A ramp going backwards anywhere is a banded picture, the failure of a
    // fixed-point exponent losing precision
    let mut worst: Option<(u32, u32)> = None;
    let monotonic = [500u32, 1000, 2200, 2800].iter().all(|&g| {
        let c = Curve { gamma_milli: g, black: 0, white: 1023 };
        (1..1024u32).all(|i| {
            let ok = c.at(i) >= c.at(i - 1);
            if !ok && worst.is_none() {
                worst = Some((g, i));
            }
            ok
        })
    });
    r.check("colour: every ramp rises, so no gamma can band the picture", monotonic,
            format!("first fall {:?}", worst));

    // It has to be the right curve too. Midpoint of gamma 2.2 is 0.5^2.2 =
    // 0.2172 of full scale, and of gamma 0.5 is 0.7071
    let mid22 = Curve { gamma_milli: 2200, black: 0, white: 1023 }.at(511);
    let mid05 = Curve { gamma_milli: 500, black: 0, white: 1023 }.at(511);
    r.check(
        "colour: gamma 2.2 puts the midpoint where the arithmetic says",
        (mid22 as i64 - 222).abs() <= 3,
        format!("at(511) = {}, expected about 222", mid22),
    );
    r.check(
        "colour: gamma 0.5 puts the midpoint where the arithmetic says",
        (mid05 as i64 - 723).abs() <= 3,
        format!("at(511) = {}, expected about 723", mid05),
    );

    // The state is what a person configures, and it guards against a claim
    // disagreeing with the pixels: every register right and the picture washed
    // out
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{HeadColour, Range};
    let def = HeadColour::DEFAULT;
    r.check(
        "colour: the default state is what the hardware does by itself",
        def.is_default() && def.matching(),
        format!("identity ramp, full range"),
    );
    let mismatched = HeadColour {
        curve: Curve { gamma_milli: 1000, black: 0, white: 1023 },
        range: Range::Limited,
        ..HeadColour::DEFAULT
    };
    r.check(
        "colour: full-range pixels under a limited-range claim are caught",
        !mismatched.matching(),
        format!("pixels 0..1023, claim 64..940"),
    );

    // The encodings, against the field layout in `clca7d.h`. The range is at
    // bit 28: at bit 1, the COLOR_SPACE field, RGB pixels are announced as
    // YUV_709 and the card goes green
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{colour_encodings, Dither};
    let (procamp_full, dither_off) = colour_encodings(&HeadColour::DEFAULT);
    r.check(
        "colour: the default encodes to the zeroes this driver always wrote",
        procamp_full == 0 && dither_off == 0,
        format!("procamp {:#x}, dither {:#x}", procamp_full, dither_off),
    );
    let (procamp_lim, _) = colour_encodings(&HeadColour {
        range: Range::Limited, ..HeadColour::DEFAULT
    });
    r.check(
        "colour: limited range sets DYNAMIC_RANGE at 28 and nothing in COLOR_SPACE",
        procamp_lim == 1 << 28,
        format!("procamp {:#010x}, colour space bits {}", procamp_lim, procamp_lim & 3),
    );
    let (_, dither_spatial) = colour_encodings(&HeadColour {
        dither: Dither::Spatial, ..HeadColour::DEFAULT
    });
    r.check(
        "colour: spatial dither is enabled, to 8 bits, in STATIC_ERR_ACC",
        dither_spatial == (1 | (1 << 4) | (1 << 8)),
        format!("dither {:#010x}", dither_spatial),
    );

    // The depth on the wire rides in the output resource, next to the sync
    // polarity, so a wrong field position would invert a sync and lose the
    // monitor
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{output_resource_value, Depth, Mode};
    let m = Mode { nhsync: true, nvsync: false, ..Mode::default() };
    let eight = output_resource_value(&m, &HeadColour::DEFAULT);
    let ten = output_resource_value(&m, &HeadColour { depth: Depth::Bpc10, ..HeadColour::DEFAULT });
    r.check(
        "colour: ten bits changes the depth field and nothing else",
        (eight ^ ten) == ((4 ^ 5) << 4) && (eight & (1 << 2)) != 0 && (ten & (1 << 2)) != 0,
        format!("8 bpc {:#010x}, 10 bpc {:#010x}", eight, ten),
    );

    // The output matrix, for colour balance. A state nobody configured produces
    // the twelve words a lit head carries under the shipping driver
    use crate::drivers::gpu::nvidia::gsp_common::modeset::ocsc0_coefficients;
    let idle = ocsc0_coefficients(&HeadColour::DEFAULT);
    r.check(
        "colour: the default output matrix is the identity the lit head carries",
        idle == [0x10000, 0, 0, 0x20, 0, 0x10000, 0, 0x20, 0, 0, 0x10000, 0x20],
        format!("c00 {:#x}, c11 {:#x}, bias {:#x}", idle[0], idle[5], idle[3]),
    );

    // Half the red is half the coefficient, and nothing else moves. A
    // coefficient in the wrong cell tints the picture
    let warm = ocsc0_coefficients(&HeadColour { balance: [500, 1000, 1000], ..HeadColour::DEFAULT });
    r.check(
        "colour: a channel gain lands on its own diagonal cell and no other",
        warm[0] == 0x8000 && warm[5] == 0x10000 && warm[10] == 0x10000
            && warm[1] == 0 && warm[2] == 0 && warm[4] == 0,
        format!("c00 {:#x}, c11 {:#x}, c22 {:#x}", warm[0], warm[5], warm[10]),
    );

    // What the state does to a value, which `display color` prints. Taking blue
    // down leaves red and green alone, so white goes yellow and cyan text goes
    // green
    let cool = HeadColour { balance: [1000, 1000, 700], ..HeadColour::DEFAULT };
    let w = cool.sample(1023);
    r.check(
        "colour: taking blue down moves blue and nothing else",
        w[0] == 1023 && w[1] == 1023 && (w[2] as i64 - 716).abs() <= 2,
        format!("white -> r {} g {} b {}", w[0], w[1], w[2]),
    );
    let black = cool.sample(0);
    r.check(
        "colour: a channel gain leaves black alone",
        black == [0, 0, 0],
        format!("black -> r {} g {} b {}", black[0], black[1], black[2]),
    );

    // Limited range: a monitor expecting it shows full-range content with
    // crushed blacks
    let lim = Curve::LIMITED;
    r.check(
        "colour: limited range lands on 64 and 940",
        lim.at(0) == 64 && lim.at(1023) == 940 && !lim.is_identity(),
        format!("black {}, white {}", lim.at(0), lim.at(1023)),
    );
}

/// The cubin reader, on files built here.
///
/// This parser reads bytes a process supplied (`gpu_program` hands it what a
/// client passed), and the build uses it on `nvcc`'s output. All pure, so the
/// hostile cases belong in QEMU.
///
/// Each case is a real ELF64 header with one field moved out of range. A panic
/// on any would be a process crashing the kernel
fn cubins(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::cubin::{kernels, Bad};

    /// The smallest file the reader looks at twice: a valid ELF64 header, one
    /// section header table with a name table and one `.text.k` section
    fn build(names: &[u8], sections: &[(u32, u64, u64)], shstrndx: u16) -> alloc::vec::Vec<u8> {
        const EHDR: usize = 0x40;
        const SHENT: usize = 0x40;
        let shoff = EHDR + names.len();
        let mut f = alloc::vec![0u8; shoff + sections.len() * SHENT];
        f[0..4].copy_from_slice(b"\x7fELF");
        f[4] = 2; // ELF64
        f[0x28..0x30].copy_from_slice(&(shoff as u64).to_le_bytes());
        f[0x3a..0x3c].copy_from_slice(&(SHENT as u16).to_le_bytes());
        f[0x3c..0x3e].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        f[0x3e..0x40].copy_from_slice(&shstrndx.to_le_bytes());
        f[EHDR..EHDR + names.len()].copy_from_slice(names);
        for (i, (name, off, size)) in sections.iter().enumerate() {
            let sh = shoff + i * SHENT;
            f[sh..sh + 4].copy_from_slice(&name.to_le_bytes());
            f[sh + 0x18..sh + 0x20].copy_from_slice(&off.to_le_bytes());
            f[sh + 0x20..sh + 0x28].copy_from_slice(&size.to_le_bytes());
        }
        f
    }

    // Offsets into the name table below
    let names: &[u8] = b"\0.shstrtab\0.text.k\0";
    let n_str = 1u32;
    let n_text = 11u32;
    let str_off = 0x40u64;
    let str_len = names.len() as u64;

    // A file that should read: name table, and a `.text.k` whose contents are
    // the name table's own bytes
    let good = build(names, &[(n_str, str_off, str_len), (n_text, str_off, 4)], 0);
    let ok = kernels(&good);
    r.check(
        "cubin: a well-formed file yields its kernel",
        matches!(&ok, Ok(k) if k.len() == 1 && k[0].name == "k" && k[0].code.len() == 4),
        match &ok {
            Ok(k) => format!("{} kernel(s)", k.len()),
            Err(e) => format!("refused: {}", e.why()),
        },
    );

    // The same file with one number moved out of bounds, each with its own
    // refusal
    let cases: [(&'static str, alloc::vec::Vec<u8>, Bad); 6] = [
        ("cubin: a file shorter than a header is refused", alloc::vec![0u8; 16], Bad::TooShort),
        ("cubin: a file with no ELF magic is refused", {
            let mut f = good.clone();
            f[1] = b'X';
            f
        }, Bad::NotElf64),
        ("cubin: a section table past the end is refused", {
            let mut f = good.clone();
            f[0x28..0x30].copy_from_slice(&u64::MAX.to_le_bytes());
            f
        }, Bad::Sections),
        ("cubin: a name table index past the count is refused", {
            let mut f = good.clone();
            f[0x3e..0x40].copy_from_slice(&9u16.to_le_bytes());
            f
        }, Bad::Names),
        ("cubin: a section's contents past the end are refused", {
            build(names, &[(n_str, str_off, str_len), (n_text, str_off, u64::MAX)], 0)
        }, Bad::Contents),
        ("cubin: a file with no .text. section is refused", {
            build(names, &[(n_str, str_off, str_len)], 0)
        }, Bad::NoKernel),
    ];
    for (name, bytes, want) in cases {
        let got = kernels(&bytes);
        r.check(
            name,
            matches!(&got, Err(e) if *e == want),
            match &got {
                Ok(k) => format!("accepted, {} kernel(s)", k.len()),
                Err(e) => format!("{}", e.why()),
            },
        );
    }

    // A name whose offset runs past the name table is not a name. It must not
    // read into the rest of the file or be mistaken for a kernel
    let wild = build(names, &[(n_str, str_off, str_len), (9999, str_off, 4)], 0);
    r.check(
        "cubin: a section name outside the name table is not a kernel",
        matches!(kernels(&wild), Err(Bad::NoKernel)),
        format!("name offset {} into a {}-byte table", 9999, str_len),
    );
}

/// The number a GPU submission hands back to ring 3.
///
/// One word holds which engine, which build of its channel, and how far that
/// channel has got, and it travels through a register the caller reads as a
/// signed integer. A fence is a success, and a success with the top bit set
/// looks like an errno
fn fences(r: &mut Runner) {
    use crate::syscall::adapters::gpu::{decode_fence, encode_fence, COMPUTE_FENCE};

    // Every fence this interface can produce has to be a non-negative i64, for
    // every engine and every counter value
    let corners = [
        (0u32, 0u32), (1, 1), (0, u32::MAX), (u32::MAX, 0), (u32::MAX, u32::MAX),
        (0x3FFF_FFFF, u32::MAX), (0x4000_0000, 1),
    ];
    let mut worst = 0u64;
    let all_positive = corners.iter().all(|&(e, s)| {
        let plain = encode_fence(e, s);
        let compute = plain | COMPUTE_FENCE;
        worst = worst.max(compute);
        (plain as i64) >= 0 && (compute as i64) >= 0
    });
    r.check(
        "gpu: every fence is a non-negative i64, so none can read as an errno",
        all_positive,
        format!("largest {:#x} -> {}", worst, worst as i64),
    );

    // A fence survives the round trip unchanged
    let rt = corners.iter().all(|&(e, s)| {
        // The epoch is 30 bits, so that is what comes back
        let want = e & 0x3FFF_FFFF;
        decode_fence(encode_fence(e, s)) == (want, s)
            && decode_fence(encode_fence(e, s) | COMPUTE_FENCE) == (want, s)
    });
    r.check(
        "gpu: a fence decodes to the epoch and counter it was made from",
        rt,
        format!("{:?} -> {:?}", corners[4], decode_fence(encode_fence(u32::MAX, u32::MAX))),
    );

    // The engine tag must not leak into either field
    let tag_clean = decode_fence(COMPUTE_FENCE) == (0, 0)
        && encode_fence(0, 0) & COMPUTE_FENCE == 0;
    r.check(
        "gpu: the engine tag is not mistaken for an epoch or a counter",
        tag_clean,
        format!("tag {:#x} decodes to {:?}", COMPUTE_FENCE, decode_fence(COMPUTE_FENCE)),
    );

    // Two engines at the same counter are two different fences, or a client
    // waiting on one would be told the other's work finished
    let distinct = encode_fence(3, 7) != (encode_fence(3, 7) | COMPUTE_FENCE);
    r.check(
        "gpu: the same counter on two engines is two different fences",
        distinct,
        format!("{:#x} vs {:#x}", encode_fence(3, 7), encode_fence(3, 7) | COMPUTE_FENCE),
    );

    // A counter that wraps is still the fence value it was encoded from;
    // whether it has passed is the channel's signed comparison
    let wrap = decode_fence(encode_fence(9, u32::MAX)).1 == u32::MAX
        && decode_fence(encode_fence(9, 0)).1 == 0;
    r.check(
        "gpu: a counter at the wrap point encodes and decodes unchanged",
        wrap,
        format!("{:#x}", encode_fence(9, u32::MAX)),
    );
}

/// Where a virtual address lands in the four levels above the page table
fn level_indices(r: &mut Runner) {
    // Below the 512 MiB limit every index is zero
    let low = [0u64, 0x1000, 0x10_0000, OLD_LIMIT - 0x1000];
    let all_zero = low.iter().all(|&va| gmmu::level_steps(va) == [0, 0, 0, 0]);
    r.check(
        "gmmu: below 512 MiB every level index is still zero",
        all_zero,
        format!("{:?} -> {:?}", low, gmmu::level_steps(low[3])),
    );

    // The first address past that limit: one step in level 2 and nothing above
    // it
    let at = gmmu::level_steps(OLD_LIMIT);
    r.check(
        "gmmu: 512 MiB is level 2 index 1",
        at == [0, 0, 0, 1],
        format!("{:#x} -> {:?}", OLD_LIMIT, at),
    );

    // Level 2 is nine bits, so it wraps into level 3 after 512 of them
    let l2_span = OLD_LIMIT * 512;
    let at = gmmu::level_steps(l2_span);
    r.check(
        "gmmu: 256 GiB carries into level 3",
        at == [0, 0, 1, 0],
        format!("{:#x} -> {:?}", l2_span, at),
    );

    let l3_span = l2_span * 512;
    let at = gmmu::level_steps(l3_span);
    r.check(
        "gmmu: 128 TiB carries into level 4",
        at == [0, 1, 0, 0],
        format!("{:#x} -> {:?}", l3_span, at),
    );

    // Level 5 is one bit: the top half of everything six levels reach
    let l4_span = l3_span * 512;
    let at = gmmu::level_steps(l4_span);
    r.check(
        "gmmu: the top bit is level 5",
        at == [1, 0, 0, 0],
        format!("{:#x} -> {:?}", l4_span, at),
    );

    // Each index takes its own field and no other. A mask one bit too wide or
    // narrow shows up here
    let mixed = (1u64 << 56) | (0x1ff << 47) | (0x1ff << 38) | (0x1ff << 29);
    let at = gmmu::level_steps(mixed);
    r.check(
        "gmmu: the four index fields do not bleed into each other",
        at == [1, 0x1ff, 0x1ff, 0x1ff],
        format!("{:#x} -> {:?}", mixed, at),
    );

    // The page offset, the level-0 index and the level-1 index are below all
    // four of these and must not reach them
    let below = (0x1ffu64 << 12) | (0xff << 21);
    let at = gmmu::level_steps(below);
    r.check(
        "gmmu: levels 1 and 0 stay out of the levels above them",
        at == [0, 0, 0, 0],
        format!("{:#x} -> {:?}", below, at),
    );
}

/// What a range check accepts, and what it refuses and does not wrap
fn range_checks(r: &mut Runner) {
    r.check(
        "gmmu: an ordinary range is accepted",
        gmmu::check_range(0x20_0000, 0x10_0000).is_ok(),
        format!("2 MiB .. +1 MiB"),
    );

    // A case this has to allow
    r.check(
        "gmmu: a range above 512 MiB is accepted now",
        gmmu::check_range(OLD_LIMIT, 0x1000).is_ok(),
        format!("{:#x} .. +4 KiB", OLD_LIMIT),
    );

    // A range straddling the boundary is one the walk re-descends in the middle
    // of, and it is legal
    r.check(
        "gmmu: a range straddling 512 MiB is accepted",
        gmmu::check_range(OLD_LIMIT - 0x1000, 0x2000).is_ok(),
        format!("{:#x} .. +8 KiB", OLD_LIMIT - 0x1000),
    );

    // 57 bits is everything six levels address. One page past it has nowhere to
    // be indexed
    let top = 1u64 << 57;
    r.check(
        "gmmu: a range past what six levels reach is refused",
        gmmu::check_range(top, 0x1000).is_err(),
        format!("{:#x} -> {:?}", top, gmmu::check_range(top, 0x1000)),
    );
    r.check(
        "gmmu: the last page inside 57 bits is accepted",
        gmmu::check_range(top - 0x1000, 0x1000).is_ok(),
        format!("{:#x} .. +4 KiB", top - 0x1000),
    );

    // Wrapping is a different range, and one that would pass a naive
    // `va + bytes` comparison
    r.check(
        "gmmu: a range that wraps the address space is refused",
        gmmu::check_range(u64::MAX - 0x800, 0x2000).is_err(),
        format!("{:?}", gmmu::check_range(u64::MAX - 0x800, 0x2000)),
    );

    // Zero bytes: `bytes - 1` on zero is a panic in a debug build and a
    // whole-address-space range in a release one
    r.check(
        "gmmu: an empty range is refused rather than underflowing",
        gmmu::check_range(0x1000, 0).is_err(),
        format!("{:?}", gmmu::check_range(0x1000, 0)),
    );
}

/// Which bindings a pool being destroyed takes with it
fn overlaps(r: &mut Runner) {
    // Half-open at both ends. A pool ending where a mapping begins shares no
    // byte with it, and unbinding the neighbour would take a live translation
    // away
    r.check(
        "gmmu: a range touching end-to-start does not overlap",
        !gmmu::ranges_overlap(0x1000, 0x1000, 0x2000, 0x1000)
            && !gmmu::ranges_overlap(0x2000, 0x1000, 0x1000, 0x1000),
        format!("[0x1000,0x2000) vs [0x2000,0x3000)"),
    );

    r.check(
        "gmmu: one page of overlap counts",
        gmmu::ranges_overlap(0x1000, 0x2000, 0x2000, 0x1000),
        format!("[0x1000,0x3000) vs [0x2000,0x3000)"),
    );

    // The usual shape: a window is one page inside a much larger pool, and
    // destroying the pool takes that window with it
    r.check(
        "gmmu: a pool contains a window mapped inside it",
        gmmu::ranges_overlap(0x50_5000, 0x1000, 0x50_0000, 0x40_0000),
        format!("a 4 KiB window inside a 4 MiB pool"),
    );

    // The other way round: `unbind_range` is called with the pool as the second
    // pair and the binding as the first
    r.check(
        "gmmu: containment is symmetric",
        gmmu::ranges_overlap(0x50_0000, 0x40_0000, 0x50_5000, 0x1000),
        format!("a 4 MiB pool against a 4 KiB window"),
    );

    r.check(
        "gmmu: distant ranges do not overlap",
        !gmmu::ranges_overlap(0x1000, 0x1000, 0x100_0000, 0x1000),
        format!("[0x1000,0x2000) vs [0x1000000,0x1001000)"),
    );

    // An empty range is nowhere. Saturating arithmetic makes this the one case
    // where a length of zero could read as "everything"
    r.check(
        "gmmu: an empty range overlaps nothing",
        !gmmu::ranges_overlap(0x1000, 0, 0x1000, 0x1000),
        format!("zero-length binding"),
    );
}

/// GSP-RM's display step report, as the card posts it at every unload:
/// `NocatRecord::display_step`
fn nocat_records(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::rpc::nocat_record;

    // The fixed part of an `NV2080CtrlNocatJournalInsertRecord`, then the diag
    let record = |source: &str, err: u64, diag: &[u8]| -> alloc::vec::Vec<u8> {
        let mut p = alloc::vec![0u8; 180];
        p[16] = 2; // ENGINE
        p[24..24 + source.len()].copy_from_slice(source.as_bytes());
        p[96..104].copy_from_slice(&err.to_le_bytes());
        p[104..104 + 14].copy_from_slice(b"Display Engine");
        p[176..180].copy_from_slice(&(diag.len() as u32).to_le_bytes());
        p.extend_from_slice(diag);
        p
    };
    let step = |p: &[u8]| nocat_record(p).and_then(|n| n.display_step());

    // From the card: {328: {4: 0x28, 5: 0}}
    let card = [0xc2, 0x14, 0x04, 0x20, 0x28, 0x28, 0x00];
    let failed = [0xc2, 0x14, 0x04, 0x20, 0x28, 0x28, 0x65];
    r.check(
        "nocat: the card's display record is step 0x28 with NV_OK; with 0x65 it is NV_ERR_TIMEOUT",
        step(&record("Display Subsystem", 0x28, &card)) == Some((0x28, 0))
            && step(&record("Display Subsystem", 0x28, &failed)) == Some((0x28, 0x65)),
        format!("{:?}", step(&record("Display Subsystem", 0x28, &card))),
    );
    r.check(
        "nocat: not a step report - the other display reporter, a step that is not errorCode, \
         a cut buffer, a field it does not carry",
        step(&record("Display subsystem", 0x28, &card)).is_none()
            && step(&record("Display Subsystem", 0x73, &card)).is_none()
            && step(&record("Display Subsystem", 0x28, &card[..6])).is_none()
            && step(&record("Display Subsystem", 0x28, &[0xc2, 0x14, 0x04, 0x20, 0x28, 0x30, 0x00])).is_none(),
        alloc::string::String::new(),
    );
}

/// The display layer's names and requests (`drivers::gpu::kms`), decided with
/// no card
fn display_names(r: &mut Runner) {
    use crate::drivers::gpu::kms::{names, Connector, HeadRequest, HeadState, Output, Vrr};
    use super::kms::{connector_kind, dp_compatible};

    // This card's seven display ids, grouped as Linux shows them on this
    // machine: one HDMI port, three DP++ ports each with a TMDS face. The
    // physical indices are made up; the card's own come from GET_CONNECTOR_DATA
    // and are in the boot log's `DISP: connector` lines
    let face = |key, physical, t: u32| names::Face {
        key, physical, connector: connector_kind(t).unwrap(),
    };
    let card = [
        face(0x100, 0, 0x61), face(0x200, 1, 0x46), face(0x400, 1, 0x46),
        face(0x800, 2, 0x46), face(0x1000, 2, 0x46), face(0x2000, 3, 0x46),
        face(0x4000, 3, 0x46),
    ];
    let named = names::name(&card);
    let got: alloc::vec::Vec<(&str, alloc::vec::Vec<u32>)> =
        named.iter().map(|n| (n.name.as_str(), n.keys.clone())).collect();
    r.check(
        "kms: seven display ids are HDMI-A-1 and DP-1..3, the panel's 0x2000 on DP-3 with its TMDS face",
        got.len() == 4
            && got[0] == ("HDMI-A-1", alloc::vec![0x100])
            && got[1] == ("DP-1", alloc::vec![0x200, 0x400])
            && got[2] == ("DP-2", alloc::vec![0x800, 0x1000])
            && got[3] == ("DP-3", alloc::vec![0x2000, 0x4000]),
        format!("{:?}", got),
    );
    r.check(
        "kms: NVKMS's connector types - DP and its serializer are DP, mini HDMI is HDMI-A, \
         USB-C kept as Unknown, a virtual or unknown type dropped, HDMI refused on a DP output",
        connector_kind(0x49) == Some(Connector::DisplayPort)
            && connector_kind(0x63) == Some(Connector::HdmiA)
            && connector_kind(0x71) == Some(Connector::Unknown)
            && connector_kind(0x70).is_none()
            && connector_kind(0xFFFF_FFFF).is_none()
            && dp_compatible(0x71) && !dp_compatible(0x61),
        alloc::string::String::new(),
    );

    // A request built from what an output does changes nothing until a field
    // is set, and then that field alone
    let o = Output {
        name: "DP-3".into(),
        connector: Connector::DisplayPort,
        key: 0x2000,
        connected: true,
        primary: true,
        monitor: None,
        head: Some(HeadState {
            head: 0, size: (1920, 1080), refresh_centihz: 24000, mode: Some(0),
            asleep: false, colour: None, vrr: Vrr::Off,
        }),
    };
    let now = HeadRequest::current(&o);
    let mut want = now.clone();
    want.vrr = true;
    r.check(
        "kms: a request from the current state is lit, mode 0, awake, fixed rate; setting vrr moves only vrr",
        now.lit && now.mode == Some(0) && !now.asleep && !now.vrr && now.output == "DP-3"
            && want.vrr && want.mode == now.mode && want.asleep == now.asleep && want.lit,
        alloc::string::String::new(),
    );
}

/// The display layer's half of a check (`kms::check::plan`): what a request
/// changes, and what it may not, on a state made up here
fn display_plan(r: &mut Runner) {
    use crate::drivers::gpu::kms::check::plan;
    use crate::drivers::gpu::kms::{
        Connector, Depth, HeadColour, HeadRequest, HeadState, Monitor, Output, Request, State, Timing, Vrr,
    };

    let t = |w, h, clock_khz, h_total, v_total| Timing {
        clock_khz, h_display: w, h_total, v_display: h, v_total, ..Default::default()
    };
    // The panel: 240 and 200 Hz at 1080p, and a 720p mode of another size
    let panel = || Monitor {
        name: "HTMS270F240".into(),
        make: "VIE".into(),
        modes: alloc::vec![
            t(1920, 1080, 561_600, 2080, 1125), t(1920, 1080, 468_000, 2080, 1125),
            t(1280, 720, 74_250, 1650, 750),
        ],
        preferred: Some(0),
        refresh_range: Some((48, 240)),
        max_bpc: 10,
    };
    let dp3 = |vrr: Vrr, asleep: bool, connected: bool| Output {
        name: "DP-3".into(),
        connector: Connector::DisplayPort,
        key: 0x2000,
        connected,
        primary: true,
        monitor: Some(panel()),
        head: Some(HeadState {
            head: 0, size: (1920, 1080), refresh_centihz: 24000, mode: Some(0),
            asleep, colour: Some(HeadColour::DEFAULT), vrr,
        }),
    };
    let hdmi = || Output {
        name: "HDMI-A-1".into(),
        connector: Connector::HdmiA,
        key: 0x100,
        connected: true,
        primary: false,
        monitor: Some(Monitor {
            name: "TV".into(), make: "SAM".into(), modes: alloc::vec![t(1920, 1080, 148_500, 2200, 1125)],
            preferred: Some(0), refresh_range: None, max_bpc: 8,
        }),
        head: None,
    };
    let state = |o: Output| State { outputs: alloc::vec![o, hdmi()] };
    let ask = |st: &State, name: &str, f: &dyn Fn(&mut HeadRequest)| {
        let mut h = HeadRequest::current(st.output(name).unwrap());
        f(&mut h);
        Request::one(h, false)
    };

    // A mode of the same size under the compositor holds the frames; one of
    // another size, or one that is not in the list, is refused
    let st = state(dp3(Vrr::Off, false, true));
    let same = plan(&ask(&st, "dp-3", &|h| h.mode = Some(1)), &st, true);
    let other = plan(&ask(&st, "DP-3", &|h| h.mode = Some(2)), &st, true);
    let missing = plan(&ask(&st, "DP-3", &|h| h.mode = Some(9)), &st, false);
    r.check(
        "kms plan: 200 Hz and 720p under the compositor are both held rasters (K3); mode 9 is not in the list",
        same.as_ref().is_ok_and(|c| c.len() == 1 && c[0].raster && c[0].hold_frames
            && !c[0].vrr_off_before && !c[0].vrr_on_after && c[0].sleep.is_none())
            && other.as_ref().is_ok_and(|c| c.len() == 1 && c[0].raster && c[0].hold_frames)
            && missing.is_err(),
        format!("{:?} / {:?}", other.err(), missing.err()),
    );

    // Variable refresh comes down before a new raster and before sleep, goes
    // up after the raster, and is not touched by a colour
    let st = state(dp3(Vrr::On { timeout_us: 20833 }, false, true));
    let mode = plan(&ask(&st, "DP-3", &|h| h.mode = Some(1)), &st, false);
    let colour = plan(&ask(&st, "DP-3", &|h| h.colour = Some(HeadColour { depth: Depth::Bpc10, ..HeadColour::DEFAULT })), &st, false);
    let sleep = plan(&ask(&st, "DP-3", &|h| h.asleep = true), &st, false);
    let steps = |p: &Result<alloc::vec::Vec<crate::drivers::gpu::kms::Change>, alloc::string::String>|
        p.as_ref().ok().and_then(|c| c.first().map(|c| (c.vrr_off_before, c.vrr_on_after)));
    r.check(
        "kms plan: under VRR a mode is off-before and on-after, a colour neither, sleep off-before only",
        steps(&mode) == Some((true, true))
            && steps(&colour) == Some((false, false)) && colour.as_ref().is_ok_and(|c| c[0].recolour)
            && steps(&sleep) == Some((true, false)) && sleep.as_ref().is_ok_and(|c| c[0].sleep == Some(true)),
        format!("{:?} {:?} {:?}", steps(&mode), steps(&colour), steps(&sleep)),
    );

    // Sleep needs no monitor (the hot-plug pass sleeps an unplugged output),
    // waking does; no head is let go; refresh does not vary on a sleeping one
    let gone = state(dp3(Vrr::Off, false, false));
    let asleep_gone = state(dp3(Vrr::Off, true, false));
    let asleep = state(dp3(Vrr::Off, true, true));
    r.check(
        "kms plan: an unplugged output may sleep and not wake; no head is released; no VRR while asleep",
        plan(&ask(&gone, "DP-3", &|h| h.asleep = true), &gone, false).is_ok_and(|c| c.len() == 1)
            && plan(&ask(&asleep_gone, "DP-3", &|h| h.asleep = false), &asleep_gone, false).is_err()
            && plan(&ask(&st, "DP-3", &|h| h.lit = false), &st, false).is_err()
            && plan(&ask(&asleep, "DP-3", &|h| h.vrr = true), &asleep, false).is_err(),
        alloc::string::String::new(),
    );

    // Ten bits only where the EDID says ten; lighting under the compositor is
    // refused; two outputs in one request are two changes, one output twice is
    // a mistake; a request that changes nothing is no change at all
    let st = state(dp3(Vrr::Off, false, true));
    let ten = Some(HeadColour { depth: Depth::Bpc10, ..HeadColour::DEFAULT });
    let light_ten = plan(&ask(&st, "HDMI-A-1", &|h| { h.lit = true; h.colour = ten; }), &st, false);
    let light_busy = plan(&ask(&st, "HDMI-A-1", &|h| h.lit = true), &st, true);
    let mut both = ask(&st, "DP-3", &|h| h.colour = ten);
    both.heads.push(ask(&st, "HDMI-A-1", &|h| h.lit = true).heads.remove(0));
    let pair = plan(&both, &st, false);
    let mut twice = ask(&st, "DP-3", &|h| h.mode = Some(1));
    twice.heads.push(twice.heads[0].clone());
    r.check(
        "kms plan: 10 bpc refused on an 8-bit TV, lighting refused under the compositor, two outputs \
         are a colour and a light, one output twice is refused, an unchanged request is empty",
        light_ten.is_err() && light_busy.is_err()
            && pair.as_ref().is_ok_and(|c| c.len() == 2 && c[0].recolour && !c[0].light && c[1].light)
            && plan(&twice, &st, false).is_err()
            && plan(&ask(&st, "DP-3", &|_| {}), &st, true).is_ok_and(|c| c.is_empty()),
        format!("{:?} {:?}", light_ten.err(), pair.err()),
    );
}

/// The NVIDIA driver's update for a request (`blackwell/apply.rs`), built into
/// a plain buffer: every head's methods, and one UPDATE
fn display_batch(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::blackwell::apply::{emit_into, Emit, Raster};
    use crate::drivers::gpu::nvidia::gsp_common::disp;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{self, HeadColour, Mode, OlutRing};

    // Method, data - the stream read back the way the channel reads it
    let decode = |w: &[u32]| {
        let mut out = alloc::vec::Vec::new();
        let mut i = 0;
        while i < w.len() {
            let n = ((w[i] >> 18) & 0x3ff) as usize;
            out.push(((w[i] >> 2 & 0x3fff) << 2, w[i + 1..i + 1 + n].to_vec()));
            i += 1 + n;
        }
        out
    };
    let mode = Mode::from_timing(&crate::drivers::gpu::kms::Timing {
        clock_khz: 468_000, h_display: 1920, h_sync_start: 1968, h_sync_end: 2000, h_total: 2080,
        v_display: 1080, v_sync_start: 1083, v_sync_end: 1088, v_total: 1125, ..Default::default()
    });
    let ring = OlutRing::new(0x0100_0000);
    let gamma = HeadColour {
        curve: modeset::Curve { gamma_milli: 2200, ..HeadColour::DEFAULT.curve },
        ..HeadColour::DEFAULT
    };
    let head = |head: u32, raster: bool, recolour: bool, sleep: Option<bool>| Emit {
        head,
        display_id: 0x2000 >> head,
        sor: 1 + head,
        protocol: modeset::SOR_PROTOCOL_DP_A,
        mode,
        colour: if recolour { gamma } else { HeadColour::DEFAULT },
        olut: Some(if recolour { ring.next().live() } else { ring.live() }),
        recolour,
        asleep: sleep.unwrap_or(false),
        feed: sleep.is_some(),
        raster: raster.then(|| Raster {
            width: 1920, height: 1080, phywins: [1, 2], tiles: None,
            keep: alloc::vec::Vec::new(), armed_tiles: [1, 0, 0, 0],
        }),
        cursor: Some((0x00b0_0000, 64)),
    };
    let updates = |s: &[(u32, alloc::vec::Vec<u32>)]| s.iter().filter(|(m, _)| *m == disp::M_UPDATE).count();

    // A new mode, a new curve and sleep on one head: one update, last, with the
    // window interlocked; the head pointed at the next LUT; the SOR let go
    let mut words = alloc::vec![0u32; 1024];
    let n = emit_into(&mut words, &[head(0, true, true, Some(true))], 1 << 0);
    let s = n.map(|n| decode(&words[..n])).unwrap_or_default();
    let olut_lo = (ring.next().live() as u32 & !0xf) | (modeset::SURFACE_TARGET_VRAM << 2) | 1;
    // `head_set_mode` switches the cursor plane off; the last word on it has
    // to be the image again, enabled
    let cursor_lo = s.iter().rev()
        .find(|(m, _)| *m == modeset::M_HEAD_SET_SURFACE_ADDRESS_LO_CURSOR)
        .and_then(|(_, d)| d.first().copied());
    r.check(
        "kms batch: mode, gamma and sleep on one head are one UPDATE, interlocked with its window, \
         the head on the next OLUT, the SOR detached, the cursor plane armed again",
        updates(&s) == 1
            && cursor_lo == Some(0x00b0_0000 | (modeset::SURFACE_TARGET_VRAM << 2) | 1)
            && s.last().is_some_and(|(m, _)| *m == disp::M_UPDATE)
            && s.iter().any(|(m, d)| *m == disp::M_SET_INTERLOCK_FLAGS && d.get(1) == Some(&1))
            && s.iter().any(|(m, d)| *m == modeset::M_HEAD_SET_SURFACE_ADDRESS_HI_OLUT && d.get(1) == Some(&olut_lo))
            && s.iter().any(|(m, d)| *m == modeset::M_SOR_SET_CONTROL + modeset::SOR_STRIDE && d == &[0u32])
            && s.iter().any(|(m, d)| *m == modeset::M_HEAD_SET_DISPLAY_ID && d == &[0u32]),
        format!("{} words, {} updates", n.unwrap_or(0), updates(&s)),
    );

    // Every head the class has but the chip does not drive gives its power-on
    // tile back (615's `EvoInitWindowMappingCA`): HEAD_SET_TILE_MASK at 0x2060
    // + head * 0x800
    let n = emit_into(&mut words, &[head(0, true, false, None)], 1 << 0);
    let s = n.map(|n| decode(&words[..n])).unwrap_or_default();
    let cleared: alloc::vec::Vec<u32> = (4..8u32)
        .filter(|h| s.iter().any(|(m, d)| *m == 0x2060 + h * 0x800 && d == &[0u32]))
        .collect();
    r.check(
        "kms batch: a new raster clears the tile mask of heads 4-7 as well, not only the chip's four",
        cleared == [4, 5, 6, 7] && s.iter().any(|(m, d)| *m == 0x2060 && d == &[1u32]),
        format!("cleared {:?}", cleared),
    );

    // A colour alone: the LUT switches inside the matrix's off/on window
    let n = emit_into(&mut words, &[head(0, false, true, None)], 0);
    let s = n.map(|n| decode(&words[..n])).unwrap_or_default();
    let at = |m: u32, v: Option<u32>| s.iter().position(|(x, d)| *x == m && v.is_none_or(|v| d.first() == Some(&v)));
    let (off, lut, on) = (
        at(modeset::M_HEAD_SET_OCSC0CONTROL, Some(modeset::OCSC0_DISABLE)),
        at(modeset::M_HEAD_SET_SURFACE_ADDRESS_HI_OLUT, None),
        s.iter().rposition(|(x, d)| *x == modeset::M_HEAD_SET_OCSC0CONTROL && d.first() == Some(&modeset::OCSC0_ENABLE)),
    );
    r.check(
        "kms batch: a colour alone is matrix off, the LUT address, matrix on, then one UPDATE with no window",
        matches!((off, lut, on), (Some(a), Some(b), Some(c)) if a < b && b < c)
            && updates(&s) == 1
            && s.iter().any(|(m, d)| *m == disp::M_SET_INTERLOCK_FLAGS && d.get(1) == Some(&0))
            && !s.iter().any(|(m, _)| *m == modeset::M_SOR_SET_CONTROL + modeset::SOR_STRIDE),
        format!("{:?} {:?} {:?}", off, lut, on),
    );

    // Two heads, a colour on one and waking the other, still one update; the
    // ring of LUTs goes round in threes
    let n = emit_into(&mut words, &[head(0, false, true, None), head(1, false, false, Some(false))], 0);
    let s = n.map(|n| decode(&words[..n])).unwrap_or_default();
    let ring3 = ring.next().next().next();
    r.check(
        "kms batch: two heads are one UPDATE, the woken one's SOR set to its head; three LUTs come back round",
        updates(&s) == 1
            && s.iter().any(|(m, d)| *m == modeset::M_SOR_SET_CONTROL + 2 * modeset::SOR_STRIDE && d.first().is_some_and(|v| *v != 0))
            && s.iter().any(|(m, d)| *m == modeset::M_HEAD_SET_DISPLAY_ID + modeset::HEAD_STRIDE && d == &[0x1000u32])
            && ring3 == ring && ring.next().live() == ring.base + modeset::OLUT_STRIDE
            && modeset::OLUT_STRIDE as usize >= modeset::OLUT_BYTES,
        format!("{} updates", updates(&s)),
    );
}

/// A program's video memory (3.7): where `Placement::Below` puts a block, and
/// what `gpu_alloc` says on a machine with nothing to give
fn program_vram(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::vram::{find, Placement, Range};
    use crate::drivers::gpu::nvidia::user_vram::{self, Refused};

    const MIB: u64 = 1 << 20;
    // A window of 256 MiB over a free list that has the kernel's blocks low
    // down: 4..64 MiB free, then 96..512 MiB free
    let free = [Range { base: 4 * MIB, size: 60 * MIB }, Range { base: 96 * MIB, size: 416 * MIB }];
    let top = find(&free, 8 * MIB, 0x1000, Placement::Below(256 * MIB));
    let low = find(&free, 8 * MIB, 0x1000, Placement::Low);
    // A span that crosses the limit is cut at it, and one wholly above it is
    // not used at all
    let above = [Range { base: 300 * MIB, size: 100 * MIB }];
    let none = find(&above, MIB, 0x1000, Placement::Below(256 * MIB));
    let tight = find(&free, 160 * MIB, 0x1000, Placement::Below(256 * MIB));
    let aligned = find(&free, 0x1800, 0x10000, Placement::Below(256 * MIB));
    r.check(
        "vram: Below puts a block at the top of the window, Low at the bottom; a span above the \
         window is not used; 160 MiB fits under it exactly; the alignment holds",
        top == Some(248 * MIB) && low == Some(4 * MIB) && none.is_none()
            && tight == Some(96 * MIB) && aligned.is_some_and(|a| a % 0x10000 == 0 && a + 0x1800 <= 256 * MIB),
        alloc::format!("top {:?} low {:?} above {:?} tight {:?} aligned {:?}", top, low, none, tight, aligned),
    );

    // A window's buffer part way into a program's block names that block
    // (`dev/gpu-window-buffers.md`): the first byte and the last are in it,
    // the byte after it is in the next one or in none
    use crate::drivers::gpu::nvidia::gsp_common::vram::{containing, Block};
    let used = [
        Block { base: 64 * MIB, size: 2 * MIB, owner: "program video memory" },
        Block { base: 8 * MIB, size: 4 * MIB, owner: "program video memory" },
        Block { base: 66 * MIB, size: MIB, owner: "program video memory" },
    ];
    let at = |addr: u64| containing(&used, addr);
    r.check(
        "vram: the block holding an address is found from any byte of it, unsorted; the byte past          one block is the next block's, and a gap is nobody's",
        at(8 * MIB) == Some((8 * MIB, 4 * MIB)) && at(12 * MIB - 1) == Some((8 * MIB, 4 * MIB))
            && at(64 * MIB + 921_600) == Some((64 * MIB, 2 * MIB))
            && at(66 * MIB) == Some((66 * MIB, MIB))
            && at(12 * MIB).is_none() && at(67 * MIB).is_none() && at(0).is_none(),
        alloc::format!("{:?} {:?} {:?} {:?} {:?}", at(8 * MIB), at(12 * MIB - 1),
                       at(64 * MIB + 921_600), at(66 * MIB), at(12 * MIB)),
    );

    // No card here: refused, by name, and nothing is counted as held
    let got = user_vram::alloc(4096);
    r.check(
        "vram: a program asking for video memory on a machine with no card is told so, and holds nothing",
        matches!(got, Err(Refused::NoCard)) && user_vram::held().0 == 0
            || crate::drivers::gpu::nvidia::device().is_some(),
        alloc::format!("{:?}", got.map(|(v, _, n)| (v, n))),
    );
}

/// The bus, as OpenRM's OBJCL knows it (`dev/gsp-foundation.md`, 1.17).
///
/// What GSP-RM is handed: a property's bit is its line in `CL_SYNC_PDB`
/// (`chipset.c:788-824`, line minus 788), and a chipset's number is its place
/// in the `nvdevid.h` enum. Both are ABI, so a few anchors are checked against
/// the source's own line numbers, and the dev box's chipset (FCH `1022:790e`,
/// root port `1022:1483`) against OpenRM's tables
fn chipset_tables(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::chipset::{self, table, Chipset, Device, Prop, PROP_NAMES};

    let anchors = [
        (Prop::PcieConfigAccessible, 0usize, "PCIE_CONFIG_ACCESSIBLE"),
        (Prop::IsChipsetInAspmPorList, 7, "IS_CHIPSET_IN_ASPM_POR_LIST"),
        (Prop::AspmL0sChipsetDisabled, 8, "ASPM_L0S_CHIPSET_DISABLED"),
        (Prop::RelaxedOrderingNotCapable, 18, "RELAXED_ORDERING_NOT_CAPABLE"),
        (Prop::RootportNeedsNosnoopWar, 21, "ROOTPORT_NEEDS_NOSNOOP_WAR"),
        (Prop::PcieConfigSkipMcfgRead, 23, "PCIE_CONFIG_SKIP_MCFG_READ"),
        (Prop::UpstreamLtrSupported, 27, "UPSTREAM_LTR_SUPPORTED"),
        (Prop::IsChipsetIoCoherent, 32, "IS_CHIPSET_IO_COHERENT"),
        (Prop::Bug3562968WarAllowPcieAtomics, 36, "BUG_3562968_WAR_ALLOW_PCIE_ATOMICS"),
        (Prop::WarAmd5107271, 37, "WAR_AMD_5107271"),
    ];
    let wrong: alloc::vec::Vec<_> = anchors.iter()
        .filter(|(p, bit, name)| *p as usize != *bit || PROP_NAMES.get(*bit) != Some(name))
        .map(|(_, bit, name)| (*bit, *name))
        .collect();
    r.check(
        "chipset: each property is the bit NVGMC_SI_CL_FLAGS gives it, 38 of them, named as in OpenRM",
        wrong.is_empty() && PROP_NAMES.len() == 38,
        alloc::format!("{:?}, {} names", wrong, PROP_NAMES.len()),
    );

    // The dev box: an X570-family FCH, recognised by the LPC bridge, and a
    // Matisse root port that takes L0s away
    let fch = table::row(0x1022, 0x790e);
    let mut c = Chipset::default();
    if let Some(setup) = fch.and_then(|row| row.setup) {
        setup(&mut c);
    }
    let after_chipset = c.props;
    if let Some(setup) = table::port_row(table::UPSTREAM_PORTS, 0x1022, 0x1483).and_then(|row| row.setup) {
        setup(&mut c);
    }
    let por = 1u64 << Prop::IsChipsetInAspmPorList as u8;
    let l0s = 1u64 << Prop::AspmL0sChipsetDisabled as u8;
    let war = 1u64 << Prop::WarAmd5107271 as u8;
    r.check(
        "chipset: 1022:790e is CS_AMD_X370 (0x1043 in 615) with the bug 5107271 WAR and nothing disabled, \
         and root port 1022:1483 disables L0s - as AMD_X370_setupFunc and AMD_RP1483_setupFunc do",
        fch.map(|row| row.chipset) == Some(0x1043) && after_chipset == por | war && c.props == por | l0s | war
            && table::port_row(table::ROOT_PORTS, 0x1022, 0x1483).is_none(),
        alloc::format!("chipset {:?}, props {:#x} then {:#x}", fch.map(|row| row.chipset), after_chipset, c.props),
    );
    r.check(
        "chipset: the Hygon FCH with the same device id is its own row (CS_HYGON_C86, no setup), \
         and an id OpenRM has never heard of is no row at all",
        matches!(table::row(0x1d94, 0x790e), Some(row) if row.chipset == 0x105b && row.setup.is_none())
            && table::row(0x1234, 0x5678).is_none(),
        alloc::string::String::new(),
    );

    // What GSP_INIT's system info carries
    let fhb = Device { bus: 0, dev: 0, func: 0, vendor: 0x1022, device: 0x1480, subvendor: 0x1043, subdevice: 0x87c0, revision: 0 };
    let id = Device { bus: 0, dev: 0x14, func: 3, vendor: 0x1022, device: 0x790e, subvendor: 0x1043, subdevice: 0x87c0, revision: 0x51 };
    let c = Chipset {
        props: por | l0s | war, chipset: 0x1043, name: "X370", fhb: Some(fhb), chipset_id: Some(id),
        upstream: Some(fhb), atomics_cpl_mask: 0x3f, ..Default::default()
    };
    let mut info = crate::drivers::gpu::nvidia::gsp_common::sysinfo::SystemInfo::default();
    chipset::fill(&c, &mut info);
    let mut none = info.clone();
    chipset::fill(&Chipset::default(), &mut none);
    r.check(
        "chipset: the system info gets the flags, the chipset, both ids and the upstream port; \
         a chipset id nobody found is PCI_INVALID, as OpenRM initialises it",
        info.cl_flags == por | l0s | war && info.chipset == 0x1043
            && info.fhb.device_id == 0x1480 && info.chipset_id.device_id == 0x790e
            && info.chipset_id.revision_id == 0x51 && info.upstream_address_valid
            && info.pcie_atomics_cpl_device_cap_mask == 0x3f
            && none.chipset_id.vendor_id == 0xFFFF && none.fhb.vendor_id == 0
            && !none.upstream_address_valid,
        alloc::format!("flags {:#x}, chipset {:#x}", info.cl_flags, info.chipset),
    );
}

fn page_sizes(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gmmu::{self, Aperture, Pages};
    use crate::drivers::gpu::nvidia::gsp_common::vram::{self, find, Placement, Range};

    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    let p = |huge, big, small| Pages { huge, big, small };
    let plan = |va, phys, bytes, aper| gmmu::page_plan(va, phys, bytes, aper);
    let cases = [
        // A full-screen surface padded to 8 MiB, both ends on 2 MiB
        (plan(0x1200_0000, 4 * MIB, 8 * MIB, Aperture::Vram), p(4, 0, 0)),
        // The same surface not padded: 2 MiB pages, then 64 KiB, then 4 KiB
        (plan(0x1200_0000, 4 * MIB, 8100 * KIB, Aperture::Vram), p(3, 30, 9)),
        // The address on 64 KiB only: 64 KiB pages, never a 2 MiB one
        (plan(0x1201_0000, 4 * MIB, 2 * MIB, Aperture::Vram), p(0, 32, 0)),
        // The memory on 64 KiB only, the address on 2 MiB: the same
        (plan(0x1200_0000, 0x31_0000, 4 * MIB, Aperture::Vram), p(0, 64, 0)),
        // The address on 4 KiB: 4 KiB pages
        (plan(0x1000_1000, 4 * MIB, MIB, Aperture::Vram), p(0, 0, 256)),
        // System memory: 4 KiB pages whatever the alignment
        (plan(0x1200_0000, 4 * MIB, 8 * MIB, Aperture::Sysmem), p(0, 0, 2048)),
    ];
    let wrong: alloc::vec::Vec<_> = cases.iter().enumerate().filter(|(_, (got, want))| got != want)
        .map(|(i, (got, want))| (i, *got, *want)).collect();
    r.check(
        "pages: aligned video memory takes 2 MiB pages, a tail 64 KiB then 4 KiB, an address or memory \
         on 64 KiB only takes 64 KiB, system memory 4 KiB",
        wrong.is_empty(),
        alloc::format!("{:?}", wrong),
    );

    // The allocator's half: `memmgrDeterminePageSize`, and the padding with it
    let policy = [
        vram::page_for(64 * KIB - 1) == 0x1000,
        vram::page_for(64 * KIB) == 64 * KIB,
        vram::page_for(2 * MIB - 1) == 64 * KIB && vram::padded(2 * MIB - 1) == 2 * MIB,
        vram::page_for(2 * MIB) == 2 * MIB,
        vram::padded(8100 * KIB) == 8 * MIB,
        vram::padded(36 * KIB) == 36 * KIB,
        // gpudemo's pool, as a client's allocation: one big page
        vram::padded((9 * KIB).max(64 * KIB)) == 64 * KIB,
    ];
    // A full-screen block over a free list that starts at 0x310000: on the next
    // 2 MiB boundary
    let free = [Range { base: 0x31_0000, size: 64 * MIB }];
    let at = find(&free, vram::padded(8100 * KIB), vram::page_for(8100 * KIB), Placement::Low);
    r.check(
        "pages: the allocator gives 2 MiB blocks from 2 MiB, 64 KiB from 64 KiB, 4 KiB below, padded \
         to the page - and puts a surface on a 2 MiB boundary",
        policy.iter().all(|ok| *ok) && at == Some(4 * MIB),
        alloc::format!("{:?} at {:?}", policy, at),
    );

    // The entries, field by field. A PDE: aperture 2:1, PCF 5:3, the address in
    // 51:12 on the small half and in 51:8 (shift 8) on the big one, one number
    // for an address on 4 KiB. A PTE: valid 0, aperture 2:1, PCF 7:3, kind
    // 11:8, the address in 39:12 for video memory and 51:12 for system memory
    let field = |v: u64, hi: u32, lo: u32| (v >> lo) & ((1u64 << (hi - lo + 1)) - 1);
    let table = 0x1_2345_6000u64;
    let sys_pde = gmmu::pde_bits(table, true);
    let vid_pde = gmmu::pde_bits(table, false);
    let vid_pte = gmmu::pte_bits(0x3_4560_0000, Aperture::Vram, false);
    let sys_pte = gmmu::pte_bits(0x8_7654_3000, Aperture::Sysmem, true);
    let encoded = [
        field(sys_pde, 0, 0) == 0 && field(sys_pde, 2, 1) == 2 && field(sys_pde, 5, 3) == 1,
        field(vid_pde, 2, 1) == 1 && field(vid_pde, 5, 3) == 2,
        field(sys_pde, 51, 12) == table >> 12 && field(sys_pde, 51, 8) == table >> 8,
        gmmu::big_pde_addr(sys_pde) == Some(table) && gmmu::big_pde_addr(vid_pde) == Some(table),
        field(vid_pte, 0, 0) == 1 && field(vid_pte, 2, 1) == 0 && field(vid_pte, 7, 3) == 0x10
            && field(vid_pte, 11, 8) == 0 && field(vid_pte, 39, 12) == 0x3_4560_0000 >> 12,
        field(sys_pte, 2, 1) == 2 && field(sys_pte, 7, 3) == 0x13
            && field(sys_pte, 51, 12) == 0x8_7654_3000 >> 12,
        // A level-1 entry holding a 2 MiB page is not a table, and nor is an
        // empty one
        gmmu::big_pde_addr(vid_pte).is_none() && gmmu::big_pde_addr(0).is_none(),
    ];
    r.check(
        "pages: PDE and PTE bits are where OpenRM's dev_mmu.h puts them, and a 2 MiB PTE is never \
         read back as a table",
        encoded.iter().all(|ok| *ok),
        alloc::format!("{:?} pde {:#x}/{:#x} pte {:#x}/{:#x}", encoded, sys_pde, vid_pde, vid_pte, sys_pte),
    );
}

fn crash_reports(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::kgsp::crashcat::extract;

    // signature | format 1 | 8-byte units | (payload/8 - 1) | type
    let hdr = |words: u64, kind: u64| 0xdead | 1 << 16 | (words - 1) << 22 | kind << 32;
    let report = |implementer: u64| -> alloc::vec::Vec<u64> {
        alloc::vec![
            hdr(7, 0),
            implementer,
            // reporter: partition 0, ucode 1, S-mode, no task
            1 << 8 | 2 << 16 | 0xff << 24,
            // v3.1, CL 12345, at 100 s
            100 << 32 | 3 << 28 | 1 << 24 | 12345,
            // source: task 5, U-mode
            3 << 16 | 5 << 24,
            // panic, contained to the hart, "condition failed"
            2 | 4 << 4 | 0x0b << 32,
            0x1234,
            0x5678,
        ]
    };
    let bytes = |words: &[u64]| -> alloc::vec::Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    };

    let mut words = report(0x4C49_424F_5333_2E31);
    // CSR state, S-mode: status ie ip epc tval cause scratch
    words.push(hdr(7, 1) | 2 << 40);
    words.extend_from_slice(&[1, 2, 3, 0xe0c0, 5, 0xd, 7]);
    // A PC trace of three
    words.push(hdr(3, 3) | 1 << 43);
    words.extend_from_slice(&[0x100, 0x104, 0x108]);
    let whole = words.len() * 8;
    // and the next report, which is the next call's
    words.extend(report(0));
    let (got, taken) = extract(&bytes(&words));
    let lines = got.as_ref().map(|g| g.lines()).unwrap_or_default();
    r.check(
        "crashcat: a report and its packets, up to the next report",
        got.is_some() && taken == whole,
        format!("{} of {} bytes", taken, words.len() * 8),
    );
    r.check(
        "crashcat: LIBOS3 containment HART impacts GSP-RM",
        got.as_ref().is_some_and(|g| g.impacts_gsp_rm()),
        format!("containment {:?}", got.as_ref().map(|g| g.containment())),
    );
    r.check(
        "crashcat: the source line reads as OpenRM prints it",
        lines.first().is_some_and(|l| l == "GSP task panic: condition failed (11) @ pc:0x1234, aux:0x5678, partition:0#0, task:5"),
        format!("{:?}", lines.first()),
    );
    r.check(
        "crashcat: the reporter line, and the CSR and trace packets",
        lines.get(1).is_some_and(|l| l.trim() == "Reported by libos partition:0#1 kernel v3.1 [12345] @ ts:100")
            && lines.iter().any(|l| l.trim() == "sepc:0x000000000000e0c0  stval:0x0000000000000005  scause:0x000000000000000d")
            && lines.iter().any(|l| l.trim() == "0x0000000000000100  0x0000000000000104  0x0000000000000108"),
        format!("{:?}", lines),
    );

    // 615's libos watchdog: the same containment, cause type 3
    let mut dog = report(0x4C49_424F_5333_2E31);
    dog[5] = 3 | 4 << 4;
    let (dog, _) = extract(&bytes(&dog));
    r.check(
        "crashcat: a libos watchdog report impacts GSP-RM but is not fatal, and reads as a watchdog timeout",
        dog.as_ref().is_some_and(|g| g.impacts_gsp_rm() && g.is_watchdog() && !g.is_fatal())
            && got.as_ref().is_some_and(|g| g.is_fatal())
            && dog.as_ref().and_then(|g| g.lines().first().cloned()).as_deref()
                == Some("GSP task watchdog timeout @ pc:0x1234, partition:0#0, task:5"),
        format!("{:?}", dog.as_ref().map(|g| g.lines().first().cloned())),
    );

    let (generic, _) = extract(&bytes(&report(0)));
    r.check(
        "crashcat: another implementer has no containment, so is logged, not fatal",
        generic.as_ref().is_some_and(|g| !g.impacts_gsp_rm() && g.containment() == 0),
        format!("{:?}", generic.as_ref().map(|g| g.containment())),
    );

    let full = bytes(&report(0x4C49_424F_5333_2E31));
    let (short, taken) = extract(&full[..40]);
    let mut bad = full.clone();
    bad.extend_from_slice(&0x1234_5678u64.to_le_bytes());
    bad.extend_from_slice(&[0; 16]);
    let (after_bad, bad_taken) = extract(&bad);
    r.check(
        "crashcat: a short report is not one; a bad packet after a report takes the rest",
        short.is_none() && taken == 0 && after_bad.is_some() && bad_taken == bad.len(),
        format!("short {} taken {}, bad {} of {}", short.is_some(), taken, bad_taken, bad.len()),
    );
}

/// The capability block as `EvoGetCapabilitiesCA` reads it (`clca73.h`), built
/// by hand: four heads, four SORs, eight windows, eight tiles of which 0-3
/// carry a scaler and a vertical filter
fn display_caps(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::dispcaps::{Caps, TILE_TYPE_0, TILE_TYPE_1};

    let block = |off: u32| -> u32 {
        match off {
            0x0 => 0x0f0f,              // heads 0-3, SORs 0-3
            0x4 => 0xff,                // windows 0-7
            0x20 => 0xff | 0xff << 8,   // tiles 0-7, all multi-tile
            0x28 => 0xff,               // phywins 0-7 multi-tile
            0x144 => 1 << 11,           // SOR 0 dual TMDS
            0x5e8..=0x5f4 => 0x85,      // pclk max 1330 MHz, every head
            0x608..=0x614 => 0x3c << 16, // TMDS max 600 MHz, every SOR
            o if (0x680..0x680 + 4 * 32).contains(&o) && (o - 0x680) % 32 == 0 => 1 << 18 | 1 << 23,
            _ => 0,
        }
    };
    let c = Caps::parse(&block);
    r.check(
        "caps: four heads, eight windows, eight tiles, as many phywins as windows",
        c.num_heads() == 4 && c.num_windows() == 8 && c.num_tiles() == 8 && c.num_phywins() == 8,
        format!("{} {} {} {}", c.num_heads(), c.num_windows(), c.num_tiles(), c.num_phywins()),
    );
    r.check(
        "caps: tiles with scaler and vfilter are TYPE_0, the rest TYPE_1",
        c.tile_type[..4].iter().all(|&t| t == TILE_TYPE_0)
            && c.tile_type[4..].iter().all(|&t| t == TILE_TYPE_1),
        format!("{:?}", c.tile_type),
    );
    r.check(
        "caps: clock ceilings are the field times 10 MHz",
        c.pclk_max_khz[0] == 1_330_000 && c.sor_tmds_max_khz[3] == 600_000 && c.sor_dual_tmds == 1,
        format!("pclk {} tmds {} dual {:#x}", c.pclk_max_khz[0], c.sor_tmds_max_khz[3], c.sor_dual_tmds),
    );
    let ones = Caps::parse(&|_| u32::MAX);
    let zeroes = Caps::parse(&|_| 0);
    r.check(
        "caps: a block of all ones or all zeroes is not believed",
        c.plausible() && !ones.plausible() && !zeroes.plausible(),
        format!("good {} ones {} zeroes {}", c.plausible(), ones.plausible(), zeroes.plausible()),
    );
}

/// `NVC372_CTRL_IS_MODE_POSSIBLE_PARAMS` as `offsetof` printed it from
/// `ctrlc372chnc.h`, and the fields NVKMS fills for one unscaled head
fn imp_params(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset::imp::*;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::Mode;
    use crate::drivers::gpu::nvidia::le;

    r.check(
        "imp: the arrays sit end to end - 8 heads of 92, 32 windows of 36, then options",
        OFF_HEAD + MAX_HEADS * HEAD_SIZE == OFF_WINDOW
            && OFF_WINDOW + MAX_WINDOWS * WINDOW_SIZE == OFF_OPTIONS
            && OFF_TILING_ASSIGNMENTS + MAX_TILES == OFF_TILE_LIST
            && OFF_TILE_LIST + 2 * MAX_TILES == OFF_WORST_CASE_DOMAIN
            && PARAMS_SIZE == 2048,
        format!("window {} options {}", OFF_HEAD + MAX_HEADS * HEAD_SIZE, OFF_WINDOW + MAX_WINDOWS * WINDOW_SIZE),
    );

    // CEA-861 1920x1080 @ 60: 148.5 MHz, 2200 x 1125, sync at 2008/1084
    let m = Mode {
        clock_khz: 148_500, h_display: 1920, h_sync_start: 2008, h_sync_end: 2052, h_total: 2200,
        v_display: 1080, v_sync_start: 1084, v_sync_end: 1089, v_total: 1125,
        ..Mode::default()
    };
    let idle = min_frame_idle(&m);
    r.check(
        "imp: min frame idle is vsync+back porch+1 lines leading, the front porch trailing",
        idle == Some((41, 4)),
        format!("{:?}", idle),
    );
    let heads = [ImpHead { head: 3, mode: m, tiles: 0 }];
    let p = params(&heads, false).unwrap_or_default();
    let h = OFF_HEAD;
    let w = OFF_WINDOW;
    r.check(
        "imp: head 3 at 148.5 MHz, raster 2200x1125, blank start 2111/1120, end 191/40",
        p.len() == PARAMS_SIZE && p[OFF_NUM_HEADS] == 1 && p[OFF_NUM_WINDOWS] == 1
            && p[h + H_HEAD_INDEX] == 3
            && le::u32_or0(&p, h + H_MAX_PIXEL_CLK) == 148_500
            && le::u32_or0(&p, h + H_RASTER_SIZE) == 2200 && le::u32_or0(&p, h + H_RASTER_SIZE + 4) == 1125
            && le::u32_or0(&p, h + H_RASTER_BLANK_START) == 2111
            && le::u32_or0(&p, h + H_RASTER_BLANK_START + 4) == 1120
            && le::u32_or0(&p, h + H_RASTER_BLANK_END) == 191
            && le::u32_or0(&p, h + H_RASTER_BLANK_END + 4) == 40
            && p[h + H_LUT] == 2 && p[h + H_CURSOR_SIZE_32P] == 8,
        format!("blank start {}/{} end {}/{}",
            le::u32_or0(&p, h + H_RASTER_BLANK_START), le::u32_or0(&p, h + H_RASTER_BLANK_START + 4),
            le::u32_or0(&p, h + H_RASTER_BLANK_END), le::u32_or0(&p, h + H_RASTER_BLANK_END + 4)),
    );
    r.check(
        "imp: the head's main window is 2*head, RGB 1/2/4 bpp, 1942 pixels a line at 1920",
        le::u32_or0(&p, w + W_WINDOW_INDEX) == 6 && le::u32_or0(&p, w + W_OWNING_HEAD) == 3
            && le::u32_or0(&p, w + W_FORMAT_USAGE_BOUND) == 7
            && le::u32_or0(&p, w + W_MAX_PIXELS_FETCHED) == 1942,
        format!("window {} pixels {}", le::u32_or0(&p, w + W_WINDOW_INDEX), le::u32_or0(&p, w + W_MAX_PIXELS_FETCHED)),
    );

    // A reply: possible, one assignment of one tile to IMP head 0, 594 MHz,
    // limited by "dispclk"
    let mut reply = p.clone();
    reply[OFF_IS_POSSIBLE] = 1;
    le::put_u32(&mut reply, OFF_DISP_CLK, 594_000);
    le::put_u32(&mut reply, OFF_NUM_TILING_ASSIGNMENTS, 1);
    reply[OFF_TILING_ASSIGNMENTS] = 1;
    reply[OFF_TILE_LIST] = 0;
    reply[OFF_WORST_CASE_DOMAIN..OFF_WORST_CASE_DOMAIN + 7].copy_from_slice(b"dispclk");
    let a = parse(&reply, &heads);
    let mut none = reply.clone();
    le::put_u32(&mut none, OFF_NUM_TILING_ASSIGNMENTS, 0);
    let b = parse(&none, &heads);
    r.check(
        "imp: the reply gives head 3 one tile; possible with no tiling assignment is not",
        a.possible && a.tiles_per_head[3] == 1 && a.dispclk_khz == 594_000 && a.worst_domain == "dispclk"
            && !b.possible,
        format!("{:?} / {}", a.tiles_per_head, b.possible),
    );
}

/// The primitive MSGQ, vblank, the GSP event thread and GPU fences sleep on
/// (`kcore::isr_event`, `dev/gpu-memory-compute.md` 3.5a): an event landing
/// between a waiter's look and its sleep is seen without sleeping, a waiter
/// whose condition holds does not sleep, and a sleep nobody ends is one
/// backstop long
fn isr_events(r: &mut Runner) {
    use crate::kcore::isr_event::IsrEvent;
    static E: IsrEvent<2> = IsrEvent::new();
    let seen = E.seq();
    E.notify();
    let t = crate::kcore::time::Stopwatch::start();
    let moved = E.sleep_unless(seen, 50, || false);
    let fast1 = t.elapsed_us();
    let t = crate::kcore::time::Stopwatch::start();
    let ready = E.sleep_unless(E.seq(), 50, || true);
    let fast2 = t.elapsed_us();
    let t = crate::kcore::time::Stopwatch::start();
    let lone = E.sleep_unless(E.seq(), 1, || false);
    let slept = t.elapsed_us();
    r.check(
        "isr_event: a notify before the sleep is seen at once; ready() skips the sleep; a lone sleep is bounded",
        moved && fast1 < 2_000 && !ready && fast2 < 2_000 && !lone && slept < 100_000,
        format!("moved {} in {} us; ready {} in {} us; lone {} after {} us", moved, fast1, ready, fast2, lone, slept),
    );
}

/// Variable refresh (`dev/display-nvkms.md`, decision 6): the four control
/// blocks as gcc lays them out from `ctrl0073system.h`, the two method words
/// as `EvoSetStallLockC9`/`EvoSetDisplayRateC9` build them, and NVKMS's
/// arithmetic on a 240 Hz mode against a 48 Hz floor
fn vrr_params(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::disp::DpcdCaps;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::vrr::*;
    use crate::drivers::gpu::nvidia::gsp_common::modeset::Mode;
    use crate::drivers::gpu::nvidia::le;

    // 1920x1080, 2000 x 1111 total at 533.28 MHz: exactly 240 Hz
    let m = Mode {
        clock_khz: 533_280, h_display: 1920, h_sync_start: 1968, h_sync_end: 2000, h_total: 2000,
        v_display: 1080, v_sync_start: 1083, v_sync_end: 1091, v_total: 1111,
        ..Mode::default()
    };
    let info = display_info_params(0x800, true);
    let elv = clear_elv_params(0x800);
    let lw = lwsv_params(0x800, true, true, &m);
    let ps = pstate_params(0x800, true, false, true, &m, 20_833);
    r.check(
        "vrr: control blocks 12/24/20/8, displayId at 4, the bools and words where gcc put them",
        info.len() == 12 && le::u32_or0(&info, 4) == 0x800 && info[8] == 1
            && elv.len() == 8 && le::u32_or0(&elv, 4) == 0x800
            && lw.len() == 20 && lw[LWSV_ARM] == 1 && lw[LWSV_VRR_STATE] == 1
            && LWSV_ARM == 8 && LWSV_VRR_STATE == 9 && LWSV_V_ACTIVE == 12 && LWSV_VFP == 16
            && le::u32_or0(&lw, LWSV_V_ACTIVE) == 1080 && le::u32_or0(&lw, LWSV_VFP) == 3
            && ps.len() == 24 && PS_VRR_STATE == 8 && PS_VRR_DIRTY == 9 && PS_VRR_ENABLED == 10
            && PS_MAX_VBLANK_EXTENSION == 12
            && ps[PS_VRR_STATE] == 0 && ps[PS_VRR_DIRTY] == 1 && ps[PS_VRR_ENABLED] == 1,
        format!("lwsv vActive {} vfp {}", le::u32_or0(&lw, LWSV_V_ACTIVE), le::u32_or0(&lw, LWSV_VFP)),
    );
    let ext = le::u32_or0(&ps, PS_MAX_VBLANK_EXTENSION);
    let quiet = pstate_params(0x800, true, true, false, &m, 20_833);
    r.check(
        "vrr: frame 4166 us, timeout 20833 us at 48 Hz, the blank may stretch 4445 lines; not when clean",
        frame_time_us(&m) == 4166 && timeout_us(48) == 20_833 && ext == 4445
            && le::u32_or0(&quiet, PS_MAX_VBLANK_EXTENSION) == 0,
        format!("frame {} timeout {} extension {}", frame_time_us(&m), timeout_us(48), ext),
    );
    r.check(
        "vrr: stall lock on 0x1005 (enable, one-shot, line lock, no pin), off 0x1000; \
         display rate one-shot with a 20833 us floor, continuous when off",
        stall_lock_word(true) == 0x1005 && stall_lock_word(false) == 0x1000
            && display_rate_word(true, 20_833) == (20_833 << 4) | 0b101
            && display_rate_word(true, 0) == 1 && display_rate_word(false, 20_833) == 0
            && M_HEAD_SET_STALL_LOCK == 0x2034 && M_HEAD_SET_DISPLAY_RATE == 0x21a8,
        format!("{:#x} {:#x} {:#x}", stall_lock_word(true), stall_lock_word(false), display_rate_word(true, 20_833)),
    );

    // DPCD 0x007 bit 6, the EDID range and the 110% rule
    let mut d = [0u8; 16];
    d[0] = 0x14; d[1] = 0x1e; d[2] = 0x84; d[7] = 0x40;
    let yes = DpcdCaps::parse(&d).unwrap_or_default();
    d[7] = 0;
    let no = DpcdCaps::parse(&d).unwrap_or_default();
    let range = edid_range(48, 240);
    let slow = Mode { clock_khz: 111_100, ..m }; // 50 Hz: under 52.8
    r.check(
        "vrr: allowed at 240 Hz on an MSA-ignoring DP sink with a 48-240 range; not at 50 Hz, \
         not without bit 6, not over HDMI, not without a range",
        yes.msa_timing_par_ignored && !no.msa_timing_par_ignored
            && allowed(true, Some(&yes), range, Some(&m)) == Ok(20_833)
            && allowed(true, Some(&yes), range, Some(&slow)).is_err()
            && allowed(true, Some(&no), range, Some(&m)).is_err()
            && allowed(false, Some(&yes), range, Some(&m)).is_err()
            && allowed(true, Some(&yes), edid_range(0, 240), Some(&m)).is_err(),
        format!("{:?} / {:?}", allowed(true, Some(&yes), range, Some(&m)),
            allowed(true, Some(&yes), range, Some(&slow))),
    );
}

/// Tiles, physical windows and SORs handed out as NVKMS hands them out
/// (`dev/display-nvkms.md`, decision 3), over the caps block of
/// `display_caps`: tiles 0-3 carry a scaler, 4-7 do not, eight physical windows
fn resource_assignment(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::dispcaps::{Caps, TILE_TYPE_0, TILE_TYPE_1};
    use crate::drivers::gpu::nvidia::gsp_common::modeset::assign::{assign, sor_exclude_mask, HeadTiles};

    let mut caps = Caps { heads: 0xf, sors: 0xf, windows: 0xff, tiles: 0xff, ..Caps::default() };
    for t in 0..8 {
        caps.tile_type[t] = if t < 4 { TILE_TYPE_0 } else { TILE_TYPE_1 };
    }
    let none = HeadTiles::default();
    let ht = |tiles: u32, a: u32, b: u32| HeadTiles { tiles, phywins: [a, b] };

    // The boot: the firmware's head keeps its tile; the core init left every
    // window with no physical window, so it gets 0 and 1
    let boot = assign(&caps, &[], ht(0x1, 0, 0), 1, TILE_TYPE_1);
    r.check(
        "assign: an inherited head keeps its tile and gets physical windows 0 and 1",
        boot == Some(ht(0x1, 0x1, 0x2)),
        format!("{:?}", boot),
    );
    // A cold head: the first trimmed tile, not the one with its own number
    let cold = assign(&caps, &[], none, 1, TILE_TYPE_1);
    r.check(
        "assign: a cold unscaled head takes the first TYPE_1 tile (4)",
        cold == Some(ht(0x10, 0x1, 0x2)),
        format!("{:?}", cold),
    );
    // The second monitor, beside the first
    let second = assign(&caps, &[ht(0x10, 0x1, 0x2)], none, 1, TILE_TYPE_1);
    r.check(
        "assign: a second head gets the next tile and physical windows 2 and 3",
        second == Some(ht(0x20, 0x4, 0x8)),
        format!("{:?}", second),
    );
    // What a head reads back as its own but another lit head holds is not
    // reused
    let stolen = assign(&caps, &[ht(0x10, 0x1, 0x2)], ht(0x10, 0x1, 0x2), 1, TILE_TYPE_1);
    r.check(
        "assign: a tile another lit head holds is never handed out again",
        stolen == Some(ht(0x20, 0x4, 0x8)),
        format!("{:?}", stolen),
    );
    // Every trimmed tile taken: the top variant will do
    let fallback = assign(&caps, &[ht(0xf0, 0x0f, 0xf0)], none, 1, TILE_TYPE_1);
    let full = assign(&caps, &[ht(0xff, 0x0f, 0xf0)], none, 1, TILE_TYPE_1);
    r.check(
        "assign: with no TYPE_1 tile left it falls back to TYPE_0; with none at all it refuses",
        fallback.is_none() && full.is_none()
            && assign(&caps, &[ht(0xf0, 0x03, 0x0c)], none, 1, TILE_TYPE_1) == Some(ht(0x1, 0x10, 0x20)),
        format!("{:?} / {:?}", fallback, full),
    );
    r.check(
        "assign: the SOR mask excludes lit outputs' SORs but not this connector's own",
        sor_exclude_mask(&[0, 2], Some(2)) == 0b001 && sor_exclude_mask(&[], None) == 0,
        format!("{:#b}", sor_exclude_mask(&[0, 2], Some(2))),
    );
}

/// The order a failed link training steps down in
/// (`LinkConfiguration::lowerConfig`)
fn dp_fallback(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset::dp_lower_config;
    let step = |lanes: u32, bw: u32, fewer: bool| {
        let (mut l, mut b) = (lanes, bw);
        let ok = dp_lower_config(&mut l, &mut b, fewer, 4);
        (ok, l, b)
    };
    r.check(
        "dp: a clock-recovery failure drops the rate; at RBR it halves the lanes",
        step(4, 0x14, false) == (true, 4, 0x0a) && step(4, 0x06, false) == (true, 2, 0x06),
        format!("{:?} {:?}", step(4, 0x14, false), step(4, 0x06, false)),
    );
    r.check(
        "dp: an equalisation failure halves the lanes; after one lane, all lanes a rate lower",
        step(4, 0x14, true) == (true, 2, 0x14) && step(1, 0x14, true) == (true, 4, 0x0a),
        format!("{:?} {:?}", step(4, 0x14, true), step(1, 0x14, true)),
    );
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{dp_in_d3, dp_link_lost};
    let good = [0x01, 0x00, 0x77, 0x77, 0x01, 0x00];
    let unlocked = [0x01, 0x00, 0x77, 0x37, 0x01, 0x00];
    let unaligned = [0x01, 0x00, 0x77, 0x77, 0x00, 0x00];
    let two = [0x01, 0x00, 0x77, 0x00, 0x01, 0x00];
    r.check(
        "dp: a link is lost with no lanes, no alignment, or a lane in use out of lock",
        !dp_link_lost(4, &good) && dp_link_lost(4, &unlocked) && dp_link_lost(4, &unaligned)
            && !dp_link_lost(2, &two) && dp_link_lost(4, &two) && dp_link_lost(0, &good)
            && dp_link_lost(4, &good[..3]),
        format!("{} {} {}", dp_link_lost(4, &good), dp_link_lost(4, &unlocked), dp_link_lost(2, &two)),
    );
    r.check(
        "dp: a sink in D3, or one that did not answer, is asleep; D0 and D3-with-AUX are not",
        !dp_in_d3(Some(1)) && !dp_in_d3(Some(5)) && dp_in_d3(Some(2)) && dp_in_d3(None) && dp_in_d3(Some(0)),
        format!("{} {} {}", dp_in_d3(Some(1)), dp_in_d3(Some(2)), dp_in_d3(None)),
    );
    r.check(
        "dp: one lane at RBR is the end of it",
        step(1, 0x06, true) == (false, 0, 0x06) && step(1, 0x06, false) == (false, 0, 0x06),
        format!("{:?} {:?}", step(1, 0x06, true), step(1, 0x06, false)),
    );
}

/// The address space's own bookkeeping (`vaspace.rs`, 3.2), with no page
/// table written: the layout reserves without a collision and covers every
/// address the driver maps at; first fit; refusals; a mapping replacing what
/// was at its address; an implicit block for a mapping in no reservation
fn va_space(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::blackwell::chanmem;
    use crate::drivers::gpu::nvidia::gmmu::Aperture;
    use crate::drivers::gpu::nvidia::gsp_common::compute as c;
    use crate::drivers::gpu::nvidia::vaspace::{Mapping, Space, VA_START};

    let mut s = Space::new(0x1000, "test");
    let layout = chanmem::layout();
    let failed: alloc::vec::Vec<&str> = layout.iter()
        .filter(|(o, va, n)| s.reserve_at(*va, *n, o).is_err())
        .map(|(o, _, _)| *o)
        .collect();
    r.check("va: layout reserves", failed.is_empty() && s.blocks.len() == layout.len(),
        alloc::format!("{} of {} reserved; refused: {:?}", s.blocks.len(), layout.len(), failed));

    // Every address the driver maps at, from the same helpers and constants
    let mut used: alloc::vec::Vec<(&str, u64, u64)> = alloc::vec::Vec::new();
    for chid in [0u32, 1, 63] {
        used.push(("gpfifo", chanmem::gpfifo_va(chid), chanmem::GPFIFO_BYTES));
        used.push(("pushbuf", chanmem::pushbuf_va(chid), chanmem::PUSHBUF_BYTES));
        used.push(("semaphore", chanmem::sema_va(chid), chanmem::SEMA_BYTES));
    }
    used.push(("context", c::CTX_BASE_VA, c::CTX_LIMIT_VA - c::CTX_BASE_VA));
    used.push(("last program", c::PROGRAM_VA + (c::MAX_PROGRAMS as u64 - 1) * c::PROGRAM_STRIDE, c::PROGRAM_STRIDE));
    used.push(("last constant bank", c::CONST0_VA + (c::MAX_SLOTS as u64 - 1) * c::CONST0_STRIDE, 0x1000));
    used.push(("last descriptor", c::QMD_VA + (c::MAX_SLOTS as u64 - 1) * c::QMD_STRIDE, 0x1000));
    used.push(("scratch", c::SCRATCH_VA, 0x1000));
    used.push(("last effects surface", crate::drivers::gpu::nvidia::blur::VA_END - c::SURFACE_STRIDE, c::SURFACE_STRIDE));
    used.push(("last client bind", c::client_va(c::MAX_CLIENT_BINDS - 1), c::CLIENT_VA_STRIDE));
    let uncovered: alloc::vec::Vec<&str> = used.iter()
        .filter(|(_, va, n)| s.block_of(*va, *n).is_none())
        .map(|(w, _, _)| *w)
        .collect();
    r.check("va: layout covers what is mapped", uncovered.is_empty(),
        alloc::format!("{} addresses checked; outside every reservation: {:?}", used.len(), uncovered));

    // First fit from 1 MiB: the hole below the context region, then past it
    let mut t = Space::new(0x2000, "fit");
    let a = t.reserve(0x10000, 0x10000, "a");
    let b = t.reserve_at(0x200000, 0x100000, "fixed");
    let c2 = t.reserve(0x100000, 0x100000, "c");
    let d = t.reserve(0x10000, 0x10000, "d");
    r.check("va: first fit", a == Ok(VA_START) && b.is_ok() && c2 == Ok(0x300000) && d == Ok(VA_START + 0x10000),
        alloc::format!("a {:?}, fixed {:?}, c {:?}, d {:?}", a, b, c2, d));
    let clash = t.reserve_at(0x280000, 0x1000, "clash");
    let below = t.reserve_at(0x1000, 0x1000, "below 1 MiB");
    r.check("va: refusals", clash.is_err() && below.is_err(),
        alloc::format!("inside a reservation {:?}, below the first megabyte {:?}", clash, below));

    // Mapping: replace at the same address, take out what overlaps, implicit
    // blocks come and go with their mappings
    let m = |va: u64, phys: u64, bytes: u64| Mapping { va, phys, bytes, aper: Aperture::Sysmem, privileged: false, pages: Default::default() };
    let prep = |t: &mut Space, va: u64, bytes: u64| t.prepare(va, bytes).map(|(_, old, imp)| (old.len(), imp));
    let p1 = prep(&mut t, 0x200000, 0x2000);
    t.commit(m(0x200000, 0xa000, 0x2000));
    let p2 = prep(&mut t, 0x200000, 0x1000);
    t.commit(m(0x200000, 0xb000, 0x1000));
    let p3 = prep(&mut t, 0x201000, 0x2000);
    t.commit(m(0x201000, 0xc000, 0x2000));
    let p4 = prep(&mut t, 0x200000, 0x2000);
    r.check("va: replace and overlap", p1 == Ok((0, false)) && p2 == Ok((1, false)) && p3 == Ok((0, false)) && p4 == Ok((2, false)),
        alloc::format!("fresh {:?}, same address {:?}, beside {:?}, over both {:?}", p1, p2, p3, p4));
    let before = t.blocks.len();
    let p5 = prep(&mut t, 0x7000_0000_0000, 0x1000);
    t.commit(m(0x7000_0000_0000, 0xd000, 0x1000));
    let with = t.blocks.len();
    let gone = t.forget_mapping(0x7000_0000_0000).is_some();
    let straddle = t.prepare(0x2ff000, 0x2000).is_err();
    r.check("va: implicit block", p5 == Ok((0, true)) && with == before + 1 && gone && t.blocks.len() == before && straddle,
        alloc::format!("{:?}, blocks {} -> {} -> {}, straddling a reservation refused {}", p5, before, with, t.blocks.len(), straddle));

    // A physical range going away finds every mapping into it, of its own
    // memory: video memory at the same offset is another place
    t.commit(m(0x300000, 0x10_0000, 0x4000));
    t.commit(m(0x310000, 0x10_3000, 0x1000));
    t.commit(m(0x320000, 0x20_0000, 0x1000));
    t.commit(Mapping { va: 0x330000, phys: 0x10_2000, bytes: 0x1000, aper: Aperture::Vram, privileged: false, pages: Default::default() });
    let hit = t.forget_phys(0x10_2000, 0x2000, Aperture::Sysmem).len();
    let left = t.lookup(0x320000).is_some();
    let vram_kept = t.lookup(0x330000).is_some();
    let vram_hit = t.forget_phys(0x10_2000, 0x1000, Aperture::Vram).len();
    let released = t.release(0x300000).map(|v| v.len());
    r.check("va: by physical range and memory, release",
        hit == 2 && left && vram_kept && vram_hit == 1 && released == Some(1),
        alloc::format!("{} mapping(s) reached into the range, the other kept {}, video memory at the same \
            number kept {} then taken {}, release handed back {:?}", hit, left, vram_kept, vram_hit, released));
}

/// A process's context promotion (3.3): its own `MAIN` and `PATCH` carry the
/// initialise half (physical address, size, `physAttr`, `bInitialize`), and the
/// engine's buffers it borrows carry only their address in its space
/// (`kgrctxPrepareInitializeCtxBuffer` adds nothing for a buffer whose flag the
/// kernel's promotion set)
fn process_context(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::compute::{buf, promote_params, PromoteEntry};
    use crate::drivers::gpu::nvidia::le;

    let own = |id, phys, virt| PromoteEntry { id, phys, virt, size: 0x10000, initialize: true, nonmapped: false, global: false };
    let borrowed = |id, phys, virt| PromoteEntry { id, phys, virt, size: 0x20000, initialize: false, nonmapped: false, global: true };
    let entries = [
        own(buf::MAIN, 0xa000_0000, 0x0100_0000),
        own(buf::PATCH, 0xa010_0000, 0x0110_0000),
        borrowed(buf::BUFFER_BUNDLE_CB, 0xb000_0000, 0x0120_0000),
        borrowed(buf::PRIV_ACCESS_MAP, 0xb010_0000, 0x0130_0000),
    ];
    let (p, n) = promote_params(0xc1, 0xc2, &entries, true, false);
    // PARAMS_HEAD 0x30, ENTRY_SIZE 32: phys, virt, size, physAttr, id,
    // bInitialize, bNonmapped
    let at = |i: usize| 0x30 + i * 32;
    let row = |i: usize| (
        le::u64_at(&p, at(i)), le::u64_at(&p, at(i) + 8), le::u64_at(&p, at(i) + 16),
        le::u32_or0(&p, at(i) + 24), p[at(i) + 30], p[at(i) + 31],
    );
    let main = row(0);
    let bundle = row(2);
    let map = row(3);
    let ok = n == 4
        && le::u32_or0(&p, 0x28) == 4
        && main == (0xa000_0000, 0x0100_0000, 0x10000, 5, 1, 0)
        && bundle == (0, 0x0120_0000, 0, 0, 0, 0)
        && map == (0, 0x0130_0000, 0, 0, 0, 0);
    r.check("process context: promotion", ok,
        alloc::format!("{} entries; MAIN {:x?}; borrowed BUNDLE_CB {:x?}; borrowed PRIV_ACCESS_MAP {:x?}",
            n, main, bundle, map));
}

/// The page GSP-RM writes its numbers into (`perf/rusd.rs`, `dev/gpu-perf.md`),
/// the controls around it, and what `gpu_info` makes of it. The layouts are
/// `cl00de.h`, `ctrl2080internal.h`, `ctrl2080perf.h` and `ctrl2080clk.h` of
/// 615 as gcc laid them out
fn perf_shared_data(r: &mut Runner) {
    use crate::drivers::gpu::nvidia::gsp_common::modeset::{imp, Mode};
    use crate::drivers::gpu::nvidia::gsp_common::perf::{self, clocks, limits, rusd};
    use crate::drivers::gpu::nvidia::le;
    use alloc::collections::VecDeque;
    use core::cell::RefCell;

    r.check("perf: shared data layout",
        rusd::SHARED_DATA_SIZE == 3184 && rusd::CLOCKS.off == 72 && rusd::THROTTLE.off == 112
            && rusd::UTIL.off == 1992 && rusd::PSTATE.off == 2280 && rusd::POWER_LIMIT.off == 2296
            && rusd::temp(0).off == 2312 && rusd::temp(4).off == 2376 && rusd::AVG_POWER.off == 2472
            && rusd::INST_POWER.off == 2496 && rusd::POWER_POLICY.off == 2520 && rusd::FANS.off == 2584
            && rusd::POLL_MASK == 0x5b,
        format!("size {}, clocks {}, pstate {}, temps {}..{}, fans {}, mask {:#x}", rusd::SHARED_DATA_SIZE,
            rusd::CLOCKS.off, rusd::PSTATE.off, rusd::temp(0).off, rusd::temp(4).off, rusd::FANS.off, rusd::POLL_MASK));

    // A page in memory, whose timestamp reads can be scripted: each read takes
    // the next scripted word, and the page's own once the script runs out
    struct Fake {
        page: alloc::vec::Vec<u8>,
        script: RefCell<VecDeque<u64>>,
    }
    impl rusd::Page for Fake {
        fn timestamp(&self, off: usize) -> u64 {
            self.script.borrow_mut().pop_front().unwrap_or_else(|| le::u64_at(&self.page, off))
        }
        fn copy(&self, off: usize, out: &mut [u8]) {
            out.copy_from_slice(&self.page[off..off + out.len()]);
        }
    }
    let fake = |ts: u64, script: &[u64]| {
        let mut page = alloc::vec![0u8; 4096];
        le::put_u64(&mut page, rusd::PSTATE.off, ts);
        le::put_u32(&mut page, rusd::PSTATE.off + 8, 1 << 8);
        Fake { page, script: RefCell::new(script.iter().copied().collect()) }
    };
    let read = |f: &Fake| rusd::read_section(f, rusd::PSTATE);
    let whole = |ts: u64| move |s: &rusd::Section| matches!(s, rusd::Section::Whole { ts: t, bytes } if *t == ts && le::u32_at(bytes, 8) == 0x100);
    let never = read(&fake(rusd::TS_INVALID, &[]));
    let plain = read(&fake(1000, &[]));
    let writing = read(&fake(1000, &[rusd::TS_WRITING, 1000, 1000]));
    let odd = read(&fake(rusd::SEQ_START + 2, &[rusd::SEQ_START + 1, rusd::SEQ_START + 2, rusd::SEQ_START + 2]));
    // Copied under 1000, then the word reads 1001: written over, copied again
    let torn = read(&fake(1001, &[1000, 1001, 1001, 1001]));
    let busy = read(&fake(1000, &[rusd::TS_WRITING; 12]));
    r.check("perf: shared data read - RusdReadData's every branch",
        never == rusd::Section::Never && whole(1000)(&plain) && whole(1000)(&writing)
            && whole(rusd::SEQ_START + 2)(&odd) && whole(1001)(&torn) && busy == rusd::Section::Busy,
        format!("never {:?}; writing {:?}; odd {:?}; torn {:?}; busy {:?}",
            never, writing.whole().map(|w| w.0), odd.whole().map(|w| w.0), torn.whole().map(|w| w.0), busy));

    // The age: PTIMER when the stamp is in its count, the time since it moved
    // when it is not; and what the lease tells GSP-RM
    let mut m = rusd::Moved::default();
    let by_ptimer = rusd::age(5_000_000_000, Some(5_300_000_000), 10, &mut m);
    let mut m = rusd::Moved::default();
    let first = rusd::age(1000, Some(5_000_000_000_000_000_000), 100, &mut m);
    let later = rusd::age(1000, Some(5_000_000_000_000_000_000), 1700, &mut m);
    let moved = rusd::age(1500, None, 1800, &mut m);
    let not_polled = rusd::age(rusd::SEQ_START + 4, None, 0, &mut m);
    // Polling begins with 2000 on the page: 2000 is old news, 2500 is not
    let mut m = rusd::Moved::unmoved(2000);
    let old_news = rusd::age(2000, None, 5000, &mut m);
    let news = rusd::age(2500, None, 5100, &mut m);
    let lease = (rusd::decide(100, 10_100, false), rusd::decide(100, 10_100, true),
        rusd::decide(10_100, 10_100, true), rusd::decide(10_100, 10_100, false));
    r.check("perf: shared data age and the polling lease",
        by_ptimer == Some(rusd::Age::Ptimer(300)) && first == Some(rusd::Age::SinceMoved(0))
            && later == Some(rusd::Age::SinceMoved(1600)) && moved == Some(rusd::Age::SinceMoved(0))
            && not_polled.is_none() && lease == (Some(rusd::POLL_MASK), None, Some(0), None)
            && old_news == Some(rusd::Age::SinceMoved(u64::MAX)) && news == Some(rusd::Age::SinceMoved(0)),
        format!("ptimer {:?}; moved {:?} {:?} {:?}; seq {:?}; lease {:?}; since polling {:?} {:?}",
            by_ptimer, first, later, moved, not_polled, lease, old_news, news));

    // The control blocks, against gcc's sizes and offsets
    let init = rusd::init_params(Some(0x1_2345_6000));
    let destroy = rusd::init_params(None);
    let poll = rusd::poll_params(rusd::POLL_MASK, rusd::INTERVAL_MS);
    let tdp = limits::tdp_params(limits::Action::ForceLock, limits::VPstate::TurboBoost);
    let mode = Mode {
        clock_khz: 148_500, h_display: 1920, h_sync_start: 2008, h_sync_end: 2052, h_total: 2200,
        v_display: 1080, v_sync_start: 1084, v_sync_end: 1089, v_total: 1125,
        ..Mode::default()
    };
    let heads = [imp::ImpHead { head: 0, mode, tiles: 0 }];
    let asked = imp::params(&heads, true).unwrap_or_default();
    let not_asked = imp::params(&heads, false).unwrap_or_default();
    let mut reply = asked.clone();
    reply[imp::OFF_IS_POSSIBLE] = 1;
    le::put_u32(&mut reply, imp::OFF_NUM_TILING_ASSIGNMENTS, 1);
    le::put_u32(&mut reply, imp::OFF_MIN_IMP_VPSTATE, 4);
    le::put_u32(&mut reply, imp::OFF_MIN_PSTATE, 8);
    // RM clearing `options` in its reply does not lose the answer
    le::put_u32(&mut reply, imp::OFF_OPTIONS, 0);
    let a = imp::parse_asked(&reply, &heads, true);
    let b = imp::parse_asked(&reply, &heads, false);
    let ok = init.len() == 16 && init[0] == 1 && le::u64_at(&init, 8) == 0x1_2345_6000 && destroy == [0u8; 16]
        && poll.len() == 16 && le::u64_at(&poll, 0) == 0x5b && le::u32_at(&poll, 8) == 500
        && tdp == [3, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]
        && clocks::PARAMS == 6024 && clocks::request().len() == clocks::PARAMS
        && imp::OFF_MIN_IMP_VPSTATE == 1916 && imp::OFF_MIN_PSTATE == 1920
        && le::u32_at(&asked, imp::OFF_OPTIONS) == 2 && le::u32_at(&not_asked, imp::OFF_OPTIONS) == 0
        && a.min_pstate == Some(8) && a.min_vpstate == Some(4) && b.min_pstate.is_none();
    r.check("perf: control blocks - shared data, poll, rated TDP, PMUMON, IMP's min v-pstate", ok,
        format!("init {:x?}; poll {:x?}; tdp {:?}; imp options {}/{}; min pstate {:?}/{:?}",
            &init[..], &poll[..], tdp, le::u32_at(&asked, imp::OFF_OPTIONS), le::u32_at(&not_asked, imp::OFF_OPTIONS),
            a.min_pstate, b.min_pstate));

    // PMUMON: samples oldest first, runs of one clock folded into a change
    let mut q = clocks::request();
    le::put_u32(&mut q, 12, 4);
    let put = |q: &mut alloc::vec::Vec<u8>, i: usize, ms: u64, gpc: u32, dram: u32| {
        let at = 24 + i * 24;
        le::put_u64(q, at, 7_000_000_000 + ms * 1_000_000);
        le::put_u32(q, at + 8, gpc);
        le::put_u32(q, at + 12, dram);
        le::put_u32(q, at + 16, clocks::INVALID);
        le::put_u32(q, at + 20, gpc);
    };
    put(&mut q, 0, 0, 210_000, 405_000);
    put(&mut q, 1, 20, 210_000, 405_000);
    put(&mut q, 2, 40, 2_632_000, 13_801_000);
    put(&mut q, 3, 60, 2_632_000, 13_801_000);
    let samples = clocks::parse(&q).unwrap_or_default();
    let ch = clocks::changes(&samples);
    let ok = samples.len() == 4 && samples[2].gpc_khz == 2_632_000 && samples[1].nvd_khz == clocks::INVALID
        && ch.len() == 2 && ch[0].samples == 2 && ch[1].after_ms == 40 && ch[1].dram_khz == 13_801_000
        && clocks::parse(&q[..100]).is_none();
    r.check("perf: PMUMON clock samples, folded into changes", ok,
        format!("{} samples; changes {:?}", samples.len(), ch));

    // What `gpu_info` makes of a page: every line from fresh numbers, none
    // from stale ones
    let mut page = alloc::vec![0u8; 4096];
    let stamp = |page: &mut alloc::vec::Vec<u8>, sec: rusd::Sec| le::put_u64(page, sec.off, 9_000_000_000);
    stamp(&mut page, rusd::CLOCKS);
    for (d, (target, khz)) in [(210, 210_000), (405, 405_500), (600, 600_000), (210, 210_000)].iter().enumerate() {
        le::put_u32(&mut page, rusd::CLOCKS.off + 8 + d * 8, *target);
        le::put_u32(&mut page, rusd::CLOCKS.off + 12 + d * 8, *khz);
    }
    stamp(&mut page, rusd::PSTATE);
    le::put_u32(&mut page, rusd::PSTATE.off + 8, 1 << 8);
    stamp(&mut page, rusd::UTIL);
    le::put_u32(&mut page, rusd::UTIL.off + 8, 3);
    le::put_u32(&mut page, rusd::UTIL.off + 12, 1);
    stamp(&mut page, rusd::AVG_POWER);
    le::put_u32(&mut page, rusd::AVG_POWER.off + 8, 12_400);
    stamp(&mut page, rusd::POWER_LIMIT);
    le::put_u32(&mut page, rusd::POWER_LIMIT.off + 8, 180_000);
    le::put_u32(&mut page, rusd::POWER_LIMIT.off + 12, 180_000);
    stamp(&mut page, rusd::temp(0));
    le::put_u32(&mut page, rusd::temp(0).off + 8, 41 * 256 + 100);
    stamp(&mut page, rusd::FANS);
    stamp(&mut page, rusd::THROTTLE);
    le::put_u32(&mut page, rusd::THROTTLE.off + 8, 1 | 1 << 10 | 1 << 20);
    let now = Some(9_100_000_000);
    let f = Fake { page: page.clone(), script: RefCell::new(VecDeque::new()) };
    let raw = rusd::read_raw(&f);
    let fresh = perf::Snapshot::from_raw(&raw, Some(rusd::Age::Ptimer(100)), now);
    let stale = perf::Snapshot::from_raw(&raw, Some(rusd::Age::Ptimer(rusd::FRESH_MS + 1)), now);
    let lines = perf::info_lines(&fresh);
    let want = "perf P8\nclock gr 210 sm 210 mem 405 video 600\nutil gpu 3 mem 1\npower 12.4 limit 180.0\ntemp gpu 41\nfan 0\n";
    let reasons = perf::throttle_names(fresh.throttle.unwrap_or(0));
    // The card's two findings: the P-state section left behind by the clocks
    // (its line goes), and SM not reported (its field goes)
    let mut behind = page;
    le::put_u64(&mut behind, rusd::PSTATE.off, 5_000_000_000);
    le::put_u32(&mut behind, rusd::CLOCKS.off + 12 + 3 * 8, 0);
    let f = Fake { page: behind, script: RefCell::new(VecDeque::new()) };
    let lagging = perf::Snapshot::from_raw(&rusd::read_raw(&f), Some(rusd::Age::Ptimer(100)), now);
    let lag_lines = perf::info_lines(&lagging);
    let want_lag = "clock gr 210 mem 405 video 600\nutil gpu 3 mem 1\npower 12.4 limit 180.0\ntemp gpu 41\nfan 0\n";
    r.check("perf: gpu_info lines from the shared data, none when stale, none from a section left behind",
        lines == want && perf::info_lines(&stale).is_empty() && fresh.temps_dc[0] == Some(413)
            && fresh.temps_dc[1].is_none() && reasons == ["idle", "reliability", "bit 20"]
            && fresh.ages.pstate == Some(100) && lagging.ages.pstate == Some(4100) && lag_lines == want_lag,
        format!("{:?}; stale {:?}; reasons {:?}; behind {:?} ({:?} ms)", lines, perf::info_lines(&stale), reasons,
            lag_lines, lagging.ages.pstate));
}
