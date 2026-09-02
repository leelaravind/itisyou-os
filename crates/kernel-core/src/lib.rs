//! Pure kernel logic shared between the kernel (no_std) and host-side tools.
//!
//! Everything in this crate must stay allocation-free and side-effect-free so
//! it can be unit-tested on the host and reused inside the kernel unchanged.

#![cfg_attr(not(test), no_std)]

pub mod bitmap;
pub mod marker;
pub mod memmap;
pub mod stage;
