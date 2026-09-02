//! Service supervision logic (V0.7): deterministic startup ordering with
//! dependency-cycle detection, service states, and a bounded restart policy.
//! Pure + host-tested; the kernel's supervisor drives real Ring 3 processes
//! with this logic.

pub const MAX_SERVICES: usize = 8;
pub const MAX_DEPS: usize = 4;
/// A service that keeps failing is restarted at most this many times before
/// it is marked `Failed` permanently (no restart storms).
pub const RESTART_LIMIT: u32 = 3;

/// Lifecycle state of a supervised service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    Running,
    /// Exited cleanly (code 0).
    Done,
    /// Crashed/exited nonzero; `restarts` restarts performed so far.
    Restarting {
        restarts: u32,
    },
    /// Exceeded the restart limit — permanently failed.
    Failed {
        restarts: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderError {
    /// A dependency names an unknown service.
    UnknownDependency,
    /// The dependency graph contains a cycle (order is impossible).
    Cycle,
    TooManyServices,
}

/// Compute a deterministic startup order for `services` (each `(name, deps)`)
/// respecting dependencies: every service starts after all of its deps.
/// Deterministic: among ready services, lower index starts first. Returns the
/// indices in start order.
pub fn startup_order(
    services: &[(&str, &[&str])],
) -> Result<([usize; MAX_SERVICES], usize), OrderError> {
    let n = services.len();
    if n > MAX_SERVICES {
        return Err(OrderError::TooManyServices);
    }
    // Resolve dependency names to indices up front (unknown = error).
    let mut deps: [[usize; MAX_DEPS]; MAX_SERVICES] = [[usize::MAX; MAX_DEPS]; MAX_SERVICES];
    let mut dep_count = [0usize; MAX_SERVICES];
    for (i, (_, ds)) in services.iter().enumerate() {
        for d in ds.iter() {
            let j = services
                .iter()
                .position(|(name, _)| name == d)
                .ok_or(OrderError::UnknownDependency)?;
            if dep_count[i] >= MAX_DEPS {
                return Err(OrderError::TooManyServices);
            }
            deps[i][dep_count[i]] = j;
            dep_count[i] += 1;
        }
    }
    // Kahn's algorithm with deterministic (lowest-index-first) selection.
    let mut order = [0usize; MAX_SERVICES];
    let mut placed = [false; MAX_SERVICES];
    let mut count = 0usize;
    while count < n {
        let mut advanced = false;
        for i in 0..n {
            if placed[i] {
                continue;
            }
            let ready = (0..dep_count[i]).all(|k| placed[deps[i][k]]);
            if ready {
                order[count] = i;
                placed[i] = true;
                count += 1;
                advanced = true;
            }
        }
        if !advanced {
            return Err(OrderError::Cycle);
        }
    }
    Ok((order, n))
}

/// Restart policy: should a service that just terminated abnormally be
/// restarted, given how many restarts it has already had? Bounded so a
/// crash-looping service converges to `Failed` instead of spinning forever.
pub fn should_restart(restarts_so_far: u32) -> bool {
    restarts_so_far < RESTART_LIMIT
}

/// Next state after a service terminates.
/// `clean` = exited with code 0.
pub fn on_exit(clean: bool, restarts_so_far: u32) -> ServiceState {
    if clean {
        ServiceState::Done
    } else if should_restart(restarts_so_far) {
        ServiceState::Restarting {
            restarts: restarts_so_far + 1,
        }
    } else {
        ServiceState::Failed {
            restarts: restarts_so_far,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_respecting_dependencies() {
        // c depends on b depends on a; d independent.
        let svcs: &[(&str, &[&str])] = &[("c", &["b"]), ("a", &[]), ("b", &["a"]), ("d", &[])];
        let (order, n) = startup_order(svcs).unwrap();
        let pos = |i: usize| order[..n].iter().position(|&x| x == i).unwrap();
        assert!(pos(1) < pos(2)); // a before b
        assert!(pos(2) < pos(0)); // b before c
        assert_eq!(n, 4);
        // Deterministic: same input, same order.
        assert_eq!(startup_order(svcs).unwrap().0[..n], order[..n]);
    }

    #[test]
    fn detects_cycles() {
        let direct: &[(&str, &[&str])] = &[("a", &["b"]), ("b", &["a"])];
        assert_eq!(startup_order(direct).unwrap_err(), OrderError::Cycle);
        let self_dep: &[(&str, &[&str])] = &[("a", &["a"])];
        assert_eq!(startup_order(self_dep).unwrap_err(), OrderError::Cycle);
        let indirect: &[(&str, &[&str])] = &[("a", &["c"]), ("b", &["a"]), ("c", &["b"])];
        assert_eq!(startup_order(indirect).unwrap_err(), OrderError::Cycle);
    }

    #[test]
    fn rejects_unknown_dependency() {
        let svcs: &[(&str, &[&str])] = &[("a", &["ghost"])];
        assert_eq!(
            startup_order(svcs).unwrap_err(),
            OrderError::UnknownDependency
        );
    }

    #[test]
    fn restart_policy_is_bounded() {
        assert_eq!(on_exit(true, 0), ServiceState::Done);
        assert_eq!(on_exit(false, 0), ServiceState::Restarting { restarts: 1 });
        assert_eq!(on_exit(false, 2), ServiceState::Restarting { restarts: 3 });
        // At the limit: no further restarts — permanently failed.
        assert_eq!(
            on_exit(false, RESTART_LIMIT),
            ServiceState::Failed {
                restarts: RESTART_LIMIT
            }
        );
        assert!(!should_restart(RESTART_LIMIT));
    }
}
