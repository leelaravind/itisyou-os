//! Builds bootable disk images for every kernel variant.
//!
//! Usage: `cargo run -p image-builder -- [output-dir]` (default `target/images`).
//! Produces `<variant>-bios.img` and `<variant>-uefi.img` plus a small
//! manifest with sizes and SHA-256 checksums for release evidence.

use anyhow::{bail, Context, Result};
use bootloader::DiskImageBuilder;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const VARIANTS: [&str; 4] = [
    "itisyou-kernel",
    "itisyou-kernel-selftest",
    "itisyou-kernel-panictest",
    "itisyou-fs-persist",
];

fn main() -> Result<()> {
    let out_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/images".to_string());
    let out_dir = PathBuf::from(out_dir);
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("creating output dir {}", out_dir.display()))?;

    // Workspace root, independent of the invocation directory.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .context("resolving workspace root")?;

    // Build the kernel in its own cargo invocation (see Cargo.toml note).
    // `strip=debuginfo`: the BIOS bootloader stages read the ENTIRE kernel
    // ELF off disk through real-mode INT 13h calls before the kernel prints
    // its first byte, so DWARF sections are paid for in boot wall-clock on
    // every QEMU leg. Nothing at runtime consumes them (no unwinding, no
    // symbolication — panic=abort), so stripping is behaviour-preserving and
    // keeps first-serial-output well inside the harness deadlines.
    let status = Command::new("cargo")
        .args(["build", "-p", "itisyou-kernel"])
        .current_dir(&root)
        .env("RUSTFLAGS", "-C strip=debuginfo")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .context("running cargo build for the kernel")?;
    if !status.success() {
        bail!("kernel build failed with {status}");
    }

    let bin_dir = root.join("target/x86_64-unknown-none/debug");
    let mut manifest = String::new();
    for name in VARIANTS {
        let kernel_path = bin_dir.join(name);
        if !kernel_path.is_file() {
            bail!("expected kernel binary missing: {}", kernel_path.display());
        }
        let builder = DiskImageBuilder::new(kernel_path.clone());

        let bios = out_dir.join(format!("{name}-bios.img"));
        builder
            .create_bios_image(&bios)
            .with_context(|| format!("building BIOS image for {name}"))?;
        record(&mut manifest, name, "bios", &bios)?;

        #[cfg(feature = "uefi")]
        {
            let uefi = out_dir.join(format!("{name}-uefi.img"));
            builder
                .create_uefi_image(&uefi)
                .with_context(|| format!("building UEFI image for {name}"))?;
            record(&mut manifest, name, "uefi", &uefi)?;
        }
    }
    #[cfg(not(feature = "uefi"))]
    println!(
        "note: UEFI images skipped — bootloader UEFI stage blocked upstream \
         (rust-osdev/bootloader#579); BIOS images only"
    );

    let manifest_path = out_dir.join("manifest.txt");
    fs::write(&manifest_path, &manifest)?;
    print!("{manifest}");
    println!("manifest: {}", manifest_path.display());
    Ok(())
}

fn record(manifest: &mut String, name: &str, kind: &str, path: &Path) -> Result<()> {
    let data = fs::read(path)?;
    let digest = sha256_hex(&data);
    writeln!(
        manifest,
        "{name}-{kind}.img size={} sha256={digest}",
        data.len()
    )?;
    Ok(())
}

/// Minimal SHA-256 (FIPS 180-4) so the manifest needs no extra dependencies.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h = [
            h[0].wrapping_add(a),
            h[1].wrapping_add(b),
            h[2].wrapping_add(c),
            h[3].wrapping_add(d),
            h[4].wrapping_add(e),
            h[5].wrapping_add(f),
            h[6].wrapping_add(g),
            h[7].wrapping_add(hh),
        ];
    }
    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(out, "{word:08x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
