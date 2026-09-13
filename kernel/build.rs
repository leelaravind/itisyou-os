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
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let out = out_dir.join("initramfs.tar");
    emit_trust_root(&workspace, &out_dir);
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
        "lsdev",
        "sandbox-probe",
        "cap-handle-probe",
        "cap-parent",
        "cap-child",
        "fs-probe",
        "echo-svc",
        "crashy-svc",
        "svc-client",
        "tickd",
        "tick-client",
        "flapd",
        "net-probe",
        "net-denied",
        "tcp-probe",
        "tcp-server-probe",
        "fs-writer",
        "fs-write-denied",
        "fs-user-persist",
        "harden-probe",
        "stack-guard-probe",
        "hello-app",
        "args-probe",
        "flags-probe",
        "line-probe",
        "burn",
        "uaccess-probe",
        "proc-probe",
        "sysinit",
        "sh",
        "gui-echo",
        "ai-probe",
    ] {
        println!(
            "cargo::rerun-if-changed={}",
            workspace.join("user").join(dir).display()
        );
    }
    // /sbin/init links kernel-core (the config grammar and restart policy).
    println!(
        "cargo::rerun-if-changed={}",
        workspace.join("crates").join("kernel-core").display()
    );

    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new();
    collect(&root, "", &mut entries);
    // Generated version file — single source: the crate version.
    entries.push((
        "etc/version".to_string(),
        format!("{}\n", std::env::var("CARGO_PKG_VERSION").unwrap()).into_bytes(),
        false,
    ));
    build_user_programs(&workspace, &mut entries);
    add_trust_material(&workspace, &mut entries);
    add_ai_material(&workspace, &out_dir, &mut entries);
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
            "-p",
            "user-sandbox-probe",
            "-p",
            "user-cap-handle-probe",
            "-p",
            "user-cap-parent",
            "-p",
            "user-cap-child",
            "-p",
            "user-fs-probe",
            "-p",
            "user-echo-svc",
            "-p",
            "user-crashy-svc",
            "-p",
            "user-svc-client",
            "-p",
            "user-tickd",
            "-p",
            "user-tick-client",
            "-p",
            "user-flapd",
            "-p",
            "user-net-probe",
            "-p",
            "user-net-denied",
            "-p",
            "user-tcp-probe",
            "-p",
            "user-tcp-server-probe",
            "-p",
            "user-fs-writer",
            "-p",
            "user-fs-write-denied",
            "-p",
            "user-fs-user-persist",
            "-p",
            "user-harden-probe",
            "-p",
            "user-stack-guard-probe",
            "-p",
            "user-hello-app",
            "-p",
            "user-args-probe",
            "-p",
            "user-flags-probe",
            "-p",
            "user-line-probe",
            "-p",
            "user-burn",
            "-p",
            "user-uaccess-probe",
            "-p",
            "user-proc-probe",
            "-p",
            "user-sysinit",
            "-p",
            "user-sh",
            "-p",
            "user-gui-echo",
            "-p",
            "user-ai-probe",
        ])
        .arg("--target-dir")
        .arg(&target_dir)
        .current_dir(workspace)
        // x86_64-unknown-none defaults to PIE (ET_DYN); the V0.2 loader
        // deliberately accepts only static ET_EXEC images.
        //
        // `strip=debuginfo`: every one of these ELFs is embedded verbatim in
        // the initramfs, which is embedded in the kernel image. Unstripped
        // debug builds are ~775 KiB each (~17 MiB total) — that bloat is read
        // off disk one BIOS INT 13h call at a time by the bootloader's real-
        // mode stages, which pushed first-serial-output past the QEMU test
        // timeouts (V0.8 boot-race root cause). The kernel's ELF loader only
        // consumes program headers and PT_LOAD contents, so DWARF sections are
        // dead weight at runtime; stripping them changes no behaviour.
        .env(
            "RUSTFLAGS",
            "-C relocation-model=static -C link-arg=--no-pie -C strip=debuginfo",
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
        ("user-sandbox-probe", "bin/sandbox-probe"),
        ("user-cap-handle-probe", "bin/cap-handle-probe"),
        ("user-cap-parent", "bin/cap-parent"),
        ("user-cap-child", "bin/cap-child"),
        ("user-fs-probe", "bin/fs-probe"),
        ("user-echo-svc", "bin/echo-svc"),
        ("user-crashy-svc", "bin/crashy-svc"),
        ("user-svc-client", "bin/svc-client"),
        ("user-tickd", "bin/tickd"),
        ("user-tick-client", "bin/tick-client"),
        ("user-flapd", "bin/flapd"),
        ("user-net-probe", "bin/net-probe"),
        ("user-net-denied", "bin/net-denied"),
        ("user-tcp-probe", "bin/tcp-probe"),
        ("user-tcp-server-probe", "bin/tcp-server-probe"),
        ("user-fs-writer", "bin/fs-writer"),
        ("user-fs-write-denied", "bin/fs-write-denied"),
        ("user-fs-user-persist", "bin/fs-user-persist"),
        ("user-harden-probe", "bin/harden-probe"),
        ("user-stack-guard-probe", "bin/stack-guard-probe"),
        ("user-args-probe", "bin/args-probe"),
        ("user-flags-probe", "bin/flags-probe"),
        ("user-line-probe", "bin/line-probe"),
        ("user-burn", "bin/burn"),
        ("user-uaccess-probe", "bin/uaccess-probe"),
        ("user-proc-probe", "bin/proc-probe"),
        ("user-sysinit", "sbin/init"),
        ("user-sh", "bin/sh"),
        ("user-gui-echo", "bin/gui-echo"),
        ("user-ai-probe", "bin/ai-probe"),
    ];
    entries.push(("bin/".to_string(), Vec::new(), true));
    entries.push(("sbin/".to_string(), Vec::new(), true));
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

    // ITPKG application packages (V0.7): hello-app packed with the same
    // format + digest code the kernel verifies with, plus adversarial
    // fixtures (corrupted payload; hostile manifest) under /pkgs.
    let hello = fs::read(bin_dir.join("user-hello-app")).unwrap();
    entries.push(("pkgs/".to_string(), Vec::new(), true));
    let manifest_v1 = "name=hello-app
version=1.0.0
caps=fs_read
";
    let manifest_v2 = "name=hello-app
version=1.0.1
caps=fs_read
";
    entries.push((
        "pkgs/hello-app-1.itpkg".to_string(),
        pack_signed(manifest_v1, &hello, &DEV_SIGNING_SECRET),
        false,
    ));
    entries.push((
        "pkgs/hello-app-2.itpkg".to_string(),
        pack_signed(manifest_v2, &hello, &DEV_SIGNING_SECRET),
        false,
    ));
    // Corrupted AFTER digest computation -> refused on integrity, before the
    // signature is ever considered.
    let mut bad = pack_signed(manifest_v1, &hello, &DEV_SIGNING_SECRET);
    let last = bad.len() - 1;
    bad[last] ^= 0x01;
    entries.push(("pkgs/hello-app-bad.itpkg".to_string(), bad, false));
    // Authenticity fixtures (V0.8). Each is INTACT: the content hashes
    // correctly and the manifest is valid, so only the trust check can refuse
    // them — which is the point.
    //   * unsigned: a V0.7-format package, still parseable.
    entries.push((
        "pkgs/hello-app-unsigned.itpkg".to_string(),
        pack_itpkg(manifest_v1, &hello),
        false,
    ));
    //   * untrusted: correctly signed by a key the kernel does not know.
    entries.push((
        "pkgs/hello-app-untrusted.itpkg".to_string(),
        pack_signed(manifest_v1, &hello, &FOREIGN_SIGNING_SECRET),
        false,
    ));
    //   * forged: signed by the trusted key, then the signature is altered.
    let mut forged = pack_signed(manifest_v1, &hello, &DEV_SIGNING_SECRET);
    forged[80] ^= 0x01;
    entries.push(("pkgs/hello-app-forged.itpkg".to_string(), forged, false));
    // Key-hierarchy fixtures (V0.9). Each is intact and correctly signed; only
    // the certificate chain can refuse it, each for a different reason:
    //   * signed by a certified key the root has since revoked;
    entries.push((
        "pkgs/hello-app-retired.itpkg".to_string(),
        pack_signed(manifest_v1, &hello, &RETIRED_SIGNING_SECRET),
        false,
    ));
    //   * signed by a key whose certificate covered only earlier releases;
    entries.push((
        "pkgs/hello-app-expired.itpkg".to_string(),
        pack_signed(manifest_v1, &hello, &EXPIRED_SIGNING_SECRET),
        false,
    ));
    //   * signed by a key whose certificate was NOT issued by the root (the
    //     certificate ships in /etc/trust and is refused when loaded);
    entries.push((
        "pkgs/hello-app-rogue.itpkg".to_string(),
        pack_signed(manifest_v1, &hello, &ROGUE_SIGNING_SECRET),
        false,
    ));
    //   * signed by the PUBLISHED test key, for a name outside its scope —
    //     the property that makes publishing that key safe.
    entries.push((
        "pkgs/other-app-1.itpkg".to_string(),
        pack_signed(
            "name=other-app
version=1.0.0
caps=fs_read
",
            &hello,
            &DEV_SIGNING_SECRET,
        ),
        false,
    ));
    // Digest-valid but the manifest demands an undefined capability -> the
    // kernel must refuse it at manifest validation.
    entries.push((
        "pkgs/evil.itpkg".to_string(),
        pack_signed(
            "name=evil
version=1
caps=kernel-root
",
            &hello,
            &DEV_SIGNING_SECRET,
        ),
        false,
    ));
}

/// Write the development signer's PUBLIC key into OUT_DIR so the kernel can
/// `include_bytes!` it as its trust root.
///
/// Deriving it here, from the same seed that signs the fixtures, is what keeps
/// the trust root and the signer in lockstep: there is no second place to
/// update and no way for them to disagree.
fn emit_trust_root(workspace: &Path, out_dir: &Path) {
    let hex_path = workspace.join("keys").join("root.pub.hex");
    println!("cargo:rerun-if-changed={}", hex_path.display());
    println!("cargo:rerun-if-changed=build.rs");
    let text = fs::read_to_string(&hex_path).expect("reading keys/root.pub.hex");
    let text = text.trim();
    assert_eq!(text.len(), 64, "keys/root.pub.hex must be 64 hex digits");
    let root: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex digit"))
        .collect();
    fs::write(out_dir.join("trust_root.bin"), root).expect("writing the trust root");
}

/// Ship the certificates and the revocation list in the initramfs under
/// `/etc/trust`. They are PUBLIC, root-signed data produced offline by
/// `tools/keytool` and committed under `keys/`; the kernel verifies each one
/// against its compiled-in root when it loads them, so a file added or edited
/// here confers nothing the root did not sign. Every certificate the build
/// relies on is also checked HERE, so a key/fixture mismatch fails the build
/// instead of surfacing as a mysterious refusal at run time.
fn add_trust_material(workspace: &Path, entries: &mut Vec<(String, Vec<u8>, bool)>) {
    let keys = workspace.join("keys");
    println!("cargo:rerun-if-changed={}", keys.display());
    entries.push(("etc/trust/".to_string(), Vec::new(), true));
    entries.push(("etc/trust/certs/".to_string(), Vec::new(), true));
    let mut names: Vec<_> = fs::read_dir(keys.join("certs"))
        .expect("reading keys/certs")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "cert"))
        .collect();
    names.sort();
    for path in names {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        entries.push((
            format!("etc/trust/certs/{name}"),
            fs::read(&path).unwrap(),
            false,
        ));
    }
    let revocations = keys.join("revocations.bin");
    println!("cargo:rerun-if-changed={}", revocations.display());
    entries.push((
        "etc/trust/revocations.bin".to_string(),
        fs::read(&revocations).expect("reading keys/revocations.bin"),
        false,
    ));
    // The fixtures above assume these certificates certify these seeds.
    let root: [u8; 32] =
        fs::read(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("trust_root.bin"))
            .unwrap()
            .try_into()
            .unwrap();
    for (file, seed) in [
        ("test-signer.cert", &DEV_SIGNING_SECRET),
        ("retired-signer.cert", &RETIRED_SIGNING_SECRET),
        ("expired-signer.cert", &EXPIRED_SIGNING_SECRET),
    ] {
        let bytes = fs::read(keys.join("certs").join(file)).unwrap();
        let key = kernel_core::trust::verify_certificate(&bytes, &root)
            .unwrap_or_else(|e| panic!("keys/certs/{file}: {}", e.name()));
        assert_eq!(
            key.public_key,
            kernel_core::ed25519::public_key(seed),
            "keys/certs/{file} does not certify the fixture seed build.rs signs with"
        );
    }
}

/// Train the diagnostic model (V0.11, MODEL11-001, ADR-0024) and ship it.
///
/// The image contains what the repository trains: `ai/scenarios.txt` goes
/// through the same host-tested generator and trainer as the kernel-core
/// tests, the bytes must match the pin `ai/diag.model.sha256` (a stale pin
/// fails the build, naming both digests), and the digest is compiled into
/// the kernel (`OUT_DIR/model_sha256.bin`) as the provenance anchor the
/// kernel checks proposals against. Hostile variants of the real file go to
/// `/etc/ai/fixtures/` so the refusal of each is exercised inside the OS.
fn add_ai_material(workspace: &Path, out_dir: &Path, entries: &mut Vec<(String, Vec<u8>, bool)>) {
    use kernel_core::model;
    use kernel_core::scenario;
    let ai = workspace.join("ai");
    let scenarios_path = ai.join("scenarios.txt");
    let pin_path = ai.join("diag.model.sha256");
    println!("cargo:rerun-if-changed={}", scenarios_path.display());
    println!("cargo:rerun-if-changed={}", pin_path.display());
    let text = fs::read_to_string(&scenarios_path).expect("reading ai/scenarios.txt");
    let mut scenarios = [scenario::Scenario::EMPTY; scenario::MAX_SCENARIOS];
    let n = scenario::parse(&text, &mut scenarios)
        .unwrap_or_else(|e| panic!("ai/scenarios.txt line {}: {}", e.line, e.reason.name()));
    let mut train = vec![scenario::Example::ZERO; model::MAX_EXAMPLES];
    let mut test = vec![scenario::Example::ZERO; model::MAX_EXAMPLES];
    let (m, _, _) =
        model::train_from_scenarios(&scenarios[..n], &model::SHIPPED, &mut train, &mut test);
    let bytes = model::encode(&m);
    let digest = kernel_core::sha256::digest(&bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let pin = fs::read_to_string(&pin_path).expect("reading ai/diag.model.sha256");
    assert_eq!(
        hex,
        pin.trim(),
        "ai/diag.model.sha256 is stale: ai/scenarios.txt now trains {hex} (update the pin on purpose)"
    );
    fs::write(out_dir.join("model_sha256.bin"), digest).expect("writing model_sha256.bin");
    entries.push(("etc/ai/".to_string(), Vec::new(), true));
    entries.push(("etc/ai/diag.model".to_string(), bytes.to_vec(), false));
    // Each fixture fails exactly one check of `model::decode`.
    entries.push(("etc/ai/fixtures/".to_string(), Vec::new(), true));
    let mut bad_magic = bytes;
    bad_magic[0] = b'X';
    let mut wrong_dims = bytes;
    wrong_dims[8] = 17;
    let mut absurd = bytes;
    let at = model::HEADER_LEN + 4;
    absurd[at..at + 4].copy_from_slice(&(model::MAX_WEIGHT + 1).to_le_bytes());
    for (name, data) in [
        ("bad-magic.model", bad_magic.to_vec()),
        ("truncated.model", bytes[..bytes.len() - 1].to_vec()),
        ("wrong-dims.model", wrong_dims.to_vec()),
        ("absurd-weight.model", absurd.to_vec()),
    ] {
        entries.push((format!("etc/ai/fixtures/{name}"), data, false));
    }
}

/// The published TEST signing seed (key id 1 in `keys/certs/test-signer.cert`).
///
/// Not a secret and not pretending to be one: a fixed byte pattern so anyone
/// can rebuild the fixture packages and reproduce the image bit for bit. Since
/// V0.9 it is no longer the trust root: the kernel trusts only the offline
/// root, which certified this key for package names starting `hello-` and
/// nothing else — so holding this seed lets you sign fixtures, not software.
/// Real packages are signed by the release key (id 10), whose seed never
/// enters the source tree.
const DEV_SIGNING_SECRET: [u8; 32] = *b"itisyou-os dev package signer v8";

/// A second key the kernel does NOT trust, used to produce the
/// "correctly signed by a stranger" fixture.
const FOREIGN_SIGNING_SECRET: [u8; 32] = *b"itisyou-os foreign signer -----8";

/// V0.9 key-hierarchy fixture signers — published like the test key, and
/// certified (or not) in `keys/certs/` to exercise each refusal: key 2 is
/// revoked in `keys/revocations.bin`, key 3's certificate covers only epochs
/// 7..=8, and key 4's certificate was signed by an impostor root.
const RETIRED_SIGNING_SECRET: [u8; 32] = *b"itisyou-os retired signer -----9";
const EXPIRED_SIGNING_SECRET: [u8; 32] = *b"itisyou-os expired signer -----9";
const ROGUE_SIGNING_SECRET: [u8; 32] = *b"itisyou-os rogue signer -------9";

/// Assemble an UNSIGNED ITPKG001 (header + manifest + payload).
///
/// Kept so the build can still produce a V0.7-format package, which is what
/// the "unsigned packages are refused" fixture needs. Nothing the system
/// installs uses this any more.
fn pack_itpkg(manifest: &str, payload: &[u8]) -> Vec<u8> {
    let digest = kernel_core::pkg::content_digest(manifest.as_bytes(), payload);
    let mut out =
        kernel_core::pkg::header(manifest.len() as u32, payload.len() as u32, &digest).to_vec();
    out.extend_from_slice(manifest.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// Assemble a SIGNED ITPKG002 using `secret`.
fn pack_signed(manifest: &str, payload: &[u8], secret: &[u8; 32]) -> Vec<u8> {
    let digest = kernel_core::pkg::content_digest(manifest.as_bytes(), payload);
    let input =
        kernel_core::pkg::signing_input(manifest.len() as u32, payload.len() as u32, &digest);
    let signature = kernel_core::ed25519::sign(secret, &input);
    let public = kernel_core::ed25519::public_key(secret);
    let mut out = kernel_core::pkg::header_signed(
        manifest.len() as u32,
        payload.len() as u32,
        &digest,
        &public,
        &signature,
    )
    .to_vec();
    out.extend_from_slice(manifest.as_bytes());
    out.extend_from_slice(payload);
    out
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
