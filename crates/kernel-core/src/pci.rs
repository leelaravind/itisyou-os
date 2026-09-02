//! PCI class/identity decoding (pure logic, host-tested). The kernel's
//! `device::pci` module performs the port I/O; this crate decodes the
//! config-space words it reads so the classification is unit-testable.

/// A decoded PCI device identity (from config-space header words).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciId {
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

impl PciId {
    /// Decode vendor/device (offset 0x00) and class/prog-if/header (0x08,0x0C).
    pub fn decode(reg00: u32, reg08: u32, reg0c: u32) -> PciId {
        PciId {
            vendor: (reg00 & 0xFFFF) as u16,
            device: (reg00 >> 16) as u16,
            prog_if: ((reg08 >> 8) & 0xFF) as u8,
            subclass: ((reg08 >> 16) & 0xFF) as u8,
            class: ((reg08 >> 24) & 0xFF) as u8,
            header_type: ((reg0c >> 16) & 0xFF) as u8,
        }
    }

    pub fn is_multifunction(&self) -> bool {
        self.header_type & 0x80 != 0
    }

    /// Is there a device here at all (vendor 0xFFFF == absent)?
    pub fn present(&self) -> bool {
        self.vendor != 0xFFFF && self.vendor != 0x0000
    }

    /// Human-readable class name for the common storage/bridge classes.
    pub fn class_name(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x01, 0x01) => "IDE controller",
            (0x01, 0x06) => "SATA/AHCI controller",
            (0x01, 0x08) => "NVMe controller",
            (0x01, _) => "mass-storage controller",
            (0x02, _) => "network controller",
            (0x03, _) => "display controller",
            (0x04, 0x01) => "multimedia audio controller",
            (0x04, 0x03) => "HD Audio controller",
            (0x04, _) => "multimedia controller",
            (0x06, 0x00) => "host bridge",
            (0x06, 0x01) => "ISA bridge",
            (0x06, 0x04) => "PCI-to-PCI bridge",
            (0x06, _) => "bridge",
            (0x0C, 0x03) => "USB controller",
            _ => "device",
        }
    }

    /// True for a mass-storage controller (class 0x01).
    pub fn is_storage(&self) -> bool {
        self.class == 0x01
    }

    /// True for a multimedia/audio controller (class 0x04).
    pub fn is_audio(&self) -> bool {
        self.class == 0x04
    }

    /// True for a USB controller (class 0x0C subclass 0x03).
    pub fn is_usb(&self) -> bool {
        self.class == 0x0C && self.subclass == 0x03
    }

    /// USB controller kind by prog-if (only meaningful when `is_usb`).
    pub fn usb_kind(&self) -> UsbKind {
        UsbKind::from_prog_if(self.prog_if)
    }

    /// True if this header type has a standard capability list (header type 0
    /// or 1 with the status-register capabilities bit set — the bit itself is
    /// read by the kernel; this only rules out unusual header layouts).
    pub fn has_cap_list_layout(&self) -> bool {
        matches!(self.header_type & 0x7F, 0x00 | 0x01)
    }
}

/// USB host-controller kind, decoded from the PCI prog-if byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbKind {
    Uhci, // 0x00 — USB 1.1
    Ohci, // 0x10 — USB 1.1
    Ehci, // 0x20 — USB 2.0
    Xhci, // 0x30 — USB 3.x
    Other(u8),
}

impl UsbKind {
    pub fn from_prog_if(prog_if: u8) -> UsbKind {
        match prog_if {
            0x00 => UsbKind::Uhci,
            0x10 => UsbKind::Ohci,
            0x20 => UsbKind::Ehci,
            0x30 => UsbKind::Xhci,
            other => UsbKind::Other(other),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            UsbKind::Uhci => "UHCI",
            UsbKind::Ohci => "OHCI",
            UsbKind::Ehci => "EHCI",
            UsbKind::Xhci => "xHCI",
            UsbKind::Other(_) => "USB",
        }
    }
}

/// PCI capability IDs found while walking the config-space capability list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityId {
    PowerManagement, // 0x01
    Agp,             // 0x02
    Vpd,             // 0x03
    Msi,             // 0x05
    VendorSpecific,  // 0x09
    PciExpress,      // 0x10
    MsiX,            // 0x11
    Other(u8),
}

impl CapabilityId {
    pub fn from_u8(id: u8) -> CapabilityId {
        match id {
            0x01 => CapabilityId::PowerManagement,
            0x02 => CapabilityId::Agp,
            0x03 => CapabilityId::Vpd,
            0x05 => CapabilityId::Msi,
            0x09 => CapabilityId::VendorSpecific,
            0x10 => CapabilityId::PciExpress,
            0x11 => CapabilityId::MsiX,
            other => CapabilityId::Other(other),
        }
    }

    pub fn id(&self) -> u8 {
        match self {
            CapabilityId::PowerManagement => 0x01,
            CapabilityId::Agp => 0x02,
            CapabilityId::Vpd => 0x03,
            CapabilityId::Msi => 0x05,
            CapabilityId::VendorSpecific => 0x09,
            CapabilityId::PciExpress => 0x10,
            CapabilityId::MsiX => 0x11,
            CapabilityId::Other(other) => *other,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            CapabilityId::PowerManagement => "PM",
            CapabilityId::Agp => "AGP",
            CapabilityId::Vpd => "VPD",
            CapabilityId::Msi => "MSI",
            CapabilityId::VendorSpecific => "VNDR",
            CapabilityId::PciExpress => "PCIe",
            CapabilityId::MsiX => "MSI-X",
            CapabilityId::Other(_) => "CAP",
        }
    }
}

/// The kind + geometry of a Base Address Register after probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    /// Unimplemented BAR (reads back as 0).
    Unused,
    /// I/O-space BAR: base port + size in bytes.
    Io { port: u16, size: u32 },
    /// Memory-space BAR: base address + size + attributes.
    Memory {
        addr: u64,
        size: u64,
        prefetchable: bool,
        is_64: bool,
    },
}

impl Bar {
    pub fn is_unused(&self) -> bool {
        matches!(self, Bar::Unused)
    }
}

/// Compute a BAR region size from the size-probe mask (the value read back
/// after writing all-ones to the BAR). `is_io` selects the address-mask width.
/// Returns 0 for an unimplemented BAR (mask has no significant bits).
///
/// The BAR size is the two's-complement of the masked value: the lowest set
/// bit of the decoded mask is the region size.
pub fn bar_size_from_mask(mask: u32, is_io: bool) -> u32 {
    let decoded = if is_io { mask & !0x3 } else { mask & !0xF };
    if decoded == 0 {
        0
    } else {
        (!decoded).wrapping_add(1)
    }
}

/// Combine the low and high 32-bit size masks of a 64-bit memory BAR into a
/// 64-bit region size.
pub fn bar_size64(low_mask: u32, high_mask: u32) -> u64 {
    let combined = ((high_mask as u64) << 32) | ((low_mask & !0xF) as u64);
    if combined == 0 {
        0
    } else {
        (!combined).wrapping_add(1)
    }
}

/// Walk a PCI capability list without allocating: `emit(cap, offset)` is called
/// for each capability. `read(offset)` returns the config dword at a
/// dword-aligned `offset`; `cap_ptr` is the initial capability pointer (config
/// offset 0x34, low byte). The walk is bounded (≤48 steps) and loop-guarded
/// (each offset visited at most once), so a malformed or circular capability
/// list — an adversarial device — always terminates. Returns the count.
///
/// Valid capability pointers are `>= 0x40` and dword-aligned; the low two bits
/// are reserved and masked off before use.
pub fn walk_capabilities(
    cap_ptr: u8,
    read: impl Fn(u8) -> u32,
    mut emit: impl FnMut(CapabilityId, u8),
) -> usize {
    let mut ptr = cap_ptr;
    let mut seen = [false; 256];
    let mut count = 0usize;
    let mut guard = 0u32;
    while ptr >= 0x40 && guard < 48 {
        let aligned = ptr & !0x3;
        if seen[aligned as usize] {
            break; // circular list — stop.
        }
        seen[aligned as usize] = true;
        let word = read(aligned);
        let id = (word & 0xFF) as u8;
        let next = ((word >> 8) & 0xFF) as u8;
        emit(CapabilityId::from_u8(id), aligned);
        count += 1;
        if next == 0 {
            break; // end of list.
        }
        ptr = next;
        guard += 1;
    }
    count
}

/// Decode a BAR value into (address, is_mmio, is_64bit).
pub fn decode_bar(bar: u32) -> (u64, bool, bool) {
    if bar & 1 == 1 {
        // I/O space BAR.
        ((bar & !0x3) as u64, false, false)
    } else {
        let is_64 = (bar >> 1) & 0x3 == 0x2;
        ((bar & !0xF) as u64, true, is_64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_nvme_identity() {
        // vendor 0x1b36 (Red Hat), device 0x0010, class 01/08/02, header 0.
        let id = PciId::decode(0x0010_1b36, 0x0108_0200, 0x0000_0000);
        assert_eq!(id.vendor, 0x1b36);
        assert_eq!(id.device, 0x0010);
        assert_eq!(id.class, 0x01);
        assert_eq!(id.subclass, 0x08);
        assert_eq!(id.prog_if, 0x02);
        assert!(id.is_storage());
        assert_eq!(id.class_name(), "NVMe controller");
        assert!(id.present());
        assert!(!id.is_multifunction());
    }

    #[test]
    fn detects_absent_device() {
        let id = PciId::decode(0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF);
        assert!(!id.present());
    }

    #[test]
    fn detects_multifunction_and_bridge() {
        let id = PciId::decode(0x1237_8086, 0x0600_0000, 0x0080_0000);
        assert_eq!(id.class_name(), "host bridge");
        assert!(id.is_multifunction());
    }

    #[test]
    fn decodes_bars() {
        assert_eq!(decode_bar(0xfebf_0004), (0xfebf_0000, true, true));
        assert_eq!(decode_bar(0xfebf_0000), (0xfebf_0000, true, false));
        assert_eq!(decode_bar(0x0000_c001), (0xc000, false, false));
    }

    #[test]
    fn decodes_capability_ids() {
        assert_eq!(CapabilityId::from_u8(0x05), CapabilityId::Msi);
        assert_eq!(CapabilityId::from_u8(0x11), CapabilityId::MsiX);
        assert_eq!(CapabilityId::from_u8(0x10), CapabilityId::PciExpress);
        assert_eq!(CapabilityId::from_u8(0x01), CapabilityId::PowerManagement);
        assert_eq!(CapabilityId::Msi.name(), "MSI");
        assert_eq!(CapabilityId::MsiX.id(), 0x11);
        assert_eq!(CapabilityId::from_u8(0x42), CapabilityId::Other(0x42));
        assert_eq!(CapabilityId::from_u8(0x42).id(), 0x42);
    }

    #[test]
    fn decodes_usb_kind() {
        assert_eq!(UsbKind::from_prog_if(0x00), UsbKind::Uhci);
        assert_eq!(UsbKind::from_prog_if(0x20), UsbKind::Ehci);
        assert_eq!(UsbKind::from_prog_if(0x30), UsbKind::Xhci);
        assert_eq!(UsbKind::Xhci.name(), "xHCI");
        // A UHCI USB controller identity (class 0C, subclass 03, prog-if 00).
        let id = PciId::decode(0x7020_8086, 0x0c03_0000, 0x0000_0000);
        assert!(id.is_usb());
        assert_eq!(id.usb_kind(), UsbKind::Uhci);
    }

    #[test]
    fn identifies_audio() {
        // AC97 (Intel 82801AA): class 0x04, subclass 0x01.
        let id = PciId::decode(0x2415_8086, 0x0401_0000, 0x0000_0000);
        assert!(id.is_audio());
        assert_eq!(id.class_name(), "multimedia audio controller");
    }

    #[test]
    fn computes_bar_sizes() {
        // A 4 KiB MMIO BAR probes back as 0xFFFF_F000 (low 12 bits clear).
        assert_eq!(bar_size_from_mask(0xFFFF_F000, false), 0x1000);
        // A 256-byte I/O BAR probes back as 0xFFFF_FF00.
        assert_eq!(bar_size_from_mask(0xFFFF_FF00, true), 0x100);
        // A 32-byte I/O BAR (AC97 NABM) probes back as 0xFFFF_FFC0.
        assert_eq!(bar_size_from_mask(0xFFFF_FFC0, true), 0x40);
        // Unimplemented BAR.
        assert_eq!(bar_size_from_mask(0x0000_0000, false), 0);
        // 64-bit BAR of 16 MiB across two registers.
        assert_eq!(bar_size64(0xFF00_0000, 0xFFFF_FFFF), 0x0100_0000);
    }

    /// Build a fake config-space cap chain: map dword-offset -> (id, next).
    fn cap_reader(map: &[(u8, u8, u8)]) -> impl Fn(u8) -> u32 + '_ {
        move |off: u8| {
            for &(o, id, next) in map {
                if o == off {
                    return (id as u32) | ((next as u32) << 8);
                }
            }
            0
        }
    }

    #[test]
    fn walks_normal_cap_chain() {
        // 0x40:PM->0x50, 0x50:MSI->0x60, 0x60:MSI-X->end.
        let map = [(0x40, 0x01, 0x50), (0x50, 0x05, 0x60), (0x60, 0x11, 0x00)];
        let mut got = Vec::new();
        let n = walk_capabilities(0x40, cap_reader(&map), |c, off| got.push((c, off)));
        assert_eq!(n, 3);
        assert_eq!(
            got,
            [
                (CapabilityId::PowerManagement, 0x40),
                (CapabilityId::Msi, 0x50),
                (CapabilityId::MsiX, 0x60),
            ]
        );
    }

    #[test]
    fn cap_walk_terminates_on_circular_list() {
        // 0x40 -> 0x50 -> 0x40 (loop). Must stop after visiting each once.
        let map = [(0x40, 0x05, 0x50), (0x50, 0x11, 0x40)];
        let mut count = 0;
        let n = walk_capabilities(0x40, cap_reader(&map), |_, _| count += 1);
        assert_eq!(n, 2);
        assert_eq!(count, 2);
    }

    #[test]
    fn cap_walk_terminates_on_self_loop() {
        // 0x40 -> 0x40 (points to itself).
        let map = [(0x40, 0x05, 0x40)];
        let n = walk_capabilities(0x40, cap_reader(&map), |_, _| {});
        assert_eq!(n, 1);
    }

    #[test]
    fn cap_walk_rejects_out_of_range_pointer() {
        // A next pointer below 0x40 is invalid; the walk stops.
        let map = [(0x40, 0x05, 0x20)];
        let n = walk_capabilities(0x40, cap_reader(&map), |_, _| {});
        assert_eq!(n, 1);
        // An initial pointer of 0 (no capabilities) yields nothing.
        assert_eq!(walk_capabilities(0x00, cap_reader(&map), |_, _| {}), 0);
    }
}
