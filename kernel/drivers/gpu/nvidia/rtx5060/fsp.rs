// FSP command transport: MCTP-over-EMEM, modeled on nouveau fsp/gh100.c
//
// The FSP exposes an 8 KiB EMEM aperture through an indexed PIO port
// (EMEMC/EMEMD) plus two register pairs acting as ring pointers:
//
//   QUEUE_HEAD/TAIL(0)  host -> FSP command ring (we produce)
//   MSGQ_HEAD/TAIL(0)   FSP -> host response ring (FSP produces)
//
// A command is one MCTP packet: transport header (u32), message header
// (u32), payload. We only ever need single-packet messages (the COT payload
// is 860 bytes; EMEM holds 8 KiB), so SOM=EOM=1 and no fragmentation logic.
//
// Protocol per nouveau gh100_fsp_send/wait/recv:
//   send: PIO-write packet to EMEM offset 0, QUEUE_TAIL <- size-4,
//         QUEUE_HEAD <- 0 (doorbell), then wait QUEUE_HEAD == QUEUE_TAIL
//         (FSP consumed the command)
//   recv: poll MSGQ_HEAD != MSGQ_TAIL, packet occupies EMEM
//         [head .. tail+4), read it out, reset MSGQ_HEAD/TAIL to 0
//
// Everything is bounded polling; the FSP answers in single-digit
// milliseconds when healthy and the secure-boot wait is capped at ~4 s
// (nouveau uses the same figure)

use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::serial_println;

use super::regs::*;

/// Response longer than any NVDM reply we expect (FSP responses are a few
/// words; COT reply is header + task id + command id + error code)
const MAX_RESPONSE_WORDS: usize = 16;

/// Wall-clock poll budgets via PTIMER (ns). MMIO reads over PCIe cost
/// microseconds each, so spin-count budgets stretch into minutes on real
/// silicon; real-time bounds keep the shell responsive. nouveau waits
/// ~2 s for command consumption / response and 4 s for secure boot
const POLL_NS_CMD: u32 = 2_000_000_000;
const POLL_NS_BOOT: u32 = 4_000_000_000;

/// PTIMER nanosecond counter (absolute BAR0 offset, wraps every ~4.3 s)
const PTIMER_TIME_0: u32 = 0x0000_9400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FspError {
    /// QUEUE_HEAD never met QUEUE_TAIL: command not consumed
    CommandStuck,
    /// No response appeared in MSGQ within the budget
    ResponseTimeout,
    /// Response MCTP/NVDM framing was not what we expect
    BadResponse,
    /// FSP responded with a non-zero NVDM error code
    Nack(u32),
    /// Payload does not fit the EMEM aperture
    TooLarge,
}

pub struct FspResponse {
    pub nvdm_type: u32,
    pub words: [u32; MAX_RESPONSE_WORDS],
    pub len_words: usize,
}

pub struct Fsp<'a> {
    bar0: &'a MmioRegion,
}

impl<'a> Fsp<'a> {
    pub fn new(bar0: &'a MmioRegion) -> Self {
        Self { bar0 }
    }

    //////////////////////////////////////////////////////
    //                          EMEM PIO                //
    //////////////////////////////////////////////////////

    /// Program EMEMC for a byte offset. Field layout: [7:2] offset-in-block,
    /// [15:8] block, i.e. the offset value itself with bits [1:0] clear
    fn emem_seek(&self, byte_off: u32, ainc: u32) {
        self.bar0.write32(PFSP_EMEMC0, (byte_off & 0x7FFC) | ainc);
    }

    fn emem_write(&self, byte_off: u32, words: &[u32]) {
        self.emem_seek(byte_off, EMEMC_AINCW);
        for &w in words {
            self.bar0.write32(PFSP_EMEMD0, w);
        }
    }

    fn emem_read(&self, byte_off: u32, out: &mut [u32]) {
        self.emem_seek(byte_off, EMEMC_AINCR);
        for w in out.iter_mut() {
            *w = self.bar0.read32(PFSP_EMEMD0);
        }
    }

	//////////////////////////////////////////////////////
    //                Status / boot                     //
	//////////////////////////////////////////////////////

    /// FSP secure-boot status byte from the THERM scratch (0xFF = success)
    pub fn boot_status(&self) -> u32 {
        self.bar0.read32(THERM_I2CS_SCRATCH) & 0xFF
    }

    fn now(&self) -> u32 {
        self.bar0.read32(PTIMER_TIME_0)
    }

    /// Wait for the FSP to report secure boot complete. On a VBIOS-posted
    /// card this is usually already true long before the OS looks
    pub fn wait_secure_boot(&self) -> Result<(), FspError> {
        let t0 = self.now();
        loop {
            if self.boot_status() == FSP_BOOT_COMPLETE_SUCCESS {
                return Ok(());
            }
            if self.now().wrapping_sub(t0) >= POLL_NS_BOOT {
                return Err(FspError::ResponseTimeout);
            }
            core::hint::spin_loop();
        }
    }

    /// One-line diagnostic snapshot of the FSP rings + boot scratch
    pub fn log_state(&self, tag: &str) {
        serial_println!(
            "[gb206/fsp] {}: boot={:#04x} cmdq h/t={:#x}/{:#x} msgq h/t={:#x}/{:#x}",
            tag,
            self.boot_status(),
            self.bar0.read32(PFSP_QUEUE_HEAD0),
            self.bar0.read32(PFSP_QUEUE_TAIL0),
            self.bar0.read32(PFSP_MSGQ_HEAD0),
            self.bar0.read32(PFSP_MSGQ_TAIL0),
        );
    }

    //////////////////////////////////////////////////////
    //         Command send / response receive          //
    //////////////////////////////////////////////////////

    /// Send one NVDM command and wait for the FSP's NVDM response.
    /// 'payload' is the NVDM payload in little-endian words (the two MCTP
    /// header words are added here)
    pub fn send_sync(&self, nvdm_type: u32, payload: &[u32]) -> Result<FspResponse, FspError> {
        let total_words = 2 + payload.len();
        if total_words * 4 > 8192 {
            return Err(FspError::TooLarge);
        }

        // Precondition (nouveau gh100_fsp_send): any previously sent
        // command must have been consumed (HEAD == TAIL) before we touch
        // the EMEM. We do NOT wait for consumption of our own command
        // afterwards - the FSP signals completion via the MSGQ response,
        // and is not required to update the cmdq pointers on any
        // particular schedule
        let t0 = self.now();
        loop {
            if self.bar0.read32(PFSP_QUEUE_HEAD0) == self.bar0.read32(PFSP_QUEUE_TAIL0) {
                break;
            }
            if self.now().wrapping_sub(t0) >= POLL_NS_CMD {
                return Err(FspError::CommandStuck);
            }
            core::hint::spin_loop();
        }

        // Packet = MCTP transport header, MCTP message header, payload
        self.emem_write(0, &[MCTP_HEADER_SINGLE, nvdm_header(nvdm_type)]);
        self.emem_write(8, payload);

        // Doorbell: tail = last byte offset of the packet, head = 0.
        // (nouveau: QUEUE_TAIL <- packet_size - sizeof(u32), then HEAD <- 0)
        let size_bytes = (total_words * 4) as u32;
        self.bar0.write32(PFSP_QUEUE_TAIL0, size_bytes - 4);
        self.bar0.write32(PFSP_QUEUE_HEAD0, 0);

        // Straight to the response poll, as nouveau does
        self.recv()
    }

    /// Wait for and read one response packet from the FSP message queue
    fn recv(&self) -> Result<FspResponse, FspError> {
        let (head, tail) = {
            let t0 = self.now();
            loop {
                let h = self.bar0.read32(PFSP_MSGQ_HEAD0);
                let t = self.bar0.read32(PFSP_MSGQ_TAIL0);
                if h != t {
                    break (h, t);
                }
                if self.now().wrapping_sub(t0) >= POLL_NS_CMD {
                    return Err(FspError::ResponseTimeout);
                }
                core::hint::spin_loop();
            }
        };

        // Packet spans [head .. tail + 4) in EMEM
        let size_bytes = tail.wrapping_sub(head).wrapping_add(4);
        if size_bytes < 8 || size_bytes % 4 != 0 {
            return Err(FspError::BadResponse);
        }
        let n_words = ((size_bytes / 4) as usize).min(2 + MAX_RESPONSE_WORDS);

        let mut raw = [0u32; 2 + MAX_RESPONSE_WORDS];
        self.emem_read(head, &mut raw[..n_words]);

        // Consume: reset the ring so the next exchange starts clean
        self.bar0.write32(PFSP_MSGQ_HEAD0, 0);
        self.bar0.write32(PFSP_MSGQ_TAIL0, 0);

        // Validate framing: single-packet MCTP, vendor-PCI NVDM from NVIDIA
        let mctp = raw[0];
        let nvdm = raw[1];
        if mctp & MCTP_HEADER_SINGLE != MCTP_HEADER_SINGLE {
            serial_println!("[gb206/fsp] response mctp header {:#010x}: not SOM|EOM", mctp);
            return Err(FspError::BadResponse);
        }
        if nvdm & 0xFF_FF7F != nvdm_header(0) & 0xFF_FF7F {
            serial_println!("[gb206/fsp] response nvdm header {:#010x}: bad type/vendor", nvdm);
            return Err(FspError::BadResponse);
        }
        let nvdm_type = nvdm >> 24;

        let mut resp = FspResponse {
            nvdm_type,
            words: [0; MAX_RESPONSE_WORDS],
            len_words: n_words - 2,
        };
        resp.words[..n_words - 2].copy_from_slice(&raw[2..n_words]);

        // NVDM_TYPE_FSP_RESPONSE payload: [task id, command nvdm type,
        // error code, ...]. Non-zero error = NACK (nouveau send_sync)
        if nvdm_type == NVDM_TYPE_FSP_RESPONSE && resp.len_words >= 3 {
            let err_code = resp.words[2];
            if err_code != 0 {
                serial_println!(
                    "[gb206/fsp] FSP NACK: task={:#x} cmd={:#x} err={:#x}",
                    resp.words[0], resp.words[1], err_code
                );
                return Err(FspError::Nack(err_code));
            }
        }

        Ok(resp)
    }
}
