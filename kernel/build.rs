//! Kernel build script: packs `initramfs/root/` plus a generated
//! `/etc/version` file into a deterministic ustar archive embedded into the
//! kernel via `include_bytes!`.
//!
//! Determinism: entries are sorted by path, all metadata fields are fixed
//! (mtime 0, mode 0644/0755), so the archive bytes depend only on content.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BLOCK: usize = 512;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest_dir.join("..");
    let root = manifest_dir.join("../initramfs/root");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("initramfs.tar");
    println!("cargo::rerun-if-changed={}", root.display());
    for dir in [
        "ulib",
        "init",
        "gp-test",
        "pf-test",
        "child",
        "parent",
        "spin-finite",
        "spin-forever",
        "gui-demo",
    ] {
        println!(
            "cargo::rerun-if-changed={}",
            workspace.join("user").join(dir).display()
        );
    }

    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new();
    collect(&root, "", &mut entries);
    // Generated version file — single source: the crate version.
    entries.push((
        "etc/version".to_string(),
        format!("{}\n", std::env::var("CARGO_PKG_VERSION").unwrap()).into_bytes(),
        false,
    ));
    build_user_programs(&workspace, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);

    let mut archive: Vec<u8> = Vec::new();
    for (name, data, is_dir) in &entries {
        archive.extend_from_slice(&header(name, data.len(), *is_dir));
        archive.extend_from_slice(data);
        while !archive.len().is_multiple_of(BLOCK) {
            archive.push(0);
        }
    }
    archive.extend_from_slice(&[0u8; BLOCK * 2]);
    fs::write(&out, archive).unwrap();
}

/// Build the Ring 3 programs with a nested cargo invocation and add their
/// ELFs (plus deliberately corrupted negative-test fixtures) to the
/// initramfs under /bin.
///
/// A separate `--target-dir` avoids deadlocking on the outer build's lock,
/// and a separate invocation keeps user-crate feature resolution isolated
/// (same rule as the kernel itself — see workspace Cargo.toml).
fn build_user_programs(workspace: &Path, entries: &mut Vec<(String, Vec<u8>, bool)>) {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target_dir = workspace.join("target-user");
    let status = Command::new(&cargo)
        .args([
            "build",
            "-p",
            "user-init",
            "-p",
            "user-gp-test",
            "-p",
            "user-pf-test",
            "-p",
            "user-child",
            "-p",
            "user-parent",
            "-p",
            "user-spin-finite",
            "-p",
            "user-spin-forever",
            "-p",
            "user-gui-demo",
            "-p",
            "user-lsdev",
        ])
        .arg("--target-dir")
        .arg(&target_dir)
        .current_dir(workspace)
        // x86_64-unknown-none defaults to PIE (ET_DYN); the V0.2 loader
        // deliberately accepts only static ET_EXEC images.
        .env(
            "RUSTFLAGS",
            "-C relocation-model=static -C link-arg=--no-pie",
        )
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("running nested cargo for user programs");
    assert!(status.success(), "user program build failed");

    let bin_dir = target_dir.join("x86_64-unknown-none/debug");
    let programs = [
        ("user-init", "bin/init"),
        ("user-gp-test", "bin/gp-test"),
        ("user-pf-test", "bin/pf-test"),
        ("user-child", "bin/child"),
        ("user-parent", "bin/parent"),
        ("user-spin-finite", "bin/spin-finite"),
        ("user-spin-forever", "bin/spin-forever"),
        ("user-gui-demo", "bin/gui-demo"),
        ("user-lsdev", "bin/lsdev"),
    ];
    entries.push(("bin/".to_string(), Vec::new(), true));
    for (artifact, dest) in programs {
        let bytes = fs::read(bin_dir.join(artifact))
            .unwrap_or_else(|e| panic!("reading user ELF {artifact}: {e}"));
        entries.push((dest.to_string(), bytes, false));
    }

    // Negative-test fixtures derived deterministically from the real init:
    // /bin/broken — corrupted ELF magic (loader must reject structurally);
    // /bin/wx-test — first executable PT_LOAD also marked writable (loader
    // must reject by W^X policy).
    let init = fs::read(bin_dir.join("user-init")).unwrap();
    let mut broken = init.clone();
    broken[0] ^= 0xFF;
    entries.push(("bin/broken".to_string(), broken, false));

    let mut wx = init;
    patch_first_exec_segment_writable(&mut wx);
    entries.push(("bin/wx-test".to_string(), wx, false));
}

/// Set PF_W on the first executable PT_LOAD program header (ELF64 layout).
fn patch_first_exec_segment_writable(elf: &mut [u8]) {
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let p_type = u32::from_le_bytes(elf[ph..ph + 4].try_into().unwrap());
        let flags_off = ph + 4;
        let flags = u32::from_le_bytes(elf[flags_off..flags_off + 4].try_into().unwrap());
        if p_type == 1 && flags & 1 != 0 {
            elf[flags_off..flags_off + 4].copy_from_slice(&(flags | 2).to_le_bytes());
            return;
        }
    }
    panic!("no executable PT_LOAD found in user-init to patch");
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>, bool)>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let ftype = entry.file_type().unwrap();
        if ftype.is_dir() {
            out.push((format!("{path}/"), Vec::new(), true));
            collect(&entry.path(), &path, out);
        } else if ftype.is_file() {
            let mut data = fs::read(entry.path()).unwrap();
            // Normalize line endings so the archive is identical regardless
            // of the git checkout's CRLF settings.
            if data.contains(&b'\r') {
                data.retain(|&b| b != b'\r');
            }
            out.push((path, data, false));
        }
    }
}

fn header(name: &str, size: usize, is_dir: bool) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    assert!(name.len() < 100, "initramfs path too long: {name}");
    h[0..name.len()].copy_from_slice(name.as_bytes());
    let mode: &[u8; 7] = if is_dir { b"0000755" } else { b"0000644" };
    h[100..107].copy_from_slice(mode);
    h[108..115].copy_from_slice(b"0000000"); // uid
    h[116..123].copy_from_slice(b"0000000"); // gid
    let size_field = format!("{size:011o}");
    h[124..135].copy_from_slice(size_field.as_bytes());
    h[136..147].copy_from_slice(b"00000000000"); // mtime 0
    h[156] = if is_dir { b'5' } else { b'0' };
    h[257..262].copy_from_slice(b"ustar");
    h[263..265].copy_from_slice(b"00");
    for b in &mut h[148..156] {
        *b = b' ';
    }
    let sum: u64 = h.iter().map(|&b| b as u64).sum();
    let chk = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(chk.as_bytes());
    h
}
