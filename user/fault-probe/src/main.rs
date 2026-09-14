//! `fault-probe <mode>` - CPU exceptions from Ring 3 end the program, never
//! the kernel (V1.0, SEC1-001/002/003).
//!
//! Each mode prints `FAULTPROBE-ARMED mode=<mode>` and then raises one
//! exception that, before V1.0, had no IDT gate - so the CPU escalated to a
//! double fault and the kernel panicked:
//!
//! * `div0` - `div` by zero (`#DE`, vector 0);
//! * `int1` - the one-byte `int1` (`#DB`, vector 1);
//! * `step` - set RFLAGS.TF with `popfq` (`#DB` after the next instruction);
//! * `stack` - `push` through a non-canonical stack pointer (`#SS`, 12);
//! * `x87` - an x87 instruction (`#NM`, 7: V1.0 turns the FPU off for
//!   programs, SEC1-002 - its registers were never switched between them);
//! * `sse` - an SSE instruction (`#UD`, 6: `CR4.OSFXSR` is clear).
//!
//! If the instruction completes, the probe prints
//! `FAULTPROBE-UNCONTAINED mode=<mode>` and exits 1: nothing faulted.
//!
//! `regs` checks SEC1-003: after `getpid` and `write` the six argument and
//! scratch registers the syscall ABI does not return (rdi, rsi, rdx, r8-r10)
//! must come back zero, not holding whatever the kernel left in them:
//! `FAULTPROBE-REGS clear=true`, or `clear=false reg=<name> value=<hex>`.

#![no_std]
#![no_main]

use core::arch::asm;
use ulib::{args, exit, split_args, write, write_u64};

const SYS_WRITE: u64 = 0;
const SYS_GETPID: u64 = 3;

fn armed(mode: &str) {
    write("FAULTPROBE-ARMED mode=");
    write(mode);
    write("\n");
}

fn uncontained(mode: &str) -> ! {
    write("FAULTPROBE-UNCONTAINED mode=");
    write(mode);
    write("\n");
    exit(1)
}

fn write_hex(v: u64) {
    let digits = b"0123456789abcdef";
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for (i, b) in buf[2..].iter_mut().enumerate() {
        *b = digits[((v >> (60 - i * 4)) & 0xF) as usize];
    }
    ulib::write_raw(buf.as_ptr() as u64, buf.len() as u64);
}

/// One syscall with the six non-returned registers preloaded with a
/// sentinel; returns what came back in them.
fn scratch_after(nr: u64, a1: u64, a2: u64, a3: u64) -> [u64; 6] {
    let (rdi, rsi, rdx, r8, r9, r10): (u64, u64, u64, u64, u64, u64);
    // SAFETY: a plain syscall; every register it may change is declared.
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") nr => _,
            inlateout("rdi") a1 => rdi,
            inlateout("rsi") a2 => rsi,
            inlateout("rdx") a3 => rdx,
            inlateout("r8") 0x5e5e_5e5e_u64 => r8,
            inlateout("r9") 0x5e5e_5e5e_u64 => r9,
            inlateout("r10") 0x5e5e_5e5e_u64 => r10,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    [rdi, rsi, rdx, r8, r9, r10]
}

fn regs() -> ! {
    const NAMES: [&str; 6] = ["rdi", "rsi", "rdx", "r8", "r9", "r10"];
    let msg = b"FAULTPROBE-REGS write\n";
    let checks = [
        scratch_after(SYS_GETPID, 0x5e5e_5e5e, 0x5e5e_5e5e, 0x5e5e_5e5e),
        scratch_after(SYS_WRITE, 1, msg.as_ptr() as u64, msg.len() as u64),
    ];
    for regs in checks {
        for (name, value) in NAMES.iter().zip(regs) {
            if value != 0 {
                write("FAULTPROBE-REGS clear=false reg=");
                write(name);
                write(" value=");
                write_hex(value);
                write("\n");
                exit(1)
            }
        }
    }
    write("FAULTPROBE-REGS clear=true checked=");
    write_u64(checks.len() as u64 * NAMES.len() as u64);
    write("\n");
    exit(0)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    let mut block = [0u8; 64];
    let len = args(&mut block) as usize;
    let mut words = split_args(&block[..len.min(block.len())]);
    let mode = words.next().unwrap_or(b"");
    // SAFETY (every asm block below): each raises a CPU exception at CPL 3,
    // which the kernel must contain by ending this process; none touches
    // memory it does not own.
    match mode {
        b"div0" => {
            armed("div0");
            unsafe {
                asm!("xor edx, edx", "xor ecx, ecx", "mov eax, 1", "div ecx",
                    out("eax") _, out("ecx") _, out("edx") _, options(nomem, nostack));
            }
            uncontained("div0")
        }
        b"int1" => {
            armed("int1");
            unsafe { asm!(".byte 0xf1", options(nomem, nostack)) };
            uncontained("int1")
        }
        b"step" => {
            armed("step");
            unsafe {
                asm!("pushfq", "or qword ptr [rsp], 0x100", "popfq", "nop", "nop");
            }
            uncontained("step")
        }
        b"stack" => {
            armed("stack");
            unsafe {
                asm!("mov rsp, {bad}", "push rax", bad = in(reg) 0x8000_0000_0000_0000_u64);
            }
            uncontained("stack")
        }
        b"x87" => {
            armed("x87");
            unsafe { asm!("fninit", "fld1", options(nomem, nostack)) };
            uncontained("x87")
        }
        b"sse" => {
            armed("sse");
            unsafe { asm!("xorps xmm0, xmm0", out("xmm0") _, options(nomem, nostack)) };
            uncontained("sse")
        }
        b"regs" => regs(),
        _ => {
            write("FAULTPROBE-USAGE div0|int1|step|stack|x87|sse|regs\n");
            exit(2)
        }
    }
}
