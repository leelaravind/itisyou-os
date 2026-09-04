//! Pure kernel logic shared between the kernel (no_std) and host-side tools.
//!
//! Everything in this crate must stay allocation-free and side-effect-free so
//! it can be unit-tested on the host and reused inside the kernel unchanged.

#![cfg_attr(not(test), no_std)]

pub mod bitmap;
pub mod capability;
pub mod caps;
pub mod elf;
pub mod font;
pub mod itfs;
pub mod manifest;
pub mod marker;
pub mod memmap;
pub mod mouse;
pub mod path;
pub mod pci;
pub mod pkg;
pub mod scancode;
pub mod service;
pub mod sha256;
pub mod shellparse;
pub mod stage;
pub mod tar;
pub mod update;
pub mod usb;
