//! Boot-stage marker emission (contract defined in `kernel_core::stage`).

use kernel_core::stage::Stage;

/// Emit the canonical serial marker for a boot stage.
pub fn emit(stage: Stage) {
    crate::serial_println!("[ITISYOU:{}] {}", stage.code(), stage.describe());
}
