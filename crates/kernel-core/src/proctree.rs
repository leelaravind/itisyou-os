//! Process-tree rules (V0.10, PROC10-002): who may wait for whom, where an
//! orphan goes, when a sleeper is due, and which zombie `wait_nohang(0)`
//! reaps. Pure decisions; the kernel's process table applies them.

/// The parent of a process whose parent died with no adopter: it is removed
/// as soon as it terminates (nobody will ever wait for it).
pub const ORPHAN_PARENT: u64 = u64::MAX;

/// May `caller` wait for a process whose parent is `child_parent`? Only its
/// own parent may: a process must not be able to collect — and so erase —
/// another process's child, or learn how it ended.
pub const fn may_wait(child_parent: u64, caller: u64) -> bool {
    child_parent == caller && child_parent != ORPHAN_PARENT
}

/// Where the children of a dying process go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adopt {
    /// To the adopter (`/sbin/init`, once it runs).
    Init(u64),
    /// Nowhere: a live child is reaped automatically when it terminates, and
    /// a child that already terminated is removed now.
    AutoReap,
}

/// The adopter for the children of `dying`, given the registered adopter
/// (`0` = none). An adopter never adopts its own children — they have no
/// one left to report to.
pub const fn adopt_target(adopter: u64, dying: u64) -> Adopt {
    if adopter != 0 && adopter != dying && adopter != ORPHAN_PARENT {
        Adopt::Init(adopter)
    } else {
        Adopt::AutoReap
    }
}

/// Is a sleep with this `deadline` (in ticks) over at `now`?
pub const fn due(deadline: u64, now: u64) -> bool {
    now >= deadline
}

/// The deadline of a sleep of `ticks` starting at `now` (saturating: a
/// deadline can never wrap around into the past).
pub const fn deadline(now: u64, ticks: u64) -> u64 {
    now.saturating_add(ticks)
}

/// The zombie `wait_nohang(0)` reaps: the lowest pid among `children`
/// (pairs of pid and "has terminated"). `None` if no child has terminated.
pub fn pick_reapable(children: impl IntoIterator<Item = (u64, bool)>) -> Option<u64> {
    children
        .into_iter()
        .filter(|&(_, terminated)| terminated)
        .map(|(pid, _)| pid)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_parent_may_wait() {
        assert!(may_wait(5, 5));
        assert!(!may_wait(5, 6));
        assert!(
            !may_wait(0, 7),
            "the console's children are not a process's"
        );
        assert!(may_wait(0, 0));
        assert!(!may_wait(ORPHAN_PARENT, ORPHAN_PARENT));
    }

    #[test]
    fn orphans_go_to_the_adopter_or_are_auto_reaped() {
        assert_eq!(adopt_target(1, 9), Adopt::Init(1));
        assert_eq!(adopt_target(0, 9), Adopt::AutoReap);
        assert_eq!(adopt_target(1, 1), Adopt::AutoReap, "init's own children");
        assert_eq!(adopt_target(ORPHAN_PARENT, 9), Adopt::AutoReap);
    }

    #[test]
    fn a_sleep_is_due_exactly_at_its_deadline() {
        let d = deadline(100, 50);
        assert_eq!(d, 150);
        assert!(!due(d, 149));
        assert!(due(d, 150));
        assert!(due(d, 151));
        assert_eq!(deadline(u64::MAX - 1, 50), u64::MAX);
    }

    #[test]
    fn the_lowest_terminated_pid_is_reaped_first() {
        assert_eq!(pick_reapable([(9, true), (4, false), (7, true)]), Some(7));
        assert_eq!(pick_reapable([(3, false), (4, false)]), None);
        assert_eq!(pick_reapable([]), None);
    }
}
