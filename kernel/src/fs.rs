//! VFS abstraction + read-only in-memory initramfs (B110).
//!
//! The initramfs is a ustar archive packed deterministically at build time
//! (kernel/build.rs) and embedded in the kernel image. Parsing goes through
//! the strictly-validating reader in `kernel_core::tar`; paths go through
//! `kernel_core::path` so traversal cannot escape the root (plan §10.9).

use crate::sync::Mutex;
use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use kernel_core::{path as kpath, tar};

static INITRAMFS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/initramfs.tar"));

/// A directory entry returned by [`list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    NotAFile,
    NotADirectory,
    InvalidPath(kpath::PathError),
    NotInitialized,
}

enum Node {
    File(&'static [u8]),
    Dir,
}

/// Canonical-path → node map. Keys never have leading/trailing slashes;
/// the root is the implicit empty key.
static ROOT: Mutex<Option<BTreeMap<String, Node>>> = Mutex::new(None);

/// Parse and mount the embedded initramfs. A malformed embedded archive is a
/// build-system invariant violation → panic (never mount corrupt data).
pub fn init() -> (usize, usize) {
    let mut map: BTreeMap<String, Node> = BTreeMap::new();
    let mut files = 0;
    let mut dirs = 0;
    for entry in tar::entries(INITRAMFS) {
        let entry = entry.expect("embedded initramfs is malformed (build bug)");
        let name = entry.name.trim_matches('/');
        if name.is_empty() {
            continue;
        }
        // Ensure parent directories exist even without explicit dir entries.
        let mut prefix = String::new();
        for comp in name.split('/').take(name.split('/').count() - 1) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(comp);
            if !map.contains_key(&prefix) {
                map.insert(prefix.clone(), Node::Dir);
                dirs += 1;
            }
        }
        if entry.is_dir {
            if !map.contains_key(name) {
                map.insert(name.to_owned(), Node::Dir);
                dirs += 1;
            }
        } else {
            map.insert(name.to_owned(), Node::File(entry.data));
            files += 1;
        }
    }
    *ROOT.lock() = Some(map);
    (files, dirs)
}

/// Canonicalize an absolute path to the internal key form.
fn canonical(path: &str) -> Result<String, FsError> {
    kpath::validate(path).map_err(FsError::InvalidPath)?;
    let mut key = String::new();
    for comp in kpath::normalized(path) {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(comp);
    }
    Ok(key)
}

/// The absolute canonical form of `path` (`/bin/../bin//tickd` is
/// `/bin/tickd`): what the kernel records and prints for a program, never the
/// string a program passed (V0.11 review - a spawn path's popped components
/// could carry a line break and a forged marker into `ps`).
pub fn canonical_path(path: &str) -> Result<String, FsError> {
    let mut key = canonical(path)?;
    key.insert(0, '/');
    Ok(key)
}

/// Read a file's full contents.
pub fn read(path: &str) -> Result<&'static [u8], FsError> {
    let key = canonical(path)?;
    let guard = ROOT.lock();
    let map = guard.as_ref().ok_or(FsError::NotInitialized)?;
    match map.get(&key) {
        Some(Node::File(data)) => Ok(data),
        Some(Node::Dir) => Err(FsError::NotAFile),
        None => Err(FsError::NotFound),
    }
}

/// List a directory's immediate children.
pub fn list(path: &str) -> Result<Vec<DirEntry>, FsError> {
    let key = canonical(path)?;
    let guard = ROOT.lock();
    let map = guard.as_ref().ok_or(FsError::NotInitialized)?;

    if !key.is_empty() {
        match map.get(&key) {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(FsError::NotADirectory),
            None => return Err(FsError::NotFound),
        }
    }
    let prefix = if key.is_empty() {
        String::new()
    } else {
        let mut p = key.clone();
        p.push('/');
        p
    };
    let mut out = Vec::new();
    for (k, node) in map.iter() {
        if let Some(rest) = k.strip_prefix(&prefix) {
            if rest.is_empty() || rest.contains('/') {
                continue;
            }
            out.push(DirEntry {
                name: rest.to_owned(),
                is_dir: matches!(node, Node::Dir),
                size: match node {
                    Node::File(d) => d.len(),
                    Node::Dir => 0,
                },
            });
        }
    }
    Ok(out)
}

/// Metadata query: (is_dir, size).
pub fn stat(path: &str) -> Result<(bool, usize), FsError> {
    let key = canonical(path)?;
    let guard = ROOT.lock();
    let map = guard.as_ref().ok_or(FsError::NotInitialized)?;
    if key.is_empty() {
        return Ok((true, 0));
    }
    match map.get(&key) {
        Some(Node::Dir) => Ok((true, 0)),
        Some(Node::File(d)) => Ok((false, d.len())),
        None => Err(FsError::NotFound),
    }
}
