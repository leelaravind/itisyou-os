//! Process status word (V0.10): how a terminated process's outcome is
//! encoded in the single `u64` that `wait` returns, shared by the kernel
//! (which produces it) and `/sbin/init` (which supervises on it).
//!
//! ```text
//! Exited(code)    = code                   0x0_0000_0000 ..= 0x0_FFFF_FFFF
//! Faulted(vector) = (1 << 32) | vector     0x1_0000_0000 ..= 0x1_0000_00FF
//! Killed          = 2 << 32                0x2_0000_0000
//! ```
//!
//! The exit and fault encodings are the ones the kernel has produced since
//! V0.2 (`proc::encode_status`). Every valid status is below `3 << 32`, so it
//! can never be confused with a syscall error, which the kernel returns from
//! the top of the `u64` range (`u64::MAX - 7 ..= u64::MAX`). [`decode`] is
//! strict: any other word — an unknown tag, a fault vector above 255, or a
//! `Killed` word carrying a payload — is refused, not guessed at.

/// Tag of a fault status (high 32 bits).
pub const FAULT_TAG: u64 = 1 << 32;

/// The one `Killed` status word.
pub const KILLED: u64 = 2 << 32;

/// How a process terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Called `exit(code)`; the low 32 bits of the requested code.
    Exited(u32),
    /// Took a CPU exception with this vector and was terminated.
    Faulted(u8),
    /// Terminated by another process or the kernel.
    Killed,
}

impl Status {
    /// True only for `exit(0)`: every other termination is a failure.
    pub const fn is_clean(self) -> bool {
        matches!(self, Status::Exited(0))
    }
}

/// Encode a status as the `u64` word `wait` returns.
pub const fn encode(status: Status) -> u64 {
    match status {
        Status::Exited(code) => code as u64,
        Status::Faulted(vector) => FAULT_TAG | vector as u64,
        Status::Killed => KILLED,
    }
}

/// Decode a `wait` status word. `None` for any word [`encode`] never
/// produces — including every syscall error value.
pub const fn decode(word: u64) -> Option<Status> {
    let payload = word & 0xFFFF_FFFF;
    match word >> 32 {
        0 => Some(Status::Exited(payload as u32)),
        1 if payload <= 0xFF => Some(Status::Faulted(payload as u8)),
        2 if payload == 0 => Some(Status::Killed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's syscall error returns, mirrored from the `ERR_*`
    /// constants in `kernel/src/syscall.rs` (read-only reference; the kernel
    /// crate cannot be a dependency of this host-tested crate).
    const ERRORS: [(&str, u64); 8] = [
        ("ERR_NOSYS", u64::MAX),
        ("ERR_FAULT", u64::MAX - 1),
        ("ERR_BADF", u64::MAX - 2),
        ("ERR_NOENT", u64::MAX - 3),
        ("ERR_AGAIN", u64::MAX - 4),
        ("ERR_INVAL", u64::MAX - 5),
        ("ERR_2BIG", u64::MAX - 6),
        ("ERR_PERM", u64::MAX - 7),
    ];

    #[test]
    fn exit_codes_round_trip() {
        for code in [
            0u32,
            1,
            2,
            7,
            42,
            127,
            255,
            256,
            0x7FFF_FFFF,
            u32::MAX - 1,
            u32::MAX,
        ] {
            let word = encode(Status::Exited(code));
            assert_eq!(word, u64::from(code));
            assert_eq!(decode(word), Some(Status::Exited(code)));
        }
    }

    #[test]
    fn fault_vectors_round_trip() {
        for vector in 0..=u8::MAX {
            let word = encode(Status::Faulted(vector));
            // Same bits as the kernel's `0x1_0000_0000 | vector`.
            assert_eq!(word, 0x1_0000_0000 | u64::from(vector));
            assert_eq!(decode(word), Some(Status::Faulted(vector)));
        }
    }

    #[test]
    fn killed_round_trips_and_only_exit_zero_is_clean() {
        assert_eq!(encode(Status::Killed), 0x2_0000_0000);
        assert_eq!(decode(KILLED), Some(Status::Killed));
        assert!(Status::Exited(0).is_clean());
        assert!(!Status::Exited(1).is_clean());
        assert!(!Status::Exited(u32::MAX).is_clean());
        assert!(!Status::Faulted(0).is_clean());
        assert!(!Status::Killed.is_clean());
    }

    #[test]
    fn non_canonical_words_are_refused() {
        for word in [
            FAULT_TAG | 0x100,
            FAULT_TAG | 0xFFFF_FFFF,
            KILLED | 1,
            KILLED | 0xFFFF_FFFF,
            3 << 32,
            1 << 63,
            u64::MAX / 2,
        ] {
            assert_eq!(decode(word), None, "{word:#x}");
        }
    }

    #[test]
    fn no_syscall_error_collides_with_a_status() {
        // Every valid status word lies in one of three narrow ranges far
        // below the error range.
        let highest_status = [
            encode(Status::Exited(u32::MAX)),
            encode(Status::Faulted(u8::MAX)),
            encode(Status::Killed),
        ]
        .into_iter()
        .max()
        .unwrap();
        for (name, err) in ERRORS {
            assert_eq!(decode(err), None, "{name} decodes as a status");
            assert!(err > highest_status, "{name} overlaps the status range");
        }
        // The range the errors occupy is exactly u64::MAX-7 ..= u64::MAX.
        assert_eq!(ERRORS.iter().map(|e| e.1).min(), Some(u64::MAX - 7));
    }
}
