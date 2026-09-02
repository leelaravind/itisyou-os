//! Kernel build script: packs `initramfs/root/` plus a generated
//! `/etc/version` file into a deterministic ustar archive embedded into the
//! kernel via `include_bytes!`.
//!
//! Determinism: entries are sorted by path, all metadata fields are fixed
//! (mtime 0, mode 0644/0755), so the archive bytes depend only on content.

use std::fs;
use std::path::{Path, PathBuf};

const BLOCK: usize = 512;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest_dir.join("../initramfs/root");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("initramfs.tar");
    println!("cargo::rerun-if-changed={}", root.display());

    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new();
    collect(&root, "", &mut entries);
    // Generated version file — single source: the crate version.
    entries.push((
        "etc/version".to_string(),
        format!("{}\n", std::env::var("CARGO_PKG_VERSION").unwrap()).into_bytes(),
        false,
    ));
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
