//! The AI-native system layer's kernel side (V0.11, ADR-0024).
//!
//! The kernel runs no model and makes no decision from one. It holds the
//! provenance anchor — the SHA-256 of the model the build trained, compiled
//! in — and, as the steps of ADR-0024 land, the approved view (`sys_view`),
//! the proposal table and the console's approval path. Inference itself runs
//! in Ring 3 (`/bin/inferd`).

/// SHA-256 of `/etc/ai/diag.model` as the build trained it (`kernel/build.rs`
/// checks it against the pin `ai/diag.model.sha256`).
pub const MODEL_SHA256: [u8; 32] = *include_bytes!(concat!(env!("OUT_DIR"), "/model_sha256.bin"));

/// Where the model lives in the initramfs.
pub const MODEL_PATH: &str = "/etc/ai/diag.model";

fn hex<'a>(d: &[u8; 32], out: &'a mut [u8; 64]) -> &'a str {
    kernel_core::audit_chain::format_head(d, out)
}

/// Boot check: the model in the initramfs is the one the kernel was built
/// with, and it decodes. Printed, never fatal — without a model the agent
/// cannot diagnose, but nothing else depends on it.
pub fn boot_check() {
    let mut buf = [0u8; 64];
    let compiled = hex(&MODEL_SHA256, &mut buf);
    match crate::fs::read(MODEL_PATH) {
        Ok(bytes) => {
            let found = kernel_core::sha256::digest(bytes);
            let decoded = match kernel_core::model::decode(bytes) {
                Ok(_) => "ok",
                Err(e) => e.name(),
            };
            crate::serial_println!(
                "[ITISYOU:AI] model sha256={compiled} bytes={} initramfs_match={} decode={decoded}",
                bytes.len(),
                found == MODEL_SHA256
            );
        }
        Err(_) => crate::serial_println!(
            "[ITISYOU:AI] model sha256={compiled} initramfs_match=false reason=missing"
        ),
    }
}
