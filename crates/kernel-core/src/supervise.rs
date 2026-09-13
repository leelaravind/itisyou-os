//! Restart policies for configured services (V0.10).
//!
//! `/etc/init.conf` gives every service one of three policies. This module is
//! the single place that turns "the service just terminated" into the next
//! [`ServiceState`], so `/sbin/init` and the kernel's supervisor cannot drift:
//!
//! | policy       | clean exit (code 0)        | failure (nonzero exit, fault, kill) |
//! |--------------|----------------------------|-------------------------------------|
//! | `always`     | failure — restart (V0.8)   | restart                             |
//! | `on-failure` | `Done`                     | restart                             |
//! | `never`      | `Done`                     | `Failed` immediately                |
//!
//! "Restart" is bounded by [`service::RESTART_LIMIT`] through
//! [`service::on_exit`]: a service that keeps failing converges to `Failed`
//! instead of restarting forever.
//!
//! `always` is the long-running (daemon) policy. A daemon is supposed to run
//! for the life of the system, so its clean exit is a failure, not success —
//! the rule the V0.8 background supervisor applies to `tickd` and `flapd`.

use crate::service::{self, ServiceState};

/// What happens when a supervised service terminates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Restart {
    /// Long-running: every exit, even a clean one, is a failure and is
    /// restarted until the ceiling.
    Always,
    /// A clean exit is `Done`; a failure is restarted until the ceiling.
    /// The default when a config line names no policy.
    #[default]
    OnFailure,
    /// Never restarted: a clean exit is `Done`, a failure is `Failed`.
    Never,
}

impl Restart {
    /// Every policy, in declaration order.
    pub const ALL: [Restart; 3] = [Restart::Always, Restart::OnFailure, Restart::Never];

    /// The policy's spelling in `/etc/init.conf` (`restart=<name>`).
    pub const fn name(self) -> &'static str {
        match self {
            Restart::Always => "always",
            Restart::OnFailure => "on-failure",
            Restart::Never => "never",
        }
    }

    /// Parse a policy name exactly as [`Restart::name`] spells it
    /// (case-sensitive; anything else is refused).
    pub fn parse(name: &str) -> Option<Restart> {
        Restart::ALL.into_iter().find(|p| p.name() == name)
    }
}

/// True for the long-running (daemon) policy: its clean exit is a failure.
pub const fn long_running(policy: Restart) -> bool {
    matches!(policy, Restart::Always)
}

/// Next state of a service that just terminated.
///
/// `clean_exit` is true only for exit code 0; a nonzero exit, a fault, or a
/// kill is a failure. `restarts_so_far` counts the restarts already
/// performed for this service.
pub fn decide(policy: Restart, clean_exit: bool, restarts_so_far: u32) -> ServiceState {
    match policy {
        // A daemon's clean exit is a failure (V0.8 rule); from there the
        // bounded restart policy applies unchanged.
        Restart::Always => service::on_exit(false, restarts_so_far),
        Restart::OnFailure => service::on_exit(clean_exit, restarts_so_far),
        Restart::Never => {
            if clean_exit {
                ServiceState::Done
            } else {
                ServiceState::Failed {
                    restarts: restarts_so_far,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::RESTART_LIMIT;

    /// Drive a service that terminates the same way every time until the
    /// policy stops restarting it; return every state it passed through.
    fn run(policy: Restart, clean_exit: bool) -> Vec<ServiceState> {
        let mut states = Vec::new();
        let mut restarts = 0u32;
        loop {
            let next = decide(policy, clean_exit, restarts);
            states.push(next);
            match next {
                ServiceState::Restarting { restarts: r } => restarts = r,
                _ => return states,
            }
            assert!(states.len() <= 10, "policy never converged");
        }
    }

    #[test]
    fn always_restarts_even_a_clean_exit_up_to_the_ceiling() {
        let expected = vec![
            ServiceState::Restarting { restarts: 1 },
            ServiceState::Restarting { restarts: 2 },
            ServiceState::Restarting { restarts: 3 },
            ServiceState::Failed { restarts: 3 },
        ];
        assert_eq!(run(Restart::Always, true), expected);
        // A failing daemon follows exactly the same path.
        assert_eq!(run(Restart::Always, false), expected);
        assert_eq!(RESTART_LIMIT, 3);
    }

    #[test]
    fn on_failure_finishes_clean_and_restarts_failures() {
        assert_eq!(decide(Restart::OnFailure, true, 0), ServiceState::Done);
        assert_eq!(
            decide(Restart::OnFailure, false, 0),
            ServiceState::Restarting { restarts: 1 }
        );
        assert_eq!(
            run(Restart::OnFailure, false).last(),
            Some(&ServiceState::Failed { restarts: 3 })
        );
        // A clean exit after some restarts still ends the service as Done.
        assert_eq!(decide(Restart::OnFailure, true, 2), ServiceState::Done);
        // It is exactly the V0.7 on-demand rule.
        for restarts in 0..=RESTART_LIMIT + 1 {
            for clean in [true, false] {
                assert_eq!(
                    decide(Restart::OnFailure, clean, restarts),
                    service::on_exit(clean, restarts)
                );
            }
        }
    }

    #[test]
    fn never_restarts() {
        assert_eq!(
            decide(Restart::Never, false, 0),
            ServiceState::Failed { restarts: 0 }
        );
        assert_eq!(decide(Restart::Never, true, 0), ServiceState::Done);
        assert_eq!(run(Restart::Never, false).len(), 1);
    }

    #[test]
    fn only_always_is_long_running() {
        assert!(long_running(Restart::Always));
        assert!(!long_running(Restart::OnFailure));
        assert!(!long_running(Restart::Never));
        assert_eq!(Restart::default(), Restart::OnFailure);
    }

    #[test]
    fn policy_names_round_trip_and_are_exact() {
        for p in Restart::ALL {
            assert_eq!(Restart::parse(p.name()), Some(p));
        }
        assert_eq!(Restart::parse("on-failure"), Some(Restart::OnFailure));
        for bad in ["", "Always", "on_failure", "onfailure", "never ", "no"] {
            assert_eq!(Restart::parse(bad), None, "{bad:?}");
        }
    }
}
