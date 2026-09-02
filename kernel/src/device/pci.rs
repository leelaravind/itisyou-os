//! PCI bus enumeration via the 0xCF8/0xCFC config mechanism (V0.3).
//!
//! Brute-force scan of bus 0 (QEMU's default q35/pc topology puts the devices
//! we care about there). Decoding lives in `kernel_core::pci`; this module
//! owns only the port I/O.

use alloc::vec::Vec;
use kernel_core::pci::{bar_size64, bar_size_from_mask, decode_bar, Bar, CapabilityId, PciId};
use x86_64::instructions::port::Port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

/// Config-space offsets.
const CMD_STATUS: u8 = 0x04;
const CAP_PTR: u8 = 0x34;
const BAR0: u8 = 0x10;

/// Command register bits.
pub const CMD_IO_SPACE: u16 = 1 << 0;
pub const CMD_MEM_SPACE: u16 = 1 << 1;
pub const CMD_BUS_MASTER: u16 = 1 << 2;

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

    /// Write a 32-bit config-space register (offset must be 4-aligned).
    pub fn write_config(&self, offset: u8, value: u32) {
        write_config(self.bus, self.slot, self.func, offset, value);
    }

    /// Read the 16-bit command register.
    pub fn command(&self) -> u16 {
        (self.read_config(CMD_STATUS) & 0xFFFF) as u16
    }

    /// Set bits in the command register (e.g. enable bus master + I/O decode),
    /// preserving the status half of the dword.
    pub fn enable_command_bits(&self, bits: u16) {
        let dword = self.read_config(CMD_STATUS);
        let new_cmd = (dword & 0xFFFF) | bits as u32;
        // Writing 1s to status bits is either no-op or write-1-to-clear; write
        // 0 to the status half so we never accidentally clear a live status.
        self.write_config(CMD_STATUS, new_cmd & 0x0000_FFFF);
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

/// Write a 32-bit PCI config register.
pub fn write_config(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    let addr = config_address(bus, slot, func, offset);
    // SAFETY: 0xCF8/0xCFC config mechanism; writes target the selected register
    // only. Callers restrict writes to the command register and to BAR sizing
    // (original value always restored).
    unsafe {
        let mut address_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);
        address_port.write(addr);
        data_port.write(value);
    }
}

/// Probe all six BARs of a device, returning their kind + size. The standard
/// sizing procedure (write all-ones, read back the mask, restore) is performed
/// with I/O + memory decode disabled so the device cannot respond to a
/// transient bogus address; the command register is restored afterwards.
pub fn probe_bars(dev: &PciDevice) -> [Bar; 6] {
    let mut bars = [Bar::Unused; 6];
    // Only header-type-0 devices have six BARs; bridges (type 1) have two.
    if dev.id.header_type & 0x7F != 0x00 {
        return bars;
    }
    let saved_cmd = dev.read_config(CMD_STATUS);
    dev.write_config(
        CMD_STATUS,
        saved_cmd & !((CMD_IO_SPACE | CMD_MEM_SPACE) as u32),
    );

    let mut i = 0usize;
    while i < 6 {
        let off = BAR0 + (i as u8) * 4;
        let orig = dev.read_config(off);
        dev.write_config(off, 0xFFFF_FFFF);
        let mask = dev.read_config(off);
        dev.write_config(off, orig);

        if orig == 0 && mask == 0 {
            i += 1;
            continue;
        }
        let (base, is_mmio, is_64) = decode_bar(orig);
        if !is_mmio {
            let size = bar_size_from_mask(mask, true);
            bars[i] = Bar::Io {
                port: (base & 0xFFFF) as u16,
                size,
            };
            i += 1;
        } else if is_64 {
            let off_hi = off + 4;
            let orig_hi = dev.read_config(off_hi);
            dev.write_config(off_hi, 0xFFFF_FFFF);
            let mask_hi = dev.read_config(off_hi);
            dev.write_config(off_hi, orig_hi);
            let addr = ((orig_hi as u64) << 32) | (base & 0xFFFF_FFF0);
            bars[i] = Bar::Memory {
                addr,
                size: bar_size64(mask, mask_hi),
                prefetchable: orig & 0x8 != 0,
                is_64: true,
            };
            i += 2; // a 64-bit BAR consumes two slots
        } else {
            bars[i] = Bar::Memory {
                addr: base,
                size: bar_size_from_mask(mask, false) as u64,
                prefetchable: orig & 0x8 != 0,
                is_64: false,
            };
            i += 1;
        }
    }
    dev.write_config(CMD_STATUS, saved_cmd);
    bars
}

/// Walk the config-space capability list, returning `(capability, offset)`
/// pairs. Bounded and loop-guarded so a malformed/circular list cannot hang.
pub fn capabilities(dev: &PciDevice) -> Vec<(CapabilityId, u8)> {
    let mut caps = Vec::new();
    if !dev.id.has_cap_list_layout() {
        return caps;
    }
    // Status register bit 4 indicates a capability list is present.
    let status = (dev.read_config(CMD_STATUS) >> 16) as u16;
    if status & (1 << 4) == 0 {
        return caps;
    }
    let ptr = (dev.read_config(CAP_PTR) & 0xFF) as u8;
    // Bounded, loop-guarded walk lives in host-tested kernel_core.
    kernel_core::pci::walk_capabilities(
        ptr,
        |off| dev.read_config(off),
        |cap, off| caps.push((cap, off)),
    );
    caps
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
