//! Deterministic QEMU exit signaling (test harness contract).
//!
//! QEMU is launched with `-device isa-debug-exit,iobase=0xf4,iosize=0x04`;
//! writing value `v` to port 0xF4 makes QEMU exit with status `(v << 1) | 1`.
//! On real hardware the write is harmless and the functions fall back to a
//! halt loop.

/// Exit codes understood by `tools/qemu-runner`.
#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum ExitCode {
    /// QEMU process exit status 33.
    Success = 0x10,
    /// QEMU process exit status 35.
    Failed = 0x11,
}

/// Request QEMU termination with the given code; halt if that has no effect.
pub fn exit(code: ExitCode) -> ! {
    use x86_64::instructions::port::Port;
    // SAFETY: port 0xF4 is the project-configured isa-debug-exit device and
    // is otherwise unused on the targeted machine models.
    unsafe {
        let mut port = Port::new(0xF4);
        port.write(code as u32);
    }
    halt_loop();
}

/// Halt the CPU forever (interrupt-wakeable, then re-halts).
pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
