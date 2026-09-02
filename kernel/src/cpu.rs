//! CPU identification (boot stage B030).

use raw_cpuid::CpuId;

/// Log CPU vendor/brand and the baseline features V0.1 cares about.
pub fn report_baseline() {
    let cpuid = CpuId::new();

    let vendor = cpuid.get_vendor_info();
    let vendor = vendor.as_ref().map(|v| v.as_str()).unwrap_or("unknown");

    if let Some(brand) = cpuid.get_processor_brand_string() {
        crate::serial_println!(
            "[ITISYOU:INFO] cpu_vendor={vendor} cpu_brand=\"{}\"",
            brand.as_str().trim()
        );
    } else {
        crate::serial_println!("[ITISYOU:INFO] cpu_vendor={vendor}");
    }

    if let Some(features) = cpuid.get_feature_info() {
        crate::serial_println!(
            "[ITISYOU:INFO] cpu_features apic={} sse2={} tsc={}",
            features.has_apic(),
            features.has_sse2(),
            features.has_tsc(),
        );
    }
}
