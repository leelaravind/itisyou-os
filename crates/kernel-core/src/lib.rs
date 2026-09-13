//! Pure kernel logic shared between the kernel (no_std) and host-side tools.
//!
//! Everything in this crate must stay allocation-free and side-effect-free so
//! it can be unit-tested on the host and reused inside the kernel unchanged.

#![cfg_attr(not(test), no_std)]
// Host-testable logic has no business with raw pointers or hardware; keeping
// every `unsafe` in the kernel crate keeps docs/UNSAFE_INVENTORY.md complete.
#![forbid(unsafe_code)]

pub mod acpi;
pub mod audit_chain;
pub mod bitmap;
pub mod capability;
pub mod caps;
pub mod ed25519;
pub mod elf;
pub mod font;
pub mod itfs;
pub mod manifest;
pub mod marker;
pub mod memmap;
pub mod mouse;
pub mod net;
pub mod path;
pub mod pci;
pub mod pkg;
pub mod scancode;
pub mod service;
pub mod sha256;
pub mod sha512;
pub mod shellparse;
pub mod stage;
pub mod tar;
pub mod update;
pub mod usb;
