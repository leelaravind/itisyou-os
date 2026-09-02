//! AC'97 (Intel 82801AA) audio output driver (V0.6).
//!
//! The smallest correct path that proves generated PCM samples traverse the
//! full driver → controller → host-output path — never "success from init
//! alone". `attach` initializes the codec (reset, unmute, full volume) and
//! records the two I/O register windows (NAM mixer, NABM bus master).
//! [`play_test_tone`] then synthesizes 16-bit stereo PCM, describes it to the
//! PCM-Out bus master through a Buffer Descriptor List (BDL) over DMA frames,
//! starts playback, and confirms the controller actually consumes the buffers
//! (the Current Index Value advances and the DMA halts on the last buffer).
//! With QEMU's `-audiodev wav` backend the emitted samples land in a WAV file
//! the harness inspects, giving true end-to-end evidence.

use crate::device::{pci, Device, Driver, DriverError};
use crate::memory::{self, paging};
use crate::serial_println;
use spin::Mutex;
use x86_64::instructions::port::Port;
use x86_64::structures::paging::PhysFrame;

// NAM (mixer) register offsets — accessed via the first I/O BAR.
const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOL: u16 = 0x02;
const NAM_PCM_VOL: u16 = 0x18;

// NABM (bus master) register offsets, PCM-Out ("PO") box — second I/O BAR.
const PO_BDBAR: u16 = 0x10; // BDL base address (u32)
const PO_CIV: u16 = 0x14; // current index value (u8, ro)
const PO_LVI: u16 = 0x15; // last valid index (u8)
const PO_SR: u16 = 0x16; // status (u16)
const PO_CR: u16 = 0x1B; // control (u8)
const GLOB_CNT: u16 = 0x2C; // global control (u32)
const GLOB_STA: u16 = 0x30; // global status (u32)

// PO_CR bits.
const CR_RPBM: u8 = 1 << 0; // run/pause bus master (1 = run)
const CR_RR: u8 = 1 << 1; // reset registers

// PO_SR bits.
const SR_DCH: u16 = 1 << 0; // DMA controller halted

const SAMPLE_RATE: u32 = 48_000;
const TONE_HZ: u32 = 440;
const AMPLITUDE: i16 = 0x2000;
/// Number of 4 KiB DMA buffers of PCM (each 1024 stereo frames ≈ 21 ms).
const NUM_BUFFERS: usize = 8;
const FRAME_BYTES: usize = 4096;
/// 16-bit samples per 4 KiB buffer (stereo → 2 samples per stereo frame).
const SAMPLES_PER_BUF: usize = FRAME_BYTES / 2;

pub struct Ac97Driver;
pub static AC97_DRIVER: Ac97Driver = Ac97Driver;

/// Initialized controller state (register windows), set by `attach`.
struct Ac97 {
    nam: u16,
    nabm: u16,
}

static STATE: Mutex<Option<Ac97>> = Mutex::new(None);

impl Driver for Ac97Driver {
    fn name(&self) -> &'static str {
        "ac97"
    }

    fn probe(&self, dev: &Device) -> bool {
        // Multimedia audio controller (class 0x04, subclass 0x01) with two I/O
        // BARs (NAM + NABM). Matches QEMU's AC97 (8086:2415).
        dev.id().class == 0x04 && dev.id().subclass == 0x01
    }

    fn attach(&self, dev: &Device) -> Result<(), DriverError> {
        dev.pci
            .enable_command_bits(pci::CMD_IO_SPACE | pci::CMD_BUS_MASTER);

        // NAM = first I/O BAR, NABM = second I/O BAR.
        let (nam, nabm) = match collect_io_bars(dev) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err(DriverError::InitFailed("ac97 needs two I/O BARs")),
        };

        // Cold-reset the controller, then reset the codec.
        unsafe {
            Port::<u32>::new(nabm + GLOB_CNT).write(0x0000_0002);
            // Reset the primary codec (any write to the NAM reset register).
            Port::<u16>::new(nam + NAM_RESET).write(0x0000);
        }
        // Wait for the primary codec to report ready (GLOB_STA bit 8), bounded.
        let mut ready = false;
        for _ in 0..100_000 {
            let sta = unsafe { Port::<u32>::new(nabm + GLOB_STA).read() };
            if sta & (1 << 8) != 0 {
                ready = true;
                break;
            }
            core::hint::spin_loop();
        }

        // Unmute + full volume on master and PCM-out (0 = 0 dB attenuation).
        unsafe {
            Port::<u16>::new(nam + NAM_MASTER_VOL).write(0x0000);
            Port::<u16>::new(nam + NAM_PCM_VOL).write(0x0000);
        }

        *STATE.lock() = Some(Ac97 { nam, nabm });
        serial_println!(
            "[ITISYOU:INFO] ac97 init nam={nam:#06x} nabm={nabm:#06x} codec_ready={ready} rate={SAMPLE_RATE}"
        );
        Ok(())
    }
}

/// The device's first two I/O BAR ports, in BAR order (NAM, NABM).
fn collect_io_bars(dev: &Device) -> (Option<u16>, Option<u16>) {
    use kernel_core::pci::Bar;
    let mut ports = [None, None];
    let mut n = 0;
    for b in dev.bars.iter() {
        if let Bar::Io { port, size } = b {
            if *size > 0 && n < 2 {
                ports[n] = Some(*port);
                n += 1;
            }
        }
    }
    (ports[0], ports[1])
}

/// Result of a test-tone playback.
pub struct PlayResult {
    pub buffers: usize,
    pub consumed: u8,
    pub halted: bool,
}

/// True once `attach` has initialized a controller.
pub fn available() -> bool {
    STATE.lock().is_some()
}

/// Synthesize a short tone, hand it to the PCM-Out bus master via a BDL, start
/// playback, and poll until the controller consumes the buffers (or a bounded
/// timeout). Returns how far the Current Index Value advanced — evidence the
/// DMA path ran. Emits `[ITISYOU:INFO] ac97 play ...`.
pub fn play_test_tone() -> Option<PlayResult> {
    let guard = STATE.lock();
    let ac97 = guard.as_ref()?;
    let nabm = ac97.nabm;
    // Re-assert full PCM-out volume (unmuted) before playing.
    unsafe { Port::<u16>::new(ac97.nam + NAM_PCM_VOL).write(0x0000) };

    // Allocate + fill PCM buffers with a square-wave tone; build the BDL.
    let mut bufs: [Option<PhysFrame>; NUM_BUFFERS] = [None; NUM_BUFFERS];
    let bdl = match memory::alloc_frame() {
        Ok(f) => f,
        Err(_) => return None,
    };
    let bdl_phys = bdl.start_address().as_u64();
    let bdl_virt = paging::phys_to_virt(bdl_phys);

    let half_period = (SAMPLE_RATE / TONE_HZ / 2).max(1) as usize;
    let mut global_frame = 0usize; // stereo-frame counter for phase continuity
    for (i, slot) in bufs.iter_mut().enumerate() {
        let frame = match memory::alloc_frame() {
            Ok(f) => f,
            Err(_) => break,
        };
        let phys = frame.start_address().as_u64();
        let virt = paging::phys_to_virt(phys) as *mut i16;
        // FRAME_BYTES/4 stereo frames per buffer.
        for s in 0..(FRAME_BYTES / 4) {
            let high = (global_frame / half_period).is_multiple_of(2);
            let val = if high { AMPLITUDE } else { -AMPLITUDE };
            // SAFETY: `virt` is a fresh exclusively-owned DMA frame; two i16
            // writes (L, R) stay within the 4 KiB frame (s < 1024).
            unsafe {
                virt.add(s * 2).write_volatile(val);
                virt.add(s * 2 + 1).write_volatile(val);
            }
            global_frame += 1;
        }
        // BDL entry i: u32 buffer phys, u16 sample count, u16 control.
        // SAFETY: bdl_virt is a fresh 4 KiB frame; entry i (i < 8) is in range.
        unsafe {
            let entry = bdl_virt.add(i * 8);
            (entry as *mut u32).write_volatile(phys as u32);
            (entry.add(4) as *mut u16).write_volatile(SAMPLES_PER_BUF as u16);
            // Control: bit15 IOC (interrupt on completion). Last buffer also
            // sets bit14 BUP (play silence on underrun after it).
            let ctl: u16 = if i == NUM_BUFFERS - 1 {
                (1 << 15) | (1 << 14)
            } else {
                1 << 15
            };
            (entry.add(6) as *mut u16).write_volatile(ctl);
        }
        *slot = Some(frame);
    }
    let filled = bufs.iter().filter(|b| b.is_some()).count();
    if filled == 0 {
        return None;
    }

    // Program the PCM-Out bus master and start it.
    unsafe {
        Port::<u8>::new(nabm + PO_CR).write(CR_RR); // reset registers
                                                    // Wait for reset to clear (bounded).
        for _ in 0..100_000 {
            if Port::<u8>::new(nabm + PO_CR).read() & CR_RR == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        Port::<u32>::new(nabm + PO_BDBAR).write(bdl_phys as u32);
        Port::<u8>::new(nabm + PO_LVI).write((filled - 1) as u8);
        Port::<u8>::new(nabm + PO_CR).write(CR_RPBM); // run
    }

    // Poll until the DMA halts (all buffers consumed) or a bounded timeout.
    // QEMU pulls samples in real time, so this also paces the tone's duration.
    let mut halted = false;
    let mut last_civ = 0u8;
    for _ in 0..2_000_000u32 {
        let sr = unsafe { Port::<u16>::new(nabm + PO_SR).read() };
        last_civ = unsafe { Port::<u8>::new(nabm + PO_CIV).read() };
        if sr & SR_DCH != 0 {
            halted = true;
            break;
        }
        core::hint::spin_loop();
    }

    // Stop the bus master and release the DMA frames.
    unsafe { Port::<u8>::new(nabm + PO_CR).write(0) };
    for f in bufs.into_iter().flatten() {
        let _ = memory::free_frame(f);
    }
    let _ = memory::free_frame(bdl);

    serial_println!(
        "[ITISYOU:INFO] ac97 play rate={SAMPLE_RATE} tone_hz={TONE_HZ} bufs={filled} civ={last_civ} halted={halted}"
    );
    Some(PlayResult {
        buffers: filled,
        consumed: last_civ,
        halted,
    })
}
