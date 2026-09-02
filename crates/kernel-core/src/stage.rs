//! Boot-stage contract (implementation plan §9.2).
//!
//! Stage identifiers are stable machine-readable codes. The kernel emits one
//! serial marker per stage; the QEMU test harness asserts on them. Do not
//! renumber existing stages — append new ones instead.

/// All defined boot stages in required emission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Firmware/loader handoff observed (implied by kernel entry).
    B000FirmwareHandoff,
    /// Kernel entry point reached in 64-bit code.
    B010KernelEntry,
    /// Early serial console initialized.
    B020SerialReady,
    /// CPU baseline identified (vendor/features).
    B030CpuBaseline,
    /// Boot memory map validated.
    B040MemoryMapValidated,
    /// Physical memory manager ready.
    B050PhysicalMemoryReady,
    /// Virtual memory abstraction ready.
    B060VirtualMemoryReady,
    /// Kernel heap ready.
    B070HeapReady,
    /// GDT/TSS/IDT descriptor + exception layer ready.
    B080DescriptorsReady,
    /// Interrupt controller and timer baseline ready.
    B090InterruptTimerReady,
    /// Scheduler initialized.
    B100SchedulerReady,
    /// VFS/initramfs mounted.
    B110VfsReady,
    /// Input path initialized.
    B120InputReady,
    /// Shell/init task running.
    B130ShellRunning,
    /// Userspace transition ready (stretch milestone).
    B140UserspaceReady,
    /// V0.1 boot acceptance reached.
    B150Acceptance,
    /// Device layer ready — PCI enumerated, storage discovered (V0.3).
    B160StorageReady,
    /// Graphics framebuffer + compositor ready (V0.5).
    B170GraphicsReady,
    /// PS/2 input online + desktop composited (V0.5).
    B180DesktopReady,
    /// Device model built — PCI enumerated, BARs/capabilities probed, drivers
    /// bound (V0.6).
    B190DeviceModelReady,
}

impl Stage {
    /// Stable machine-readable code, e.g. `B010`.
    pub const fn code(self) -> &'static str {
        match self {
            Stage::B000FirmwareHandoff => "B000",
            Stage::B010KernelEntry => "B010",
            Stage::B020SerialReady => "B020",
            Stage::B030CpuBaseline => "B030",
            Stage::B040MemoryMapValidated => "B040",
            Stage::B050PhysicalMemoryReady => "B050",
            Stage::B060VirtualMemoryReady => "B060",
            Stage::B070HeapReady => "B070",
            Stage::B080DescriptorsReady => "B080",
            Stage::B090InterruptTimerReady => "B090",
            Stage::B100SchedulerReady => "B100",
            Stage::B110VfsReady => "B110",
            Stage::B120InputReady => "B120",
            Stage::B130ShellRunning => "B130",
            Stage::B140UserspaceReady => "B140",
            Stage::B150Acceptance => "B150",
            Stage::B160StorageReady => "B160",
            Stage::B170GraphicsReady => "B170",
            Stage::B180DesktopReady => "B180",
            Stage::B190DeviceModelReady => "B190",
        }
    }

    /// Human-readable stage description for the serial log.
    pub const fn describe(self) -> &'static str {
        match self {
            Stage::B000FirmwareHandoff => "firmware/loader handoff observed",
            Stage::B010KernelEntry => "kernel entry reached in 64-bit mode",
            Stage::B020SerialReady => "early serial console ready",
            Stage::B030CpuBaseline => "CPU baseline established",
            Stage::B040MemoryMapValidated => "boot memory map validated",
            Stage::B050PhysicalMemoryReady => "physical memory manager ready",
            Stage::B060VirtualMemoryReady => "virtual memory abstraction ready",
            Stage::B070HeapReady => "kernel heap ready",
            Stage::B080DescriptorsReady => "descriptor/exception layer ready",
            Stage::B090InterruptTimerReady => "interrupt controller/timer ready",
            Stage::B100SchedulerReady => "scheduler initialized",
            Stage::B110VfsReady => "VFS/initramfs initialized",
            Stage::B120InputReady => "input path initialized",
            Stage::B130ShellRunning => "shell/init task running",
            Stage::B140UserspaceReady => "userspace transition ready",
            Stage::B150Acceptance => "V0.1 boot acceptance reached",
            Stage::B160StorageReady => "device layer / storage ready",
            Stage::B170GraphicsReady => "graphics framebuffer + compositor ready",
            Stage::B180DesktopReady => "PS/2 input online + desktop composited",
            Stage::B190DeviceModelReady => "device model built — PCI/BAR/caps + drivers",
        }
    }

    /// Look up a stage from its code (host-side parsing).
    pub fn from_code(code: &str) -> Option<Stage> {
        ALL_STAGES.iter().copied().find(|s| s.code() == code)
    }
}

/// Every stage in canonical order.
pub const ALL_STAGES: [Stage; 20] = [
    Stage::B000FirmwareHandoff,
    Stage::B010KernelEntry,
    Stage::B020SerialReady,
    Stage::B030CpuBaseline,
    Stage::B040MemoryMapValidated,
    Stage::B050PhysicalMemoryReady,
    Stage::B060VirtualMemoryReady,
    Stage::B070HeapReady,
    Stage::B080DescriptorsReady,
    Stage::B090InterruptTimerReady,
    Stage::B100SchedulerReady,
    Stage::B110VfsReady,
    Stage::B120InputReady,
    Stage::B130ShellRunning,
    Stage::B140UserspaceReady,
    Stage::B150Acceptance,
    Stage::B160StorageReady,
    Stage::B170GraphicsReady,
    Stage::B180DesktopReady,
    Stage::B190DeviceModelReady,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_ordered() {
        for pair in ALL_STAGES.windows(2) {
            assert!(pair[0] < pair[1]);
            assert_ne!(pair[0].code(), pair[1].code());
        }
    }

    #[test]
    fn from_code_round_trips() {
        for stage in ALL_STAGES {
            assert_eq!(Stage::from_code(stage.code()), Some(stage));
        }
        assert_eq!(Stage::from_code("B999"), None);
        assert_eq!(Stage::from_code(""), None);
    }
}
