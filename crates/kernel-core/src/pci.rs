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
}
