//! PCI bus enumeration via the 0xCF8/0xCFC config mechanism (V0.3).
//!
//! Brute-force scan of bus 0 (QEMU's default q35/pc topology puts the devices
//! we care about there). Decoding lives in `kernel_core::pci`; this module
//! owns only the port I/O.

use alloc::vec::Vec;
use kernel_core::pci::PciId;
use x86_64::instructions::port::Port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

/// A device found during enumeration.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub id: PciId,
}

impl PciDevice {
    /// Read a 32-bit config-space register (offset must be 4-aligned).
    pub fn read_config(&self, offset: u8) -> u32 {
        read_config(self.bus, self.slot, self.func, offset)
    }
}

fn config_address(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

/// Read a 32-bit PCI config register.
pub fn read_config(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    let addr = config_address(bus, slot, func, offset);
    // SAFETY: the 0xCF8/0xCFC config mechanism is the architectural PCI
    // access path; reads have no side effects beyond selecting the register.
    unsafe {
        let mut address_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);
        address_port.write(addr);
        data_port.read()
    }
}

fn probe(bus: u8, slot: u8, func: u8) -> Option<PciId> {
    let id = PciId::decode(
        read_config(bus, slot, func, 0x00),
        read_config(bus, slot, func, 0x08),
        read_config(bus, slot, func, 0x0C),
    );
    id.present().then_some(id)
}

/// Enumerate PCI devices on buses 0..=`max_bus`.
pub fn enumerate(max_bus: u8) -> Vec<PciDevice> {
    let mut out = Vec::new();
    for bus in 0..=max_bus {
        for slot in 0..32 {
            let Some(id0) = probe(bus, slot, 0) else {
                continue;
            };
            let funcs = if id0.is_multifunction() { 8 } else { 1 };
            for func in 0..funcs {
                if let Some(id) = probe(bus, slot, func) {
                    out.push(PciDevice {
                        bus,
                        slot,
                        func,
                        id,
                    });
                }
            }
        }
    }
    out
}

/// Enumerate and log every device; return the list (evidence for B160).
pub fn scan_and_report() -> Vec<PciDevice> {
    let devices = enumerate(0);
    crate::serial_println!("[ITISYOU:INFO] pci_devices count={}", devices.len());
    for d in &devices {
        crate::serial_println!(
            "[ITISYOU:INFO] pci {:02x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#04x} subclass={:#04x} name=\"{}\"",
            d.bus,
            d.slot,
            d.func,
            d.id.vendor,
            d.id.device,
            d.id.class,
            d.id.subclass,
            d.id.class_name(),
        );
    }
    devices
}

/// First NVMe controller (class 0x01, subclass 0x08), if any.
pub fn find_nvme(devices: &[PciDevice]) -> Option<PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.id.class == 0x01 && d.id.subclass == 0x08)
}

/// First storage controller of any kind — NVMe preferred over legacy IDE.
pub fn find_storage(devices: &[PciDevice]) -> Option<PciDevice> {
    find_nvme(devices).or_else(|| devices.iter().copied().find(|d| d.id.is_storage()))
}
