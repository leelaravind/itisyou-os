//! Device layer.
//!
//! V0.3 introduced PCI enumeration + a read-only NVMe driver. V0.6 adds a
//! generic device model on top: every PCI function is probed into a [`Device`]
//! (identity + sized BARs + capability list), registered drivers are matched
//! against it (`probe` → `attach`), and the result is a queryable device table
//! surfaced to diagnostics, the shell (`lsdev`), and a userspace syscall.
//!
//! Drivers stay cleanly separated from the kernel core: each implements the
//! [`Driver`] trait and is listed in [`DRIVERS`]; the model owns discovery,
//! binding, and the device table, never a specific device's registers.

pub mod ac97;
pub mod block;
pub mod nvme;
pub mod pci;

use alloc::vec::Vec;
use kernel_core::pci::{Bar, CapabilityId, PciId};
use pci::PciDevice;
use spin::Mutex;

/// A fully-probed hardware device.
pub struct Device {
    pub pci: PciDevice,
    pub bars: [Bar; 6],
    pub caps: Vec<(CapabilityId, u8)>,
    /// Name of the driver bound to this device, if any.
    pub driver: Option<&'static str>,
}

impl Device {
    pub fn id(&self) -> PciId {
        self.pci.id
    }

    pub fn has_cap(&self, cap: CapabilityId) -> bool {
        self.caps.iter().any(|(c, _)| *c == cap)
    }

    /// The first implemented I/O BAR as `(port, size)`.
    pub fn first_io_bar(&self) -> Option<(u16, u32)> {
        self.bars.iter().find_map(|b| match b {
            Bar::Io { port, size } if *size > 0 => Some((*port, *size)),
            _ => None,
        })
    }

    /// The first implemented memory BAR as `(addr, size)`.
    pub fn first_mem_bar(&self) -> Option<(u64, u64)> {
        self.bars.iter().find_map(|b| match b {
            Bar::Memory { addr, size, .. } if *size > 0 => Some((*addr, *size)),
            _ => None,
        })
    }
}

/// Why a driver declined or failed to attach to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    /// The driver's `probe` matched but the device is not usable as expected.
    Unsupported,
    /// Hardware initialization failed with a reason.
    InitFailed(&'static str),
}

/// A device driver: it declares whether it handles a device (`probe`) and, if
/// so, initializes it (`attach`). Must be `Sync` because instances are static.
pub trait Driver: Sync {
    fn name(&self) -> &'static str;
    /// Does this driver handle `dev`? Must be side-effect-free.
    fn probe(&self, dev: &Device) -> bool;
    /// Initialize the device. Called at most once per matching device.
    fn attach(&self, dev: &Device) -> Result<(), DriverError>;
}

/// Registered drivers, probed in order against each device. Kept small and
/// explicit — no dynamic registration — so binding is deterministic. Drivers
/// are appended here as they land (USB, ...).
static DRIVERS: &[&dyn Driver] = &[&ac97::AC97_DRIVER];

/// The device table, populated by [`enumerate`].
static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());

/// Enumerate PCI (bus 0 — the QEMU `pc`/i440fx machine places every device
/// there), probe each function's BARs + capabilities, and store the table.
/// Returns the device count.
pub fn enumerate() -> usize {
    let mut devices = Vec::new();
    for p in pci::enumerate(0) {
        devices.push(Device {
            pci: p,
            bars: pci::probe_bars(&p),
            caps: pci::capabilities(&p),
            driver: None,
        });
    }
    let count = devices.len();
    *DEVICES.lock() = devices;
    count
}

/// Probe every registered driver against every device; attach the first match
/// and record the binding. Returns the number of devices bound.
pub fn bind_drivers() -> usize {
    let mut bound = 0;
    let mut table = DEVICES.lock();
    for dev in table.iter_mut() {
        if dev.driver.is_some() {
            continue;
        }
        for drv in DRIVERS {
            if drv.probe(dev) {
                match drv.attach(dev) {
                    Ok(()) => {
                        dev.driver = Some(drv.name());
                        bound += 1;
                        crate::serial_println!(
                            "[ITISYOU:INFO] driver_bound name=\"{}\" dev={:02x}:{:02x}.{}",
                            drv.name(),
                            dev.pci.bus,
                            dev.pci.slot,
                            dev.pci.func,
                        );
                    }
                    Err(e) => {
                        crate::serial_println!(
                            "[ITISYOU:INFO] driver_attach_failed name=\"{}\" dev={:02x}:{:02x}.{} err={:?}",
                            drv.name(),
                            dev.pci.bus,
                            dev.pci.slot,
                            dev.pci.func,
                            e,
                        );
                    }
                }
                break;
            }
        }
    }
    bound
}

/// Emit one structured diagnostic line per device (B190 evidence).
pub fn report() {
    let table = DEVICES.lock();
    crate::serial_println!("[ITISYOU:INFO] devmodel devices={}", table.len());
    for d in table.iter() {
        let mut caps = alloc::string::String::new();
        for (i, (c, _)) in d.caps.iter().enumerate() {
            if i > 0 {
                caps.push(',');
            }
            caps.push_str(c.name());
        }
        crate::serial_println!(
            "[ITISYOU:INFO] dev {:02x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#04x}/{:#04x} name=\"{}\" caps=[{}] driver={}",
            d.pci.bus,
            d.pci.slot,
            d.pci.func,
            d.id().vendor,
            d.id().device,
            d.id().class,
            d.id().subclass,
            d.id().class_name(),
            caps,
            d.driver.unwrap_or("none"),
        );
    }
}

/// Run the full device-model bring-up (B190): enumerate, report, bind drivers.
pub fn init() -> usize {
    let n = enumerate();
    report();
    let bound = bind_drivers();
    crate::serial_println!("[ITISYOU:INFO] devmodel bound_drivers={bound}");
    n
}

/// Borrow the device table (for the shell / devinfo syscall).
pub fn with_devices<R>(f: impl FnOnce(&[Device]) -> R) -> R {
    f(&DEVICES.lock())
}

pub fn device_count() -> usize {
    DEVICES.lock().len()
}
