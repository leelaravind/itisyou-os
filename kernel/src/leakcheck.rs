//! Resource accounting for the soak leg (V1.0, V1-REL-003).
//!
//! `leakcheck mark` records what the kernel holds right now; a later
//! `leakcheck` compares the same counters and says whether anything grew in
//! between. Run after a warm-up (so bounded rings, queues and tables that
//! fill once are already full) and between identical workloads, every
//! counter must come back EXACTLY where it was: a process that exited left
//! no frame, no heap allocation, no table slot, no capability handle, no
//! queued message, no socket and no output buffer behind.
//!
//! The heap counter excludes the audit trail's own strings: its ring and
//! window are bounded, but each record spells out a sequence number, a tick
//! and a pid, which grow a digit at a time, so its footprint creeps with
//! the clock rather than with any leak (`audit::heap_bytes` measures it
//! exactly, and is subtracted).

use crate::sync::Mutex;

/// What the kernel holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub frames_free: i64,
    pub heap_used: i64,
    pub processes: i64,
    pub handles: i64,
    pub ipc_queued: i64,
    pub sockets: i64,
    pub tcp: i64,
    pub out_slots: i64,
}

static MARK: Mutex<Option<Counters>> = Mutex::new(None);

fn n(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Sample every counter now.
pub fn sample() -> Counters {
    let frames_free = crate::memory::stats().map_or(0, |(free, _)| free) as i64;
    let (heap_used, _) = crate::memory::heap::stats();
    let mut tcp = 0usize;
    crate::net::tcp::for_each(|_| tcp += 1);
    Counters {
        frames_free,
        heap_used: n(heap_used) - n(crate::audit::heap_bytes()),
        processes: n(crate::proc::live_count()),
        handles: n(crate::capability::count_live()),
        ipc_queued: n(crate::ipc::queued()),
        sockets: n(crate::net::socket::with_sockets(|s| s.len())),
        tcp: n(tcp),
        out_slots: n(crate::console_out::slots_in_use()),
    }
}

/// Console `leakcheck [mark]`.
pub fn console(args: &[&str]) {
    let now = sample();
    if args.first() == Some(&"mark") {
        *MARK.lock() = Some(now);
        crate::serial_println!(
            "[ITISYOU:LEAK] mark frames_free={} heap_used={} processes={} handles={} ipc_queued={} sockets={} tcp={} out_slots={}",
            now.frames_free,
            now.heap_used,
            now.processes,
            now.handles,
            now.ipc_queued,
            now.sockets,
            now.tcp,
            now.out_slots
        );
        return;
    }
    let Some(mark) = *MARK.lock() else {
        crate::serial_println!("leakcheck: no mark (run `leakcheck mark` first)");
        return;
    };
    // Frames: a leak shows as FEWER free frames; everything else as more.
    let d = Counters {
        frames_free: mark.frames_free - now.frames_free,
        heap_used: now.heap_used - mark.heap_used,
        processes: now.processes - mark.processes,
        handles: now.handles - mark.handles,
        ipc_queued: now.ipc_queued - mark.ipc_queued,
        sockets: now.sockets - mark.sockets,
        tcp: now.tcp - mark.tcp,
        out_slots: now.out_slots - mark.out_slots,
    };
    let clean = d
        == Counters {
            frames_free: 0,
            heap_used: 0,
            processes: 0,
            handles: 0,
            ipc_queued: 0,
            sockets: 0,
            tcp: 0,
            out_slots: 0,
        };
    crate::serial_println!(
        "[ITISYOU:LEAK] check frames={} heap={} processes={} handles={} ipc_queued={} sockets={} tcp={} out_slots={} result={}",
        d.frames_free,
        d.heap_used,
        d.processes,
        d.handles,
        d.ipc_queued,
        d.sockets,
        d.tcp,
        d.out_slots,
        if clean { "clean" } else { "leak" }
    );
}
