//! V1-REL-005: deterministic mutation fuzzing of every `kernel_core` decoder
//! that parses bytes a program, a disk, a package, firmware, a device or the
//! network supplies.
//!
//! Every target starts from VALID inputs built with the crate's own encoders
//! (or the unit tests' fixtures, and the real signed trust material under
//! `keys/`), so mutation explores the neighbourhood of well-formed input:
//! bit flips, boundary bytes, insertions, deletions, truncation, extension,
//! splicing with another seed, little- and big-endian integer overwrites with
//! boundary values (0, 1, max, len±2), arithmetic nudges of length fields and
//! dictionary tokens for the text formats. Formats protected by a checksum,
//! CRC or digest are re-sealed for part of the inputs so the checks behind the
//! checksum are reached too. A small share of inputs is pure random bytes and
//! a small share is an unmodified seed.
//!
//! A panic, an arithmetic overflow (the debug build checks overflow), or a
//! hang fails the target. Where an encoder exists, accepted inputs must round
//! trip; where a module documents a stronger property (sysview's "every
//! single-byte change either decodes to a different view or is refused" —
//! implied by `encode(decode(b)) == b`; linebuf's "no kernel marker survives
//! on the wire"; itfs's "what reaches the disk must mount again, unchanged";
//! the key hierarchy's refusal order) it is asserted on every accepted input.
//!
//! Iteration `i` of target `t` draws only from a PRNG seeded by
//! (`ITISYOU_FUZZ_SEED`, t, i), so every failure replays in isolation:
//! `ITISYOU_FUZZ_REPLAY=<target>:<iteration>` runs just that iteration and
//! prints its input. A failing iteration prints that exact command.
//!
//! Plain `cargo test` runs a fast pass of every target. The `#[ignore]`d
//! `full_*` tests run `ITISYOU_FUZZ_ITERS` (default 1 000 000) iterations per
//! target and print one `FUZZ-TARGET` line each; `scripts/fuzz-decoders.ps1`
//! runs them in release and, with a smaller count, in debug (overflow checks).

use std::cell::Cell;
use std::fmt::Write as _;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel_core::net::{arp, checksum, dhcp, dns, eth, icmp, ipv4, ipv6, tcp, udp};
use kernel_core::{
    acpi, audit_chain, capability, caps, ed25519, elf, font, infer, initconf, initctl, itfs,
    linebuf, linedisc, manifest, marker, memmap, model, mouse, path, pci, pkg, policy, procstatus,
    progargs, scancode, scenario, shellparse, stage, svcreport, sysview, tar, trust, update, usb,
    wm,
};

// ===========================================================================
// Harness
// ===========================================================================

/// Default base seed (override with `ITISYOU_FUZZ_SEED`).
const BASE_SEED: u64 = 0x1715_1000_5EED_0005;
/// Iterations of a cost-1 target in the plain `cargo test` pass.
const FAST: u64 = 10_000;
/// Iterations per target in the `full_*` tests unless `ITISYOU_FUZZ_ITERS`
/// says otherwise.
const FULL_DEFAULT: u64 = 1_000_000;

fn env_u64(name: &str) -> Option<u64> {
    let v = std::env::var(name).ok()?;
    let v = v.trim().replace(['_', ' '], "");
    if let Some(hex) = v.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        v.parse().ok()
    }
}

fn base_seed() -> u64 {
    env_u64("ITISYOU_FUZZ_SEED").unwrap_or(BASE_SEED)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Fast,
    Full,
}

/// The splitmix64 finalizer.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// splitmix64: small, fast, fully specified.
#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn for_iteration(target_seed: u64, i: u64) -> Rng {
        Rng(mix(target_seed ^ mix(i.wrapping_add(0x632B_E59B_D9B4_E019))))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }
    /// Uniform in `0..n` (`0` when `n == 0`).
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    fn one_in(&mut self, n: u64) -> bool {
        self.next().is_multiple_of(n)
    }
    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
    fn u16(&mut self) -> u16 {
        self.next() as u16
    }
    fn u32(&mut self) -> u32 {
        self.next() as u32
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    /// A value near a boundary: 0, 1, a power-of-two edge, a type maximum,
    /// `len` ± 2, or a random value of random width.
    fn interesting(&mut self, len: usize) -> u64 {
        match self.below(16) {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 0x7f,
            4 => 0x80,
            5 => 0xff,
            6 => 0x100,
            7 => 0x7fff,
            8 => 0x8000,
            9 => 0xffff,
            10 => 0x1_0000,
            11 => 0x7fff_ffff,
            12 => 0xffff_ffff,
            13 => u64::MAX,
            14 => (len as u64)
                .wrapping_add(self.below(5) as u64)
                .wrapping_sub(2),
            _ => self.next() & [0xff, 0xffff, 0xffff_ffff, u64::MAX][self.below(4)],
        }
    }
}

/// Per-thread iteration context: the last input a body noted (printed if the
/// iteration fails) and target-specific counters.
#[derive(Default)]
struct Ctx {
    input: Vec<u8>,
    aux: [u64; 4],
}

impl Ctx {
    fn note(&mut self, b: &[u8]) {
        self.input.clear();
        self.input.extend_from_slice(b);
    }
    fn count(&mut self, k: usize) {
        self.aux[k] += 1;
    }
}

struct Target {
    name: &'static str,
    /// Relative cost of one iteration: the fast pass runs `FAST / cost`.
    cost: u64,
    /// Split the full run across threads (signature-heavy targets).
    parallel: bool,
    /// Labels for the `Ctx::aux` counters that are printed.
    aux: &'static [&'static str],
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn replay_request() -> Option<(String, u64)> {
    let v = std::env::var("ITISYOU_FUZZ_REPLAY").ok()?;
    let (name, i) = v.rsplit_once(':')?;
    Some((name.to_string(), i.trim().parse().ok()?))
}

fn fuzz_threads() -> usize {
    if let Some(n) = env_u64("ITISYOU_FUZZ_THREADS") {
        return (n as usize).max(1);
    }
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    (cores / 2).clamp(1, 8)
}

/// Run one target: `setup` builds the seeds once, `body` runs one iteration
/// and returns whether the primary decoder accepted its input.
fn run<S: Sync>(
    t: Target,
    mode: Mode,
    setup: impl FnOnce() -> S,
    body: impl Fn(&S, &mut Rng, &mut Ctx) -> bool + Sync,
) {
    let seed = mix(base_seed() ^ fnv1a(t.name));
    let replay = replay_request();
    let (first, iters) = match &replay {
        Some((name, i)) if name == t.name => (*i, 1),
        Some(_) => return,
        None => match mode {
            Mode::Fast => (0, (FAST / t.cost).max(10)),
            Mode::Full => (0, env_u64("ITISYOU_FUZZ_ITERS").unwrap_or(FULL_DEFAULT)),
        },
    };
    let state = setup();
    let threads = if t.parallel && mode == Mode::Full && replay.is_none() {
        fuzz_threads()
    } else {
        1
    };
    let started = Instant::now();
    let progress = Arc::new(AtomicU64::new(0));
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let current: Arc<Vec<AtomicU64>> = Arc::new((0..threads).map(|_| AtomicU64::new(0)).collect());
    let dog = watchdog(t.name, progress.clone(), stopped, current.clone());
    let chunk = iters.div_ceil(threads as u64);
    let results: Vec<(u64, [u64; 4])> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|k| {
                let lo = first + k as u64 * chunk;
                let hi = (lo + chunk).min(first + iters);
                let (state, body, progress, current) = (&state, &body, &progress, &current);
                let name = t.name;
                let verbose = replay.is_some();
                std::thread::Builder::new()
                    .stack_size(16 << 20)
                    .spawn_scoped(s, move || {
                        run_chunk(
                            name,
                            seed,
                            lo,
                            hi,
                            verbose,
                            state,
                            body,
                            progress,
                            &current[k],
                        )
                    })
                    .expect("spawn fuzz thread")
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|e| resume_unwind(e)))
            .collect()
    });
    let secs = started.elapsed().as_secs_f64();
    drop(stop);
    let _ = dog.join();
    let accepted: u64 = results.iter().map(|r| r.0).sum();
    let mut aux = [0u64; 4];
    for r in &results {
        for (a, b) in aux.iter_mut().zip(r.1) {
            *a += b;
        }
    }
    let mut line = format!(
        "FUZZ-TARGET name={} iters={} accepted={} refused={}",
        t.name,
        iters,
        accepted,
        iters - accepted
    );
    for (label, v) in t.aux.iter().zip(aux) {
        let _ = write!(line, " {label}={v}");
    }
    let _ = write!(line, " threads={threads} secs={secs:.2}");
    println!("{line}");
}

#[allow(clippy::too_many_arguments)]
fn run_chunk<S>(
    name: &str,
    seed: u64,
    lo: u64,
    hi: u64,
    verbose: bool,
    state: &S,
    body: &(impl Fn(&S, &mut Rng, &mut Ctx) -> bool + Sync),
    progress: &AtomicU64,
    current: &AtomicU64,
) -> (u64, [u64; 4]) {
    let mut ctx = Ctx::default();
    let mut accepted = 0u64;
    let at = Cell::new(lo);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        for i in lo..hi {
            at.set(i);
            current.store(i, Ordering::Relaxed);
            let mut rng = Rng::for_iteration(seed, i);
            if body(state, &mut rng, &mut ctx) {
                accepted += 1;
            }
            progress.fetch_add(1, Ordering::Relaxed);
            if verbose {
                println!(
                    "FUZZ-REPLAY target={name} iteration={i} input({})={}",
                    ctx.input.len(),
                    hex(&ctx.input)
                );
            }
        }
    }));
    if let Err(e) = outcome {
        let i = at.get();
        eprintln!(
            "FUZZ-FAIL target={name} iteration={i} replay: ITISYOU_FUZZ_REPLAY={name}:{i} (seed {:#x})",
            base_seed()
        );
        eprintln!(
            "FUZZ-FAIL last input ({} bytes): {}",
            ctx.input.len(),
            hex(&ctx.input)
        );
        resume_unwind(e);
    }
    (accepted, ctx.aux)
}

/// Fail the whole process if a target stops making progress: a decoder that
/// loops forever on some input must not be reported as "still running".
fn watchdog(
    name: &'static str,
    progress: Arc<AtomicU64>,
    stopped: std::sync::mpsc::Receiver<()>,
    current: Arc<Vec<AtomicU64>>,
) -> std::thread::JoinHandle<()> {
    let limit = Duration::from_secs(env_u64("ITISYOU_FUZZ_HANG_SECS").unwrap_or(120));
    std::thread::spawn(move || {
        let mut last = u64::MAX;
        let mut since = Instant::now();
        // The sender is dropped when the target finishes.
        while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
            stopped.recv_timeout(Duration::from_millis(200))
        {
            let p = progress.load(Ordering::Relaxed);
            if p != last {
                last = p;
                since = Instant::now();
            } else if since.elapsed() > limit {
                let at: Vec<u64> = current.iter().map(|c| c.load(Ordering::Relaxed)).collect();
                eprintln!(
                    "FUZZ-HANG target={name}: no iteration finished for {}s; iterations in flight {at:?} (replay one with ITISYOU_FUZZ_REPLAY={name}:<iteration>)",
                    limit.as_secs()
                );
                std::process::exit(3);
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Mutation engine
// ---------------------------------------------------------------------------

const BOUNDARY_BYTES: [u8; 10] = [0, 1, 2, 0x20, 0x7f, 0x80, 0xfe, 0xff, b'0', b'a'];

fn write_int(buf: &mut [u8], at: usize, width: usize, v: u64, big_endian: bool) {
    for k in 0..width {
        let shift = if big_endian {
            8 * (width - 1 - k)
        } else {
            8 * k
        };
        if let Some(b) = buf.get_mut(at + k) {
            *b = (v >> shift) as u8;
        }
    }
}

fn read_int(buf: &[u8], at: usize, width: usize, big_endian: bool) -> u64 {
    let mut v = 0u64;
    for k in 0..width {
        let b = u64::from(buf.get(at + k).copied().unwrap_or(0));
        let shift = if big_endian {
            8 * (width - 1 - k)
        } else {
            8 * k
        };
        v |= b << shift;
    }
    v
}

/// Apply 1..=8 (geometrically distributed) mutations to `buf`.
fn mutate(rng: &mut Rng, buf: &mut Vec<u8>, seeds: &[Vec<u8>], dict: &[&[u8]], max_len: usize) {
    let rounds = 1 + (rng.next().trailing_zeros() as usize).min(7);
    for _ in 0..rounds {
        let len = buf.len();
        match rng.below(18) {
            // Bit flip.
            0 | 1 => {
                if len > 0 {
                    let i = rng.below(len);
                    buf[i] ^= 1 << rng.below(8);
                }
            }
            // A byte set to a boundary value, or to anything.
            2 => {
                if len > 0 {
                    let i = rng.below(len);
                    buf[i] = *rng.pick(&BOUNDARY_BYTES);
                }
            }
            3 => {
                if len > 0 {
                    let i = rng.below(len);
                    buf[i] = rng.byte();
                }
            }
            // Insert or delete one byte.
            4 => {
                let i = rng.below(len + 1);
                buf.insert(i, rng.byte());
            }
            5 => {
                if len > 0 {
                    buf.remove(rng.below(len));
                }
            }
            // Truncate.
            6 => buf.truncate(rng.below(len + 1)),
            // Extend with zeros, ones or noise.
            7 => {
                let n = rng.range(1, 64);
                let fill = rng.below(3);
                for _ in 0..n {
                    let b = match fill {
                        0 => 0,
                        1 => 0xff,
                        _ => rng.byte(),
                    };
                    buf.push(b);
                }
            }
            // Splice with another seed: overwrite, insert, or cross over.
            8 | 9 => {
                let other: &[u8] = if seeds.is_empty() {
                    &[]
                } else {
                    rng.pick(seeds)
                };
                if !other.is_empty() {
                    let a = rng.below(other.len());
                    let n = rng.range(1, (other.len() - a).min(96));
                    let chunk = &other[a..a + n];
                    match rng.below(3) {
                        0 => {
                            let at = if a < len && rng.one_in(2) {
                                a
                            } else {
                                rng.below(len + 1)
                            };
                            for (k, &c) in chunk.iter().enumerate() {
                                match buf.get_mut(at + k) {
                                    Some(b) => *b = c,
                                    None => buf.push(c),
                                }
                            }
                        }
                        1 => {
                            let at = rng.below(len + 1);
                            buf.splice(at..at, chunk.iter().copied());
                        }
                        _ => {
                            let cut = rng.below(len + 1);
                            let ocut = rng.below(other.len() + 1);
                            buf.truncate(cut);
                            buf.extend_from_slice(&other[ocut..]);
                        }
                    }
                }
            }
            // Integer field overwrite with a boundary value (either order).
            10 | 11 => {
                let width = [1usize, 2, 4, 8][rng.below(4)];
                if len >= width {
                    let at = rng.below(len - width + 1);
                    let v = rng.interesting(len);
                    let be = rng.one_in(2);
                    write_int(buf, at, width, v, be);
                }
            }
            // Arithmetic nudge of an integer field (a length off by a few).
            12 => {
                let width = [1usize, 2, 4][rng.below(3)];
                if len >= width {
                    let at = rng.below(len - width + 1);
                    let be = rng.one_in(2);
                    let delta = rng.range(0, 16) as i64 - 8;
                    let v = read_int(buf, at, width, be).wrapping_add(delta as u64);
                    write_int(buf, at, width, v, be);
                }
            }
            // Dictionary token: insert or overwrite.
            13 | 14 => {
                if !dict.is_empty() {
                    let tok = *rng.pick(dict);
                    let at = rng.below(len + 1);
                    if rng.one_in(2) {
                        buf.splice(at..at, tok.iter().copied());
                    } else {
                        for (k, &c) in tok.iter().enumerate() {
                            match buf.get_mut(at + k) {
                                Some(b) => *b = c,
                                None => buf.push(c),
                            }
                        }
                    }
                }
            }
            // Duplicate a chunk elsewhere.
            15 => {
                if len > 0 {
                    let a = rng.below(len);
                    let n = rng.range(1, (len - a).min(64));
                    let chunk: Vec<u8> = buf[a..a + n].to_vec();
                    let at = rng.below(len + 1);
                    buf.splice(at..at, chunk);
                }
            }
            // Delete a range.
            16 => {
                if len > 0 {
                    let a = rng.below(len);
                    let n = rng.range(1, (len - a).min(64));
                    buf.drain(a..a + n);
                }
            }
            // Fill a range with one byte.
            _ => {
                if len > 0 {
                    let a = rng.below(len);
                    let n = rng.range(1, (len - a).min(32));
                    let v = *rng.pick(&BOUNDARY_BYTES);
                    buf[a..a + n].fill(v);
                }
            }
        }
    }
    buf.truncate(max_len);
}

/// One input: mostly a mutated seed, sometimes an unmodified seed, sometimes
/// pure noise.
fn gen_bytes(rng: &mut Rng, seeds: &[Vec<u8>], dict: &[&[u8]], max_len: usize) -> Vec<u8> {
    match rng.below(64) {
        0 | 1 => {
            let n = rng.below(max_len.min(1024) + 1);
            rng.bytes(n)
        }
        2 | 3 => rng.pick(seeds).clone(),
        _ => {
            let mut b = rng.pick(seeds).clone();
            mutate(rng, &mut b, seeds, dict, max_len);
            b
        }
    }
}

/// A known integer field of a binary format: (offset, width, big-endian).
type Field = (usize, usize, bool);

/// Length-field edits: set one of a format's known length, count or offset
/// fields to a boundary value, or nudge it by a few.
fn mutate_field(rng: &mut Rng, buf: &mut [u8], fields: &[Field]) {
    if fields.is_empty() {
        return;
    }
    let (at, width, be) = *rng.pick(fields);
    if at + width > buf.len() {
        return;
    }
    let v = if rng.one_in(3) {
        let delta = rng.range(0, 16) as u64;
        read_int(buf, at, width, be)
            .wrapping_add(delta)
            .wrapping_sub(8)
    } else {
        rng.interesting(buf.len())
    };
    write_int(buf, at, width, v, be);
}

/// [`gen_bytes`], plus targeted field edits for a quarter of the inputs.
fn gen_bytes_f(
    rng: &mut Rng,
    seeds: &[Vec<u8>],
    dict: &[&[u8]],
    fields: &[Field],
    max_len: usize,
) -> Vec<u8> {
    let mut b = gen_bytes(rng, seeds, dict, max_len);
    if rng.one_in(4) {
        for _ in 0..rng.range(1, 2) {
            mutate_field(rng, &mut b, fields);
        }
    }
    b
}

/// Text inputs: the kernel checks UTF-8 before any of these parsers sees the
/// bytes, so a mutated byte string reaches them as lossily repaired UTF-8.
fn gen_text(rng: &mut Rng, seeds: &[Vec<u8>], dict: &[&[u8]], max_len: usize) -> String {
    String::from_utf8_lossy(&gen_bytes(rng, seeds, dict, max_len)).into_owned()
}

fn text_seeds(items: &[&str]) -> Vec<Vec<u8>> {
    items.iter().map(|s| s.as_bytes().to_vec()).collect()
}

macro_rules! targets {
    ($($fast:ident, $full:ident => $f:path;)*) => {
        $(
            #[test]
            fn $fast() {
                $f(Mode::Fast)
            }
            #[test]
            #[ignore = "V1-REL-005 full run (scripts/fuzz-decoders.ps1; ITISYOU_FUZZ_ITERS, default 1000000)"]
            fn $full() {
                $f(Mode::Full)
            }
        )*
    };
}

// ===========================================================================
// Bytes a Ring 3 program supplies: syscall arguments, IPC records, output
// ===========================================================================

targets! {
    elf_parse, full_elf_parse => t_elf;
    progargs_validate_block, full_progargs_validate_block => t_progargs;
    svcreport_decode, full_svcreport_decode => t_svcreport;
    sysview_view_decode, full_sysview_view_decode => t_sysview;
    infer_decode, full_infer_decode => t_infer;
    policy_decode, full_policy_decode => t_policy_decode;
    policy_table, full_policy_table => t_policy_table;
    initctl_decode, full_initctl_decode => t_initctl;
    wm_decode, full_wm_decode => t_wm;
    caps_parse, full_caps_parse => t_caps;
    path_validate, full_path_validate => t_path;
    shellparse_parse, full_shellparse_parse => t_shellparse;
    linebuf_streams, full_linebuf_streams => t_linebuf;
    audit_chain_escape_untrusted, full_audit_chain_escape_untrusted => t_escape;
    capability_table, full_capability_table => t_capability;
    procstatus_decode, full_procstatus_decode => t_procstatus;
}

// --- elf ---------------------------------------------------------------------

struct Ph {
    ty: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// An ELF64 x86-64 ET_EXEC image: header, program headers at 64, then
/// `total_len` bytes in all (segment data is a byte pattern).
fn build_elf(entry: u64, phentsize: usize, phs: &[Ph], total_len: usize) -> Vec<u8> {
    let mut e: Vec<u8> = (0..total_len.max(64 + phentsize * phs.len()))
        .map(|i| (i * 7) as u8)
        .collect();
    e[..64].fill(0);
    e[0..4].copy_from_slice(b"\x7fELF");
    e[4] = 2;
    e[5] = 1;
    e[6] = 1;
    e[16..18].copy_from_slice(&2u16.to_le_bytes());
    e[18..20].copy_from_slice(&62u16.to_le_bytes());
    e[20..24].copy_from_slice(&1u32.to_le_bytes());
    e[24..32].copy_from_slice(&entry.to_le_bytes());
    e[32..40].copy_from_slice(&64u64.to_le_bytes());
    e[52..54].copy_from_slice(&64u16.to_le_bytes());
    e[54..56].copy_from_slice(&(phentsize as u16).to_le_bytes());
    e[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
    for (i, ph) in phs.iter().enumerate() {
        let o = 64 + i * phentsize;
        e[o..o + phentsize].fill(0);
        e[o..o + 4].copy_from_slice(&ph.ty.to_le_bytes());
        e[o + 4..o + 8].copy_from_slice(&ph.flags.to_le_bytes());
        e[o + 8..o + 16].copy_from_slice(&ph.offset.to_le_bytes());
        e[o + 16..o + 24].copy_from_slice(&ph.vaddr.to_le_bytes());
        e[o + 24..o + 32].copy_from_slice(&ph.vaddr.to_le_bytes()); // paddr
        e[o + 32..o + 40].copy_from_slice(&ph.filesz.to_le_bytes());
        e[o + 40..o + 48].copy_from_slice(&ph.memsz.to_le_bytes());
        e[o + 48..o + 56].copy_from_slice(&ph.align.to_le_bytes());
    }
    e
}

fn load(flags: u32, offset: u64, vaddr: u64, filesz: u64, memsz: u64, align: u64) -> Ph {
    Ph {
        ty: PT_LOAD,
        flags,
        offset,
        vaddr,
        filesz,
        memsz,
        align,
    }
}

/// The unit tests' minimal image: one RX segment of 16 bytes at 0x400000.
fn minimal_elf() -> Vec<u8> {
    build_elf(
        0x40_0000,
        56,
        &[load(PF_R | PF_X, 120, 0x40_0000, 16, 16, 0)],
        136,
    )
}

fn elf_seeds() -> Vec<Vec<u8>> {
    let note = |ty: u32| Ph {
        ty,
        flags: PF_R,
        offset: 64,
        vaddr: 0x40_0040,
        filesz: 0x20,
        memsz: 0x20,
        align: 8,
    };
    let seeds = vec![
        minimal_elf(),
        // Text and data+bss, 0x100-aligned, as a linked user program has.
        build_elf(
            0x40_0100,
            56,
            &[
                load(PF_R | PF_X, 0x100, 0x40_0100, 0x40, 0x40, 0x100),
                load(PF_R | PF_W, 0x200, 0x60_0200, 0x20, 0x1000, 0x100),
            ],
            0x220,
        ),
        // Non-load headers around the load (PHDR, NOTE, GNU_STACK).
        build_elf(
            0x40_0180,
            56,
            &[
                note(6),
                note(4),
                load(PF_R | PF_X, 0x180, 0x40_0180, 0x30, 0x30, 0x10),
                Ph {
                    ty: 0x6474_e551,
                    flags: PF_R | PF_W,
                    offset: 0,
                    vaddr: 0,
                    filesz: 0,
                    memsz: 0,
                    align: 16,
                },
            ],
            0x1c0,
        ),
        // A wider program-header entry, entry in the second segment.
        build_elf(
            0x50_0010,
            64,
            &[
                load(PF_R, 0xc0, 0x40_00c0, 0x10, 0x10, 0x40),
                load(PF_R | PF_X, 0x100, 0x50_0000, 0x40, 0x80, 0x100),
            ],
            0x140,
        ),
    ];
    for s in &seeds {
        assert!(elf::parse(s).is_ok(), "ELF seed must parse");
    }
    seeds
}

const ELF_DICT: &[&[u8]] = &[
    b"\x7fELF",
    &[2, 1, 1],
    &[1, 0, 0, 0],
    &[62, 0],
    &[2, 0],
    &[56, 0],
    &[5, 0, 0, 0],
    &[0, 0, 0x40, 0, 0, 0, 0, 0],
    &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
];

/// ELF header fields, then the first two program headers' (phentsize 56).
const ELF_FIELDS: &[Field] = &[
    (16, 2, false),
    (18, 2, false),
    (24, 8, false),
    (32, 8, false),
    (54, 2, false),
    (56, 2, false),
    (64, 4, false),
    (68, 4, false),
    (72, 8, false),
    (80, 8, false),
    (96, 8, false),
    (104, 8, false),
    (112, 8, false),
    (120, 4, false),
    (128, 8, false),
    (136, 8, false),
    (152, 8, false),
    (160, 8, false),
    (168, 8, false),
];

fn t_elf(mode: Mode) {
    run(
        Target {
            name: "elf::parse",
            cost: 1,
            parallel: false,
            aux: &["segments"],
        },
        mode,
        elf_seeds,
        |seeds, rng, ctx| {
            let b = gen_bytes_f(rng, seeds, ELF_DICT, ELF_FIELDS, 4096);
            ctx.note(&b);
            let Ok(img) = elf::parse(&b) else {
                return false;
            };
            // What the loader relies on: every segment's file data lies inside
            // the image and inside its memory size, no range overflows, and
            // the entry point is inside an executable segment.
            let (mut n, mut entry_ok) = (0, false);
            let base = b.as_ptr() as usize;
            for seg in img.load_segments() {
                n += 1;
                assert!(seg.data.len() as u64 <= seg.mem_size);
                let end = seg.vaddr.checked_add(seg.mem_size).expect("vaddr + memsz");
                let p = seg.data.as_ptr() as usize;
                assert!(seg.data.is_empty() || (p >= base && p + seg.data.len() <= base + b.len()));
                if seg.executable && img.entry >= seg.vaddr && img.entry < end {
                    entry_ok = true;
                }
            }
            assert!((1..=64).contains(&n) && img.entry != 0 && entry_ok);
            ctx.aux[0] += n as u64;
            true
        },
    );
}

// --- progargs ----------------------------------------------------------------

fn t_progargs(mode: Mode) {
    run(
        Target {
            name: "progargs::validate_block+split",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let lists: Vec<Vec<String>> = vec![
                vec![],
                vec!["alpha".into(), "beta".into(), "gamma".into()],
                vec!["a".into(); progargs::MAX_ARGS],
                vec!["x".repeat(progargs::MAX_BLOCK - 1)],
                vec!["y".repeat(255), "y".repeat(255)],
                vec![(0x21u8..=0x7e).map(char::from).collect()],
                vec!["--".into(), "caps=gui".into(), "from-config".into()],
            ];
            lists
                .iter()
                .map(|l| {
                    let mut out = [0u8; progargs::MAX_BLOCK];
                    let n = progargs::encode(l, &mut out).expect("seed encodes");
                    assert_eq!(progargs::validate_block(&out[..n]), Ok(l.len()));
                    out[..n].to_vec()
                })
                .collect::<Vec<_>>()
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, &[b"\0", b" ", b"\x7f", b"\xff", b"a\0"], 700);
            ctx.note(&b);
            // `split` is total on any block: its parts, rejoined, are the block.
            let parts: Vec<&[u8]> = progargs::split(&b).collect();
            let mut rejoined = parts.join(&0u8);
            if b.last() == Some(&0) {
                rejoined.push(0);
            }
            assert_eq!(rejoined, b, "split lost or invented bytes");
            match progargs::validate_block(&b) {
                Ok(n) => {
                    assert_eq!(parts.len(), n);
                    assert!(n <= progargs::MAX_ARGS && b.len() <= progargs::MAX_BLOCK);
                    for p in &parts {
                        assert!(!p.is_empty() && p.iter().all(|&c| progargs::is_arg_byte(c)));
                    }
                    let mut out = [0u8; progargs::MAX_BLOCK];
                    let m = progargs::encode(&parts, &mut out).expect("a valid block re-encodes");
                    assert_eq!(&out[..m], &b[..]);
                    ctx.count(0);
                    true
                }
                Err(e) => {
                    let _ = e.name();
                    false
                }
            }
        },
    );
}

// --- svcreport ---------------------------------------------------------------

fn t_svcreport(mode: Mode) {
    use kernel_core::service::RESTART_LIMIT;
    run(
        Target {
            name: "svcreport::decode",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let mut seeds = Vec::new();
            for (k, event) in svcreport::Event::ALL.iter().enumerate() {
                for (j, name) in ["a", "tickd", "0-9", "abcdefghijklmno"].iter().enumerate() {
                    let r = svcreport::Report {
                        event: *event,
                        long_running: (k + j) % 2 == 0,
                        clean_exit: (k + j) % 3 == 0,
                        restarts: ((k + j) as u32) % (RESTART_LIMIT + 1),
                        pid: [0, 1, 4096, u64::MAX][j],
                        name: svcreport::ServiceName::new(name).unwrap(),
                    };
                    let b = svcreport::encode(&r);
                    assert_eq!(svcreport::decode(&b), Ok(r));
                    seeds.push(b.to_vec());
                }
            }
            seeds
        },
        |seeds, rng, ctx| {
            let fields = [(0, 1, false), (1, 1, false), (3, 1, false), (4, 4, false)];
            let b = gen_bytes_f(rng, seeds, &[b"\0", b"-", b"\x03"], &fields, 64);
            ctx.note(&b);
            match svcreport::decode(&b) {
                Ok(r) => {
                    // Strict decoding: the input is the one encoding of the report.
                    assert_eq!(&svcreport::encode(&r)[..], &b[..]);
                    assert_eq!(svcreport::ServiceName::new(r.name.as_str()), Ok(r.name));
                    assert!(r.restarts <= RESTART_LIMIT);
                    let _ = (r.event.name(), r.event.code());
                    ctx.count(0);
                    true
                }
                Err(e) => {
                    let _ = e.name();
                    false
                }
            }
        },
    );
}

// --- sysview -----------------------------------------------------------------

fn sysview_seeds() -> Vec<Vec<u8>> {
    use sysview::{ServiceRow, SvcState, View};
    let mut booted = View {
        uptime_ticks: 1234,
        processes: 4,
        runnable: 2,
        waiting: 1,
        ended: 0,
        always_on: true,
        paused: false,
        truncated: false,
        max_gap_ms: 61,
        busy_slices: 10,
        idle_slices: 567,
        job_slices: 8,
        audit_total: 9,
        audit_denials: 0,
        recent_records: 9,
        recent_denials: 0,
        net_rx: 3,
        net_tx: 2,
        net_refused: 0,
        ..View::default()
    };
    booted.push_service(ServiceRow::new("tickd", SvcState::Running, 0, true).unwrap());
    booted.push_service(ServiceRow::new("flapd", SvcState::Failed, 3, true).unwrap());
    let mut full = View {
        uptime_ticks: u64::MAX,
        processes: 40,
        runnable: 10,
        waiting: 10,
        ended: 10,
        always_on: true,
        paused: true,
        max_gap_ms: u32::MAX,
        audit_total: u32::MAX,
        audit_denials: 7,
        recent_records: u16::MAX,
        recent_denials: u16::MAX,
        net_refused: u32::MAX,
        ..View::default()
    };
    let states = [
        SvcState::Stopped,
        SvcState::Running,
        SvcState::Done,
        SvcState::Restarting,
        SvcState::Failed,
        SvcState::Running,
        SvcState::Failed,
        SvcState::Done,
        SvcState::Running,
    ];
    for (i, st) in states.iter().enumerate() {
        full.push_service(
            ServiceRow::new(&format!("svc-{i}"), *st, 70_000 * i as u32, i % 2 == 0).unwrap(),
        );
    }
    let mut one = View {
        processes: 1,
        runnable: 1,
        ..View::default()
    };
    one.push_service(ServiceRow::new("abcdefghijklm-9", SvcState::Restarting, 2, false).unwrap());
    [booted, View::default(), full, one]
        .iter()
        .map(|v| {
            assert_eq!(View::decode(&v.encode()), Ok(*v));
            v.encode().to_vec()
        })
        .collect()
}

const SYSVIEW_FIELDS: &[Field] = &[
    (8, 8, false),
    (16, 2, false),
    (18, 2, false),
    (20, 2, false),
    (22, 2, false),
    (24, 1, false),
    (25, 1, false),
    (26, 2, false),
    (28, 4, false),
    (44, 4, false),
    (48, 4, false),
    (52, 2, false),
    (54, 2, false),
    (64, 4, false),
    (68, 4, false),
    (88, 1, false),
    (89, 1, false),
    (90, 2, false),
    (108, 1, false),
    (109, 1, false),
];

fn t_sysview(mode: Mode) {
    run(
        Target {
            name: "sysview::View::decode",
            cost: 1,
            parallel: false,
            aux: &["round_trips", "single_byte_checks"],
        },
        mode,
        sysview_seeds,
        |seeds, rng, ctx| {
            let b = gen_bytes_f(rng, seeds, &[b"ITVIEW01", b"\0\0\0\0"], SYSVIEW_FIELDS, 300);
            ctx.note(&b);
            match sysview::View::decode(&b) {
                Ok(v) => {
                    // Canonical: the input is the only encoding of `v`. That
                    // is the documented "every single-byte change either
                    // decodes to a different view or is refused", for every
                    // accepted input rather than one fixture.
                    assert_eq!(&v.encode()[..], &b[..]);
                    for f in sysview::features(&v) {
                        assert!((0..=sysview::FEATURE_MAX).contains(&f));
                    }
                    for row in v.rows() {
                        let n = row.name_str();
                        assert!(!n.is_empty() && n.len() < sysview::NAME_LEN);
                        let _ = row.state.name();
                    }
                    ctx.count(0);
                    if rng.one_in(4) {
                        let i = rng.below(b.len());
                        let mut c = b.clone();
                        c[i] = c[i].wrapping_add(rng.range(1, 255) as u8);
                        assert_ne!(sysview::View::decode(&c), Ok(v), "byte {i}");
                        ctx.count(1);
                    }
                    true
                }
                Err(e) => {
                    let _ = e.name();
                    false
                }
            }
        },
    );
}

// --- infer -------------------------------------------------------------------

/// The shipped diagnostic model, trained exactly as the build does.
fn shipped_model() -> &'static model::Model {
    static M: std::sync::OnceLock<model::Model> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        let text = include_str!("../../../ai/scenarios.txt");
        let mut s = [scenario::Scenario::EMPTY; scenario::MAX_SCENARIOS];
        let n = scenario::parse(text, &mut s).expect("the shipped scenarios parse");
        let mut tr = vec![scenario::Example::ZERO; model::MAX_EXAMPLES];
        let mut te = vec![scenario::Example::ZERO; model::MAX_EXAMPLES];
        model::train_from_scenarios(&s[..n], &model::SHIPPED, &mut tr, &mut te).0
    })
}

fn random_features(rng: &mut Rng, hostile: bool) -> [i32; sysview::FEATURES] {
    let mut x = [0i32; sysview::FEATURES];
    for v in x.iter_mut() {
        *v = if hostile && rng.one_in(3) {
            let r = rng.u32() as i32;
            *rng.pick(&[i32::MIN, -1, 1001, i32::MAX, r])
        } else {
            rng.below(1001) as i32
        };
    }
    x
}

fn t_infer(mode: Mode) {
    run(
        Target {
            name: "infer::Request+Reply::decode",
            cost: 1,
            parallel: false,
            aux: &["requests", "replies"],
        },
        mode,
        || {
            let m = shipped_model();
            let digest = kernel_core::sha256::digest(&model::encode(m));
            let mut rng = Rng(7);
            let mut seeds = Vec::new();
            for k in 0..8u64 {
                let req = infer::Request {
                    nonce: [0, 1, u64::MAX, k << 40][k as usize % 4],
                    x: random_features(&mut rng, false),
                };
                assert_eq!(infer::Request::decode(&req.encode()), Ok(req));
                seeds.push(req.encode().to_vec());
                let rep = infer::answer(m, &digest, &req);
                assert_eq!(infer::Reply::decode(&rep.encode()), Ok(rep));
                seeds.push(rep.encode().to_vec());
            }
            seeds
        },
        |seeds, rng, ctx| {
            let fields = [
                (0, 8, false),
                (8, 4, false),
                (8, 1, false),
                (12, 4, false),
                (16, 8, false),
                (24, 8, false),
                (32, 8, false),
                (68, 4, false),
            ];
            let b = gen_bytes_f(rng, seeds, &[], &fields, 100);
            ctx.note(&b);
            let req = infer::Request::decode(&b);
            let rep = infer::Reply::decode(&b);
            if let Ok(r) = req {
                assert_eq!(&r.encode()[..], &b[..]);
                assert!(r.x.iter().all(|v| (0..=sysview::FEATURE_MAX).contains(v)));
                ctx.count(0);
            }
            if let Ok(r) = rep {
                assert_eq!(&r.encode()[..], &b[..]);
                for (c, s) in scenario::Condition::ALL.iter().zip(r.scores) {
                    assert_eq!(r.conditions.has(*c), s > 0);
                }
                assert_eq!(r.conditions.0 & !0b111, 0);
                ctx.count(1);
            }
            for e in [req.err(), rep.err()].into_iter().flatten() {
                let _ = e.name();
            }
            req.is_ok() || rep.is_ok()
        },
    );
}

// --- policy ------------------------------------------------------------------

fn policy_record(
    action: policy::Action,
    condition: scenario::Condition,
    target: &str,
) -> policy::Record {
    let mut t = [0u8; policy::NAME_LEN];
    t[..target.len()].copy_from_slice(target.as_bytes());
    policy::Record {
        action,
        condition,
        target: t,
        model: [1; 32],
        view: [2; 32],
    }
}

fn policy_seeds() -> Vec<Vec<u8>> {
    use policy::Action::{ResumeScheduler, RetryService};
    use scenario::Condition::{DenialBurst, SchedulerPaused, ServiceFailed};
    let mut seeds: Vec<Vec<u8>> = [
        policy_record(ResumeScheduler, SchedulerPaused, ""),
        policy_record(RetryService, ServiceFailed, "flapd"),
        policy_record(RetryService, ServiceFailed, "a"),
        policy_record(RetryService, ServiceFailed, "abcdefghijklm-9"),
    ]
    .iter()
    .map(|r| {
        assert_eq!(policy::decode(&r.encode()), Ok(*r));
        r.encode().to_vec()
    })
    .collect();
    // A refused shape is a seed too: the closed condition -> action table.
    seeds.push(
        policy_record(RetryService, DenialBurst, "x")
            .encode()
            .to_vec(),
    );
    seeds
}

fn t_policy_decode(mode: Mode) {
    run(
        Target {
            name: "policy::decode",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        policy_seeds,
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, &[b"[ITISYOU:", b"\0"], 120);
            ctx.note(&b);
            match policy::decode(&b) {
                Ok(r) => {
                    assert_eq!(&r.encode()[..], &b[..]);
                    assert_eq!(policy::allowed_action(r.condition), Some(r.action));
                    let t = r.target_str();
                    assert_eq!(r.action.has_target(), !t.is_empty());
                    assert!(t
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'));
                    let _ = (
                        r.action.name(),
                        r.action.risk(),
                        r.action.rollback(),
                        r.action.verify_ticks(),
                        r.action.reversible(),
                    );
                    ctx.count(0);
                    true
                }
                Err(e) => {
                    let _ = e.name();
                    false
                }
            }
        },
    );
}

const POLICY_EVENTS: [policy::Event; 6] = [
    policy::Event::Approve,
    policy::Event::Deny,
    policy::Event::Expire,
    policy::Event::Execute,
    policy::Event::Pass,
    policy::Event::Fail,
];

fn t_policy_table(mode: Mode) {
    use policy::{Refusal, State, TransitionError, MAX_PROPOSALS};
    run(
        Target {
            name: "policy::Table",
            cost: 4,
            parallel: true,
            aux: &["steps", "filed"],
        },
        mode,
        policy_seeds,
        |seeds, rng, ctx| {
            let mut t = policy::Table::new();
            let mut issued: Vec<u64> = Vec::new();
            let mut max_id = 0u64;
            let mut now = rng.next() % 100_000;
            let mut filed = false;
            for _ in 0..rng.range(1, 48) {
                ctx.count(0);
                match rng.below(8) {
                    0..=2 => {
                        let b = gen_bytes(rng, seeds, &[], 100);
                        ctx.note(&b);
                        if let Ok(rec) = policy::decode(&b) {
                            let who = rng.below(6) as u64;
                            let entries: Vec<_> = t.entries().collect();
                            let pending = entries
                                .iter()
                                .any(|e| e.submitter == who && e.state == State::Pending);
                            let full = entries.len() == MAX_PROPOSALS
                                && entries.iter().all(|e| !e.state.is_final());
                            match t.insert(who, rec, now) {
                                Ok(id) => {
                                    assert!(!pending && !full);
                                    assert!(id > max_id, "an id was reused");
                                    max_id = id;
                                    issued.push(id);
                                    let e = t.get(id).expect("just filed");
                                    assert_eq!(
                                        (e.state, e.submitter, e.record),
                                        (State::Pending, who, rec)
                                    );
                                    filed = true;
                                    ctx.count(1);
                                }
                                Err(Refusal::AlreadyPending) => assert!(pending),
                                Err(Refusal::TableFull) => assert!(full && !pending),
                                Err(e) => panic!("insert refused with {e:?}"),
                            }
                        }
                    }
                    3 | 4 => {
                        let id = if !issued.is_empty() && !rng.one_in(4) {
                            *rng.pick(&issued)
                        } else {
                            rng.interesting(8)
                        };
                        let ev = *rng.pick(&POLICY_EVENTS);
                        let before = t.get(id);
                        match t.transition(id, ev) {
                            Ok(s) => {
                                let b =
                                    before.expect("transitioned a proposal that does not exist");
                                assert_eq!(policy::next(b.state, ev), Some(s));
                                assert_eq!(t.get(id).map(|e| e.state), Some(s));
                            }
                            Err(TransitionError::NoSuchProposal) => assert!(before.is_none()),
                            Err(e @ TransitionError::NotAllowed(s)) => {
                                assert_eq!(before.map(|e| e.state), Some(s));
                                assert_eq!(policy::next(s, ev), None);
                                let _ = e.name();
                            }
                        }
                    }
                    5 => {
                        now = now.saturating_add(rng.interesting(64));
                        t.expire(now, |e| {
                            assert_eq!(e.state, State::Expired);
                            assert!(now.saturating_sub(e.filed) >= policy::TTL_TICKS);
                        });
                    }
                    6 => {
                        let _ = t.get(rng.next());
                    }
                    _ => now = now.wrapping_add(rng.below(10_000) as u64),
                }
                let entries: Vec<_> = t.entries().collect();
                assert!(entries.len() <= MAX_PROPOSALS);
                assert!(
                    entries.windows(2).all(|w| w[0].id < w[1].id),
                    "oldest first, ids unique"
                );
                for s in 0..6 {
                    let n = entries
                        .iter()
                        .filter(|e| e.submitter == s && e.state == State::Pending)
                        .count();
                    assert!(n <= 1, "two pending proposals from one submitter");
                }
            }
            filed
        },
    );
}

// --- initctl -----------------------------------------------------------------

fn t_initctl(mode: Mode) {
    use initctl::{Ack, Command, Op};
    run(
        Target {
            name: "initctl::Command+Ack::decode",
            cost: 1,
            parallel: false,
            aux: &["commands", "acks"],
        },
        mode,
        || {
            let mut seeds = Vec::new();
            for (seq, op, name) in [
                (7, Op::Retry, "flapd"),
                (0, Op::Stop, "a"),
                (u32::MAX, Op::Retry, "abcdefghijklmno"),
            ] {
                let c = Command::new(seq, op, name).unwrap();
                assert_eq!(Command::decode(&c.encode()), Ok(c));
                seeds.push(c.encode().to_vec());
            }
            for (seq, ok, pid) in [(1, true, 9), (u32::MAX, false, 0), (3, true, u64::MAX)] {
                let a = Ack { seq, ok, pid };
                assert_eq!(Ack::decode(&a.encode()), Ok(a));
                seeds.push(a.encode().to_vec());
            }
            seeds
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, &[b"\0", b"["], 40);
            ctx.note(&b);
            let c = Command::decode(&b);
            let a = Ack::decode(&b);
            if let Ok(c) = c {
                assert_eq!(&c.encode()[..], &b[..]);
                assert_eq!(Command::new(c.seq, c.op, c.name_str()), Some(c));
                let _ = c.op.name();
                ctx.count(0);
            }
            if let Ok(a) = a {
                assert_eq!(&a.encode()[..], &b[..]);
                ctx.count(1);
            }
            for e in [c.err(), a.err()].into_iter().flatten() {
                let _ = e.name();
            }
            c.is_ok() || a.is_ok()
        },
    );
}

// --- wm ----------------------------------------------------------------------

fn random_wm_event(rng: &mut Rng) -> wm::Event {
    match rng.below(4) {
        0 => wm::Event::FocusIn,
        1 => wm::Event::FocusOut,
        2 => wm::Event::Key(rng.byte()),
        _ => wm::Event::Click {
            x: rng.u16(),
            y: rng.u16(),
        },
    }
}

fn hostile_i32(rng: &mut Rng) -> i32 {
    match rng.below(4) {
        0 => *rng.pick(&[i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX]),
        1 => rng.u32() as i32,
        _ => rng.below(2000) as i32 - 500,
    }
}

fn hostile_u32(rng: &mut Rng) -> u32 {
    if rng.one_in(3) {
        rng.u32()
    } else {
        rng.below(3000) as u32
    }
}

fn t_wm(mode: Mode) {
    run(
        Target {
            name: "wm::decode+EventRing+hit",
            cost: 2,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            [
                wm::Event::FocusIn,
                wm::Event::FocusOut,
                wm::Event::Key(b'a'),
                wm::Event::Key(0x1b),
                wm::Event::Click { x: 0, y: 0 },
                wm::Event::Click { x: 199, y: 65535 },
            ]
            .iter()
            .map(|e| {
                assert_eq!(wm::decode(&wm::encode(*e)), Some(*e));
                wm::encode(*e).to_vec()
            })
            .collect::<Vec<_>>()
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, &[], 16);
            ctx.note(&b);
            let ev = wm::decode(&b);
            if let Some(e) = ev {
                assert_eq!(&wm::encode(e)[..], &b[..]);
                ctx.count(0);
            }
            // The event ring against a drop-oldest FIFO model.
            let mut ring = wm::EventRing::new();
            let mut model = std::collections::VecDeque::new();
            let mut dropped = 0u64;
            for _ in 0..rng.range(0, 40) {
                if rng.one_in(3) {
                    assert_eq!(ring.pop(), model.pop_front());
                } else {
                    let e = match ev {
                        Some(e) if rng.one_in(2) => e,
                        _ => random_wm_event(rng),
                    };
                    ring.push(e);
                    if model.len() == wm::EVENT_RING {
                        model.pop_front();
                        dropped += 1;
                    }
                    model.push_back(e);
                }
                assert_eq!(
                    (ring.len(), ring.is_empty(), ring.dropped()),
                    (model.len(), model.is_empty(), dropped)
                );
            }
            // Hit testing with hostile frames against brute force.
            let frames: Vec<wm::Frame> = (0..rng.below(6))
                .map(|k| wm::Frame {
                    id: k as u32,
                    x: hostile_i32(rng),
                    y: hostile_i32(rng),
                    w: hostile_u32(rng),
                    h: hostile_u32(rng),
                })
                .collect();
            let (x, y) = (hostile_i32(rng), hostile_i32(rng));
            let want = frames
                .iter()
                .rev()
                .find(|f| {
                    let (fx, fy) = (i64::from(f.x), i64::from(f.y));
                    let (px, py) = (i64::from(x), i64::from(y));
                    px >= fx && py >= fy && px < fx + i64::from(f.w) && py < fy + i64::from(f.h)
                })
                .map(|f| f.id);
            let hit = wm::hit(&frames, x, y);
            assert_eq!(hit, want);
            let focus = if rng.one_in(2) {
                None
            } else {
                Some(rng.u32() % 6)
            };
            if let Some(c) = wm::click(focus, hit) {
                assert_eq!(c.out, focus);
                assert_eq!(Some(c.into), hit);
                assert_ne!(Some(c.into), focus);
            }
            let mut order: Vec<u32> = (0..frames.len() as u32).collect();
            let id = rng.u32() % 8;
            wm::raise(&mut order, id);
            if (id as usize) < frames.len() {
                assert_eq!(order.last(), Some(&id));
            }
            // Window text a program chose, rendered glyph by glyph.
            for &c in &b {
                let (col, row) = (rng.below(10), rng.below(10));
                let set = font::pixel(c, col, row);
                assert!(!set || (col < font::GLYPH_WIDTH && row < font::GLYPH_HEIGHT));
                let _ = font::glyph(c);
            }
            ev.is_some()
        },
    );
}

// --- caps --------------------------------------------------------------------

const CAP_NAMES: &[&str] = &[
    "spawn",
    "ipc",
    "gui",
    "dev",
    "fs_read",
    "audio",
    "sys_admin",
    "fs_write",
    "network",
    "proc_control",
    "service",
    "sys_view",
    "propose",
];

const CAPS_DICT: &[&[u8]] = &[
    b"spawn",
    b"ipc",
    b"gui",
    b"dev",
    b"fs_read",
    b"audio",
    b"sys_admin",
    b"fs_write",
    b"network",
    b"proc_control",
    b"service",
    b"sys_view",
    b"propose",
    b",",
    b" ",
    b"\t",
    b"\xe3\x80\x80",
    b"GUI",
    b"-",
];

fn t_caps(mode: Mode) {
    run(
        Target {
            name: "caps::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let all = CAP_NAMES.join(",");
            text_seeds(&[
                "",
                "gui",
                "gui,fs_read",
                " spawn , ipc ",
                &all,
                "ipc,ipc",
                "ipc:6-7,fs_read",
                "ipc:2-3",
                "ipc:0-7,service",
                "ipc:2,ipc:3",
            ])
        },
        |seeds, rng, ctx| {
            let s = gen_text(rng, seeds, CAPS_DICT, 200);
            ctx.note(s.as_bytes());
            let parent = rng.next();
            let rights = capability::rights_from_bits(parent);
            assert!(rights.iter().all(|r| r & !capability::rights::ALL == 0));
            match caps::parse(&s) {
                Ok(bits) => {
                    assert_eq!(bits & !caps::CAP_ALL_KNOWN, 0);
                    // V1.0 (ADR-0025): every parsed set is written back as a
                    // list that parses to the same bits, channels included.
                    let written = format!("{}", caps::Describe(bits));
                    assert_eq!(caps::parse(&written), Ok(bits), "{written}");
                    for n in caps::names(bits).filter(|&n| n != "ipc") {
                        assert_eq!(caps::parse(n).map(caps::name_of), Ok(n));
                    }
                    if bits & caps::CAP_IPC != 0 {
                        assert!(caps::ipc_channels(bits).is_some());
                    }
                    let d = caps::delegate(parent, bits);
                    assert_eq!(d & !(parent & bits), 0, "delegation amplified");
                    assert_eq!(d & caps::CAP_CONSOLE_ONLY, 0);
                    ctx.count(0);
                    true
                }
                Err(_) => false,
            }
        },
    );
}

// --- path --------------------------------------------------------------------

/// Unbounded reference normalization: the components and the deepest the
/// walk went (`None` if it escapes the root).
fn reference_normalize(p: &str) -> Option<(Vec<&str>, usize)> {
    let mut stack = Vec::new();
    let mut deepest = 0;
    for comp in p.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            c => {
                stack.push(c);
                deepest = deepest.max(stack.len());
            }
        }
    }
    Some((stack, deepest))
}

const PATH_DICT: &[&[u8]] = &[
    b"/",
    b"..",
    b".",
    b"//",
    b"/../",
    b"/./",
    b"apps",
    b"hello",
    b"bin",
    b"\0",
    b"\\",
    b"%2e%2e",
    b"\xe2\x80\xae",
];

fn t_path(mode: Mode) {
    run(
        Target {
            name: "path::validate+normalized+is_within",
            cost: 2,
            parallel: false,
            aux: &["valid", "deeper_than_32"],
        },
        mode,
        || {
            let deep: String = (0..40).map(|i| format!("/d{i}")).collect();
            let long = format!("/{}", "x".repeat(path::MAX_COMPONENT));
            let longer = format!("/{}", "x".repeat(path::MAX_COMPONENT + 1));
            text_seeds(&[
                "/etc/version",
                "//etc/./version/",
                "/a/b/../c",
                "/apps/hello/../../bin/init",
                "/apps/hello/data.txt",
                "/",
                "relative/path",
                "/../etc",
                &deep,
                &long,
                &longer,
            ])
        },
        |seeds, rng, ctx| {
            let p = gen_text(rng, seeds, PATH_DICT, 400);
            let prefix = gen_text(rng, seeds, PATH_DICT, 160);
            ctx.note(p.as_bytes());
            let comps: Vec<&str> = path::normalized(&p).collect();
            assert!(comps.len() <= 32);
            for c in &comps {
                assert!(!c.is_empty() && *c != "." && *c != "..");
            }
            let within = path::is_within(&prefix, &p);
            match path::validate(&p) {
                Ok(()) => {
                    assert!(p.starts_with('/'));
                    assert!(comps.iter().all(|c| c.len() <= path::MAX_COMPONENT));
                    assert!(path::is_within("/", &p) && path::is_within(&p, &p));
                    let (want, deepest) = reference_normalize(&p).expect("validated path escapes");
                    if deepest <= 32 {
                        assert_eq!(comps, want, "normalization differs from the reference");
                    } else {
                        // Documented: the walk keeps at most 32 components.
                        ctx.count(1);
                    }
                    if within {
                        assert!(path::validate(&prefix).is_ok());
                        let (pre, pre_deepest) = reference_normalize(&prefix).unwrap();
                        if deepest <= 32 && pre_deepest <= 32 {
                            assert!(
                                want.starts_with(&pre),
                                "is_within accepted a path outside the prefix"
                            );
                        }
                    }
                    ctx.count(0);
                    true
                }
                Err(_) => {
                    assert!(!within && !path::is_within("/", &p));
                    false
                }
            }
        },
    );
}

// --- shellparse --------------------------------------------------------------

fn t_shellparse(mode: Mode) {
    run(
        Target {
            name: "shellparse::parse",
            cost: 1,
            parallel: false,
            aux: &["lines"],
        },
        mode,
        || {
            let max: String = (0..shellparse::MAX_ARGS)
                .map(|i| format!("t{i} "))
                .collect();
            let over = format!("{max} more");
            text_seeds(&[
                "cat /etc/version",
                "  echo   a\t b ",
                "run /bin/args-probe - - -- a b c",
                "",
                "   \t  ",
                &max,
                &over,
            ])
        },
        |seeds, rng, ctx| {
            let s = gen_text(
                rng,
                seeds,
                &[b" ", b"\t", b"\n", b"\x0c", b"--", b"\xc2\xa0"],
                400,
            );
            ctx.note(s.as_bytes());
            let tokens: Vec<&str> = s.split_ascii_whitespace().collect();
            match shellparse::parse(&s) {
                Ok(cl) => {
                    assert!(tokens.len() <= shellparse::MAX_ARGS);
                    assert_eq!(cl.command(), tokens.first().copied());
                    assert_eq!(cl.is_empty(), tokens.is_empty());
                    assert_eq!(cl.args(), tokens.get(1..).unwrap_or(&[]));
                    ctx.count(0);
                    true
                }
                Err(shellparse::ParseError::TooManyArgs) => {
                    assert!(tokens.len() > shellparse::MAX_ARGS);
                    false
                }
            }
        },
    );
}

// --- linebuf -----------------------------------------------------------------

enum LineOp {
    Push(Vec<u8>),
    Prompt,
    Flush,
}

/// What reaches the port: every emitted chunk neutralized on its own, as
/// `console_out::emit` does, then concatenated. Each chunk's escaped console
/// text is checked on the way.
fn linebuf_wire<const N: usize>(ops: &[LineOp]) -> Vec<u8> {
    let mut b = linebuf::LineBuf::<N>::new();
    let mut out = Vec::new();
    let mut emit = |c: &[u8]| {
        let mut c = c.to_vec();
        linebuf::neutralize_markers(&mut c);
        let mut text = String::new();
        linebuf::write_escaped(&c, &mut text).unwrap();
        assert!(text.chars().all(|ch| ch == '\n'
            || ch == '\t'
            || !(ch.is_control() || linebuf::reorders_or_hides(ch))));
        out.extend_from_slice(&c);
    };
    for op in ops {
        match op {
            LineOp::Push(bytes) => b.push(bytes, &mut emit),
            LineOp::Prompt => b.flush_partial(&mut emit),
            LineOp::Flush => b.flush(&mut emit),
        }
        assert!(b.len() <= N);
    }
    assert!(b.is_empty());
    out
}

fn has_kernel_marker(s: &[u8]) -> bool {
    s.windows(linebuf::KERNEL_MARKER.len())
        .any(|w| w == linebuf::KERNEL_MARKER)
}

const LINE_PIECES: &[&[u8]] = &[
    b"[ITISYOU:",
    b"[ITI",
    b"SYOU:",
    b"[",
    b"[ITISYOU",
    b"U:",
    b"xyz",
    b"\n",
    b"\r",
    b"\t",
    b"\x1b[2J",
    b"\xe2\x80\xae",
    b"\xc2\x9b",
    b"\xff",
    b"[RING3-U:",
];

fn t_linebuf(mode: Mode) {
    run(
        Target {
            name: "linebuf::LineBuf+neutralize_markers+write_escaped",
            cost: 6,
            parallel: true,
            aux: &["bytes"],
        },
        mode,
        || (),
        |_, rng, ctx| {
            let mut stream = Vec::new();
            for _ in 0..rng.range(1, 80) {
                if rng.one_in(4) {
                    let n = rng.range(1, 6);
                    stream.extend(rng.bytes(n));
                } else {
                    stream.extend_from_slice(rng.pick(LINE_PIECES));
                }
            }
            ctx.note(&stream);
            ctx.aux[0] += stream.len() as u64;
            let mut ops = Vec::new();
            let mut i = 0;
            while i < stream.len() {
                let n = rng.range(1, 40).min(stream.len() - i);
                ops.push(LineOp::Push(stream[i..i + n].to_vec()));
                if rng.one_in(4) {
                    ops.push(LineOp::Prompt);
                }
                i += n;
            }
            ops.push(LineOp::Flush);
            let a = linebuf_wire::<9>(&ops);
            let b = linebuf_wire::<16>(&ops);
            let c = linebuf_wire::<256>(&ops);
            // Every byte arrives once, in order, as if the whole stream had
            // been neutralized at once (plus the exit flush's newline), and no
            // kernel marker survives on the wire, however the stream was cut.
            let mut whole = stream.clone();
            let n = linebuf::neutralize_markers(&mut whole);
            assert!(!has_kernel_marker(&whole));
            assert!(n <= stream.len() / linebuf::KERNEL_MARKER.len());
            for w in [&a, &b, &c] {
                assert!(!has_kernel_marker(w), "a kernel marker reached the wire");
                assert_eq!(&w[..whole.len()], &whole[..]);
                assert!(w.len() - whole.len() <= 1);
            }
            assert_eq!(a, b);
            assert_eq!(b, c);
            let k = linebuf::marker_start_at_end(&stream);
            assert!(k < linebuf::KERNEL_MARKER.len());
            assert!(stream.ends_with(&linebuf::KERNEL_MARKER[..k]));
            true
        },
    );
}

// --- audit_chain::escape_untrusted ------------------------------------------

const ESCAPE_DICT: &[&[u8]] = &[
    b"[ITISYOU:",
    b"[ITISYOU",
    b"[RING3-U:",
    b"\n",
    b"\r",
    b"\\",
    b"\"",
    b"\\x0a",
    b"\xe2\x80\xae",
    b"\xe2\x80\x8b",
    b"\xef\xbb\xbf",
    b"\xc2\x85",
    b"\xc2\x9b",
    b"\xe2\x81\xa0",
    b"\xc3\xa9",
    b"\xe4\xb8\xad",
    b"\x7f",
];

fn t_escape(mode: Mode) {
    run(
        Target {
            name: "audit_chain::escape_untrusted",
            cost: 1,
            parallel: false,
            aux: &["unchanged"],
        },
        mode,
        || {
            let all: String = (0u32..0x300).filter_map(char::from_u32).collect();
            text_seeds(&[
                "path=/data/notes.txt bytes=4",
                "path=/data/a\n[ITISYOU:AUDIT] forged",
                "\u{e9}[ITISYOU:[ITISYOU:x",
                "/data/\u{202e}:UOYSITI]",
                "[ITI\u{2060}SYOU:AUDIT]",
                "say \"hi\" \\x0a",
                &all,
            ])
        },
        |seeds, rng, ctx| {
            let s = gen_text(rng, seeds, ESCAPE_DICT, 400);
            ctx.note(s.as_bytes());
            let mut out = String::new();
            audit_chain::escape_untrusted(&s, &mut out).unwrap();
            // One line of printable text, no kernel marker, nothing that
            // reorders or hides what the operator reads.
            assert!(!out
                .chars()
                .any(|c| c.is_control() || linebuf::reorders_or_hides(c)));
            assert!(!out.contains("[ITISYOU:"));
            assert_eq!(marker::parse_line(&out), None);
            let plain = s.chars().all(|c| {
                !c.is_control() && !linebuf::reorders_or_hides(c) && c != '\\' && c != '"'
            });
            if plain && !s.contains("[ITISYOU:") {
                assert_eq!(out, s, "harmless text must pass through unchanged");
                ctx.count(0);
            }
            true
        },
    );
}

// --- capability --------------------------------------------------------------

#[derive(Clone, Copy)]
struct Issued {
    raw: u64,
    owner: u64,
    kind: capability::CapabilityKind,
    scope: capability::ResourceScope,
    rights: u32,
    expires: Option<u64>,
    delegable: bool,
    live: bool,
}

fn random_scope(rng: &mut Rng) -> capability::ResourceScope {
    if rng.one_in(4) {
        return capability::ResourceScope::ANY;
    }
    let a = rng.interesting(100).min(1000);
    let b = rng.interesting(100).min(1000);
    capability::ResourceScope {
        start: a.min(b),
        end: a.max(b),
    }
}

fn t_capability(mode: Mode) {
    use capability::{CapabilityError as E, CapabilityHandle, CapabilityKind};
    const SLOTS: usize = 6;
    run(
        Target {
            name: "capability::CapabilityTable",
            cost: 4,
            parallel: true,
            aux: &["steps", "forged_refused"],
        },
        mode,
        || (),
        |_, rng, ctx| {
            let mut t = capability::CapabilityTable::<SLOTS>::new();
            let mut model: Vec<Issued> = Vec::new();
            let mut now = rng.below(100) as u64;
            let mut granted = false;
            for _ in 0..rng.range(1, 40) {
                ctx.count(0);
                // A handle: an issued one (live or dead), one with a bit
                // flipped, or a guessed integer.
                let raw = match rng.below(4) {
                    0 | 1 if !model.is_empty() => rng.pick(&model).raw,
                    2 if !model.is_empty() => rng.pick(&model).raw ^ (1u64 << rng.below(64)),
                    _ => rng.interesting(SLOTS),
                };
                ctx.note(&raw.to_le_bytes());
                let h = CapabilityHandle::from_raw(raw);
                let live = model.iter().position(|m| m.live && m.raw == raw);
                let owner = rng.below(4) as u64;
                let kind = CapabilityKind::from_index(rng.below(CapabilityKind::COUNT)).unwrap();
                let scope = random_scope(rng);
                let extra = if rng.one_in(8) { 1 << 7 } else { 0 };
                let rights = rng.u32() & (capability::rights::ALL | extra);
                let expiry = if rng.one_in(3) {
                    Some(now + rng.below(50) as u64)
                } else {
                    None
                };
                // What the table's `entry` lookup decides for this handle.
                let found = live
                    .map(|i| model[i])
                    .filter(|e| !e.expires.is_some_and(|d| now >= d));
                match rng.below(6) {
                    0 => {
                        let delegable = rng.one_in(2);
                        let live_n = model.iter().filter(|m| m.live).count();
                        match t.grant(owner, kind, scope, rights, expiry, delegable) {
                            Ok(nh) => {
                                assert!(live_n < SLOTS);
                                assert!(
                                    model.iter().all(|m| m.raw != nh.raw()),
                                    "a handle value was issued twice"
                                );
                                model.push(Issued {
                                    raw: nh.raw(),
                                    owner,
                                    kind,
                                    scope,
                                    rights,
                                    expires: expiry,
                                    delegable,
                                    live: true,
                                });
                                granted = true;
                            }
                            Err(E::TableFull) => assert_eq!(live_n, SLOTS),
                            Err(e) => panic!("grant failed with {e:?}"),
                        }
                    }
                    1 => {
                        let got = t.check(h, owner, kind, scope, rights, now);
                        match found {
                            None => {
                                assert!(
                                    matches!(got, Err(E::InvalidHandle | E::Revoked | E::Expired)),
                                    "{got:?}"
                                );
                                ctx.count(1);
                            }
                            Some(e) if e.owner != owner => assert_eq!(got, Err(E::NotOwner)),
                            Some(e) if e.kind != kind => assert_eq!(got, Err(E::WrongKind)),
                            Some(e) if !e.scope.contains(scope) || rights & !e.rights != 0 => {
                                assert_eq!(got, Err(E::ScopeDenied))
                            }
                            Some(_) => assert_eq!(got, Ok(())),
                        }
                    }
                    2 => {
                        let child = rng.below(4) as u64;
                        let got = t.delegate(h, owner, child, scope, rights, now);
                        match found {
                            None => assert!(matches!(
                                got,
                                Err(E::InvalidHandle | E::Revoked | E::Expired)
                            )),
                            Some(e) if e.owner != owner => assert_eq!(got, Err(E::NotOwner)),
                            Some(e) if !e.delegable => assert_eq!(got, Err(E::NotDelegable)),
                            Some(e) if !e.scope.contains(scope) || rights & !e.rights != 0 => {
                                assert_eq!(got, Err(E::ScopeDenied))
                            }
                            Some(e) => match got {
                                Ok(nh) => model.push(Issued {
                                    raw: nh.raw(),
                                    owner: child,
                                    kind: e.kind,
                                    scope,
                                    rights,
                                    expires: e.expires,
                                    delegable: true,
                                    live: true,
                                }),
                                Err(err) => assert_eq!(err, E::TableFull),
                            },
                        }
                    }
                    3 => {
                        let got = t.revoke(h, owner, now);
                        match found {
                            None => assert!(got.is_err()),
                            Some(e) if e.owner != owner => assert_eq!(got, Err(E::NotOwner)),
                            Some(_) => {
                                assert_eq!(got, Ok(()));
                                model[live.unwrap()].live = false;
                            }
                        }
                    }
                    4 => {
                        let got = t.restrict(h, owner, scope, rights, expiry, now);
                        match found {
                            None => assert!(got.is_err()),
                            Some(e) if e.owner != owner => assert_eq!(got, Err(E::NotOwner)),
                            Some(e) if !e.scope.contains(scope) => {
                                assert_eq!(got, Err(E::ScopeDenied))
                            }
                            Some(e) => {
                                let nh = got.expect("restricting a live, owned handle");
                                model[live.unwrap()].live = false;
                                let expires = match (e.expires, expiry) {
                                    (Some(o), Some(n)) => Some(o.min(n)),
                                    (Some(o), None) => Some(o),
                                    (None, n) => n,
                                };
                                model.push(Issued {
                                    raw: nh.raw(),
                                    owner,
                                    kind: e.kind,
                                    scope,
                                    rights: rights & e.rights,
                                    expires,
                                    delegable: e.delegable,
                                    live: true,
                                });
                            }
                        }
                    }
                    _ => {
                        now += rng.below(20) as u64;
                        if rng.one_in(3) {
                            let n = t.revoke_owner(owner);
                            let mut want = 0;
                            for m in model.iter_mut().filter(|m| m.live && m.owner == owner) {
                                m.live = false;
                                want += 1;
                            }
                            assert_eq!(n, want);
                        }
                    }
                }
                for o in 0..4 {
                    let want = model.iter().filter(|m| m.live && m.owner == o).count();
                    assert_eq!(t.count_owned(o), want);
                }
            }
            granted
        },
    );
}

// --- procstatus --------------------------------------------------------------

fn t_procstatus(mode: Mode) {
    run(
        Target {
            name: "procstatus::decode",
            cost: 1,
            parallel: false,
            aux: &[],
        },
        mode,
        || (),
        |_, rng, ctx| {
            let w = match rng.below(5) {
                0 => rng.interesting(0),
                1 => rng.next(),
                2 => ((rng.below(4) as u64) << 32) | (rng.interesting(0) & 0xffff_ffff),
                3 => u64::MAX - rng.below(8) as u64,
                _ => rng.next() & 0x3_ffff_ffff,
            };
            ctx.note(&w.to_le_bytes());
            match procstatus::decode(w) {
                Some(s) => {
                    assert_eq!(procstatus::encode(s), w);
                    assert!(w < 3 << 32);
                    let _ = s.is_clean();
                    true
                }
                None => false,
            }
        },
    );
}

// ===========================================================================
// Bytes a disk, the initramfs or a package supplies
// ===========================================================================

targets! {
    manifest_parse, full_manifest_parse => t_manifest;
    pkg_parse, full_pkg_parse => t_pkg;
    trust_certificates, full_trust_certificates => t_trust;
    ed25519_verify, full_ed25519_verify => t_ed25519;
    itfs_superblock_decode, full_itfs_superblock_decode => t_itfs;
    initconf_parse, full_initconf_parse => t_initconf;
    model_decode, full_model_decode => t_model;
    audit_chain_parse_header, full_audit_chain_parse_header => t_audit_header;
    tar_entries, full_tar_entries => t_tar;
    update_parse_store_name, full_update_parse_store_name => t_update;
    scenario_parse, full_scenario_parse => t_scenario;
    sha_streaming, full_sha_streaming => t_sha;
}

// --- manifest ------------------------------------------------------------------

const MANIFEST_DICT: &[&[u8]] = &[
    b"name=",
    b"version=",
    b"caps=",
    b"\n",
    b"\r\n",
    b"#",
    b"=",
    b",",
    b" ",
    b".",
    b"-",
    b"gui",
    b"fs_read",
    b"sys_view",
    b"kernel",
    b"xxxxxxxxxxxxxxxx",
    b"1.0.0",
];

fn t_manifest(mode: Mode) {
    run(
        Target {
            name: "manifest::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let all = format!("name=all-caps\nversion=1\ncaps={}\n", CAP_NAMES.join(","));
            let seeds = text_seeds(&[
                "# hello app\nname=hello-app\nversion=1.0.0\ncaps=gui,fs_read\n",
                "name=quiet\nversion=1\ncaps=\n",
                "  name = hello-args \r\nversion=2.10\r\ncaps= ipc , fs_read \r\n",
                "name=abcdefghijklmn-9\nversion=1234567890.12345\ncaps=network\n",
                &all,
            ]);
            for s in &seeds {
                assert!(manifest::parse(std::str::from_utf8(s).unwrap()).is_ok());
            }
            seeds
        },
        |seeds, rng, ctx| {
            let s = gen_text(rng, seeds, MANIFEST_DICT, 1100);
            ctx.note(s.as_bytes());
            match manifest::parse(&s) {
                Ok(m) => {
                    assert!(s.len() <= manifest::MAX_MANIFEST_LEN);
                    let n = m.name.as_bytes();
                    assert!(!n.is_empty() && n.len() <= manifest::MAX_NAME_LEN && n[0] != b'-');
                    assert!(n
                        .iter()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-'));
                    let v = m.version.as_bytes();
                    assert!(!v.is_empty() && v.len() <= manifest::MAX_VERSION_LEN);
                    assert!(v.iter().all(|b| b.is_ascii_digit() || *b == b'.'));
                    assert!(v[0] != b'.' && v[v.len() - 1] != b'.');
                    assert_eq!(m.caps & !caps::CAP_ALL_KNOWN, 0);
                    // The canonical spelling parses to the same manifest.
                    let names: Vec<&str> = caps::names(m.caps).collect();
                    let text = format!(
                        "name={}\nversion={}\ncaps={}\n",
                        m.name,
                        m.version,
                        names.join(",")
                    );
                    assert_eq!(manifest::parse(&text), Ok(m));
                    ctx.count(0);
                    true
                }
                Err(_) => false,
            }
        },
    );
}

// --- pkg + the key hierarchy -----------------------------------------------------

/// The published fixture signing seeds from `kernel/build.rs`. Not secrets:
/// the build signs the image's fixture packages with them so anyone can
/// reproduce it, and `keys/certs/` certifies (or deliberately does not
/// certify) each one.
const DEV_SIGNING_SECRET: [u8; 32] = *b"itisyou-os dev package signer v8";
const FOREIGN_SIGNING_SECRET: [u8; 32] = *b"itisyou-os foreign signer -----8";
const RETIRED_SIGNING_SECRET: [u8; 32] = *b"itisyou-os retired signer -----9";
const EXPIRED_SIGNING_SECRET: [u8; 32] = *b"itisyou-os expired signer -----9";
const ROGUE_SIGNING_SECRET: [u8; 32] = *b"itisyou-os rogue signer -------9";

/// `TRUST_EPOCH` in `kernel/src/platform.rs`.
const KERNEL_TRUST_EPOCH: u32 = 9;

const REAL_CERTS: [&[u8]; 5] = [
    include_bytes!("../../../keys/certs/test-signer.cert"),
    include_bytes!("../../../keys/certs/release-signer.cert"),
    include_bytes!("../../../keys/certs/retired-signer.cert"),
    include_bytes!("../../../keys/certs/expired-signer.cert"),
    include_bytes!("../../../keys/certs/rogue-signer.cert"),
];
const REAL_REVOCATIONS: &[u8] = include_bytes!("../../../keys/revocations.bin");

fn unhex32(text: &str) -> [u8; 32] {
    let t = text.trim();
    assert_eq!(t.len(), 64);
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&t[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// The kernel's offline root (`keys/root.pub.hex`).
fn real_root() -> [u8; 32] {
    unhex32(include_str!("../../../keys/root.pub.hex"))
}

/// The trust store exactly as the kernel builds it from `/etc/trust`.
fn real_store() -> trust::TrustStore {
    let mut s = trust::TrustStore::new(real_root(), KERNEL_TRUST_EPOCH);
    for c in REAL_CERTS {
        let _ = s.add_certificate(c);
    }
    s.apply_revocations(REAL_REVOCATIONS)
        .expect("keys/revocations.bin verifies under the real root");
    assert_eq!(s.keys().count(), 4, "the rogue certificate is refused");
    s
}

fn pack(manifest: &str, payload: &[u8]) -> Vec<u8> {
    let d = pkg::content_digest(manifest.as_bytes(), payload);
    let mut out = pkg::header(manifest.len() as u32, payload.len() as u32, &d).to_vec();
    out.extend_from_slice(manifest.as_bytes());
    out.extend_from_slice(payload);
    out
}

fn pack_signed(manifest: &str, payload: &[u8], secret: &[u8; 32], public: &[u8; 32]) -> Vec<u8> {
    let (ml, pl) = (manifest.len() as u32, payload.len() as u32);
    let d = pkg::content_digest(manifest.as_bytes(), payload);
    let sig = ed25519::sign(secret, &pkg::signing_input(ml, pl, &d));
    let mut out = pkg::header_signed(ml, pl, &d, public, &sig).to_vec();
    out.extend_from_slice(manifest.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// The refusal order `TrustStore::verify_package` documents, restated from
/// the store's public state (signature, then signer, revocation, window,
/// scope) — the reference the store is checked against.
fn expected_trust(
    store: &trust::TrustStore,
    p: &pkg::Package<'_>,
    sig_ok: bool,
) -> Result<u32, pkg::TrustError> {
    use pkg::TrustError::*;
    let Some(block) = p.signature else {
        return Err(Unsigned);
    };
    if !sig_ok {
        return Err(BadSignature);
    }
    let key = store
        .keys()
        .find(|k| k.public_key == block.public_key)
        .ok_or(UntrustedSigner)?;
    if store.revocations().contains(key.key_id) {
        return Err(RevokedSigner);
    }
    if store.epoch() < key.first_epoch {
        return Err(CertificateNotYetValid);
    }
    if store.epoch() > key.last_epoch {
        return Err(CertificateExpired);
    }
    if !p.manifest.name.starts_with(key.scope()) {
        return Err(OutOfScope);
    }
    Ok(key.key_id)
}

/// Check a parsed package against `store` and `verify_trust` the way the
/// kernel's platform layer does, against the reference decision.
fn check_trust(store: &trust::TrustStore, p: &pkg::Package<'_>, trusted: &[u8; 32]) {
    let sig_ok = p.signature.is_some_and(|b| {
        let input = pkg::signing_input(
            p.manifest_raw.len() as u32,
            p.payload.len() as u32,
            &p.digest,
        );
        ed25519::verify(&b.public_key, &input, &b.signature)
    });
    let want = expected_trust(store, p, sig_ok);
    let got = store.verify_package(p).map(|k| k.key_id);
    assert_eq!(
        got, want,
        "trust decision differs from the documented order"
    );
    if let Err(e) = got {
        let _ = e.name();
    }
    let flat = pkg::verify_trust(p, &[*trusted]);
    let want_flat = match p.signature {
        None => Err(pkg::TrustError::Unsigned),
        Some(_) if !sig_ok => Err(pkg::TrustError::BadSignature),
        Some(b) if b.public_key != *trusted => Err(pkg::TrustError::UntrustedSigner),
        Some(_) => Ok(()),
    };
    assert_eq!(flat, want_flat);
}

struct PkgState {
    seeds: Vec<Vec<u8>>,
    store: trust::TrustStore,
    signers: Vec<([u8; 32], [u8; 32])>,
    dev_public: [u8; 32],
}

const PKG_DICT: &[&[u8]] = &[
    b"ITPKG001",
    b"ITPKG002",
    b"name=",
    b"version=",
    b"caps=",
    b"\n",
    b"hello-",
    b"\x7fELF",
];

fn t_pkg(mode: Mode) {
    run(
        Target {
            name: "pkg::parse",
            cost: 6,
            parallel: true,
            aux: &["trust_checks", "elf_payloads", "resigned"],
        },
        mode,
        || {
            let signers: Vec<([u8; 32], [u8; 32])> = [
                DEV_SIGNING_SECRET,
                FOREIGN_SIGNING_SECRET,
                RETIRED_SIGNING_SECRET,
                EXPIRED_SIGNING_SECRET,
                ROGUE_SIGNING_SECRET,
            ]
            .iter()
            .map(|s| (*s, ed25519::public_key(s)))
            .collect();
            let elf = minimal_elf();
            let noise: Vec<u8> = (0..300u32).map(|i| (i * 31) as u8).collect();
            let mut seeds = vec![
                pack("name=hello-app\nversion=1.0.0\ncaps=gui\n", &elf),
                pack("name=x\nversion=1\ncaps=\n", b"P"),
                pack(
                    "# c\nname=hello-args\nversion=2\ncaps=ipc,fs_read\n",
                    &noise,
                ),
            ];
            for (i, (secret, public)) in signers.iter().enumerate() {
                let man = [
                    "name=hello-app\nversion=1.0.0\ncaps=gui\n",
                    "name=other\nversion=3\ncaps=ipc\n",
                    "name=hello-old\nversion=1\ncaps=\n",
                    "name=hello-exp\nversion=1.2\ncaps=fs_read\n",
                    "name=hello-rogue\nversion=9\ncaps=\n",
                ][i];
                seeds.push(pack_signed(man, &elf, secret, public));
            }
            let dev = &signers[0];
            seeds.push(pack_signed(
                "name=evil\nversion=1\ncaps=\n",
                &noise,
                &dev.0,
                &dev.1,
            ));
            let store = real_store();
            for s in &seeds {
                let p = pkg::parse(s).expect("package seed parses");
                check_trust(&store, &p, &dev.1);
            }
            // The seed store says what the kernel says about each fixture.
            let p = pkg::parse(&seeds[3]).unwrap();
            assert_eq!(store.verify_package(&p).map(|k| k.key_id), Ok(1));
            PkgState {
                seeds,
                store,
                dev_public: dev.1,
                signers,
            }
        },
        |st, rng, ctx| {
            let mut b = gen_bytes_f(
                rng,
                &st.seeds,
                PKG_DICT,
                &[(8, 4, false), (12, 4, false)],
                2048,
            );
            // Re-stamp lengths and the digest for part of the inputs, so the
            // manifest parser and the trust decision behind the integrity
            // check see hostile content too.
            if b.len() >= pkg::HEADER_LEN {
                let signed = b[..8] == *pkg::MAGIC_SIGNED;
                let hlen = if signed {
                    pkg::HEADER_LEN_SIGNED
                } else {
                    pkg::HEADER_LEN
                };
                if b.len() >= hlen {
                    let rest = b.len() - hlen;
                    let cur = read_int(&b, 8, 4, false) as usize;
                    if rng.one_in(3) {
                        let mlen = if cur <= rest && rng.one_in(2) {
                            cur
                        } else {
                            rng.below(rest.min(1100) + 1)
                        };
                        write_int(&mut b, 8, 4, mlen as u64, false);
                        write_int(&mut b, 12, 4, (rest - mlen) as u64, false);
                    }
                    let mlen = read_int(&b, 8, 4, false) as usize;
                    if mlen <= rest && !rng.one_in(3) {
                        let d = pkg::content_digest(&b[hlen..hlen + mlen], &b[hlen + mlen..]);
                        b[16..48].copy_from_slice(&d);
                        if signed && rng.one_in(48) {
                            let (secret, public) = rng.pick(&st.signers);
                            let plen = read_int(&b, 12, 4, false) as u32;
                            let sig =
                                ed25519::sign(secret, &pkg::signing_input(mlen as u32, plen, &d));
                            b[48..80].copy_from_slice(public);
                            b[80..144].copy_from_slice(&sig);
                            ctx.count(2);
                        }
                    }
                }
            }
            ctx.note(&b);
            let p = match pkg::parse(&b) {
                Ok(p) => p,
                Err(_) => return false,
            };
            let hlen = if p.signature.is_some() {
                pkg::HEADER_LEN_SIGNED
            } else {
                pkg::HEADER_LEN
            };
            assert_eq!(hlen + p.manifest_raw.len() + p.payload.len(), b.len());
            assert_eq!(read_int(&b, 8, 4, false) as usize, p.manifest_raw.len());
            assert!(!p.payload.is_empty() && p.payload.len() <= pkg::MAX_PAYLOAD);
            assert!(p.manifest_raw.len() <= manifest::MAX_MANIFEST_LEN);
            assert_eq!(pkg::content_digest(p.manifest_raw, p.payload), p.digest);
            let text = std::str::from_utf8(p.manifest_raw).expect("manifest is UTF-8");
            assert_eq!(manifest::parse(text), Ok(p.manifest));
            if rng.one_in(32) {
                check_trust(&st.store, &p, &st.dev_public);
                ctx.count(0);
            }
            if elf::parse(p.payload).is_ok() {
                ctx.count(1);
            }
            true
        },
    );
}

// --- trust: certificates and revocation lists --------------------------------

const TEST_ROOT_SECRET: [u8; 32] = *b"fuzz harness certificate root 01";
const IMPOSTOR_ROOT_SECRET: [u8; 32] = *b"fuzz harness impostor root ---02";
/// A key that is not a canonical point encoding: `ed25519::verify` refuses it
/// before any curve arithmetic, so structural parsing runs at full speed.
const FAST_FAIL_ROOT: [u8; 32] = [0xff; 32];

fn sign_cert_body(root: &[u8; 32], body: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; trust::CERT_CONTEXT.len() + trust::MAX_CERT_BODY];
    let sig = ed25519::sign(root, trust::cert_signing_input(body, &mut buf));
    let mut out = body.to_vec();
    out.extend_from_slice(&sig);
    out
}

fn sign_revocation_body(root: &[u8; 32], body: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; trust::REVOCATION_CONTEXT.len() + trust::MAX_REVOCATION_BODY];
    let sig = ed25519::sign(root, trust::revocation_signing_input(body, &mut buf));
    let mut out = body.to_vec();
    out.extend_from_slice(&sig);
    out
}

#[allow(clippy::too_many_arguments)]
fn make_cert(
    root: &[u8; 32],
    id: u32,
    public: &[u8; 32],
    window: (u32, u32),
    scope: &str,
    label: &str,
) -> Vec<u8> {
    let mut body = [0u8; trust::MAX_CERT_BODY];
    let n = trust::encode_cert_body(&mut body, id, public, window.0, window.1, scope, label)
        .expect("cert body");
    sign_cert_body(root, &body[..n])
}

fn make_revocations(root: &[u8; 32], seq: u32, ids: &[u32]) -> Vec<u8> {
    let mut body = [0u8; trust::MAX_REVOCATION_BODY];
    let n = trust::encode_revocation_body(&mut body, seq, ids).expect("revocation body");
    sign_revocation_body(root, &body[..n])
}

struct TrustState {
    seeds: Vec<Vec<u8>>,
    real_root: [u8; 32],
    test_root: [u8; 32],
    /// Validly signed (by the test root) certificates and what verification
    /// makes of each.
    certs: Vec<(Vec<u8>, Result<trust::CertifiedKey, trust::CertError>)>,
    revs: Vec<(Vec<u8>, Result<trust::Revocations, trust::CertError>)>,
    /// Packages signed by the pool's signers (and one unsigned, one forged).
    pkgs: Vec<Vec<u8>>,
}

const TRUST_DICT: &[&[u8]] = &[b"ITCERT01", b"ITREVL01", b"hello-", b" ", b"\0", b"\x7f"];

fn t_trust(mode: Mode) {
    use trust::CertError;
    run(
        Target {
            name: "trust::verify_certificate+verify_revocations+TrustStore",
            cost: 30,
            parallel: true,
            aux: &[
                "accepted_after_resign",
                "full_signature_checks",
                "store_episodes",
            ],
        },
        mode,
        || {
            let test_root = ed25519::public_key(&TEST_ROOT_SECRET);
            let signers: Vec<([u8; 32], [u8; 32])> = (0..10u8)
                .map(|i| {
                    let s = [0x40 + i; 32];
                    (s, ed25519::public_key(&s))
                })
                .collect();
            let pk = |i: usize| &signers[i].1;
            let r = &TEST_ROOT_SECRET;
            let mut cert_bytes = vec![
                make_cert(r, 1, pk(0), (0, 20), "", "any"),
                make_cert(r, 2, pk(1), (0, 20), "fz-", "fuzz-scope"),
                make_cert(r, 3, pk(2), (5, 6), "fz-a", "narrow-window"),
                make_cert(r, 4, pk(3), (10, 12), "", "later"),
                make_cert(r, 5, pk(4), (0, 0), "zz", "epoch-zero"),
                make_cert(r, 6, pk(5), (0, u32::MAX), "fz-", &"L".repeat(32)),
                make_cert(r, 7, pk(6), (3, 9), &"s".repeat(32), ""),
                make_cert(r, 8, pk(7), (0, 20), "fz-", "eighth"),
                make_cert(r, 9, pk(8), (0, 20), "", "ninth"),
                make_cert(r, 1, pk(9), (0, 20), "", "duplicate-id"),
                make_cert(r, 20, pk(0), (0, 20), "", "duplicate-key"),
                make_cert(&IMPOSTOR_ROOT_SECRET, 30, pk(9), (0, 20), "", "impostor"),
            ];
            // Signed by the right root but refused after the signature: a
            // scope with a space, and a window that ends before it starts.
            let mut body = [0u8; trust::MAX_CERT_BODY];
            let n = trust::encode_cert_body(&mut body, 40, pk(9), 0, 20, "fz-x", "bad").unwrap();
            body[54 + 2] = b' ';
            cert_bytes.push(sign_cert_body(r, &body[..n]));
            let n = trust::encode_cert_body(&mut body, 41, pk(9), 0, 20, "", "window").unwrap();
            body[44..48].copy_from_slice(&30u32.to_le_bytes());
            cert_bytes.push(sign_cert_body(r, &body[..n]));
            let certs: Vec<_> = cert_bytes
                .into_iter()
                .map(|b| {
                    let v = trust::verify_certificate(&b, &test_root);
                    (b, v)
                })
                .collect();
            assert!(certs[..9].iter().all(|c| c.1.is_ok()));
            assert_eq!(certs[11].1, Err(CertError::BadRootSignature));
            assert_eq!(certs[12].1, Err(CertError::BadText));
            assert_eq!(certs[13].1, Err(CertError::BadWindow));
            let all: Vec<u32> = (1..=9).collect();
            let revs: Vec<_> = [
                make_revocations(r, 0, &[]),
                make_revocations(r, 1, &[2]),
                make_revocations(r, 5, &[1, 3]),
                make_revocations(r, 3, &[]),
                make_revocations(r, 5, &[4]),
                make_revocations(r, u32::MAX, &all),
                make_revocations(r, 7, &(100..164).collect::<Vec<u32>>()),
                make_revocations(&IMPOSTOR_ROOT_SECRET, 9, &[]),
            ]
            .into_iter()
            .map(|b| {
                let v = trust::verify_revocations(&b, &test_root);
                (b, v)
            })
            .collect();
            assert!(revs[..7].iter().all(|r| r.1.is_ok()));
            let mut pkgs = Vec::new();
            for (i, (secret, public)) in signers.iter().enumerate() {
                let name = ["fz-ok", "fz-a-b", "zz-app", "other"][i % 4];
                let man = format!("name={name}\nversion=1\ncaps=\n");
                pkgs.push(pack_signed(&man, b"payload", secret, public));
            }
            let mut forged = pkgs[0].clone();
            forged[100] ^= 1; // the signature, not the content
            pkgs.push(forged);
            pkgs.push(pack("name=fz-plain\nversion=1\ncaps=\n", b"p"));
            let real = real_root();
            let mut seeds: Vec<Vec<u8>> = REAL_CERTS.iter().map(|c| c.to_vec()).collect();
            seeds.push(REAL_REVOCATIONS.to_vec());
            for c in &REAL_CERTS[..4] {
                assert!(trust::verify_certificate(c, &real).is_ok());
            }
            seeds.extend(certs.iter().map(|c| c.0.clone()));
            seeds.extend(revs.iter().map(|r| r.0.clone()));
            TrustState {
                seeds,
                real_root: real,
                test_root,
                certs,
                revs,
                pkgs,
            }
        },
        |st, rng, ctx| {
            let fields = [
                (8, 4, false),
                (12, 2, false),
                (44, 4, false),
                (48, 4, false),
                (52, 1, false),
                (53, 1, false),
            ];
            let mut b = gen_bytes_f(rng, &st.seeds, TRUST_DICT, &fields, 420);
            // Most inputs are checked against a root that fails fast, which
            // exercises every structural rule at full speed. One in 64 is
            // re-signed by the test root, so the rules behind the signature
            // (printable text, the validity window) see hostile content; one
            // in 64 runs a full signature check against a real root.
            let mut resigned = false;
            let root = match rng.below(64) {
                0 if b.len() >= 14 + ed25519::SIGNATURE_LEN => {
                    let body_len = b.len() - ed25519::SIGNATURE_LEN;
                    let body = b[..body_len].to_vec();
                    let signed = if body.starts_with(trust::REVOCATION_MAGIC) {
                        sign_revocation_body(&TEST_ROOT_SECRET, &body)
                    } else {
                        sign_cert_body(&TEST_ROOT_SECRET, &body)
                    };
                    b = signed;
                    resigned = true;
                    st.test_root
                }
                1 => {
                    ctx.count(1);
                    if rng.one_in(2) {
                        st.real_root
                    } else {
                        st.test_root
                    }
                }
                _ => FAST_FAIL_ROOT,
            };
            ctx.note(&b);
            let cert = trust::verify_certificate(&b, &root);
            let rev = trust::verify_revocations(&b, &root);
            let body = &b[..b.len().saturating_sub(ed25519::SIGNATURE_LEN)];
            if let Ok(k) = cert {
                let (scope, label) = (k.scope(), k.label());
                assert!(scope.len() <= trust::MAX_SCOPE_LEN && label.len() <= trust::MAX_LABEL_LEN);
                assert!(k.first_epoch <= k.last_epoch);
                assert!(k.covers(&format!("{scope}x")));
                let mut out = [0u8; trust::MAX_CERT_BODY];
                let n = trust::encode_cert_body(
                    &mut out,
                    k.key_id,
                    &k.public_key,
                    k.first_epoch,
                    k.last_epoch,
                    scope,
                    label,
                )
                .expect("an accepted certificate re-encodes");
                assert_eq!(&out[..n], body, "certificate does not round-trip");
            }
            if let Ok(l) = rev {
                assert!(l.revoked().len() <= trust::MAX_REVOKED);
                let mut out = [0u8; trust::MAX_REVOCATION_BODY];
                let n = trust::encode_revocation_body(&mut out, l.sequence, l.revoked()).unwrap();
                assert_eq!(&out[..n], body, "revocation list does not round-trip");
            }
            if cert.is_ok() || rev.is_ok() {
                // Only what a root really signed verifies: an unmodified
                // seed, or bytes this iteration signed itself. Anything else
                // accepted would be a forged certificate or list.
                assert!(
                    resigned || st.seeds.contains(&b),
                    "a mutated certificate or revocation list verified"
                );
                ctx.count(0);
            }
            for e in [cert.err(), rev.err()].into_iter().flatten() {
                let _ = e.name();
            }
            // A fresh store makes exactly the free functions' decision.
            if root == FAST_FAIL_ROOT {
                let mut s = trust::TrustStore::new(root, rng.u32());
                assert_eq!(s.add_certificate(&b), cert);
                assert_eq!(s.apply_revocations(&b), rev.map(|l| l.sequence));
            }
            // Now and then, a store episode over validly signed material,
            // against a model of the documented rules.
            if rng.one_in(256) {
                ctx.count(2);
                trust_episode(st, rng);
            }
            cert.is_ok() || rev.is_ok()
        },
    );
}

fn trust_episode(st: &TrustState, rng: &mut Rng) {
    use trust::CertError;
    let epoch = rng.range(0, 21) as u32;
    let mut store = trust::TrustStore::new(st.test_root, epoch);
    let mut keys: Vec<trust::CertifiedKey> = Vec::new();
    let mut revs = trust::Revocations::EMPTY;
    for _ in 0..rng.range(1, 12) {
        if rng.one_in(3) {
            let (bytes, verified) = rng.pick(&st.revs);
            let want = match verified {
                Err(e) => Err(*e),
                Ok(l) if l.sequence < revs.sequence => Err(CertError::Rollback),
                Ok(l) => {
                    revs = *l;
                    Ok(l.sequence)
                }
            };
            assert_eq!(store.apply_revocations(bytes), want);
        } else {
            let (bytes, verified) = rng.pick(&st.certs);
            let want = match verified {
                Err(e) => Err(*e),
                Ok(k)
                    if keys
                        .iter()
                        .any(|x| x.key_id == k.key_id || x.public_key == k.public_key) =>
                {
                    Err(CertError::Duplicate)
                }
                Ok(_) if keys.len() == trust::MAX_CERTS => Err(CertError::StoreFull),
                Ok(k) => {
                    keys.push(*k);
                    Ok(*k)
                }
            };
            assert_eq!(store.add_certificate(bytes), want);
        }
        assert_eq!(store.keys().copied().collect::<Vec<_>>(), keys);
        assert_eq!(*store.revocations(), revs);
        if rng.one_in(2) {
            let bytes = rng.pick(&st.pkgs);
            let p = pkg::parse(bytes).expect("pool package parses");
            check_trust(&store, &p, &st.test_root);
        }
    }
}

// --- ed25519 -------------------------------------------------------------------

/// RFC 8032 §7.1 test vectors 1-3: (public key, message, signature).
const RFC8032: [(&str, &str, &str); 3] = [
    (
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn t_ed25519(mode: Mode) {
    run(
        Target {
            name: "ed25519::verify",
            cost: 300,
            parallel: true,
            aux: &[],
        },
        mode,
        || {
            // Input layout: public key (32) || signature (64) || message.
            let mut seeds = Vec::new();
            for (pk, msg, sig) in RFC8032 {
                seeds.push([unhex(pk), unhex(sig), unhex(msg)].concat());
            }
            for i in 0..6u8 {
                let secret = [0x11 * (i + 1); 32];
                let pk = ed25519::public_key(&secret);
                let msg: Vec<u8> = match i {
                    0 => Vec::new(),
                    1 => b"x".to_vec(),
                    2 => pkg::signing_input(40, 136, &[i; 32]).to_vec(),
                    _ => (0..(40 * i as usize)).map(|k| k as u8).collect(),
                };
                let sig = ed25519::sign(&secret, &msg);
                seeds.push([&pk[..], &sig[..], &msg[..]].concat());
            }
            for s in &seeds {
                let (pk, sig) = (s[..32].try_into().unwrap(), s[32..96].try_into().unwrap());
                assert!(ed25519::verify(pk, &s[96..], sig), "seed must verify");
            }
            seeds
        },
        |seeds, rng, ctx| {
            let mut b = gen_bytes(rng, seeds, &[], 256);
            b.resize(b.len().max(96), 0);
            ctx.note(&b);
            let pk: [u8; 32] = b[..32].try_into().unwrap();
            let sig: [u8; 64] = b[32..96].try_into().unwrap();
            let ok = ed25519::verify(&pk, &b[96..], &sig);
            // Exactly the genuine triples verify: a seed must, and anything
            // else accepted - a mutated key, R, S or message - would be a
            // forgery or a malleable encoding (S + L, a non-canonical point).
            assert_eq!(
                ok,
                seeds.contains(&b),
                "verify disagrees on {} bytes",
                b.len()
            );
            ok
        },
    );
}

// --- itfs ------------------------------------------------------------------------

fn itfs_reseal(b: &mut [u8]) {
    let crc = itfs::crc32(&b[..508]);
    b[508..512].copy_from_slice(&crc.to_le_bytes());
}

fn itfs_seeds() -> Vec<Vec<u8>> {
    use itfs::SuperBlock;
    let mut sbs: Vec<SuperBlock> = [2u32, 3, 4, 16, 64, 2048, u32::MAX]
        .iter()
        .map(|&t| SuperBlock::empty(t))
        .collect();
    let mut sb = SuperBlock::empty(64);
    sb.allocate("hello.txt", 600).unwrap();
    sb.allocate("empty", 0).unwrap();
    sb.allocate("notes", 1024).unwrap();
    sbs.push(sb);
    sb.remove("empty").unwrap();
    sbs.push(sb);
    let mut full = SuperBlock::empty(2 + 12);
    for i in 0..itfs::MAX_FILES {
        full.allocate(&format!("f{i}"), 512).unwrap();
    }
    sbs.push(full);
    let mut gaps = SuperBlock::empty(2 + 6);
    for n in ["a", "b", "c", "d", "e", "f"] {
        gaps.allocate(n, 512).unwrap();
    }
    for n in ["b", "d", "f"] {
        gaps.remove(n).unwrap();
    }
    sbs.push(gaps);
    let mut end = SuperBlock::empty(8);
    end.allocate("caf\u{e9}.txt", 6 * 512).unwrap();
    sbs.push(end);
    let mut out: Vec<Vec<u8>> = sbs.iter().map(|s| s.encode().to_vec()).collect();
    // One commit left before the generation counter is exhausted.
    let mut last = sbs[8];
    last.generation = u64::MAX - 1;
    out.push(last.encode().to_vec());
    // A V0.9 image with a leaked gap: entry 0 cleared by hand, high-water
    // mark left where it was.
    let mut v09 = SuperBlock::empty(2 + 4);
    v09.allocate("a", 1024).unwrap();
    v09.allocate("b", 512).unwrap();
    let mut block = v09.encode();
    block[48..48 + 36].fill(0);
    block[24..28].copy_from_slice(&1u32.to_le_bytes());
    itfs_reseal(&mut block);
    out.push(block.to_vec());
    for b in &out {
        assert!(SuperBlock::decode(b).is_ok(), "itfs seed must mount");
    }
    out
}

const ITFS_NAMES: &[&str] = &[
    "hello.txt",
    "notes",
    "a",
    "f3",
    "new-file",
    "caf\u{e9}.txt",
    "",
    "a\nb",
    "\u{202e}x",
    "123456789012345678901234",
    "1234567890123456789012345",
];

/// The superblock header, then every directory entry's size, start and
/// flags.
fn itfs_fields() -> Vec<Field> {
    let mut f = vec![
        (8, 8, false),
        (16, 4, false),
        (20, 4, false),
        (24, 4, false),
        (28, 4, false),
    ];
    for i in 0..itfs::MAX_FILES {
        let o = 48 + i * 36;
        f.extend([(o + 24, 4, false), (o + 28, 4, false), (o + 32, 4, false)]);
    }
    f
}

fn t_itfs(mode: Mode) {
    use itfs::{FsError, SuperBlock};
    run(
        Target {
            name: "itfs::SuperBlock::decode+choose+transactions",
            cost: 3,
            parallel: true,
            aux: &["transactions", "ops_ok"],
        },
        mode,
        || (itfs_seeds(), itfs_fields()),
        |(seeds, fields), rng, ctx| {
            let mut b = gen_bytes_f(rng, seeds, &[b"ITFS0001"], fields, 600);
            if b.len() >= itfs::BLOCK_SIZE && !rng.one_in(4) {
                itfs_reseal(&mut b);
            }
            ctx.note(&b);
            // The other slot: a seed, sometimes mutated and re-sealed too.
            let mut other = rng.pick(seeds).clone();
            if rng.one_in(2) {
                mutate(rng, &mut other, seeds, &[], 512);
                other.resize(itfs::BLOCK_SIZE, 0);
                itfs_reseal(&mut other);
            }
            let a = SuperBlock::decode(&b);
            let o = SuperBlock::decode(&other);
            match (itfs::choose(&b, &other), a, o) {
                (Ok((sb, 0)), Ok(x), _) => assert_eq!(sb, x),
                (Ok((sb, 1)), _, Ok(y)) => assert_eq!(sb, y),
                (Err(FsError::NoValidSuperblock), Err(_), Err(_)) => {}
                (got, _, _) => panic!("choose disagrees with decode: {got:?}"),
            }
            let Ok(sb) = a else {
                if let Err(e) = a {
                    assert_ne!(e, FsError::NoSpace);
                }
                return false;
            };
            // What a mount relies on.
            assert_eq!(SuperBlock::decode(&sb.encode()), Ok(sb));
            let live: Vec<_> = sb.entries.iter().filter(|e| e.used()).collect();
            assert_eq!(live.len() as u32, sb.file_count);
            assert!(
                sb.next_free_block >= itfs::DATA_START_BLOCK
                    && sb.next_free_block <= sb.total_blocks
            );
            for (i, e) in live.iter().enumerate() {
                let x = e.extent();
                assert!(
                    x.blocks == 0
                        || (x.start >= itfs::DATA_START_BLOCK
                            && x.end() <= u64::from(sb.total_blocks))
                );
                assert!(e.name_str().is_some());
                for f in &live[i + 1..] {
                    assert!(
                        !x.overlaps(&f.extent()),
                        "decoded directory has overlapping files"
                    );
                }
            }
            let pinned = o.unwrap_or(sb);
            let s = sb.space(&[&pinned]);
            assert_eq!(s.used + s.free, s.data_blocks);
            assert!(s.pinned <= s.free && s.largest_run <= s.free - s.pinned);
            let _ = sb.run_is_unreferenced(rng.u32(), rng.u32() % 64);
            // A transaction on a copy, exactly as fs_disk runs one: the
            // committed and the fallback superblock are pinned.
            ctx.count(0);
            let mut next = sb;
            for _ in 0..rng.range(1, 4) {
                let name = if !live.is_empty() && rng.one_in(2) {
                    rng.pick(&live).name_str().unwrap().to_string()
                } else {
                    rng.pick(ITFS_NAMES).to_string()
                };
                let size = match rng.below(4) {
                    0 => 0,
                    1 => rng.below(4096) as u32,
                    2 => rng.u32(),
                    _ => (rng.below(16) as u32) * 512,
                };
                let before = next;
                let r = match rng.below(3) {
                    0 => next
                        .allocate_avoiding(&name, size, &[&sb, &pinned])
                        .map(Some),
                    1 => next
                        .replace_avoiding(&name, size, &[&sb, &pinned])
                        .map(Some),
                    _ => next.remove(&name).map(|()| None),
                };
                match r {
                    Ok(placed) => {
                        ctx.count(1);
                        if let Some((slot, start)) = placed {
                            let e = next.entries[slot];
                            assert!(e.used() && e.name_str() == Some(name.as_str()));
                            assert_eq!(e.start_block, start);
                            // The crash invariant: new data only ever goes
                            // where no surviving superblock points.
                            let run = e.extent();
                            assert!(sb.run_is_unreferenced(start, run.blocks));
                            for x in pinned.entries.iter().filter(|x| x.used()) {
                                assert!(!run.overlaps(&x.extent()));
                            }
                        }
                        assert!(next.generation > before.generation);
                        // What reaches the disk must mount again, unchanged.
                        assert_eq!(SuperBlock::decode(&next.encode()), Ok(next));
                    }
                    Err(_) => assert_eq!(next, before, "a refused operation changed the directory"),
                }
            }
            true
        },
    );
}

// --- initconf ----------------------------------------------------------------------

const INITCONF_DICT: &[&[u8]] = &[
    b"service ",
    b" ",
    b"\t",
    b"\n",
    b"\r\n",
    b"\r",
    b"/bin/",
    b"/sbin/",
    b" caps=",
    b" restart=",
    b" after=",
    b" -- ",
    b"always",
    b"on-failure",
    b"never",
    b"ipc",
    b"gui",
    b"spawn",
    b"fs_read",
    b"sys_view",
    b"propose",
    b",",
    b"-",
    b"#",
    b"..",
    b"//",
    b"=",
    b"\xff",
    b"\xc3\xa9",
];

fn t_initconf(mode: Mode) {
    run(
        Target {
            name: "initconf::parse_bytes",
            cost: 2,
            parallel: false,
            aux: &["services"],
        },
        mode,
        || {
            let eight: String = (0..8).map(|i| format!("service s{i} /bin/x\n")).collect();
            let deps = "service a /bin/x\nservice b /bin/x\nservice c /bin/x\nservice d /bin/x\nservice e /bin/x\nservice z /bin/x after=a,b,c,d";
            let args16 = format!("service a /bin/x --{}", " a".repeat(progargs::MAX_ARGS));
            let long = format!(
                "service a /bin/x -- {}",
                "y".repeat(initconf::MAX_LINE - 20)
            );
            text_seeds(&[
                include_str!("../../../initramfs/root/etc/init.conf"),
                include_str!("../../../initramfs/root/etc/init-tests/demo.conf"),
                include_str!("../../../initramfs/root/etc/init-tests/cycle.conf"),
                include_str!("../../../initramfs/root/etc/init-tests/badcap.conf"),
                "# header\r\n\r\n   \t\n  # indented comment\nservice a /bin/x caps=ipc\r\n\tservice\t b  /bin/y\t restart=always   \r\n# trailing comment",
                "service top /bin/x after=l,r\nservice l /bin/x after=base\nservice r /bin/x after=base\nservice base /bin/x",
                "service c /bin/x after=b\nservice a /bin/x\nservice b /bin/x after=a\nservice d /bin/x",
                "service a /bin/x caps=ipc --  one\t\ttwo  caps=gui -- ",
                "service b /sbin/y restart=never after=b2 caps=gui\nservice b2 /bin/z",
                &eight,
                deps,
                &args16,
                &long,
            ])
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, INITCONF_DICT, 1200);
            ctx.note(&b);
            let r = initconf::parse_bytes(&b);
            if let Ok(text) = std::str::from_utf8(&b) {
                assert_eq!(initconf::parse(text), r, "parse and parse_bytes disagree");
            }
            let cfg = match r {
                Ok(c) => c,
                Err(e) => {
                    let _ = e.reason.name();
                    assert!(e.line <= b.iter().filter(|&&c| c == b'\n').count() + 1);
                    return false;
                }
            };
            assert!(cfg.len <= initconf::MAX_SERVICES);
            let (order, n) = cfg.order().expect("an accepted config has a start order");
            assert_eq!(n, cfg.len);
            let mut started = [false; initconf::MAX_SERVICES];
            for &i in &order[..n] {
                let svc = cfg.get(i).expect("order names a service");
                for dep in svc.deps() {
                    let d = cfg.find(dep).expect("a dependency resolves");
                    assert!(started[d], "{} started before {dep}", svc.name);
                }
                assert!(!started[i]);
                started[i] = true;
            }
            for svc in cfg.services() {
                assert!(svcreport::ServiceName::is_valid(svc.name.as_bytes()));
                assert!(svc.path.len() <= initconf::MAX_PATH);
                assert!(svc.path.starts_with("/bin/") || svc.path.starts_with("/sbin/"));
                assert_eq!(svc.caps & !caps::CAP_ALL_KNOWN, 0);
                assert_eq!(svc.caps & caps::CAP_CONSOLE_ONLY, 0);
                assert!(svc.n_after <= initconf::MAX_DEPS);
                let mut block = [0u8; progargs::MAX_BLOCK];
                let len = svc
                    .encode_args(&mut block)
                    .expect("never fails for a service parse accepted");
                assert_eq!(
                    progargs::validate_block(&block[..len]),
                    Ok(svc.arg_tokens().count())
                );
                let _ = (svc.long_running(), svc.restart.name());
                ctx.count(0);
            }
            true
        },
    );
}

// --- model ---------------------------------------------------------------------------

fn t_model(mode: Mode) {
    use model::{Detector, Model, CONDITIONS, MAX_WEIGHT};
    run(
        Target {
            name: "model::decode+scores+detect",
            cost: 2,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let shipped = *shipped_model();
            let extreme = Model {
                detectors: [Detector {
                    bias: -MAX_WEIGHT,
                    weights: [MAX_WEIGHT; sysview::FEATURES],
                }; CONDITIONS],
                epochs: u32::MAX,
                seed: u64::MAX,
                train_examples: 0,
                test_examples: u32::MAX,
                test_exact_bp: 10_000,
                data_digest: [0xff; 32],
            };
            let zero = Model {
                detectors: [Detector::default(); CONDITIONS],
                test_exact_bp: 0,
                ..extreme
            };
            [shipped, extreme, zero]
                .iter()
                .map(|m| {
                    assert_eq!(model::decode(&model::encode(m)), Ok(*m));
                    model::encode(m).to_vec()
                })
                .collect::<Vec<_>>()
        },
        |seeds, rng, ctx| {
            let fields = [
                (8, 2, false),
                (10, 2, false),
                (12, 4, false),
                (32, 4, false),
                (36, 4, false),
                (72, 4, false),
                (76, 4, false),
                (140, 4, false),
                (208, 4, false),
                (272, 4, false),
            ];
            let b = gen_bytes_f(rng, seeds, &[b"ITMODEL1"], &fields, 400);
            ctx.note(&b);
            let m = match model::decode(&b) {
                Ok(m) => m,
                Err(e) => {
                    let _ = e.name();
                    return false;
                }
            };
            assert_eq!(&model::encode(&m)[..], &b[..]);
            let digest = kernel_core::sha256::digest(&b);
            for _ in 0..4 {
                let x = random_features(rng, true);
                let scores = m.scores(&x);
                let fired = m.detect(&x);
                for (c, s) in scenario::Condition::ALL.iter().zip(scores) {
                    assert_eq!(fired.has(*c), s > 0);
                }
                // What inferd answers must be something its client accepts.
                let req = infer::Request {
                    nonce: rng.next(),
                    x,
                };
                let rep = infer::answer(&m, &digest, &req);
                assert_eq!(infer::Reply::decode(&rep.encode()), Ok(rep));
            }
            ctx.count(0);
            true
        },
    );
}

// --- audit_chain::parse_header ----------------------------------------------------

const AUDIT_DICT: &[&[u8]] = &[
    b"itisyou-audit v2",
    b"itisyou-audit v1",
    b" boot=",
    b" count=",
    b" base=",
    b" head=",
    b"0000000000000000000000000000000000000000000000000000000000000000",
    b"18446744073709551615",
    b"18446744073709551616",
    b"00",
    b" ",
    b"+",
    b"-",
    b"A",
    b"f",
];

fn t_audit_header(mode: Mode) {
    use audit_chain::{TrailHeader, VerifyError, GENESIS};
    run(
        Target {
            name: "audit_chain::parse_header+parse_head",
            cost: 2,
            parallel: false,
            aux: &["headers", "noncanonical_accepted"],
        },
        mode,
        || {
            let mut seeds = Vec::new();
            let h1 = audit_chain::compute(&GENESIS, &[b"1 100 0 pkg_install cap=0x0 ok"]);
            for (boot, count, base, head) in [
                (7u64, 2usize, h1, audit_chain::extend(&h1, b"x")),
                (0, 0, GENESIS, GENESIS),
                (u64::MAX, usize::MAX, [0xab; 32], [0x5a; 32]),
            ] {
                let mut line = String::new();
                audit_chain::write_header(
                    &TrailHeader {
                        boot,
                        count,
                        base,
                        head,
                    },
                    &mut line,
                )
                .unwrap();
                seeds.push(line.into_bytes());
            }
            let mut buf = [0u8; 64];
            let head = audit_chain::format_head(&h1, &mut buf).to_string();
            seeds.push(format!("itisyou-audit v1 boot=0 count=3 head={head}").into_bytes());
            seeds
        },
        |seeds, rng, ctx| {
            let s = gen_text(rng, seeds, AUDIT_DICT, 300);
            ctx.note(s.as_bytes());
            let mut buf = [0u8; 64];
            for piece in [s.as_str(), s.rsplit('=').next().unwrap_or("")] {
                if let Some(h) = audit_chain::parse_head(piece) {
                    assert_eq!(audit_chain::format_head(&h, &mut buf), piece);
                }
            }
            let Some(h) = audit_chain::parse_header(&s) else {
                return false;
            };
            let mut line = String::new();
            audit_chain::write_header(&h, &mut line).unwrap();
            assert_eq!(audit_chain::parse_header(&line), Some(h));
            if s.starts_with(audit_chain::MAGIC_V2) && line != s {
                // Two spellings of one header (a zero-padded number).
                ctx.count(1);
            }
            let recs: Vec<Vec<u8>> = (0..rng.below(4))
                .map(|_| {
                    let n = rng.below(40);
                    rng.bytes(n)
                })
                .collect();
            let refs: Vec<&[u8]> = recs.iter().map(|r| r.as_slice()).collect();
            let want = if refs.len() != h.count {
                Err(VerifyError::CountMismatch)
            } else if audit_chain::compute(&h.base, &refs) != h.head {
                Err(VerifyError::HeadMismatch)
            } else {
                Ok(())
            };
            assert_eq!(h.verify(&refs), want);
            ctx.count(0);
            true
        },
    );
}

// --- tar -------------------------------------------------------------------------------

fn tar_entry(name: &[u8], data: &[u8], typeflag: u8) -> Vec<u8> {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name);
    header[100..107].copy_from_slice(b"0000644");
    header[108..115].copy_from_slice(b"0000000");
    header[116..123].copy_from_slice(b"0000000");
    header[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
    header[136..147].copy_from_slice(b"00000000000");
    header[156] = typeflag;
    header[257..262].copy_from_slice(b"ustar");
    header[263..265].copy_from_slice(b"00");
    tar_fix_header(&mut header);
    let mut out = header.to_vec();
    out.extend_from_slice(data);
    while out.len() % 512 != 0 {
        out.push(0);
    }
    out
}

fn tar_fix_header(h: &mut [u8]) {
    h[148..156].fill(b' ');
    let sum: u64 = h.iter().map(|&b| u64::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// Re-seal the checksum of every header the parser will visit, following
/// its own size field, so the checks behind the checksum are reached.
fn tar_fix(b: &mut [u8]) {
    let mut off = 0usize;
    while off + 512 <= b.len() {
        let h = &mut b[off..off + 512];
        if h.iter().all(|&x| x == 0) {
            break;
        }
        tar_fix_header(h);
        let mut size = 0u64;
        for &c in &h[124..136] {
            match c {
                b'0'..=b'7' => size = size.saturating_mul(8).saturating_add(u64::from(c - b'0')),
                _ => break,
            }
        }
        let Some(next) = (size as usize)
            .checked_add(511)
            .map(|v| v / 512 * 512)
            .and_then(|v| v.checked_add(off + 512))
        else {
            break;
        };
        off = next;
    }
}

fn t_tar(mode: Mode) {
    run(
        Target {
            name: "tar::entries",
            cost: 4,
            parallel: true,
            aux: &["entries"],
        },
        mode,
        || {
            let z = vec![0u8; 1024];
            let seeds = vec![
                [
                    tar_entry(b"etc/", b"", b'5'),
                    tar_entry(b"etc/version", b"0.1.0-dev\n", b'0'),
                    z.clone(),
                ]
                .concat(),
                [
                    tar_entry(b"link", b"", b'2'),
                    tar_entry(b"real", b"data", b'0'),
                    tar_entry(b"pax", b"30 path=long/name/for/pax\n", b'x'),
                    tar_entry(b"old", &[7u8; 600], 0),
                ]
                .concat(),
                [
                    tar_entry(&[b'n'; 100], &[1u8; 512], b'0'),
                    tar_entry(b"bin/x", &[0xcc; 1], b'0'),
                    z.clone(),
                ]
                .concat(),
                z.clone(),
            ];
            for s in &seeds {
                assert!(tar::entries(s).all(|e| e.is_ok()), "tar seed must read");
            }
            seeds
        },
        |seeds, rng, ctx| {
            let mut b = gen_bytes(
                rng,
                seeds,
                &[b"ustar", b"00000000000", b"0", b"5", b"7777777777"],
                6000,
            );
            // Length-field edits: the size field is octal text.
            if rng.one_in(4) && b.len() >= 136 {
                let at = if b.len() >= 1024 && rng.one_in(2) {
                    512
                } else {
                    0
                };
                let v = rng.interesting(b.len()) & ((1 << 33) - 1);
                let text = match rng.below(3) {
                    0 => format!("{v:011o}"),
                    1 => format!("{v:o}\0"),
                    _ => format!(" {v:o} "),
                };
                for (k, c) in text.bytes().take(12).enumerate() {
                    b[at + 124 + k] = c;
                }
            }
            if rng.one_in(2) {
                tar_fix(&mut b);
            }
            ctx.note(&b);
            let (mut items, mut err) = (0usize, false);
            let base = b.as_ptr() as usize;
            for e in tar::entries(&b) {
                assert!(!err, "the iterator went on after an error");
                items += 1;
                assert!(items <= b.len() / 512 + 1, "the iterator did not advance");
                match e {
                    Ok(entry) => {
                        if entry.is_dir {
                            assert!(entry.data.is_empty());
                        }
                        let p = entry.data.as_ptr() as usize;
                        assert!(
                            entry.data.is_empty()
                                || (p >= base && p + entry.data.len() <= base + b.len())
                        );
                        ctx.count(0);
                    }
                    Err(_) => err = true,
                }
            }
            items > 0 && !err
        },
    );
}

// --- update --------------------------------------------------------------------------------

const UPDATE_DICT: &[&[u8]] = &[
    b".pkg",
    b".ok",
    b".",
    b"hello",
    b"+",
    b"-",
    b"0",
    b"4294967296",
    b"audit.log",
];

fn t_update(mode: Mode) {
    use update::Kind;
    run(
        Target {
            name: "update::parse_store_name+resolve",
            cost: 3,
            parallel: false,
            aux: &["store_names"],
        },
        mode,
        || {
            text_seeds(&[
                "hello.1.pkg",
                "hello.1.ok",
                "hello-app.12.ok",
                "hello.2.pkg",
                "audit.log",
                "hello-app.+2.ok",
                "a.4294967295.pkg",
                "x.y.pkg",
                "a.ok.txt",
                ".1.pkg",
                "hello.3.ok",
            ])
        },
        |seeds, rng, ctx| {
            let s = gen_text(rng, seeds, UPDATE_DICT, 64);
            ctx.note(s.as_bytes());
            let parsed = update::parse_store_name(&s);
            assert_eq!(
                update::kernel_owned(&s),
                s == update::AUDIT_TRAIL || parsed.is_some()
            );
            if let Some((app, v, kind)) = parsed {
                assert!(!app.is_empty());
                let ext = if kind == Kind::Pkg { "pkg" } else { "ok" };
                assert_eq!(
                    update::parse_store_name(&format!("{app}.{v}.{ext}")),
                    Some((app, v, kind))
                );
                ctx.count(0);
            }
            // Resolve a listing of mutated names, against a model of the
            // rules (the resolver keeps at most 16 of each kind).
            let app = parsed.map_or("hello", |p| p.0);
            let names: Vec<String> = (0..rng.below(24))
                .map(|_| gen_text(rng, seeds, UPDATE_DICT, 40))
                .collect();
            let got = update::resolve(app, names.iter().map(|n| n.as_str()));
            let mut pkgs = Vec::new();
            let mut oks = Vec::new();
            for n in &names {
                if let Some((a, v, k)) = update::parse_store_name(n) {
                    if a == app {
                        let list = if k == Kind::Pkg { &mut pkgs } else { &mut oks };
                        if list.len() < 16 {
                            list.push(v);
                        }
                    }
                }
            }
            let committed: Vec<u32> = pkgs.iter().copied().filter(|v| oks.contains(v)).collect();
            let active = committed.iter().copied().max();
            let want = update::StoreState {
                active,
                previous: committed
                    .iter()
                    .copied()
                    .filter(|&v| Some(v) < active)
                    .max(),
                orphan_staged: pkgs.iter().copied().filter(|v| !oks.contains(v)).max(),
                dangling_ok: oks.iter().copied().filter(|v| !pkgs.contains(v)).max(),
            };
            assert_eq!(got, want);
            parsed.is_some()
        },
    );
}

// --- sha256 / sha512 ------------------------------------------------------------------------------

fn t_sha(mode: Mode) {
    use kernel_core::{sha256, sha512};
    run(
        Target {
            name: "sha256+sha512::streaming",
            cost: 3,
            parallel: false,
            aux: &["bytes"],
        },
        mode,
        || (),
        |_, rng, ctx| {
            // Package content, audit records and signed messages are hashed
            // in pieces; where the pieces break must not matter.
            let len = match rng.below(16) {
                0 => rng.below(20_000),
                1 => 55 + rng.below(20),
                2 => 111 + rng.below(20),
                _ => rng.below(600),
            };
            let data = rng.bytes(len);
            ctx.note(&data[..len.min(64)]);
            ctx.aux[0] += len as u64;
            let (mut a, mut b) = (sha256::Sha256::new(), sha512::Sha512::new());
            let mut parts: Vec<&[u8]> = Vec::new();
            let mut i = 0;
            while i < len {
                let n = rng.range(0, 200).min(len - i);
                a.update(&data[i..i + n]);
                b.update(&data[i..i + n]);
                parts.push(&data[i..i + n]);
                i += n;
            }
            assert_eq!(a.finalize(), sha256::digest(&data));
            let whole = sha512::hash(&data);
            assert_eq!(b.finish(), whole);
            assert_eq!(sha512::hash_parts(&parts), whole);
            true
        },
    );
}

// --- scenario ----------------------------------------------------------------------------------

fn t_scenario(mode: Mode) {
    run(
        Target {
            name: "scenario::parse",
            cost: 4,
            parallel: false,
            aux: &["scenarios"],
        },
        mode,
        || {
            let line = |c: &str| format!("scenario {c} {}", "0..1 ".repeat(sysview::FEATURES));
            let cover = [
                line("healthy"),
                line("service_failed"),
                line("scheduler_paused"),
                line("denial_burst,service_failed"),
            ]
            .join("\n");
            text_seeds(&[include_str!("../../../ai/scenarios.txt"), &cover])
        },
        |seeds, rng, ctx| {
            let s = gen_text(
                rng,
                seeds,
                &[
                    b"scenario ",
                    b"healthy",
                    b"service_failed",
                    b"scheduler_paused",
                    b"denial_burst",
                    b",",
                    b"..",
                    b"1000",
                    b"1001",
                    b" ",
                    b"\n",
                    b"#",
                ],
                4000,
            );
            ctx.note(s.as_bytes());
            let mut out = [scenario::Scenario::EMPTY; scenario::MAX_SCENARIOS];
            match scenario::parse(&s, &mut out) {
                Ok(n) => {
                    assert!((1..=scenario::MAX_SCENARIOS).contains(&n));
                    for sc in &out[..n] {
                        for (lo, hi) in sc.ranges {
                            assert!(0 <= lo && lo <= hi && hi <= sysview::FEATURE_MAX);
                        }
                    }
                    let mut r = scenario::SplitMix64(rng.next());
                    for k in 0..4 {
                        let e = scenario::example(&out[..n], &mut r, k);
                        let sc = &out[k % n];
                        assert_eq!(e.y, sc.conditions);
                        for (f, v) in e.x.iter().enumerate() {
                            assert!((sc.ranges[f].0..=sc.ranges[f].1).contains(v));
                        }
                    }
                    ctx.aux[0] += n as u64;
                    true
                }
                Err(e) => {
                    let _ = e.reason.name();
                    false
                }
            }
        },
    );
}

// ===========================================================================
// Bytes the network supplies
// ===========================================================================

targets! {
    net_eth_parse, full_net_eth_parse => t_eth;
    net_arp_parse, full_net_arp_parse => t_arp;
    net_ipv4_parse, full_net_ipv4_parse => t_ipv4;
    net_icmp_parse_echo, full_net_icmp_parse_echo => t_icmp;
    net_udp_parse, full_net_udp_parse => t_udp;
    net_tcp_parse, full_net_tcp_parse => t_tcp_parse;
    net_tcp_tcb, full_net_tcp_tcb => t_tcb;
    net_dhcp_parse_reply, full_net_dhcp_parse_reply => t_dhcp;
    net_dns_parse_response, full_net_dns_parse_response => t_dns;
    net_ipv6_parse_icmp, full_net_ipv6_parse_icmp => t_ipv6;
    net_checksum, full_net_checksum => t_checksum;
    net_receive_path, full_net_receive_path => t_receive_path;
}

const MAC_A: eth::MacAddr = eth::MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
const MAC_B: eth::MacAddr = eth::MacAddr([0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
const IP_A: ipv4::Ipv4Addr = ipv4::Ipv4Addr::new(10, 0, 2, 15);
const IP_B: ipv4::Ipv4Addr = ipv4::Ipv4Addr::new(10, 0, 2, 2);
const IP_DNS: ipv4::Ipv4Addr = ipv4::Ipv4Addr::new(10, 0, 2, 3);

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn frame(ethertype: eth::EtherType, dst: eth::MacAddr, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 14 + payload.len().max(46)];
    let n = eth::build_into(&mut buf, dst, MAC_B, ethertype, payload, true).unwrap();
    buf.truncate(n);
    buf
}

fn ip_packet(src: ipv4::Ipv4Addr, dst: ipv4::Ipv4Addr, protocol: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 20 + payload.len()];
    let b = ipv4::Ipv4Builder {
        identification: 0x1234,
        ..ipv4::Ipv4Builder::new(src, dst, protocol)
    };
    let n = ipv4::build_into(&mut buf, &b, payload).unwrap();
    buf.truncate(n);
    buf
}

fn udp_datagram(
    src: ipv4::Ipv4Addr,
    dst: ipv4::Ipv4Addr,
    sp: u16,
    dp: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut buf = vec![0u8; 8 + payload.len()];
    let n = udp::build_into(&mut buf, src, dst, sp, dp, payload).unwrap();
    buf.truncate(n);
    buf
}

fn tcp_segment(
    src: ipv4::Ipv4Addr,
    dst: ipv4::Ipv4Addr,
    hdr: &tcp::SegmentHeader,
    mss: Option<u16>,
    payload: &[u8],
) -> Vec<u8> {
    let mut buf = vec![0u8; 24 + payload.len()];
    let n = tcp::build_into(&mut buf, src, dst, hdr, mss, payload).unwrap();
    buf.truncate(n);
    buf
}

fn echo_message(kind: icmp::EchoKind, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 8 + payload.len()];
    let n = icmp::build_echo_into(&mut buf, kind, id, seq, payload).unwrap();
    buf.truncate(n);
    buf
}

/// Re-seal an IPv4 header checksum (IHL from the first byte).
fn fix_ipv4_checksum(b: &mut [u8]) {
    if b.len() < 20 {
        return;
    }
    let hl = usize::from(b[0] & 0x0f) * 4;
    if hl < 20 || b.len() < hl {
        return;
    }
    b[10] = 0;
    b[11] = 0;
    let sum = checksum::checksum(&b[..hl]);
    b[10..12].copy_from_slice(&sum.to_be_bytes());
}

/// Re-seal a TCP/UDP checksum at `offset` against the IPv4 pseudo-header.
fn fix_transport_checksum(
    b: &mut [u8],
    src: ipv4::Ipv4Addr,
    dst: ipv4::Ipv4Addr,
    proto: u8,
    offset: usize,
) {
    if b.len() < offset + 2 {
        return;
    }
    if let Some(sum) =
        checksum::transport_checksum(src.octets(), dst.octets(), proto, b, offset, &[])
    {
        let v = if proto == ipv4::proto::UDP && sum == 0 {
            0xffff
        } else {
            sum
        };
        b[offset..offset + 2].copy_from_slice(&v.to_be_bytes());
    }
}

// --- eth -----------------------------------------------------------------------------

fn t_eth(mode: Mode) {
    run(
        Target {
            name: "net::eth::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let ip = ip_packet(IP_B, IP_A, ipv4::proto::UDP, &[1, 2, 3]);
            let mut short = frame(eth::EtherType::Arp, MAC_A, &[9; 6]);
            short.truncate(20);
            vec![
                frame(eth::EtherType::Ipv4, MAC_A, &ip),
                frame(eth::EtherType::Arp, eth::MacAddr::BROADCAST, &[0; 28]),
                frame(eth::EtherType::Ipv6, MAC_A, &[0x60; 48]),
                frame(eth::EtherType::Other(eth::EtherType::VLAN), MAC_A, &[0; 46]),
                frame(eth::EtherType::Ipv4, MAC_A, &[0xaa; 1500]),
                short,
            ]
        },
        |seeds, rng, ctx| {
            let b = gen_bytes_f(
                rng,
                seeds,
                &[&[0x81, 0x00], &[0x88, 0xa8], &[0x08, 0x06], &[0x86, 0xdd]],
                &[(12, 2, true)],
                1600,
            );
            ctx.note(&b);
            let strict = rng.one_in(2);
            let Ok(f) = eth::parse(&b, strict) else {
                return false;
            };
            assert_eq!(f.payload, &b[eth::HEADER_LEN..]);
            assert!(!strict || b.len() >= eth::MIN_FRAME_LEN);
            let et = f.ethertype.as_u16();
            assert!(et != eth::EtherType::VLAN && et != eth::EtherType::VLAN_QINQ);
            assert_eq!(eth::EtherType::from_u16(et), f.ethertype);
            if f.payload.len() <= eth::MAX_PAYLOAD_LEN {
                let mut out = vec![0u8; b.len()];
                let n =
                    eth::build_into(&mut out, f.dst, f.src, f.ethertype, f.payload, false).unwrap();
                assert_eq!(&out[..n], &b[..]);
                ctx.count(0);
            }
            let mut buf = [0u8; 17];
            let o = f.src.octets();
            let want = format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                o[0], o[1], o[2], o[3], o[4], o[5]
            );
            assert_eq!(f.src.format(&mut buf), want);
            assert_eq!(f.src.is_multicast(), !f.src.is_unicast());
            true
        },
    );
}

// --- arp -----------------------------------------------------------------------------

fn t_arp(mode: Mode) {
    run(
        Target {
            name: "net::arp::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let req = arp::request(MAC_A, IP_A, IP_B);
            let reply = arp::request(MAC_B, IP_B, IP_A).reply_with(IP_A, MAC_A);
            let enc = |p: &arp::ArpPacket| {
                let mut b = vec![0u8; 46];
                arp::build_into(&mut b, p).unwrap();
                b
            };
            vec![enc(&req), enc(&reply), enc(&req)[..28].to_vec()]
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(rng, seeds, &[&[0, 1], &[8, 0], &[6, 4]], 80);
            ctx.note(&b);
            let Ok(p) = arp::parse(&b) else {
                return false;
            };
            let mut out = [0u8; arp::PACKET_LEN];
            arp::build_into(&mut out, &p).unwrap();
            assert_eq!(&out[..], &b[..arp::PACKET_LEN]);
            let r = p.reply_with(IP_A, MAC_A);
            arp::build_into(&mut out, &r).unwrap();
            assert_eq!(arp::parse(&out), Ok(r));
            assert_eq!(r.target_ip, p.sender_ip);
            let _ = p.is_request_for(IP_A);
            ctx.count(0);
            true
        },
    );
}

// --- ipv4 ----------------------------------------------------------------------------

fn t_ipv4(mode: Mode) {
    run(
        Target {
            name: "net::ipv4::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let mut opts = vec![0u8; 28];
            opts[0] = 0x46;
            opts[2..4].copy_from_slice(&28u16.to_be_bytes());
            opts[8] = 64;
            opts[9] = ipv4::proto::UDP;
            opts[12..16].copy_from_slice(&IP_B.0);
            opts[16..20].copy_from_slice(&IP_A.0);
            opts[20..24].copy_from_slice(&[1, 1, 1, 0]);
            opts[24..28].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
            fix_ipv4_checksum(&mut opts);
            let mut padded = ip_packet(IP_B, IP_A, ipv4::proto::ICMP, &[7; 3]);
            padded.resize(46, 0);
            let seeds = vec![
                ip_packet(IP_B, IP_A, ipv4::proto::UDP, &[1, 2, 3, 4, 5]),
                ip_packet(IP_B, IP_A, ipv4::proto::TCP, &[0; 20]),
                padded,
                opts,
                ip_packet(IP_A, ipv4::Ipv4Addr::BROADCAST, 99, &[0xee; 300]),
            ];
            for s in &seeds {
                assert!(ipv4::parse(s).is_ok());
            }
            seeds
        },
        |seeds, rng, ctx| {
            let fields = [
                (0, 1, false),
                (2, 2, true),
                (6, 2, true),
                (8, 1, false),
                (9, 1, false),
            ];
            let mut b = gen_bytes_f(
                rng,
                seeds,
                &[&[0x45], &[0x4f], &[0x20, 0], &[0x40, 0]],
                &fields,
                1600,
            );
            if rng.one_in(2) {
                fix_ipv4_checksum(&mut b);
            }
            ctx.note(&b);
            let Ok(p) = ipv4::parse(&b) else {
                return false;
            };
            let h = p.header;
            assert!((20..=60).contains(&h.header_len) && h.header_len % 4 == 0);
            let total = usize::from(h.total_length);
            assert!(total >= h.header_len && total <= b.len());
            assert_eq!(p.payload, &b[h.header_len..total]);
            assert_eq!(p.options, &b[20..h.header_len]);
            assert!(checksum::is_valid(&b[..h.header_len]));
            assert!(!h.more_fragments && h.fragment_offset == 0 && h.ttl != 0);
            for a in [h.src, h.dst] {
                let mut buf = [0u8; 15];
                assert_eq!(
                    a.format(&mut buf),
                    std::net::Ipv4Addr::from(a.0).to_string()
                );
            }
            // An option-free header with only DF (or nothing) in the flags
            // byte is exactly what the builder emits.
            if h.header_len == 20 && (b[6] == 0x40 || b[6] == 0) && b[7] == 0 {
                let bld = ipv4::Ipv4Builder {
                    src: h.src,
                    dst: h.dst,
                    protocol: h.protocol,
                    ttl: h.ttl,
                    identification: h.identification,
                    dont_fragment: h.dont_fragment,
                    dscp_ecn: h.dscp_ecn,
                };
                let mut out = vec![0u8; total];
                ipv4::build_into(&mut out, &bld, p.payload).unwrap();
                assert_eq!(&out[..10], &b[..10]);
                assert_eq!(&out[12..], &b[12..total]);
                assert!(ipv4::parse(&out).is_ok());
                ctx.count(0);
            }
            true
        },
    );
}

// --- icmp ----------------------------------------------------------------------------

fn t_icmp(mode: Mode) {
    use icmp::EchoKind;
    run(
        Target {
            name: "net::icmp::parse_echo",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            vec![
                echo_message(EchoKind::Request, 0xabcd, 7, b"itisyou-os"),
                echo_message(EchoKind::Reply, 1, 2, b"x"),
                echo_message(EchoKind::Reply, 0, 0, b""),
                echo_message(EchoKind::Request, 0x1234, 1, b"odd"),
                echo_message(EchoKind::Request, 9, 9, &[0xff; 1400]),
            ]
        },
        |seeds, rng, ctx| {
            let mut b = gen_bytes(rng, seeds, &[&[8, 0], &[0, 0], &[3], &[5]], 1600);
            if b.len() >= 4 && rng.one_in(2) {
                b[2] = 0;
                b[3] = 0;
                let sum = checksum::checksum(&b);
                b[2..4].copy_from_slice(&sum.to_be_bytes());
            }
            ctx.note(&b);
            let Ok(e) = icmp::parse_echo(&b) else {
                return false;
            };
            assert_eq!(e.payload, &b[8..]);
            let mut out = vec![0u8; b.len()];
            icmp::build_echo_into(&mut out, e.kind, e.identifier, e.sequence, e.payload).unwrap();
            assert_eq!((&out[..2], &out[4..]), (&b[..2], &b[4..]));
            assert_eq!(icmp::parse_echo(&out), Ok(e));
            let _ = e.answers(e.identifier, e.sequence);
            ctx.count(0);
            true
        },
    );
}

// --- udp -----------------------------------------------------------------------------

fn t_udp(mode: Mode) {
    run(
        Target {
            name: "net::udp::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips"],
        },
        mode,
        || {
            let mut omitted = udp_datagram(IP_B, IP_A, 53, 40000, b"hello");
            omitted[6] = 0;
            omitted[7] = 0;
            let mut padded = udp_datagram(IP_B, IP_A, 67, 68, b"abc");
            padded.extend_from_slice(&[0xee; 13]);
            vec![
                udp_datagram(IP_B, IP_A, 40000, 7, b"hello"),
                udp_datagram(IP_B, IP_A, 1, 65535, b""),
                udp_datagram(IP_B, IP_A, 5353, 5353, &[0x5a; 1472]),
                omitted,
                padded,
            ]
        },
        |seeds, rng, ctx| {
            let mut b = gen_bytes_f(
                rng,
                seeds,
                &[&[0, 0], &[0xff, 0xff], &[0, 8]],
                &[(4, 2, true), (6, 2, true)],
                1600,
            );
            let (src, dst) = if rng.one_in(8) {
                (
                    ipv4::Ipv4Addr::from_u32(rng.u32()),
                    ipv4::Ipv4Addr::from_u32(rng.u32()),
                )
            } else {
                (IP_B, IP_A)
            };
            match rng.below(6) {
                0 | 1 => {
                    // Checksum what the length field says is the datagram.
                    let len = if b.len() >= 8 {
                        usize::from(be16(&b, 4)).min(b.len())
                    } else {
                        b.len()
                    };
                    fix_transport_checksum(&mut b[..len], src, dst, ipv4::proto::UDP, 6);
                }
                2 if b.len() >= 8 => {
                    b[6] = 0;
                    b[7] = 0;
                }
                _ => {}
            }
            ctx.note(&b);
            let Ok(d) = udp::parse(&b, src, dst) else {
                return false;
            };
            let length = usize::from(be16(&b, 4));
            assert!(length >= 8 && length <= b.len());
            assert_eq!(d.payload, &b[8..length]);
            assert_eq!(d.checksum_omitted, b[6] == 0 && b[7] == 0);
            if !d.checksum_omitted && d.src_port != 0 && d.payload.len() <= udp::MAX_PAYLOAD_LEN {
                let mut out = vec![0u8; length];
                udp::build_into(&mut out, src, dst, d.src_port, d.dst_port, d.payload).unwrap();
                assert_eq!(&out[..], &b[..length]);
                ctx.count(0);
            }
            true
        },
    );
}

// --- tcp: the segment codec ------------------------------------------------------

fn tcp_seeds() -> Vec<Vec<u8>> {
    use tcp::flags::{ACK, FIN, PSH, RST, SYN};
    let h = |seq: u32, ack: u32, flags: u8, window: u16| tcp::SegmentHeader {
        src_port: 80,
        dst_port: 40000,
        seq,
        ack,
        flags,
        window,
    };
    let mut seeds = vec![
        tcp_segment(IP_B, IP_A, &h(1000, 0, SYN, 65535), Some(1460), &[]),
        tcp_segment(IP_B, IP_A, &h(1000, 5000, SYN | ACK, 4096), Some(536), &[]),
        tcp_segment(
            IP_B,
            IP_A,
            &h(1001, 5001, ACK | PSH, 4096),
            None,
            b"GET / HTTP/1.0\r\n\r\n",
        ),
        tcp_segment(IP_B, IP_A, &h(u32::MAX, 1, FIN | ACK, 0), None, &[]),
        tcp_segment(IP_B, IP_A, &h(7, 0, RST, 0), None, &[]),
        tcp_segment(IP_B, IP_A, &h(9, 9, ACK, 100), None, &[0x77; 1400]),
    ];
    // Options this stack skips: NOP, window scale, SACK-permitted,
    // timestamps, then end of list.
    let mut opt = tcp_segment(IP_B, IP_A, &h(3, 4, SYN | ACK, 1024), None, &[]);
    let options = [
        1u8, 3, 3, 7, 4, 2, 8, 10, 0, 0, 0, 1, 0, 0, 0, 2, 2, 4, 5, 0xb4, 0, 0, 0, 0,
    ];
    opt.splice(20..20, options);
    opt[12] = (((20 + options.len()) / 4) << 4) as u8;
    fix_transport_checksum(&mut opt, IP_B, IP_A, ipv4::proto::TCP, 16);
    seeds.push(opt);
    for s in &seeds {
        assert!(tcp::parse(s, IP_B, IP_A).is_ok(), "tcp seed must parse");
    }
    seeds
}

const TCP_DICT: &[&[u8]] = &[
    &[2, 4],
    &[1],
    &[0],
    &[3, 3, 7],
    &[0x50],
    &[0x60],
    &[0xf0],
    &[8, 10],
];

fn t_tcp_parse(mode: Mode) {
    run(
        Target {
            name: "net::tcp::parse",
            cost: 1,
            parallel: false,
            aux: &["round_trips", "with_mss"],
        },
        mode,
        tcp_seeds,
        |seeds, rng, ctx| {
            let fields = [
                (4, 4, true),
                (8, 4, true),
                (12, 1, false),
                (13, 1, false),
                (14, 2, true),
                (20, 1, false),
                (21, 1, false),
                (22, 2, true),
            ];
            let mut b = gen_bytes_f(rng, seeds, TCP_DICT, &fields, 1600);
            if rng.one_in(2) {
                fix_transport_checksum(&mut b, IP_B, IP_A, ipv4::proto::TCP, 16);
            }
            ctx.note(&b);
            let Ok(seg) = tcp::parse(&b, IP_B, IP_A) else {
                return false;
            };
            let hl = usize::from(b[12] >> 4) * 4;
            assert_eq!(seg.payload, &b[hl..]);
            assert_eq!(seg.header.flags, b[13] & 0x3f);
            assert!(seg.header.src_port != 0 && seg.header.dst_port != 0);
            if seg.mss.is_some() {
                ctx.count(1);
            }
            let _ = seg.seq_len();
            // A header the builder could have written builds back to itself.
            let canonical = (hl == 20 || (hl == 24 && b[20..22] == [2, 4]))
                && b[12] & 0x0f == 0
                && b[13] & 0xc0 == 0
                && b[18..20] == [0, 0];
            if canonical {
                let mut out = vec![0u8; b.len()];
                let n = tcp::build_into(&mut out, IP_B, IP_A, &seg.header, seg.mss, seg.payload)
                    .unwrap();
                assert_eq!((&out[..16], &out[18..n]), (&b[..16], &b[18..]));
                let again = tcp::parse(&out, IP_B, IP_A).expect("a built segment parses");
                assert_eq!(
                    (again.header, again.mss, again.payload),
                    (seg.header, seg.mss, seg.payload)
                );
                ctx.count(0);
            }
            true
        },
    );
}

// --- tcp: the connection state machine against a hostile wire ------------------

/// Encode every queued segment, check the codec round-trips it, and put it on
/// the wire.
fn drain_outbox(out: &mut tcp::Outbox, wire: &mut std::collections::VecDeque<Vec<u8>>) {
    assert!(out.len() <= tcp::OUTBOX_CAP);
    for seg in out.segments() {
        let mut buf = [0u8; 1600];
        let n = seg.encode(&mut buf).expect("an emitted segment encodes");
        let back = tcp::parse(&buf[..n], seg.src, seg.dst).expect("an emitted segment parses");
        assert_eq!(
            (back.header, back.mss, back.payload),
            (seg.header, seg.mss, seg.payload())
        );
        wire.push_back(buf[..n].to_vec());
    }
    out.clear();
}

fn check_tcb(t: &tcp::Tcb) {
    assert!(t.recv_available() <= tcp::RECV_BUF_LEN);
    assert_eq!(t.rcv_wnd() as usize, tcp::RECV_BUF_LEN - t.recv_available());
    assert!((tcp::MIN_PEER_MSS..=1460).contains(&t.peer_mss()));
    if !matches!(t.state(), tcp::State::Closed | tcp::State::Listen) {
        assert!(
            tcp::seq_le(t.snd_una(), t.snd_nxt()),
            "acknowledged past what was sent"
        );
    }
}

fn t_tcb(mode: Mode) {
    use tcp::{Event, State};
    run(
        Target {
            name: "net::tcp::Tcb",
            cost: 40,
            parallel: true,
            aux: &["segments_delivered", "established", "hostile_injected"],
        },
        mode,
        || (),
        |_, rng, ctx| {
            let mut a = Box::new(tcp::Tcb::new());
            let mut b = Box::new(tcp::Tcb::new());
            let mut out = Box::new(tcp::Outbox::new());
            let mut wire = std::collections::VecDeque::new();
            let mut now = rng.below(1000) as u64;
            let (pa, pb) = (40000 + rng.below(100) as u16, *rng.pick(&[80u16, 7, 443]));
            b.listen_in_place(IP_B, pb, rng.u32());
            a.connect_in_place(IP_A, pa, IP_B, pb, rng.u32(), now, &mut out);
            drain_outbox(&mut out, &mut wire);
            let mut established = false;
            for _ in 0..rng.range(4, 64) {
                match rng.below(10) {
                    // Deliver a segment from the wire: in order or not, maybe
                    // lost, duplicated or damaged.
                    0..=4 => {
                        if wire.is_empty() {
                            continue;
                        }
                        let i = if rng.one_in(4) {
                            rng.below(wire.len())
                        } else {
                            0
                        };
                        let mut bytes = wire.remove(i).unwrap();
                        if rng.one_in(10) {
                            continue;
                        }
                        if rng.one_in(10) {
                            wire.push_back(bytes.clone());
                        }
                        let to_b = bytes.len() >= 4 && be16(&bytes, 2) == pb;
                        let (src, dst) = if to_b { (IP_A, IP_B) } else { (IP_B, IP_A) };
                        if rng.one_in(6) {
                            mutate(rng, &mut bytes, &[], TCP_DICT, 1600);
                            if rng.one_in(2) {
                                fix_transport_checksum(&mut bytes, src, dst, ipv4::proto::TCP, 16);
                            }
                        }
                        ctx.note(&bytes);
                        if let Ok(seg) = tcp::parse(&bytes, src, dst) {
                            ctx.count(0);
                            let t = if to_b { &mut b } else { &mut a };
                            let ev = t.on_segment(&seg, now, &mut out);
                            if matches!(ev, Event::Reset | Event::Closed | Event::TimedOut) {
                                assert_eq!(t.state(), State::Closed);
                            }
                        }
                    }
                    // Inject a hostile segment aimed at one side, with numbers
                    // near what that side expects so it gets past the window.
                    5 => {
                        let to_b = rng.one_in(2);
                        let (t, src, dst, sp, dp) = if to_b {
                            (&mut b, IP_A, IP_B, pa, pb)
                        } else {
                            (&mut a, IP_B, IP_A, pb, pa)
                        };
                        let near = |rng: &mut Rng, x: u32| {
                            if rng.one_in(4) {
                                rng.u32()
                            } else {
                                x.wrapping_add(rng.below(4200) as u32).wrapping_sub(100)
                            }
                        };
                        let hdr = tcp::SegmentHeader {
                            src_port: if rng.one_in(8) { rng.u16() | 1 } else { sp },
                            dst_port: dp,
                            seq: near(rng, t.rcv_nxt()),
                            ack: near(rng, t.snd_nxt()),
                            flags: rng.byte() & 0x3f,
                            window: if rng.one_in(3) { 0 } else { rng.u16() },
                        };
                        let mss = if rng.one_in(3) { Some(rng.u16()) } else { None };
                        let len = if rng.one_in(2) { 0 } else { rng.below(1500) };
                        let payload = rng.bytes(len);
                        let mut bytes = tcp_segment(src, dst, &hdr, mss, &payload);
                        if rng.one_in(4) {
                            mutate(rng, &mut bytes, &[], TCP_DICT, 1600);
                            fix_transport_checksum(&mut bytes, src, dst, ipv4::proto::TCP, 16);
                        }
                        ctx.note(&bytes);
                        if let Ok(seg) = tcp::parse(&bytes, src, dst) {
                            ctx.count(2);
                            let _ = t.on_segment(&seg, now, &mut out);
                            if rng.one_in(4) {
                                let _ = tcp::reset_reply(&seg, &mut out);
                            }
                        }
                    }
                    // The owner's calls.
                    6 => {
                        let t = if rng.one_in(2) { &mut a } else { &mut b };
                        match rng.below(8) {
                            0..=2 => {
                                let len = rng.below(3000);
                                let data = rng.bytes(len);
                                let n = t.send(&data, now, &mut out);
                                assert!(n <= data.len());
                            }
                            3..=5 => {
                                let before = t.recv_available();
                                let mut dst = vec![0u8; rng.below(5000)];
                                let n = t.recv(&mut dst);
                                assert!(n <= before && n <= dst.len());
                                assert_eq!(t.recv_available(), before - n);
                            }
                            6 => {
                                let _ = t.close(now, &mut out);
                            }
                            _ => {
                                if rng.one_in(4) {
                                    t.abort(&mut out);
                                    assert_eq!(t.state(), State::Closed);
                                }
                            }
                        }
                    }
                    // Time passes.
                    7 | 8 => {
                        now = now.saturating_add(match rng.below(4) {
                            0 => rng.interesting(0) % 200_000,
                            _ => rng.below(700) as u64,
                        });
                        let _ = a.on_timer(now, &mut out);
                        drain_outbox(&mut out, &mut wire);
                        let _ = b.on_timer(now, &mut out);
                    }
                    _ => {
                        let mut sink = [0u8; 4096];
                        let _ = a.recv(&mut sink);
                        let _ = b.recv(&mut sink);
                    }
                }
                drain_outbox(&mut out, &mut wire);
                check_tcb(&a);
                check_tcb(&b);
                if a.state() == State::Established {
                    established = true;
                }
                // A misbehaving peer cannot make the wire grow without bound
                // either: each side queues at most one outbox per call.
                while wire.len() > 64 {
                    wire.pop_front();
                }
            }
            if established {
                ctx.count(1);
            }
            established
        },
    );
}

// --- dhcp ------------------------------------------------------------------------------

const DHCP_XID: u32 = 0x1234_5678;

fn dhcp_reply(kind: u8, extra: &[&[u8]]) -> Vec<u8> {
    let mut m = vec![0u8; 240];
    m[0] = 2;
    m[1] = 1;
    m[2] = 6;
    m[4..8].copy_from_slice(&DHCP_XID.to_be_bytes());
    m[16..20].copy_from_slice(&[10, 0, 2, 15]);
    m[28..34].copy_from_slice(&MAC_A.0);
    m[236..240].copy_from_slice(&[99, 130, 83, 99]);
    m.extend_from_slice(&[53, 1, kind]);
    m.extend_from_slice(&[54, 4, 10, 0, 2, 2]);
    m.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
    m.extend_from_slice(&[3, 4, 10, 0, 2, 2]);
    m.extend_from_slice(&[6, 8, 10, 0, 2, 3, 8, 8, 8, 8]);
    m.extend_from_slice(&[51, 4, 0, 1, 0x51, 0x80]);
    for e in extra {
        m.extend_from_slice(e);
    }
    m.push(0);
    m.push(255);
    m
}

fn t_dhcp(mode: Mode) {
    use dhcp::MessageType;
    run(
        Target {
            name: "net::dhcp::parse_reply",
            cost: 1,
            parallel: false,
            aux: &["leases"],
        },
        mode,
        || {
            let seeds = vec![
                dhcp_reply(2, &[]),
                dhcp_reply(5, &[&[58, 4, 0, 0, 0x2a, 0x30], &[59, 4, 0, 0, 0x4e, 0x20]]),
                dhcp_reply(6, &[&[56, 5, b'n', b'o', b'p', b'e', b'!']]),
                dhcp_reply(2, &[&[0, 0, 0], &[43, 0], &[252, 3, 1, 2, 3]]),
            ];
            for s in &seeds {
                assert!(dhcp::parse_reply(s, DHCP_XID, MAC_A).is_ok());
            }
            seeds
        },
        |seeds, rng, ctx| {
            let b = gen_bytes(
                rng,
                seeds,
                &[&[53, 1], &[54, 4], &[255], &[0], &[99, 130, 83, 99]],
                700,
            );
            ctx.note(&b);
            let (xid, mac) = if rng.one_in(16) {
                (rng.u32(), eth::MacAddr([rng.byte(); 6]))
            } else {
                (DHCP_XID, MAC_A)
            };
            let Ok(lease) = dhcp::parse_reply(&b, xid, mac) else {
                return false;
            };
            match lease.kind {
                MessageType::Offer | MessageType::Ack => {
                    assert!(!lease.address.is_unspecified() && !lease.address.is_broadcast());
                    assert!(!lease.server.is_unspecified());
                }
                MessageType::Nak => {}
                other => panic!("accepted a {other:?} as a server reply"),
            }
            let _ = lease.renew_after();
            // What the client does next: a REQUEST naming the offer. Its own
            // messages are never mistaken for a server's.
            let mut out = [0u8; dhcp::MAX_MESSAGE_LEN];
            let n = dhcp::build_request(&mut out, xid, mac, &lease).unwrap();
            assert_eq!(
                dhcp::parse_reply(&out[..n], xid, mac),
                Err(dhcp::DhcpError::NotAReply)
            );
            let n = dhcp::build_discover(&mut out, xid, mac).unwrap();
            assert_eq!(
                dhcp::parse_reply(&out[..n], xid, mac),
                Err(dhcp::DhcpError::NotAReply)
            );
            ctx.count(0);
            true
        },
    );
}

// --- dns ---------------------------------------------------------------------------------

/// A response to `(id, name)` echoing the question and carrying `answers`.
fn dns_response(name: &str, id: u16, answers: &[&[u8]]) -> Option<Vec<u8>> {
    let mut q = [0u8; dns::MAX_QUERY_LEN];
    let n = dns::build_query(&mut q, id, name).ok()?;
    let mut m = q[..n].to_vec();
    m[2] = 0x81;
    m[3] = 0x80;
    m[6..8].copy_from_slice(&(answers.len() as u16).to_be_bytes());
    for a in answers {
        m.extend_from_slice(a);
    }
    Some(m)
}

fn dns_a_record(addr: [u8; 4], ttl: u32) -> Vec<u8> {
    let mut r = vec![0xc0, dns::HEADER_LEN as u8, 0, 1, 0, 1];
    r.extend_from_slice(&ttl.to_be_bytes());
    r.extend_from_slice(&[0, 4]);
    r.extend_from_slice(&addr);
    r
}

struct DnsState {
    msgs: Vec<Vec<u8>>,
    queries: Vec<(u16, String)>,
}

fn t_dns(mode: Mode) {
    run(
        Target {
            name: "net::dns::parse_response",
            cost: 2,
            parallel: false,
            aux: &["answers", "differential"],
        },
        mode,
        || {
            // CNAME: a pointer to the question's name, then "www" + pointer.
            let q = dns::HEADER_LEN as u8;
            let cname = vec![
                0xc0, q, 0, 5, 0, 1, 0, 0, 0, 60, 0, 6, 3, b'w', b'w', b'w', 0xc0, q,
            ];
            let long_label = "abcdefghij".repeat(6);
            let cases: Vec<(u16, &str, Vec<Vec<u8>>)> = vec![
                (
                    0x1234,
                    "os.itisyou.app",
                    vec![dns_a_record([1, 2, 3, 4], 300)],
                ),
                (
                    0x0001,
                    "OS.ItIsYou.App",
                    vec![dns_a_record([9, 9, 9, 9], 1)],
                ),
                (
                    0xffff,
                    "a.b",
                    vec![cname.clone(), dns_a_record([5, 6, 7, 8], 60)],
                ),
                (0x0bad, "example.com.", vec![cname]),
                (0x7777, &long_label, vec![dns_a_record([10, 0, 2, 3], 0)]),
            ];
            let mut msgs = Vec::new();
            let mut queries = Vec::new();
            for (id, name, answers) in &cases {
                let refs: Vec<&[u8]> = answers.iter().map(|a| a.as_slice()).collect();
                msgs.push(dns_response(name, *id, &refs).unwrap());
                queries.push((*id, name.to_string()));
            }
            DnsState { msgs, queries }
        },
        |st, rng, ctx| {
            let i = rng.below(st.msgs.len());
            let mut b = st.msgs[i].clone();
            if !rng.one_in(32) {
                mutate(
                    rng,
                    &mut b,
                    &st.msgs,
                    &[&[0xc0], &[0xc0, 0x0c], &[0x3f], &[0x40], &[0]],
                    600,
                );
            }
            if rng.one_in(4) {
                mutate_field(
                    rng,
                    &mut b,
                    &[
                        (2, 1, false),
                        (3, 1, false),
                        (4, 2, true),
                        (6, 2, true),
                        (12, 1, false),
                    ],
                );
            }
            let (mut id, mut name) = st.queries[i].clone();
            if rng.one_in(16) {
                id = rng.u16();
                let seeds: Vec<Vec<u8>> =
                    st.queries.iter().map(|q| q.1.as_bytes().to_vec()).collect();
                name = gen_text(rng, &seeds, &[b".", b"..", b"a"], 300);
            }
            ctx.note(&b);
            let r = dns::parse_response(&b, id, &name);
            if r.is_ok() {
                ctx.count(0);
            }
            // Differential: any name this client can ask about, answered by
            // a well-formed response, resolves to exactly that answer.
            if rng.one_in(8) {
                let addr = [rng.byte(), rng.byte(), rng.byte(), rng.byte()];
                let ttl = rng.u32();
                if let Some(resp) = dns_response(&name, id, &[&dns_a_record(addr, ttl)]) {
                    let got = dns::parse_response(&resp, id, &name);
                    assert_eq!(
                        got,
                        Ok(dns::Answer {
                            address: ipv4::Ipv4Addr(addr),
                            ttl
                        }),
                        "{name:?}"
                    );
                    ctx.count(1);
                }
            }
            r.is_ok()
        },
    );
}

// --- ipv6 + ICMPv6 + NDP --------------------------------------------------------

fn v6(s: &str) -> ipv6::Ipv6Addr {
    ipv6::Ipv6Addr(s.parse::<std::net::Ipv6Addr>().unwrap().octets())
}

fn v6_packet(src: ipv6::Ipv6Addr, dst: ipv6::Ipv6Addr, hop: u8, nh: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; 40 + payload.len()];
    ipv6::build_packet(&mut buf, src, dst, hop, nh, payload).unwrap();
    buf
}

/// Re-seal the ICMPv6 checksum of an IPv6 packet's payload (as far as the
/// payload-length field and the buffer agree), against its own addresses.
fn fix_icmpv6(b: &mut [u8]) {
    if b.len() < 44 || b[6] != ipv6::next_header::ICMPV6 {
        return;
    }
    let len = usize::from(be16(b, 4)).min(b.len() - 40);
    if len < 4 {
        return;
    }
    let src = ipv6::Ipv6Addr(b[8..24].try_into().unwrap());
    let dst = ipv6::Ipv6Addr(b[24..40].try_into().unwrap());
    let msg = &mut b[40..40 + len];
    msg[2] = 0;
    msg[3] = 0;
    let sum = ipv6::transport_checksum(src, dst, 58, msg, 2, &[]).unwrap();
    msg[2..4].copy_from_slice(&sum.to_be_bytes());
}

fn qemu_ra() -> Vec<u8> {
    let mut m = vec![134, 0, 0, 0, 64, 0, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0];
    m.extend([1, 1, 0x52, 0x56, 0, 0, 0, 2]);
    m.extend([
        3, 4, 64, 0xc0, 0, 1, 0x51, 0x80, 0, 0, 0x38, 0x40, 0, 0, 0, 0,
    ]);
    m.extend(v6("fec0::").0);
    m.extend([25, 3, 0, 0, 0, 0, 0x04, 0xb0]);
    m.extend(v6("fec0::3").0);
    m.extend([5, 1, 0, 0, 0, 0, 0x05, 0xdc]);
    m
}

fn ipv6_seeds() -> Vec<Vec<u8>> {
    use icmp::EchoKind;
    let me = ipv6::Ipv6Addr::link_local_from_mac(MAC_A);
    let (router, peer) = (v6("fe80::2"), v6("fec0::2"));
    let all_nodes = ipv6::Ipv6Addr::ALL_NODES;
    let mut buf = [0u8; 256];
    let mut seeds = Vec::new();
    let n = ipv6::build_echo(&mut buf, peer, me, EchoKind::Request, 0xbeef, 7, b"hi!").unwrap();
    seeds.push(v6_packet(peer, me, 64, 58, &buf[..n]));
    let n = ipv6::build_echo(&mut buf, peer, me, EchoKind::Reply, 1, 2, &[0x42; 100]).unwrap();
    seeds.push(v6_packet(peer, me, 64, 58, &buf[..n]));
    let n =
        ipv6::build_router_solicit(&mut buf, me, ipv6::Ipv6Addr::ALL_ROUTERS, Some(MAC_A)).unwrap();
    seeds.push(v6_packet(
        me,
        ipv6::Ipv6Addr::ALL_ROUTERS,
        255,
        58,
        &buf[..n],
    ));
    let n = ipv6::build_neighbor_solicit(&mut buf, router, me.solicited_node(), me, Some(MAC_B))
        .unwrap();
    seeds.push(v6_packet(router, me.solicited_node(), 255, 58, &buf[..n]));
    let n = ipv6::build_neighbor_solicit(
        &mut buf,
        ipv6::Ipv6Addr::UNSPECIFIED,
        me.solicited_node(),
        me,
        None,
    )
    .unwrap();
    seeds.push(v6_packet(
        ipv6::Ipv6Addr::UNSPECIFIED,
        me.solicited_node(),
        255,
        58,
        &buf[..n],
    ));
    let na = ipv6::NeighborAdvert {
        router: true,
        solicited: true,
        overrides: true,
        target: router,
        target_ll: Some(MAC_B),
    };
    let n = ipv6::build_neighbor_advert(&mut buf, router, me, &na).unwrap();
    seeds.push(v6_packet(router, me, 255, 58, &buf[..n]));
    let mut ra = v6_packet(router, all_nodes, 255, 58, &qemu_ra());
    fix_icmpv6(&mut ra);
    seeds.push(ra);
    seeds.push(v6_packet(
        peer,
        me,
        64,
        17,
        &[0, 53, 0x9c, 0x40, 0, 8, 0, 0],
    ));
    for s in &seeds[..7] {
        let p = ipv6::parse(s).expect("ipv6 seed parses");
        assert!(ipv6::parse_icmp(&p).is_ok(), "icmpv6 seed parses");
    }
    seeds
}

fn check_v6_text(a: ipv6::Ipv6Addr) {
    let std_addr = std::net::Ipv6Addr::from(a.0);
    if std_addr.to_ipv4_mapped().is_none() {
        let mut buf = [0u8; 39];
        assert_eq!(a.format(&mut buf), std_addr.to_string());
    }
}

fn t_ipv6(mode: Mode) {
    use ipv6::Icmpv6Message as M;
    run(
        Target {
            name: "net::ipv6::parse+parse_icmp",
            cost: 2,
            parallel: false,
            aux: &["icmpv6_messages", "builder_round_trips"],
        },
        mode,
        ipv6_seeds,
        |seeds, rng, ctx| {
            let fields = [
                (4, 2, true),
                (6, 1, false),
                (7, 1, false),
                (40, 1, false),
                (41, 1, false),
                (56, 1, false),
                (57, 1, false),
                (64, 1, false),
                (65, 1, false),
            ];
            let mut b = gen_bytes_f(
                rng,
                seeds,
                &[
                    &[0x60],
                    &[255],
                    &[58],
                    &[1, 1],
                    &[3, 4],
                    &[5, 1],
                    &[0xff, 2],
                ],
                &fields,
                1600,
            );
            if rng.one_in(4) && b.len() >= 40 {
                let len = (b.len() - 40) as u16;
                b[4..6].copy_from_slice(&len.to_be_bytes());
            }
            if rng.one_in(2) {
                fix_icmpv6(&mut b);
            }
            ctx.note(&b);
            let Ok(p) = ipv6::parse(&b) else {
                return false;
            };
            assert_eq!(p.payload, &b[40..40 + usize::from(be16(&b, 4))]);
            assert!(!p.src.is_multicast());
            check_v6_text(p.src);
            check_v6_text(p.dst);
            let Ok(m) = ipv6::parse_icmp(&p) else {
                return true;
            };
            ctx.count(0);
            let mut buf = [0u8; 1600];
            match m {
                M::Echo(e) => {
                    let n = ipv6::build_echo(
                        &mut buf,
                        p.src,
                        p.dst,
                        e.kind,
                        e.identifier,
                        e.sequence,
                        e.payload,
                    )
                    .unwrap();
                    assert_eq!((&buf[..2], &buf[4..n]), (&p.payload[..2], &p.payload[4..]));
                    ctx.count(1);
                }
                M::RouterAdvert(ra) => {
                    assert!(p.src.is_link_local() && p.hop_limit == ipv6::NDP_HOP_LIMIT);
                    for pi in ra.prefixes.iter().flatten() {
                        assert!(pi.prefix_len <= 128);
                        if let Some(pre) = pi.slaac_prefix() {
                            let a = ipv6::Ipv6Addr::with_prefix(pre, MAC_A);
                            assert!(!a.is_multicast() && !a.is_link_local());
                        }
                    }
                    if let Some(mac) = ra.source_ll {
                        assert!(!mac.is_multicast() && !mac.is_zero());
                    }
                }
                M::NeighborSolicit(ns) => {
                    assert!(!ns.target.is_multicast());
                    let n = ipv6::build_neighbor_solicit(
                        &mut buf,
                        p.src,
                        p.dst,
                        ns.target,
                        ns.source_ll,
                    )
                    .expect("the builder accepts what the parser accepted");
                    let raw = v6_packet(p.src, p.dst, 255, 58, &buf[..n]);
                    let q = ipv6::parse(&raw).unwrap();
                    assert_eq!(ipv6::parse_icmp(&q), Ok(M::NeighborSolicit(ns)));
                    ctx.count(1);
                }
                M::NeighborAdvert(na) => {
                    assert!(!na.target.is_multicast());
                    let n = ipv6::build_neighbor_advert(&mut buf, p.src, p.dst, &na)
                        .expect("the builder accepts what the parser accepted");
                    let raw = v6_packet(p.src, p.dst, 255, 58, &buf[..n]);
                    let q = ipv6::parse(&raw).unwrap();
                    assert_eq!(ipv6::parse_icmp(&q), Ok(M::NeighborAdvert(na)));
                    ctx.count(1);
                }
                M::RouterSolicit(rs) => {
                    let n = ipv6::build_router_solicit(&mut buf, p.src, p.dst, rs.source_ll)
                        .expect("the builder accepts what the parser accepted");
                    let raw = v6_packet(p.src, p.dst, 255, 58, &buf[..n]);
                    let q = ipv6::parse(&raw).unwrap();
                    assert_eq!(ipv6::parse_icmp(&q), Ok(M::RouterSolicit(rs)));
                    ctx.count(1);
                }
            }
            let _ = (
                p.dst.solicited_node(),
                p.dst.multicast_mac(),
                p.src.segments(),
            );
            true
        },
    );
}

// --- checksum --------------------------------------------------------------------------

fn t_checksum(mode: Mode) {
    run(
        Target {
            name: "net::checksum",
            cost: 2,
            parallel: false,
            aux: &[],
        },
        mode,
        || (),
        |_, rng, ctx| {
            let len = if rng.one_in(64) {
                rng.below(70_000)
            } else {
                rng.below(2000)
            };
            let mut data = rng.bytes(len);
            if rng.one_in(4) {
                data.fill(0xff);
            }
            ctx.note(&data[..data.len().min(64)]);
            let want = checksum::checksum(&data);
            // Chunk boundaries (odd ones included) are invisible.
            let mut c = checksum::Checksum::new();
            let mut i = 0;
            while i < data.len() {
                let n = rng.range(1, 64).min(data.len() - i);
                if rng.one_in(8) {
                    c.push_zeros(0);
                }
                c.push(&data[i..i + n]);
                i += n;
            }
            assert_eq!(c.finish(), want);
            // push_zeros is push of zeros, whatever the phase.
            let z = rng.below(80);
            let (mut x, mut y) = (checksum::Checksum::new(), checksum::Checksum::new());
            let head = rng.below(3);
            x.push(&data[..head.min(data.len())]);
            y.push(&data[..head.min(data.len())]);
            x.push_zeros(z);
            y.push(&vec![0u8; z]);
            assert_eq!(x.folded_sum(), y.folded_sum());
            // The zeroed-field helper equals zeroing a copy.
            let off = rng.below(len + 4);
            let mut copy = data.clone();
            if off + 2 <= copy.len() {
                copy[off] = 0;
                copy[off + 1] = 0;
            }
            assert_eq!(
                checksum::checksum_with_zeroed_field(&data, off),
                checksum::checksum(&copy)
            );
            // A checksum written into an aligned field makes the data verify.
            if off % 2 == 0 && off + 2 <= copy.len() {
                let s = checksum::checksum(&copy);
                copy[off..off + 2].copy_from_slice(&s.to_be_bytes());
                assert!(checksum::is_valid(&copy));
            }
            let t = checksum::transport_checksum(
                [1, 2, 3, 4],
                [5, 6, 7, 8],
                17,
                &data[..len.min(20)],
                6,
                &data[len.min(20)..],
            );
            assert_eq!(t.is_none(), len > usize::from(u16::MAX));
            true
        },
    );
}

// --- the receive path, frame to socket ------------------------------------------------

const DNS_ID: u16 = 0x4242;
const DNS_NAME: &str = "os.itisyou.app";

/// Re-seal every checksum a frame carries, following its own headers the
/// way the stack reads them.
fn fix_frame(b: &mut [u8]) {
    if b.len() < 14 {
        return;
    }
    match be16(b, 12) {
        eth::EtherType::IPV4 => {
            let ip = &mut b[14..];
            fix_ipv4_checksum(ip);
            if ip.len() < 20 {
                return;
            }
            let hl = usize::from(ip[0] & 0x0f) * 4;
            let total = usize::from(be16(ip, 2)).min(ip.len());
            if hl < 20 || total < hl {
                return;
            }
            let (src, dst) = (
                ipv4::Ipv4Addr(ip[12..16].try_into().unwrap()),
                ipv4::Ipv4Addr(ip[16..20].try_into().unwrap()),
            );
            let proto = ip[9];
            let t = &mut ip[hl..total];
            match proto {
                ipv4::proto::UDP => fix_transport_checksum(t, src, dst, proto, 6),
                ipv4::proto::TCP => fix_transport_checksum(t, src, dst, proto, 16),
                ipv4::proto::ICMP if t.len() >= 4 => {
                    t[2] = 0;
                    t[3] = 0;
                    let s = checksum::checksum(t);
                    t[2..4].copy_from_slice(&s.to_be_bytes());
                }
                _ => {}
            }
        }
        eth::EtherType::IPV6 => fix_icmpv6(&mut b[14..]),
        _ => {}
    }
}

struct RxState {
    seeds: Vec<Vec<u8>>,
}

fn t_receive_path(mode: Mode) {
    use icmp::EchoKind;
    run(
        Target {
            name: "net::receive_path",
            cost: 4,
            parallel: true,
            aux: &["arp", "ipv4_transport", "ipv6_icmp", "application"],
        },
        mode,
        || {
            let arp_req = {
                let mut b = [0u8; 28];
                arp::build_into(&mut b, &arp::request(MAC_B, IP_B, IP_A)).unwrap();
                frame(eth::EtherType::Arp, eth::MacAddr::BROADCAST, &b)
            };
            let offer = dhcp_reply(2, &[]);
            let dhcp = frame(
                eth::EtherType::Ipv4,
                eth::MacAddr::BROADCAST,
                &ip_packet(
                    IP_B,
                    ipv4::Ipv4Addr::BROADCAST,
                    ipv4::proto::UDP,
                    &udp_datagram(IP_B, ipv4::Ipv4Addr::BROADCAST, 67, 68, &offer),
                ),
            );
            let answer =
                dns_response(DNS_NAME, DNS_ID, &[&dns_a_record([1, 2, 3, 4], 300)]).unwrap();
            let dns = frame(
                eth::EtherType::Ipv4,
                MAC_A,
                &ip_packet(
                    IP_DNS,
                    IP_A,
                    ipv4::proto::UDP,
                    &udp_datagram(IP_DNS, IP_A, 53, 40001, &answer),
                ),
            );
            let ping = frame(
                eth::EtherType::Ipv4,
                MAC_A,
                &ip_packet(
                    IP_B,
                    IP_A,
                    ipv4::proto::ICMP,
                    &echo_message(EchoKind::Request, 3, 4, b"ping"),
                ),
            );
            let syn = tcp::SegmentHeader {
                src_port: 50000,
                dst_port: 80,
                seq: 77,
                ack: 0,
                flags: tcp::flags::SYN,
                window: 64240,
            };
            let tcp_syn = frame(
                eth::EtherType::Ipv4,
                MAC_A,
                &ip_packet(
                    IP_B,
                    IP_A,
                    ipv4::proto::TCP,
                    &tcp_segment(IP_B, IP_A, &syn, Some(1460), &[]),
                ),
            );
            let mut seeds = vec![arp_req, dhcp, dns, ping, tcp_syn];
            for p in ipv6_seeds() {
                seeds.push(frame(eth::EtherType::Ipv6, MAC_A, &p));
            }
            RxState { seeds }
        },
        |st, rng, ctx| {
            let fields = [
                (12, 2, true),
                (14, 1, false),
                (16, 2, true),
                (23, 1, false),
                (38, 2, true),
                (18, 2, true),
            ];
            let mut b = gen_bytes_f(
                rng,
                &st.seeds,
                &[&[0x08, 0x00], &[0x86, 0xdd], &[0x45], &[0x60]],
                &fields,
                1600,
            );
            if !rng.one_in(3) {
                fix_frame(&mut b);
            }
            ctx.note(&b);
            let Ok(f) = eth::parse(&b, rng.one_in(2)) else {
                return false;
            };
            match f.ethertype {
                eth::EtherType::Arp => {
                    if arp::parse(f.payload).is_ok() {
                        ctx.count(0);
                        return true;
                    }
                }
                eth::EtherType::Ipv4 => {
                    let Ok(ip) = ipv4::parse(f.payload) else {
                        return false;
                    };
                    let (src, dst) = (ip.header.src, ip.header.dst);
                    match ip.header.protocol {
                        ipv4::proto::ICMP => {
                            if icmp::parse_echo(ip.payload).is_ok() {
                                ctx.count(1);
                                return true;
                            }
                        }
                        ipv4::proto::UDP => {
                            if let Ok(d) = udp::parse(ip.payload, src, dst) {
                                ctx.count(1);
                                let app = match d.dst_port {
                                    68 => dhcp::parse_reply(d.payload, DHCP_XID, MAC_A).is_ok(),
                                    _ if d.src_port == 53 => {
                                        dns::parse_response(d.payload, DNS_ID, DNS_NAME).is_ok()
                                    }
                                    _ => false,
                                };
                                if app {
                                    ctx.count(3);
                                }
                                return true;
                            }
                        }
                        ipv4::proto::TCP => {
                            if let Ok(seg) = tcp::parse(ip.payload, src, dst) {
                                ctx.count(1);
                                let mut out = tcp::Outbox::new();
                                let mut listener = tcp::Tcb::listen(IP_A, 80, 1);
                                let _ = listener.on_segment(&seg, 0, &mut out);
                                let _ = tcp::reset_reply(&seg, &mut out);
                                drain_outbox(&mut out, &mut std::collections::VecDeque::new());
                                return true;
                            }
                        }
                        _ => return true,
                    }
                }
                eth::EtherType::Ipv6 => {
                    let Ok(p) = ipv6::parse(f.payload) else {
                        return false;
                    };
                    if ipv6::parse_icmp(&p).is_ok() {
                        ctx.count(2);
                    }
                    return true;
                }
                eth::EtherType::Other(_) => return true,
            }
            false
        },
    );
}

// ===========================================================================
// Bytes firmware, a device or a serial line supplies
// ===========================================================================

targets! {
    acpi_tables, full_acpi_tables => t_acpi;
    usb_descriptors, full_usb_descriptors => t_usb;
    pci_walk_capabilities, full_pci_walk_capabilities => t_pci;
    mouse_feed, full_mouse_feed => t_mouse;
    scancode_feed, full_scancode_feed => t_scancode;
    linedisc_feed, full_linedisc_feed => t_linedisc;
    memmap_validate, full_memmap_validate => t_memmap;
    marker_parse_line, full_marker_parse_line => t_marker;
}

// --- acpi ----------------------------------------------------------------------------

fn acpi_sum_fix(t: &mut [u8], at: usize) {
    t[at] = 0;
    let s = t.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    t[at] = 0u8.wrapping_sub(s);
}

fn acpi_table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(sig);
    t.extend_from_slice(&((acpi::SDT_HEADER_LEN + body.len()) as u32).to_le_bytes());
    t.push(1);
    t.push(0);
    t.extend_from_slice(b"ITISYO");
    t.extend_from_slice(b"TESTTBL ");
    t.extend_from_slice(&[0; 12]);
    t.extend_from_slice(body);
    acpi_sum_fix(&mut t, 9);
    t
}

/// Re-seal the checksum(s) a buffer's own header declares: the RSDP's two,
/// or a system description table's one over its declared length.
fn acpi_fix(b: &mut [u8]) {
    if b.len() >= acpi::RSDP_V1_LEN && &b[..8] == b"RSD PTR " {
        acpi_sum_fix(&mut b[..acpi::RSDP_V1_LEN], 8);
        if b.len() >= acpi::RSDP_V2_LEN {
            let l = (read_int(b, 20, 4, false) as usize).min(b.len());
            if l >= 33 {
                acpi_sum_fix(&mut b[..l], 32);
            }
        }
    } else if b.len() >= acpi::SDT_HEADER_LEN {
        let l = (read_int(b, 4, 4, false) as usize).min(b.len());
        if l > 9 {
            acpi_sum_fix(&mut b[..l], 9);
        }
    }
}

fn acpi_seeds() -> Vec<Vec<u8>> {
    let mut rsdp = Vec::new();
    rsdp.extend_from_slice(b"RSD PTR ");
    rsdp.push(0);
    rsdp.extend_from_slice(b"ITISYO");
    rsdp.push(2);
    rsdp.extend_from_slice(&0x1000u32.to_le_bytes());
    rsdp.extend_from_slice(&36u32.to_le_bytes());
    rsdp.extend_from_slice(&0x2000u64.to_le_bytes());
    rsdp.extend_from_slice(&[0; 4]);
    acpi_fix(&mut rsdp);
    let mut v1 = rsdp[..20].to_vec();
    v1[15] = 0;
    acpi_fix(&mut v1);
    let mut madt = Vec::new();
    madt.extend_from_slice(&0xfee0_0000u32.to_le_bytes());
    madt.extend_from_slice(&1u32.to_le_bytes());
    madt.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]);
    madt.extend_from_slice(&[0, 8, 1, 1, 1, 0, 0, 0]);
    let mut io = [1u8, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    io[4..8].copy_from_slice(&0xfec0_0000u32.to_le_bytes());
    madt.extend_from_slice(&io);
    let mut io2 = io;
    io2[2] = 1;
    io2[8..12].copy_from_slice(&24u32.to_le_bytes());
    madt.extend_from_slice(&io2);
    madt.extend_from_slice(&[2, 10, 0, 0, 2, 0, 0, 0, 0, 0]);
    madt.extend_from_slice(&[2, 10, 0, 9, 9, 0, 0, 0, 0x0d, 0]);
    madt.extend_from_slice(&[4, 6, 0xff, 0, 0, 1]);
    madt.extend_from_slice(&[5, 12, 0, 0, 0, 0, 0xe0, 0xfe, 0, 0, 0, 0]);
    madt.extend_from_slice(&[9, 16, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    let mut fadt = vec![0u8; 244 - 36];
    fadt[4..8].copy_from_slice(&0x0ffe_0040u32.to_le_bytes());
    fadt[10..12].copy_from_slice(&9u16.to_le_bytes());
    fadt[12..16].copy_from_slice(&0xb2u32.to_le_bytes());
    fadt[16] = 0xf1;
    fadt[28..32].copy_from_slice(&0x604u32.to_le_bytes());
    fadt[104..112].copy_from_slice(&0x1_0000_0040u64.to_le_bytes());
    let mut rsdt = Vec::new();
    rsdt.extend_from_slice(&0x1111u32.to_le_bytes());
    rsdt.extend_from_slice(&0x2222u32.to_le_bytes());
    let xsdt = 0x1_0000_0000u64.to_le_bytes();
    let qemu_s5 = [
        0x10u8, 0x08, b'_', b'S', b'5', b'_', 0x12, 0x06, 0x04, 0x00, 0x00, 0x00, 0x00,
    ];
    let rooted_s5 = [
        0x08u8, b'\\', b'_', b'S', b'5', b'_', 0x12, 0x07, 0x02, 0x0a, 0x05, 0x0b, 0x05, 0x00,
    ];
    let mut noisy = vec![
        0x5b, 0x80, b'_', b'S', b'5', b'X', 0x70, b'_', b'S', b'5', b'_', 0x60,
    ];
    noisy.extend_from_slice(&qemu_s5);
    // A MADT whose second I/O APIC's window ends at the top of the GSI
    // space: legal (a GSI is any u32), and the edge of `ioapic_for`.
    let mut high = madt.clone();
    high[44..48].copy_from_slice(&(u32::MAX - 23).to_le_bytes());
    vec![
        rsdp,
        v1,
        acpi_table(b"APIC", &madt),
        acpi_table(b"APIC", &high),
        acpi_table(b"FACP", &fadt),
        acpi_table(b"FACP", &fadt[..116 - 36]),
        acpi_table(b"RSDT", &rsdt),
        acpi_table(b"XSDT", &xsdt),
        acpi_table(b"DSDT", &qemu_s5),
        acpi_table(b"DSDT", &rooted_s5),
        acpi_table(b"DSDT", &noisy),
    ]
}

fn t_acpi(mode: Mode) {
    run(
        Target {
            name: "acpi::Rsdp+Madt+Fadt+root_entries+find_s5",
            cost: 3,
            parallel: false,
            aux: &["rsdp", "madt", "fadt", "s5"],
        },
        mode,
        acpi_seeds,
        |seeds, rng, ctx| {
            // Table and RSDP lengths, MADT entry lengths and GSI bases (as
            // laid out in the seed MADT), the _S5_ package length.
            let fields = [
                (4, 4, false),
                (20, 4, false),
                (45, 1, false),
                (53, 1, false),
                (61, 1, false),
                (68, 4, false),
                (73, 1, false),
                (80, 4, false),
                (85, 1, false),
                (88, 4, false),
                (98, 4, false),
                (105, 1, false),
                (111, 1, false),
                (123, 1, false),
                (43, 1, false),
                (44, 1, false),
            ];
            let mut b = gen_bytes_f(
                rng,
                seeds,
                &[
                    b"RSD PTR ",
                    b"APIC",
                    b"FACP",
                    b"DSDT",
                    b"_S5_",
                    &[0x08],
                    &[0x12],
                    &[0x0a],
                    &[0x0b],
                ],
                &fields,
                600,
            );
            if !rng.one_in(4) {
                acpi_fix(&mut b);
            }
            ctx.note(&b);
            let mut any = false;
            if let Ok(r) = acpi::Rsdp::parse(&b) {
                assert!(r.revision < 2 || b.len() >= acpi::RSDP_V2_LEN);
                ctx.count(0);
                any = true;
            }
            if let Ok(h) = acpi::peek_header(&b) {
                assert!(h.length as usize >= acpi::SDT_HEADER_LEN);
                if let Ok(t) = acpi::validate_table(&b, None) {
                    assert_eq!(t.len(), h.length as usize);
                    assert_eq!(t.iter().fold(0u8, |a, &x| a.wrapping_add(x)), 0);
                }
            }
            for sig in [b"APIC", b"FACP", b"DSDT", b"RSDT", b"XSDT"] {
                if let Ok(t) = acpi::validate_table(&b, Some(sig)) {
                    assert_eq!(&t[..4], sig);
                }
            }
            let mut out = vec![0u64; rng.below(16)];
            for xsdt in [false, true] {
                if let Ok(n) = acpi::root_entries(&b, xsdt, &mut out) {
                    assert!(n <= out.len());
                }
            }
            if let Ok(m) = acpi::Madt::parse(&b) {
                assert!(m.cpus <= acpi::MAX_CPUS);
                for _ in 0..8 {
                    let irq = rng.byte();
                    assert_eq!(m.isa_route(irq).source, irq);
                }
                for _ in 0..4 {
                    // The redirection-entry count comes from the chip's
                    // version register: 1..=256.
                    let entries = rng.range(1, 256) as u32;
                    let chips: Vec<_> = m.ioapics.iter().flatten().collect();
                    let gsi = match rng.below(3) {
                        0 if !chips.is_empty() => rng
                            .pick(&chips)
                            .gsi_base
                            .wrapping_add(rng.below(300) as u32),
                        1 => u32::MAX - rng.below(300) as u32,
                        _ => rng.u32(),
                    };
                    // The I/O APIC whose window really contains `gsi`.
                    let want = m
                        .ioapics
                        .iter()
                        .flatten()
                        .find(|a| {
                            gsi >= a.gsi_base
                                && u64::from(gsi) < u64::from(a.gsi_base) + u64::from(entries)
                        })
                        .copied();
                    assert_eq!(m.ioapic_for(gsi, |_| entries), want);
                }
                ctx.count(1);
                any = true;
            }
            if let Ok(f) = acpi::Fadt::parse(&b) {
                let _ = f.dsdt;
                ctx.count(2);
                any = true;
            }
            if let Ok(s) = acpi::find_s5(&b) {
                assert!(s.a <= 7 && s.b <= 7);
                let v = acpi::pm1_sleep_value(rng.u16(), s.a);
                assert_eq!((v >> 10) & 0b111, u16::from(s.a));
                assert_ne!(v & (1 << 13), 0);
                ctx.count(3);
                any = true;
            }
            any
        },
    );
}

// --- usb -------------------------------------------------------------------------------

fn t_usb(mode: Mode) {
    run(
        Target {
            name: "usb::descriptors+hid",
            cost: 1,
            parallel: false,
            aux: &["device", "hid_endpoint"],
        },
        mode,
        || {
            let kbd_desc = vec![
                18, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 8, 0x27, 0x06, 0x01, 0x00, 0x00, 0x00,
                0x01, 0x02, 0x00, 0x01,
            ];
            let kbd_cfg = vec![
                9, 0x02, 34, 0, 1, 1, 0, 0xa0, 50, 9, 0x04, 0, 0, 1, 0x03, 0x01, 0x01, 0, 9, 0x21,
                0x11, 0x01, 0, 1, 0x22, 63, 0, 7, 0x05, 0x81, 0x03, 8, 0, 10,
            ];
            let two_ifaces = vec![
                9, 0x02, 50, 0, 2, 1, 0, 0xa0, 50, 9, 0x04, 0, 0, 1, 0x08, 0x06, 0x50, 0, 7, 0x05,
                0x02, 0x02, 0, 2, 0, 9, 0x04, 1, 0, 1, 0x03, 0x01, 0x02, 0, 7, 0x05, 0x82, 0x03, 4,
                0, 10,
            ];
            vec![
                kbd_desc,
                kbd_cfg,
                two_ifaces,
                vec![0x02, 0, 0x04, 0x05, 0, 0, 0, 0],
                vec![0x01, 5, 0xfb, 1],
            ]
        },
        |seeds, rng, ctx| {
            let fields = [
                (0, 1, false),
                (9, 1, false),
                (18, 1, false),
                (27, 1, false),
                (34, 1, false),
            ];
            let b = gen_bytes_f(
                rng,
                seeds,
                &[&[9, 4], &[7, 5], &[0x81, 3], &[3, 1]],
                &fields,
                300,
            );
            ctx.note(&b);
            let mut any = false;
            if let Some(d) = usb::DeviceDescriptor::parse(&b) {
                assert!(b.len() >= 18 && b[1] == usb::desc_type::DEVICE);
                assert_eq!(d.vendor, u16::from_le_bytes([b[8], b[9]]));
                ctx.count(0);
                any = true;
            }
            if let Some((i, e)) = usb::find_hid_interrupt_in(&b) {
                assert!(i.class == usb::CLASS_HID && e.is_in() && e.is_interrupt());
                assert!(e.number() <= 15);
                ctx.count(1);
                any = true;
            }
            if let Some(c) = usb::hid_keyboard_ascii(&b) {
                assert!(c.is_ascii_graphic() || matches!(c, b' ' | b'\n' | 0x1b | 0x08));
            }
            match usb::hid_mouse(&b) {
                Some(m) => assert!(b.len() >= 3 && m.dx == b[1] as i8 && m.dy == b[2] as i8),
                None => assert!(b.len() < 3),
            }
            for &u in b.iter().take(8) {
                let _ = (
                    usb::hid_usage_ascii(u, false),
                    usb::hid_usage_ascii(u, true),
                );
            }
            any
        },
    );
}

// --- pci -------------------------------------------------------------------------------

fn pci_space(chain: &[(u8, u8, u8)], cap_ptr: u8, id: [u32; 3]) -> Vec<u8> {
    let mut cs = vec![0u8; 256];
    cs[0..4].copy_from_slice(&id[0].to_le_bytes());
    cs[8..12].copy_from_slice(&id[1].to_le_bytes());
    cs[12..16].copy_from_slice(&id[2].to_le_bytes());
    cs[0x10..0x14].copy_from_slice(&0xfebf_0004u32.to_le_bytes());
    cs[0x18..0x1c].copy_from_slice(&0x0000_c001u32.to_le_bytes());
    cs[0x34] = cap_ptr;
    for &(off, capid, next) in chain {
        cs[off as usize] = capid;
        cs[off as usize + 1] = next;
    }
    cs
}

fn t_pci(mode: Mode) {
    run(
        Target {
            name: "pci::walk_capabilities+PciId+BAR",
            cost: 1,
            parallel: false,
            aux: &["capabilities"],
        },
        mode,
        || {
            let nvme = [0x0010_1b36, 0x0108_0200, 0];
            vec![
                pci_space(
                    &[
                        (0x40, 0x01, 0x50),
                        (0x50, 0x05, 0x60),
                        (0x60, 0x11, 0x70),
                        (0x70, 0x10, 0),
                    ],
                    0x40,
                    nvme,
                ),
                pci_space(
                    &[(0x40, 0x05, 0x50), (0x50, 0x11, 0x40)],
                    0x40,
                    [0x2415_8086, 0x0401_0000, 0x0080_0000],
                ),
                pci_space(&[(0x40, 0x05, 0x40)], 0x43, [0x7020_8086, 0x0c03_0000, 0]),
                pci_space(&[(0x40, 0x05, 0x20)], 0x40, [0xffff_ffff; 3]),
                pci_space(
                    &(0x10u8..0x3f)
                        .map(|i| (i * 4, i, i * 4 + 4))
                        .collect::<Vec<_>>(),
                    0x40,
                    nvme,
                ),
            ]
        },
        |seeds, rng, ctx| {
            let mut cs = gen_bytes(
                rng,
                seeds,
                &[&[0x40], &[0x10], &[0x11], &[0xfc], &[0xff]],
                256,
            );
            cs.resize(256, 0);
            ctx.note(&cs);
            let read = |off: u8| {
                let o = usize::from(off);
                assert!(o % 4 == 0 && o + 4 <= 256, "unaligned config read {off:#x}");
                u32::from_le_bytes(cs[o..o + 4].try_into().unwrap())
            };
            let mut seen = Vec::new();
            let n = pci::walk_capabilities(cs[0x34], read, |c, off| seen.push((c, off)));
            assert_eq!(n, seen.len());
            assert!(n <= 48);
            for (i, (c, off)) in seen.iter().enumerate() {
                assert!(*off >= 0x40 && off % 4 == 0);
                assert!(
                    seen[..i].iter().all(|(_, o)| o != off),
                    "a capability offset was visited twice"
                );
                assert_eq!(pci::CapabilityId::from_u8(c.id()), *c);
                let _ = c.name();
            }
            let id = pci::PciId::decode(read(0), read(8), read(0xc));
            let _ = (
                id.class_name(),
                id.present(),
                id.is_multifunction(),
                id.has_cap_list_layout(),
            );
            let _ = (
                id.is_storage(),
                id.is_audio(),
                id.is_usb(),
                id.usb_kind().name(),
            );
            for i in 0..6u8 {
                let bar = read(0x10 + 4 * i);
                let (addr, mmio, is64) = pci::decode_bar(bar);
                assert!(mmio || (addr & 0x3 == 0 && !is64));
                let _ = pci::bar_size_from_mask(bar, !mmio);
            }
            let _ = pci::bar_size64(rng.u32(), rng.u32());
            ctx.aux[0] += n as u64;
            n > 0
        },
    );
}

// --- byte-stream state machines --------------------------------------------------

fn stream_input(rng: &mut Rng, seeds: &[Vec<u8>], dict: &[&[u8]], max: usize) -> Vec<u8> {
    let mut s = Vec::new();
    for _ in 0..rng.range(1, 8) {
        s.extend(gen_bytes(rng, seeds, dict, max));
    }
    s.truncate(max * 4);
    s
}

fn t_mouse(mode: Mode) {
    run(
        Target {
            name: "mouse::Mouse::feed",
            cost: 1,
            parallel: false,
            aux: &["events"],
        },
        mode,
        || {
            vec![
                vec![0x08, 5, 3],
                vec![0x08 | 0x01 | 0x10 | 0x20, 0xfb, 0xfe],
                vec![0xfa, 0x28, 0x28, 0xe7],
                vec![0x08 | 0x40 | 0x80, 0x7f, 0x7f],
                vec![0x00, 0x04, 0x0f, 0xff, 0x80],
            ]
        },
        |seeds, rng, ctx| {
            let s = stream_input(rng, seeds, &[&[0x08], &[0xfa], &[0xff]], 64);
            ctx.note(&s);
            let mut m = mouse::Mouse::new();
            let (mut any, mut calls) = (false, 0usize);
            for &byte in &s {
                if rng.one_in(32) {
                    m.resync();
                    calls += 1;
                }
                if let Some(e) = m.feed(byte) {
                    assert!((-256..=255).contains(&e.dx) && (-256..=255).contains(&e.dy));
                    ctx.count(0);
                    any = true;
                }
            }
            assert!(m.resyncs as usize <= s.len() + calls);
            any
        },
    );
}

fn t_scancode(mode: Mode) {
    run(
        Target {
            name: "scancode::Keyboard::feed",
            cost: 1,
            parallel: false,
            aux: &["events"],
        },
        mode,
        || {
            vec![
                vec![0x1e, 0x9e],
                vec![0x2a, 0x1e, 0xaa, 0x1e],
                vec![0xe0, 0x48, 0xe0, 0xc8],
                vec![0x02, 0x36, 0x02, 0xb6, 0x1c, 0x0e, 0x39],
            ]
        },
        |seeds, rng, ctx| {
            let s = stream_input(rng, seeds, &[&[0xe0], &[0x2a], &[0xaa], &[0x36]], 64);
            ctx.note(&s);
            let mut kb = scancode::Keyboard::new();
            let mut any = false;
            for &byte in &s {
                if let Some(e) = kb.feed(byte) {
                    assert!(e.scancode < 0x80);
                    assert_eq!(e.pressed, byte & 0x80 == 0);
                    if let Some(c) = e.ascii {
                        assert!(c.is_ascii_graphic() || matches!(c, b' ' | b'\n' | b'\t' | 0x08));
                    }
                    ctx.count(0);
                    any = true;
                } else {
                    assert_eq!(byte, 0xe0);
                }
                let _ = kb.shift_held();
            }
            any
        },
    );
}

fn t_linedisc(mode: Mode) {
    use linedisc::{Echo, Event, LineDisc, MAX_LINE};
    run(
        Target {
            name: "linedisc::LineDisc::feed",
            cost: 2,
            parallel: false,
            aux: &["lines", "overflows"],
        },
        mode,
        || {
            let mut long = vec![b'x'; MAX_LINE + 1];
            long.push(b'\n');
            vec![
                b"echo hi\n".to_vec(),
                b"\x08ab\x08c\x7f\x7f\x7fd\r".to_vec(),
                b"a\x01\x1b\x80\xffb\n".to_vec(),
                long,
            ]
        },
        |seeds, rng, ctx| {
            let s = stream_input(
                rng,
                seeds,
                &[b"\n", b"\r", b"\x08", b"\x7f", b"\x1b[A"],
                320,
            );
            ctx.note(&s);
            let mut ld = LineDisc::new();
            // A reference model of the documented editing rules.
            let (mut line, mut overflow) = (Vec::new(), false);
            let mut scratch = [0u8; 3];
            let mut any = false;
            for &byte in &s {
                let (echo, ev) = ld.feed(byte);
                let echoed = echo.bytes(&mut scratch).len();
                match byte {
                    b'\r' | b'\n' => {
                        assert_eq!(echo, Echo::Newline);
                        assert_eq!(
                            ev,
                            if overflow {
                                Event::Overflow
                            } else {
                                Event::Line
                            }
                        );
                    }
                    0x08 | 0x7f => {
                        let erased = line.pop().is_some();
                        assert_eq!(
                            (echo, ev),
                            (
                                if erased { Echo::Erase } else { Echo::Nothing },
                                Event::Pending
                            )
                        );
                    }
                    b' '..=b'~' => {
                        if line.len() < MAX_LINE {
                            line.push(byte);
                            assert_eq!(echo, Echo::Byte(byte));
                        } else {
                            overflow = true;
                            assert_eq!(echo, Echo::Nothing);
                        }
                        assert_eq!(ev, Event::Pending);
                    }
                    _ => assert_eq!((echo, ev, echoed), (Echo::Nothing, Event::Pending, 0)),
                }
                assert_eq!(ld.line(), &line[..]);
                if ev != Event::Pending {
                    if ev == Event::Line {
                        ctx.count(0);
                        any = true;
                    } else {
                        ctx.count(1);
                    }
                    ld.reset();
                    line.clear();
                    overflow = false;
                }
            }
            any
        },
    );
}

// --- memmap --------------------------------------------------------------------------

const REGION_BYTES: usize = 17;

fn regions_of(b: &[u8]) -> Vec<memmap::RawRegion> {
    b.as_chunks::<REGION_BYTES>()
        .0
        .iter()
        .take(256)
        .map(|c| memmap::RawRegion {
            start: read_int(c, 0, 8, false),
            end: read_int(c, 8, 8, false),
            kind: if c[16] & 1 == 0 {
                memmap::RegionKind::Usable
            } else {
                memmap::RegionKind::Reserved
            },
        })
        .collect()
}

fn region_bytes(list: &[(u64, u64, bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(s, e, reserved) in list {
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&e.to_le_bytes());
        out.push(u8::from(reserved));
    }
    out
}

fn t_memmap(mode: Mode) {
    use memmap::{RegionKind, FRAME_SIZE};
    run(
        Target {
            name: "memmap::validate+usable_frames",
            cost: 3,
            parallel: false,
            aux: &["frames_checked"],
        },
        mode,
        || {
            vec![
                // A QEMU-like map.
                region_bytes(&[
                    (0, 0x9fc00, false),
                    (0x9fc00, 0xa0000, true),
                    (0xf0000, 0x100000, true),
                    (0x100000, 0x7fe0000, false),
                    (0x7fe0000, 0x8000000, true),
                    (0xfffc_0000, 0x1_0000_0000, true),
                ]),
                region_bytes(&[(0x1000, 0x6000, false), (0x3000, 0x4000, true)]),
                region_bytes(&[(0x1800, 0x3000, false), (0x3800, 0x5000, true)]),
                region_bytes(&[(0, u64::MAX, true), (0x1000, 0x2000, false)]),
            ]
        },
        |seeds, rng, ctx| {
            let fields: Vec<Field> = (0..8)
                .flat_map(|i| {
                    [
                        (i * REGION_BYTES, 8, false),
                        (i * REGION_BYTES + 8, 8, false),
                    ]
                })
                .collect();
            let b = gen_bytes_f(rng, seeds, &[], &fields, REGION_BYTES * 32);
            ctx.note(&b);
            let mut regions = regions_of(&b);
            let Ok(stats) = memmap::validate(&mut regions) else {
                return false;
            };
            assert!(regions.windows(2).all(|w| w[0].start <= w[1].start));
            let usable: Vec<_> = regions
                .iter()
                .filter(|r| r.kind == RegionKind::Usable)
                .collect();
            assert!(
                usable.windows(2).all(|w| w[0].end <= w[1].start),
                "usable regions overlap"
            );
            let total: u64 = usable
                .iter()
                .map(|r| memmap::frames_in(r.start, r.end))
                .sum();
            assert_eq!(stats.usable_frames, total);
            assert!(stats.usable_frames > 0);
            // The frames the allocator will be handed: aligned, inside usable
            // memory, clear of every reserved region, strictly increasing.
            // (The walk visits every usable frame, so bound it by that count.)
            if stats.usable_frames > 4096 {
                return true;
            }
            let mut last = None;
            for f in memmap::usable_frames(&regions) {
                assert_eq!(f % FRAME_SIZE, 0);
                let end = f + FRAME_SIZE;
                assert!(usable.iter().any(|r| r.start <= f && end <= r.end));
                assert!(!regions
                    .iter()
                    .any(|r| r.kind == RegionKind::Reserved && f < r.end && end > r.start));
                assert!(last.is_none_or(|l| l < f));
                last = Some(f);
                ctx.count(0);
            }
            true
        },
    );
}

// --- marker ------------------------------------------------------------------------------

fn t_marker(mode: Mode) {
    run(
        Target {
            name: "marker::parse_line",
            cost: 1,
            parallel: false,
            aux: &["markers"],
        },
        mode,
        || {
            let mut lines: Vec<String> = stage::ALL_STAGES
                .iter()
                .map(|s| format!("[ITISYOU:{}] {}", s.code(), s.describe()))
                .collect();
            lines.extend(
                [
                    "garbage\x1b[0m[ITISYOU:PANIC] oh no",
                    "[ITISYOU:SELFTEST] pass=12 fail=0",
                    "[ITISYOU:TEST] name=heap_alloc result=pass",
                    "[ITISYOU:INFO] key=value free text",
                    "[ITISYOU:MODE] selftest",
                    "[RING3-U:B010] not a marker",
                ]
                .map(String::from),
            );
            lines
                .into_iter()
                .map(String::into_bytes)
                .collect::<Vec<_>>()
        },
        |seeds, rng, ctx| {
            let s = gen_text(
                rng,
                seeds,
                &[
                    b"[ITISYOU:",
                    b"]",
                    b"PANIC",
                    b"SELFTEST",
                    b"TEST",
                    b"pass=",
                    b"fail=",
                    b"name=",
                    b"result=",
                    b"B210",
                    b"4294967296",
                ],
                300,
            );
            ctx.note(s.as_bytes());
            let Some(m) = marker::parse_line(&s) else {
                return false;
            };
            assert!(s.contains(marker::PREFIX));
            if let marker::Marker::Stage(st, _) = m {
                assert_eq!(stage::Stage::from_code(st.code()), Some(st));
                assert!(s.contains(&format!("{}{}]", marker::PREFIX, st.code())));
            }
            ctx.count(0);
            true
        },
    );
}
