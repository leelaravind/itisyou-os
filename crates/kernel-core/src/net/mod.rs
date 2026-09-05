//! Network protocol parsing and construction — the host-testable half of the
//! stack (V0.8).
//!
//! Everything here follows the pattern the rest of this crate uses for
//! untrusted input: the parsers borrow from the caller's DMA buffer, allocate
//! nothing, and refuse anything they do not fully understand rather than
//! guessing. A frame arriving from the wire is the most hostile input this
//! kernel accepts, so each layer validates its own header before exposing a
//! payload to the next one, and every rejection has a distinct error variant
//! the driver can count and log instead of a silent drop.
//!
//! Splitting the protocol logic out of the driver is what makes it testable:
//! these modules run under `cargo test` on the host against hand-built byte
//! sequences — including truncated, oversized, malformed and hostile ones —
//! with no emulator, no NIC and no kernel in the loop.

pub mod arp;
pub mod checksum;
pub mod dns;
pub mod eth;
pub mod icmp;
pub mod ipv4;
pub mod udp;
