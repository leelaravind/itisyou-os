//! `sysfuzz` - seeded random syscalls against the kernel (V1.0, V1-REL-004).
//!
//! `sysfuzz <seed> <calls>` issues `calls` syscalls, each a pseudo-random
//! syscall number (every defined one, and numbers past the last) with three
//! pseudo-random arguments drawn from values that find bugs: zero and small
//! integers, pointers into a writable scratch buffer, into this program's
//! read-only code, into its stack, the edges of the user window and of the
//! stack, kernel addresses, error-code-shaped values and plain random words.
//! The buffer holds random bytes, refreshed as it goes, so a call that reads
//! a structure from user memory gets garbage; a third of the calls also get
//! a real path from a small table as a (pointer, length) pair, and the buffer
//! holds a well-formed request record, so calls get past their argument
//! checks into the code behind them. The kernel must survive every call.
//! At the end it prints
//! `SYSFUZZ-OK seed=<s> calls=<n> ok=<k> errors=<e> reached=<numbers>`,
//! `reached` listing every syscall number that succeeded at least once.
//! Replaying a seed with a smaller count replays a prefix of the run, so a
//! crash bisects to one call. `sysfuzz drain` empties the IPC channels
//! afterwards (see [`drain`]).
//!
//! Left out of the draw, because they wait legitimately rather than
//! misbehave: `exit` (it would end the run), `console_read` and `gui_event`
//! (they wait for input), `net_resolve` and `tcp_connect` (network
//! timeouts). `sleep` is drawn with at most one tick, and `write` with at
//! most 256 bytes, so the run stays short and its output readable.
//! `cap_revoke` and `cap_restrict` are drawn at 1/128 of the normal rate:
//! handle ids are small numbers, so otherwise the fuzzer revokes its own
//! capabilities within a few hundred calls and spends the rest of the run
//! on the denial paths.
//!
//! The kernel writes wherever it is pointed, so nothing the fuzzer needs may
//! sit where it points: the counters and the generator live in statics, the
//! scratch buffer in the outermost stack frame (a write past it runs into
//! dead locals and then the unmapped page above the stack, which the kernel
//! refuses whole), and stack pointers are drawn from the bottom of the stack,
//! tens of KiB below the frames in use. No `unsafe`.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

use ulib::{args, exit, msg_recv, raw_syscall, split_args, write, write_u64};

const SYS_WRITE: u64 = 0;
const SYS_EXIT: u64 = 1;
const SYS_CAP_REVOKE: u64 = 17;
const SYS_CAP_RESTRICT: u64 = 18;
const SYS_NET_RESOLVE: u64 = 25;
const SYS_TCP_CONNECT: u64 = 29;
const SYS_SLEEP: u64 = 38;
const SYS_CONSOLE_READ: u64 = 40;
const SYS_GUI_EVENT: u64 = 41;
/// One past the last defined syscall; a few numbers beyond it are drawn too.
const SYS_END: u64 = 45;

/// The scratch buffer. Pointers are drawn into its first `TARGET` bytes; the
/// rest takes a kernel write that runs past them.
const SCRATCH: usize = 16 * 1024;
const TARGET: u64 = 4096;
/// Where the request record sits in the buffer: `{ptr: u64, len: u64}`, the
/// shape `fs_read` and `fs_write` take, pointing further into the buffer.
const REQ_AT: usize = 256;
const REQ_DATA_AT: u64 = 1024;
/// Paths the kernel resolves: a program, files inside and outside each
/// fuzz leg's sandbox, directories and traversals. At the start of the
/// buffer, each followed by a NUL.
const STRINGS: [&[u8]; 7] = [
    b"/bin/child",
    b"/data/fz",
    b"/data",
    b"/etc/ai/diag.model",
    b"/etc/ai/../../bin/child",
    b"/",
    b"/data/../etc/ai/diag.model",
];
/// The user stack: 64 KiB below a 64 KiB-aligned top (`USER_STACK_TOP`).
const STACK_SIZE: u64 = 64 * 1024;
/// Stack pointers are drawn from this much of the bottom of the stack.
const STACK_DRAW: u64 = 16 * 1024;
const PROGRESS_EVERY: u64 = 25_000;

static RNG: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static TOTAL: AtomicU64 = AtomicU64::new(0);
static OK: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);
/// Bit `n` set: syscall `n` succeeded at least once.
static REACHED: AtomicU64 = AtomicU64::new(0);
static BUF: AtomicU64 = AtomicU64::new(0);
static STACK_FLOOR: AtomicU64 = AtomicU64::new(0);

fn next() -> u64 {
    // splitmix64
    let s = RNG.load(Relaxed).wrapping_add(0x9E37_79B9_7F4A_7C15);
    RNG.store(s, Relaxed);
    let mut z = s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn below(n: u64) -> u64 {
    next() % n
}

/// Offset and length of `STRINGS[k]` in the buffer.
fn string(k: usize) -> (u64, u64) {
    let at: usize = STRINGS[..k].iter().map(|s| s.len() + 1).sum();
    (at as u64, STRINGS[k].len() as u64)
}

/// One argument, from the shapes that find bugs.
fn arg(code: u64) -> u64 {
    let buf = BUF.load(Relaxed);
    let floor = STACK_FLOOR.load(Relaxed);
    match below(16) {
        0 => 0,
        // Descriptors, channels, handles, pids, window ids: small numbers.
        1 => 1 + below(8),
        15 => below(256),
        2 => buf + below(TARGET),
        3 => buf + TARGET - below(16),
        4 => code + below(256),
        5 => floor + below(STACK_DRAW),
        6 => [
            0x1000,
            0xF_FFFF,
            0x10_0000,
            floor - 8,              // straddles the guard page below the stack
            floor + STACK_SIZE - 8, // straddles the top of the stack
            0x80_0000_0000 - 8,
            0x80_0000_0000,
        ][below(7) as usize],
        7 => 0xFFFF_8000_0000_0000 + below(1 << 30) * 8,
        8 => u64::MAX - below(16),
        9 => 1 << below(64),
        10 => below(1 << 16),
        11 => buf + REQ_AT as u64,
        12 => buf + string(below(STRINGS.len() as u64) as usize).0,
        _ => next(),
    }
}

/// Refresh the buffer: random bytes, then the strings and the request.
fn refill(scratch: &mut [u8; SCRATCH], code: u64) {
    for b in scratch[..TARGET as usize].iter_mut() {
        *b = next() as u8;
    }
    for (k, s) in STRINGS.iter().enumerate() {
        let at = string(k).0 as usize;
        scratch[at..at + s.len()].copy_from_slice(s);
        scratch[at + s.len()] = 0;
    }
    let ptr = BUF.load(Relaxed) + REQ_DATA_AT;
    let len = match below(3) {
        0 => below(64),
        1 => below(SCRATCH as u64 - REQ_DATA_AT),
        _ => arg(code),
    };
    scratch[REQ_AT..REQ_AT + 8].copy_from_slice(&ptr.to_le_bytes());
    scratch[REQ_AT + 8..REQ_AT + 16].copy_from_slice(&len.to_le_bytes());
}

/// One fuzzed syscall.
fn step(code: u64) {
    let nr = loop {
        let nr = match below(20) {
            0 => SYS_END + below(8),
            1 => [64, 1000, 0xFFFF, u64::MAX][below(4) as usize],
            _ => below(SYS_END),
        };
        if matches!(
            nr,
            SYS_EXIT | SYS_NET_RESOLVE | SYS_TCP_CONNECT | SYS_CONSOLE_READ | SYS_GUI_EVENT
        ) {
            continue;
        }
        if matches!(nr, SYS_CAP_REVOKE | SYS_CAP_RESTRICT) && below(128) != 0 {
            continue;
        }
        break nr;
    };
    let mut a = [arg(code), arg(code), arg(code)];
    if below(3) == 0 {
        // A real path as a (pointer, length) pair, at either position.
        let (at, len) = string(below(STRINGS.len() as u64) as usize);
        let p = below(2) as usize;
        a[p] = BUF.load(Relaxed) + at;
        a[p + 1] = len;
        if p == 0 && below(2) == 0 {
            // fs_read(path, len, req) and fs_write(path, len, req).
            a[2] = BUF.load(Relaxed) + REQ_AT as u64;
        }
    }
    if nr == SYS_SLEEP {
        a[0] = below(2);
    }
    if nr == SYS_WRITE {
        a[2] = a[2].min(256); // write(fd, ptr, len): the length
    }
    let r = raw_syscall(nr, a[0], a[1], a[2]);
    if r > u64::MAX - 16 {
        ERRORS.fetch_add(1, Relaxed);
    } else {
        OK.fetch_add(1, Relaxed);
        if nr < 64 {
            REACHED.fetch_or(1 << nr, Relaxed);
        }
    }
}

#[inline(never)]
fn fuzz(seed: u64, calls: u64, scratch: &mut [u8; SCRATCH]) -> ! {
    RNG.store(seed, Relaxed);
    TOTAL.store(calls, Relaxed);
    BUF.store(scratch.as_mut_ptr() as u64, Relaxed);
    let here = &seed as *const u64 as u64;
    STACK_FLOOR.store(here & !(STACK_SIZE - 1), Relaxed);
    let code = fuzz as *const () as u64;
    write("SYSFUZZ-START seed=");
    write_u64(seed);
    write(" calls=");
    write_u64(calls);
    write("\n");
    loop {
        let i = DONE.load(Relaxed);
        if i >= TOTAL.load(Relaxed) {
            break;
        }
        DONE.store(i + 1, Relaxed);
        if i.is_multiple_of(64) {
            refill(scratch, code);
        }
        if i > 0 && i.is_multiple_of(PROGRESS_EVERY) {
            write("\nSYSFUZZ-PROGRESS calls=");
            write_u64(i);
            write("\n");
        }
        step(code);
    }
    let (ok, errors) = (OK.load(Relaxed), ERRORS.load(Relaxed));
    if ok + errors != calls || DONE.load(Relaxed) != calls {
        write("\nSYSFUZZ-FAILED step=count ok=");
        write_u64(ok);
        write(" errors=");
        write_u64(errors);
        write("\n");
        exit(1)
    }
    write("\nSYSFUZZ-OK seed=");
    write_u64(seed);
    write(" calls=");
    write_u64(calls);
    write(" ok=");
    write_u64(ok);
    write(" errors=");
    write_u64(errors);
    write(" reached=");
    let reached = REACHED.load(Relaxed);
    let mut first = true;
    for n in 0..64 {
        if reached & (1 << n) != 0 {
            if !first {
                write(",");
            }
            write_u64(n);
            first = false;
        }
    }
    write("\n");
    exit(0)
}

/// `sysfuzz drain` (run with `ipc:0-7`): take every message queued on the
/// eight channels. A message outlives its sender (docs/KNOWN_LIMITATIONS.md),
/// so what a fuzz run with channel authority sent stays queued - bounded,
/// 8 x 8 - and would be read by the next client of that channel as if it
/// were an answer. The fuzz legs drain before each leak sample and before
/// the sanity pass.
fn drain() -> ! {
    let mut buf = [0u8; 256];
    let mut taken = 0u64;
    for channel in 0..8 {
        // Bounded: a channel holds at most 8, but a service may refill it.
        for _ in 0..64 {
            if msg_recv(channel, &mut buf) > u64::MAX - 16 {
                break;
            }
            taken += 1;
        }
    }
    write("SYSFUZZ-DRAINED messages=");
    write_u64(taken);
    write("\n");
    exit(0)
}

#[unsafe(no_mangle)]
extern "C" fn _start() -> ! {
    // The outermost frame: the scratch buffer sits above every frame the
    // fuzzer uses, so a kernel write that runs past it cannot reach them.
    let mut scratch = [0u8; SCRATCH];
    let mut block = [0u8; 64];
    let len = args(&mut block) as usize;
    let mut words = split_args(&block[..len.min(block.len())]);
    let first = words.next();
    if first == Some(&b"drain"[..]) {
        drain()
    }
    let seed = first.and_then(parse);
    let calls = words.next().and_then(parse).filter(|&n| n <= 10_000_000);
    match (seed, calls) {
        (Some(s), Some(n)) => fuzz(s, n, &mut scratch),
        _ => {
            write("SYSFUZZ-FAILED step=args\n");
            exit(1)
        }
    }
}

fn parse(b: &[u8]) -> Option<u64> {
    if b.is_empty() || b.len() > 19 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(b.iter().fold(0u64, |n, d| {
        n.wrapping_mul(10).wrapping_add(u64::from(d - b'0'))
    }))
}
