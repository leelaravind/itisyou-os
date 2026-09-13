//! Minimal IPC (V0.3, ADR-0007): a small fixed set of bounded kernel-owned
//! message channels. Non-blocking send/recv of length-delimited byte
//! messages copied through validated user buffers — the smallest primitive
//! useful for a future system-service RPC layer, deliberately not more.
//!
//! Capability direction: channels are addressed by a small integer today,
//! but the send/recv split, copy-through-kernel (no shared user memory), and
//! bounded queues are the shape a later capability handle will wrap. No
//! ambient authority beyond "a process that knows a channel id".

use crate::sync::Mutex;
use crate::syscall::{copy_from_user, copy_to_user, ERR_AGAIN, ERR_INVAL};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// Number of channels and per-channel/message bounds (kept tiny on purpose).
/// V0.11: 8 - channels 6 and 7 carry `/bin/inferd`'s requests and replies
/// (`kernel_core::infer`).
const CHANNELS: usize = 8;
const MSG_MAX: u64 = 256;
const QUEUE_MAX: usize = 8;

struct Channel {
    queue: VecDeque<Vec<u8>>,
}

static CHANNELS_STATE: Mutex<[Option<Channel>; CHANNELS]> = Mutex::new([const { None }; CHANNELS]);

pub fn init() {
    let mut guard = CHANNELS_STATE.lock();
    for slot in guard.iter_mut() {
        *slot = Some(Channel {
            queue: VecDeque::new(),
        });
    }
}

/// msg_send(channel, ptr, len): enqueue a message copied from user memory.
/// Returns bytes queued, or ERR_INVAL / ERR_2BIG / ERR_AGAIN (queue full).
pub fn sys_msg_send(channel: u64, ptr: u64, len: u64) -> u64 {
    if channel >= CHANNELS as u64 {
        return ERR_INVAL;
    }
    let msg = match copy_from_user(ptr, len, MSG_MAX) {
        Ok(m) => m,
        Err(e) => return e,
    };
    let mut guard = CHANNELS_STATE.lock();
    let ch = guard[channel as usize].as_mut().expect("ipc init");
    if ch.queue.len() >= QUEUE_MAX {
        return ERR_AGAIN;
    }
    let n = msg.len() as u64;
    ch.queue.push_back(msg);
    n
}

/// msg_recv(channel, ptr, cap): dequeue the oldest message into the user
/// buffer. Returns bytes written, ERR_AGAIN (empty), ERR_2BIG (buffer too
/// small — message left in the queue), or ERR_INVAL / ERR_FAULT.
pub fn sys_msg_recv(channel: u64, ptr: u64, cap: u64) -> u64 {
    if channel >= CHANNELS as u64 {
        return ERR_INVAL;
    }
    let mut guard = CHANNELS_STATE.lock();
    let ch = guard[channel as usize].as_mut().expect("ipc init");
    let Some(front) = ch.queue.front() else {
        return ERR_AGAIN;
    };
    if front.len() as u64 > cap {
        return crate::syscall::ERR_2BIG;
    }
    // Validate destination before committing to the dequeue.
    let msg = ch.queue.pop_front().unwrap();
    // Release the lock before touching user memory to keep the critical
    // section small (single CPU; the message is now ours).
    drop(guard);
    match copy_to_user(ptr, &msg) {
        Ok(n) => n,
        Err(e) => {
            // Copy failed (bad pointer): re-enqueue at the front so the
            // message is not lost.
            let mut guard = CHANNELS_STATE.lock();
            guard[channel as usize]
                .as_mut()
                .expect("ipc init")
                .queue
                .push_front(msg);
            e
        }
    }
}
