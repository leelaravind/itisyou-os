//! The proposal policy (V0.11, ACT11-001, ADR-0024): what an agent may
//! propose, when an action applies, how risky and reversible it is, what the
//! operator is shown, and the one-way lifecycle a proposal goes through.
//!
//! Pure and host-tested; the kernel feeds it facts it gathered itself and
//! acts only on its verdicts. Nothing here executes anything, and no path
//! leads from a proposal to an action except the console's `approve`.
//!
//! The proposal record an agent submits (`propose`, syscall 43), 88 bytes:
//!
//! ```text
//! offset size field
//!      0    1 version (1)
//!      1    1 action (1 resume-scheduler, 2 retry-service)
//!      2    1 condition (0 service_failed, 1 scheduler_paused, 2 denial_burst)
//!      3    1 runbook id (must equal the condition: one entry per condition)
//!      4   16 target service name, NUL-padded ([a-z0-9-]{1,15}; empty for
//!             resume-scheduler)
//!     20   32 SHA-256 of the model the diagnosis came from
//!     52   32 SHA-256 of the view it was computed on
//!     84    4 reserved, zero
//! ```

use crate::scenario::Condition;

pub const RECORD_LEN: usize = 88;
pub const VERSION: u8 = 1;
pub const NAME_LEN: usize = 16;

/// A proposal expires this many ticks after it was filed (60 s at 100 Hz).
pub const TTL_TICKS: u64 = 6000;
/// A proposal must cite a view served to its submitter at most this many
/// ticks earlier (5 s).
pub const VIEW_MAX_AGE_TICKS: u64 = 500;
/// The kernel's proposal table.
pub const MAX_PROPOSALS: usize = 8;

/// The actions an agent may propose. There are two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ResumeScheduler = 1,
    RetryService = 2,
}

impl Action {
    pub const fn from_u8(v: u8) -> Option<Action> {
        match v {
            1 => Some(Action::ResumeScheduler),
            2 => Some(Action::RetryService),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Action::ResumeScheduler => "resume-scheduler",
            Action::RetryService => "retry-service",
        }
    }

    pub const fn risk(self) -> &'static str {
        match self {
            Action::ResumeScheduler | Action::RetryService => "low",
        }
    }

    pub const fn reversible(self) -> bool {
        true
    }

    /// Whether the action names a service.
    pub const fn has_target(self) -> bool {
        matches!(self, Action::RetryService)
    }

    /// What rollback does, for the preview.
    pub const fn rollback(self) -> &'static str {
        match self {
            Action::ResumeScheduler => "pause the scheduler again",
            Action::RetryService => "init stops the retried instance; the row returns to failed",
        }
    }

    /// How long verification watches after execution, in ticks.
    pub const fn verify_ticks(self) -> u64 {
        match self {
            Action::ResumeScheduler => 100,
            Action::RetryService => 300,
        }
    }
}

/// The only action each condition may propose (a closed table): a diagnosis
/// that the scheduler is paused cannot ask to retry a service.
pub const fn allowed_action(c: Condition) -> Option<Action> {
    match c {
        Condition::ServiceFailed => Some(Action::RetryService),
        Condition::SchedulerPaused => Some(Action::ResumeScheduler),
        Condition::DenialBurst => None,
    }
}

pub fn condition_from_u8(v: u8) -> Option<Condition> {
    Condition::ALL.get(usize::from(v)).copied()
}

/// A decoded proposal record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub action: Action,
    pub condition: Condition,
    pub target: [u8; NAME_LEN],
    pub model: [u8; 32],
    pub view: [u8; 32],
}

impl Record {
    /// The target name (empty when the action has none).
    pub fn target_str(&self) -> &str {
        let end = self.target.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.target[..end]).unwrap_or("")
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0] = VERSION;
        b[1] = self.action as u8;
        b[2] = self.condition as u8;
        b[3] = self.condition as u8;
        b[4..20].copy_from_slice(&self.target);
        b[20..52].copy_from_slice(&self.model);
        b[52..84].copy_from_slice(&self.view);
        b
    }
}

/// Why a proposal was refused. Each has a name the kernel prints and audits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    BadLength,
    BadVersion,
    UnknownAction,
    UnknownCondition,
    /// The runbook id is not the condition's own entry.
    UnknownRunbook,
    /// A target name that is malformed, present where none belongs, or
    /// missing where one is required.
    BadField,
    BadReserved,
    /// The cited condition may not propose this action.
    ActionNotAllowed,
    /// The model digest is not the model the kernel was built with.
    UnknownModel,
    /// The cited view is not the last one served to this process.
    SnapshotMismatch,
    /// That view was served too long ago.
    SnapshotStale,
    /// Recomputing the condition from the cited view with the shipped model
    /// does not give the cited condition.
    DiagnosisMismatch,
    /// The action does not apply to the system as it is now.
    NotApplicable,
    /// The submitter already has a proposal pending.
    AlreadyPending,
    TableFull,
}

impl Refusal {
    pub const fn name(self) -> &'static str {
        match self {
            Refusal::BadLength => "bad_length",
            Refusal::BadVersion => "bad_version",
            Refusal::UnknownAction => "unknown_action",
            Refusal::UnknownCondition => "unknown_condition",
            Refusal::UnknownRunbook => "unknown_runbook",
            Refusal::BadField => "bad_field",
            Refusal::BadReserved => "bad_reserved",
            Refusal::ActionNotAllowed => "action_not_allowed",
            Refusal::UnknownModel => "unknown_model",
            Refusal::SnapshotMismatch => "snapshot_mismatch",
            Refusal::SnapshotStale => "snapshot_stale",
            Refusal::DiagnosisMismatch => "diagnosis_mismatch",
            Refusal::NotApplicable => "not_applicable",
            Refusal::AlreadyPending => "already_pending",
            Refusal::TableFull => "table_full",
        }
    }
}

fn valid_name(b: &[u8]) -> bool {
    !b.is_empty()
        && b.len() < NAME_LEN
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Decode a submitted record, checking only what the record alone can show:
/// format, vocabulary and the condition -> action table.
pub fn decode(b: &[u8]) -> Result<Record, Refusal> {
    if b.len() != RECORD_LEN {
        return Err(Refusal::BadLength);
    }
    if b[0] != VERSION {
        return Err(Refusal::BadVersion);
    }
    let action = Action::from_u8(b[1]).ok_or(Refusal::UnknownAction)?;
    let condition = condition_from_u8(b[2]).ok_or(Refusal::UnknownCondition)?;
    if b[3] != b[2] {
        return Err(Refusal::UnknownRunbook);
    }
    let name = &b[4..20];
    let end = name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
    let padded = name[end..].iter().all(|&c| c == 0);
    let ok_target = if action.has_target() {
        end < NAME_LEN && padded && valid_name(&name[..end])
    } else {
        name.iter().all(|&c| c == 0)
    };
    if !ok_target {
        return Err(Refusal::BadField);
    }
    if b[84..88].iter().any(|&c| c != 0) {
        return Err(Refusal::BadReserved);
    }
    if allowed_action(condition) != Some(action) {
        return Err(Refusal::ActionNotAllowed);
    }
    let mut target = [0u8; NAME_LEN];
    target.copy_from_slice(name);
    let mut model = [0u8; 32];
    model.copy_from_slice(&b[20..52]);
    let mut view = [0u8; 32];
    view.copy_from_slice(&b[52..84]);
    Ok(Record {
        action,
        condition,
        target,
        model,
        view,
    })
}

/// A service as the kernel knows it now, for applicability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceFacts {
    pub failed: bool,
    /// The row is owned by the live `/sbin/init`.
    pub owned_by_init: bool,
    /// The name is one `/etc/init.conf` defines.
    pub in_init_conf: bool,
}

/// What the kernel knows about the system now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub always_on: bool,
    pub paused: bool,
    /// The proposal's target service, if it names one and the row exists.
    pub target: Option<ServiceFacts>,
}

/// Does `action` apply to the system as `facts` describe it? Checked when a
/// proposal is filed AND again when it is approved (the system may have
/// changed in between).
pub fn applies(action: Action, facts: &Facts) -> bool {
    match action {
        Action::ResumeScheduler => facts.always_on && facts.paused,
        Action::RetryService => facts
            .target
            .is_some_and(|s| s.failed && s.owned_by_init && s.in_init_conf),
    }
}

/// A proposal's lifecycle. One way only: nothing ever returns to Pending, a
/// decided proposal is never decided again, and only an Approved one runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Approved,
    Denied,
    Expired,
    /// Executed; verification is watching.
    Executed,
    Verified,
    RolledBack,
}

impl State {
    pub const fn name(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Approved => "approved",
            State::Denied => "denied",
            State::Expired => "expired",
            State::Executed => "executed",
            State::Verified => "verified",
            State::RolledBack => "rolled_back",
        }
    }

    pub const fn is_final(self) -> bool {
        matches!(
            self,
            State::Denied | State::Expired | State::Verified | State::RolledBack
        )
    }
}

/// What can happen to a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Approve,
    Deny,
    Expire,
    Execute,
    Pass,
    Fail,
}

/// The transition, or `None` when the event is not allowed in this state.
pub const fn next(s: State, e: Event) -> Option<State> {
    match (s, e) {
        (State::Pending, Event::Approve) => Some(State::Approved),
        (State::Pending, Event::Deny) => Some(State::Denied),
        (State::Pending, Event::Expire) => Some(State::Expired),
        (State::Approved, Event::Execute) => Some(State::Executed),
        (State::Executed, Event::Pass) => Some(State::Verified),
        (State::Executed, Event::Fail) => Some(State::RolledBack),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(action: Action, condition: Condition, target: &str) -> Record {
        let mut t = [0u8; NAME_LEN];
        t[..target.len()].copy_from_slice(target.as_bytes());
        Record {
            action,
            condition,
            target: t,
            model: [1; 32],
            view: [2; 32],
        }
    }

    #[test]
    fn records_round_trip() {
        for r in [
            rec(Action::ResumeScheduler, Condition::SchedulerPaused, ""),
            rec(Action::RetryService, Condition::ServiceFailed, "flapd"),
            rec(
                Action::RetryService,
                Condition::ServiceFailed,
                &"a".repeat(15),
            ),
        ] {
            assert_eq!(decode(&r.encode()), Ok(r));
        }
        assert_eq!(
            rec(Action::RetryService, Condition::ServiceFailed, "flapd").target_str(),
            "flapd"
        );
    }

    #[test]
    fn every_malformed_record_is_refused_by_name() {
        let good = rec(Action::RetryService, Condition::ServiceFailed, "flapd").encode();
        let bad = |f: &dyn Fn(&mut [u8; RECORD_LEN])| {
            let mut b = good;
            f(&mut b);
            decode(&b)
        };
        assert_eq!(decode(&good[..87]), Err(Refusal::BadLength));
        assert_eq!(bad(&|b| b[0] = 2), Err(Refusal::BadVersion));
        assert_eq!(bad(&|b| b[1] = 0), Err(Refusal::UnknownAction));
        assert_eq!(bad(&|b| b[1] = 3), Err(Refusal::UnknownAction));
        assert_eq!(bad(&|b| b[2] = 3), Err(Refusal::UnknownCondition));
        assert_eq!(bad(&|b| b[3] = 1), Err(Refusal::UnknownRunbook));
        assert_eq!(bad(&|b| b[4] = b'F'), Err(Refusal::BadField));
        assert_eq!(bad(&|b| b[4] = 0), Err(Refusal::BadField));
        assert_eq!(bad(&|b| b[4..20].fill(b'a')), Err(Refusal::BadField));
        assert_eq!(bad(&|b| b[12] = b'x'), Err(Refusal::BadField));
        // A marker smuggled into a field never gets past the decoder.
        assert_eq!(
            bad(&|b| b[4..11].copy_from_slice(b"[ITISYO")),
            Err(Refusal::BadField)
        );
        assert_eq!(bad(&|b| b[86] = 1), Err(Refusal::BadReserved));
        // resume-scheduler takes no target.
        let r = rec(Action::ResumeScheduler, Condition::SchedulerPaused, "").encode();
        let mut b = r;
        b[4] = b'a';
        assert_eq!(decode(&b), Err(Refusal::BadField));
        // retry-service needs one.
        let mut b = good;
        b[4..20].fill(0);
        assert_eq!(decode(&b), Err(Refusal::BadField));
    }

    #[test]
    fn a_condition_may_propose_only_its_own_action() {
        let paused_retry = rec(Action::RetryService, Condition::SchedulerPaused, "flapd");
        let mut b = paused_retry.encode();
        b[3] = b[2];
        assert_eq!(decode(&b), Err(Refusal::ActionNotAllowed));
        let failed_resume = rec(Action::ResumeScheduler, Condition::ServiceFailed, "");
        assert_eq!(
            decode(&failed_resume.encode()),
            Err(Refusal::ActionNotAllowed)
        );
        for a in [Action::ResumeScheduler, Action::RetryService] {
            let r = rec(
                a,
                Condition::DenialBurst,
                if a.has_target() { "x" } else { "" },
            );
            assert_eq!(decode(&r.encode()), Err(Refusal::ActionNotAllowed));
        }
    }

    #[test]
    fn applicability_is_exact() {
        let svc = ServiceFacts {
            failed: true,
            owned_by_init: true,
            in_init_conf: true,
        };
        let f = Facts {
            always_on: true,
            paused: true,
            target: Some(svc),
        };
        assert!(applies(Action::ResumeScheduler, &f));
        assert!(!applies(
            Action::ResumeScheduler,
            &Facts { paused: false, ..f }
        ));
        assert!(!applies(
            Action::ResumeScheduler,
            &Facts {
                always_on: false,
                ..f
            }
        ));
        assert!(applies(Action::RetryService, &f));
        for s in [
            ServiceFacts {
                failed: false,
                ..svc
            },
            ServiceFacts {
                owned_by_init: false,
                ..svc
            },
            ServiceFacts {
                in_init_conf: false,
                ..svc
            },
        ] {
            assert!(!applies(
                Action::RetryService,
                &Facts {
                    target: Some(s),
                    ..f
                }
            ));
        }
        assert!(!applies(Action::RetryService, &Facts { target: None, ..f }));
    }

    #[test]
    fn the_lifecycle_only_goes_one_way() {
        use Event::*;
        use State::*;
        let states = [
            Pending, Approved, Denied, Expired, Executed, Verified, RolledBack,
        ];
        let events = [Approve, Deny, Expire, Execute, Pass, Fail];
        let legal = [
            (Pending, Approve, Approved),
            (Pending, Deny, Denied),
            (Pending, Expire, Expired),
            (Approved, Execute, Executed),
            (Executed, Pass, Verified),
            (Executed, Fail, RolledBack),
        ];
        for s in states {
            for e in events {
                let want = legal
                    .iter()
                    .find(|(a, b, _)| *a == s && *b == e)
                    .map(|t| t.2);
                assert_eq!(next(s, e), want, "{s:?} + {e:?}");
            }
            // Nothing leaves a final state, and nothing returns to Pending.
            if s.is_final() {
                assert!(events.iter().all(|e| next(s, *e).is_none()));
            }
        }
        assert!(states
            .iter()
            .all(|s| events.iter().all(|e| next(*s, *e) != Some(Pending))));
        // Denied can never become Approved, the case the console must refuse.
        assert_eq!(next(Denied, Approve), None);
    }

    #[test]
    fn refusal_names_are_distinct() {
        let all = [
            Refusal::BadLength,
            Refusal::BadVersion,
            Refusal::UnknownAction,
            Refusal::UnknownCondition,
            Refusal::UnknownRunbook,
            Refusal::BadField,
            Refusal::BadReserved,
            Refusal::ActionNotAllowed,
            Refusal::UnknownModel,
            Refusal::SnapshotMismatch,
            Refusal::SnapshotStale,
            Refusal::DiagnosisMismatch,
            Refusal::NotApplicable,
            Refusal::AlreadyPending,
            Refusal::TableFull,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.name(), b.name());
            }
        }
    }
}
