# ADR-0024: The AI-native system layer — intelligence without authority

## Status

Accepted, and implemented in V0.11 (in development — in no release yet; the
latest release is v0.10.0). Steps S1–S11 have landed on the V0.11 branch in
the order below, each gated by the whole local matrix. The rows are
IMPLEMENTED+VERIFIED in `docs/REQUIREMENTS.md` with QEMU evidence:
MODEL11-001, VIEW11-001, INFER11-001, KB11-001, AGENT11-001, PROP11-001,
ACT11-001, ACT11-002, and OUT11-001 (the Ring 3 escaping of Decision 8);
every row records a negative control. They
rest on the audit-trail fixes AUDIT11-001, AUDIT11-002 and SEC11-001. The
design went through two independent reviews before any code — a security
review and a feasibility review against the V0.10 code — and this record is
the result; what they changed is listed under "What the reviews changed",
and where the code departs from the text, under "Implementation notes".

## Context

The roadmap asks for "a local inference service outside the kernel (a small
model running in Ring 3, trained reproducibly from the repository), system
knowledge over an approved, read-only view of local state, a diagnostic
agent, policy-controlled system actions with preview and explicit approval,
provenance and audit, and post-action verification with rollback", under an
unchanged invariant: **the agent has intelligence, not authority**. The
implementation plan's §5.2 pipeline is binding: agent intent → policy →
risk/permission evaluation → user approval → deterministic privileged
service → kernel operation → verification → audit → rollback.

What V0.10 gives to build on: a non-preemptible kernel with bounded
background slices (ADR-0022), a process tree with `/sbin/init` as pid 1
(ADR-0023), capability handles revalidated on every syscall (ADR-0013), a
hash-chained audit trail (made trustworthy for this in V0.11 first:
AUDIT11-001/002, SEC11-001), line-atomic Ring 3 output that cannot print a
kernel marker, and QEMU legs that assert on serial lines.

## Decision

1. **The model is a linear classifier, and it is called that.** Three
   independent yes/no detectors — `service_failed`, `scheduler_paused`,
   `denial_burst` — each 16 integer weights and a bias over 16 integer
   features, trained by an averaged perceptron in integer arithmetic
   (`kernel_core::model`, no floating point anywhere: the kernel target is
   soft-float with no FPU state saved, and integer training is deterministic
   by construction). "Healthy" is no detector firing. Several can fire at
   once, because a real system has several conditions at once (flapd is
   Failed on every boot). The training data is **synthetic**: examples
   generated with a fixed-seed splitmix64 from hand-written scenario ranges
   in `ai/scenarios.txt`; the model is only as good as those assumptions,
   and the docs say so. Reported: held-out accuracy on synthetic examples
   from the same generator (a different seed) next to a one-rule baseline
   and a confusion count — a measure of how separable the designed
   distributions are, not of real diagnostic accuracy — and, separately, the
   situations the QEMU legs construct and whether each produced the expected
   conditions.
2. **Trained at build time, pinned.** `kernel/build.rs` trains the model
   with the same host-tested code, writes `/etc/ai/diag.model` into the
   initramfs and compiles its SHA-256 into the kernel. `ai/diag.model.sha256`
   pins the bytes: a host test and the build both fail on any change, so a
   Windows build and a Linux CI build producing the same pin is the
   cross-host reproducibility proof. The scenario parser strips `\r`, and the
   training digest covers the generated examples, not the file's bytes.
3. **The approved view** is syscall 42 `sys_view`: a fixed, versioned binary
   snapshot of counts and allow-listed rows (process counts by state, service
   rows — validated names, state, restarts — scheduler state, audit and
   network counters, uptime); no paths, arguments, memory or payloads; the
   process part is capped with a `truncated` flag. Gated by `CAP_SYS_VIEW`
   (bit 11 → SystemAdministration READ). The kernel remembers, per process,
   the digest and tick of the last view it served it, and forgets it when
   the process ends.
4. **Inference runs in Ring 3.** `/bin/inferd` is a service in
   `/etc/init.conf` (appended third, so tickd and flapd keep pids 2 and 3):
   it loads the model, refuses a hostile one with a named reason, and answers
   requests over IPC. IPC grows from 4 to 8 channels; a request carries a
   nonce the reply must echo, and the agent's wait is bounded. IPC is still
   unauthenticated, so an answer from inferd is a claim — see 7.
5. **Knowledge is a fixed map, not retrieval.** Each condition has one
   runbook entry, `/etc/ai/kb/<condition>.txt` (meaning, likely causes, the
   action it may propose, risk, reversibility); the agent cites the entry id
   and its SHA-256, and the kernel accepts only ids from the closed table.
6. **The agent is a deterministic Ring 3 program** (`/bin/agent`): view →
   features → inferd → runbook → print the diagnosis → file at most one
   proposal per actionable condition. It does not plan, generate text or
   learn, and it holds no authority to act: without CONTROL, ADMIN or
   filesystem write, its direct attempts at the syscalls that exist are
   refused, and for the rest ("resume the scheduler", "retry a service")
   there is no syscall at all.
7. **Proposals** are syscall 43 `propose` with a strict binary record
   (version, action, target, condition, model digest, view digest, runbook
   id), gated by `CAP_PROPOSE` (bit 12 → SystemAdministration USE). The
   kernel refuses, each for a named reason: an unknown model (the digest is
   not the compiled-in one), a view it never served this process or served
   more than N ticks ago, an action the cited condition may not propose (a
   fixed condition → action table), a runbook id outside the table, a target
   that does not exist or an action that does not apply to its current
   state, a malformed field, a submitter with a proposal already pending, and
   a full table (8). It then recomputes the cited condition from the cited
   view with the shipped model and refuses a mismatch — so a spoofed inferd
   cannot make it file what the model does not say. The kernel never acts on
   the model's output; the recomputation only checks a claim, and the claim
   grants nothing.
8. **Approval is the console's alone.** `proposals` lists each proposal with
   its preview as kernel `[ITISYOU:AI]` lines (a program cannot print that
   prefix, and since V0.11 its control characters are escaped, so it cannot
   redraw them either); `approve <id>` and `deny <id>` exist only in the
   kernel console, which reads no input while the Ring 3 shell holds it. Ids
   are monotonic per boot and never reused. Every proposal needs approval —
   the policy has no automatic path. At `approve` the kernel re-checks the
   TTL and the precondition (`precondition_changed`), and the lifecycle is
   one-way: pending → approved | denied | expired → executed → verified |
   rolled back.
9. **Two actions, both kernel code.** `resume-scheduler` (reversible; the
   verification is the busy-point measurement — others must progress — and
   rollback pauses again). `retry-service <name>` (only a Failed row owned by
   the live init for a name in `/etc/init.conf`): the kernel posts `retry
   <name>` to init through a one-slot kernel mailbox that only init can read
   (syscall 44 `init_ctl`, refused `not_init` to anyone else); verification
   is event-driven in `svc_report` (a restart or failure of that name inside
   the window fails it); rollback posts `stop <name>`, and the kernel kills
   the instance only after init acknowledges. Stopping arbitrary processes
   and a "runaway process" condition are out: there is no per-process CPU
   accounting and no Ring 3 kill to refuse.
10. **Everything is audited** with its provenance: `proposal_submitted`
    (model and view digests, condition, runbook id), `proposal_approved` /
    `denied` / `expired`, `action_executed`, `action_verified` or
    `action_rolled_back`. Agent-supplied fields are validated
    (`[a-z0-9_-]{1,15}`) before they reach any line, and every echo goes
    through the V0.11 escaping.
11. **Capabilities.** `CAP_SYS_VIEW` and `CAP_PROPOSE` are outside
    `CAP_LEGACY_FULL`, so `run` without a caps list, `rsh`, desktop apps and
    package manifests cannot get them; they are granted only by the console
    and are not delegable to a child.

## Step order

S1 view and scenario modules · S2 the model (trainer, codec, pin) · S3
build-time training and the boot check of the model digest · S4 `sys_view` ·
S5 inferd and 8 IPC channels · S6 the agent, diagnose only · S7 the policy
module (vocabulary, applicability, risk, preview, state machine) · S8
proposals with nothing executable · S9 approval and `resume-scheduler` · S10
the init mailbox and `retry-service` · S11 closure. Each step: host tests, a
QEMU leg in both `scripts/test.ps1` and CI, a negative control, the whole
local gate.

## What the reviews changed

From the security review: a kernel-owned mailbox instead of IPC for
kernel→init control; approval re-checks the precondition and a TTL; the
kernel recomputes the diagnosis; the closed runbook table; one pending
proposal per submitter; the new capability bits outside the legacy set and
not delegable; agent fields validated before they reach a line; Ring 3
control characters escaped; `stop-process` dropped. And, before any of it,
the audit trail itself: AUDIT11-001, AUDIT11-002 and SEC11-001.

From the feasibility review: channels 6 and 7 did not exist (4 channels);
IPC carries no sender, so "the kernel is the sender" was false; the planned
rollback for `retry-service` could not work (init cannot stop anything, a
kernel-restored row would be taken from init, and flapd fails again before a
tick-based window ends), hence the mailbox protocol with acknowledgement and
event-driven verification; a single-label softmax did not fit a system with
several conditions at once, and floating point had no place in a soft-float
kernel target — hence integer yes/no detectors; the verification of
`resume-scheduler` has to use the busy-point measurement, because job slices
run even while paused; inferd is appended to `/etc/init.conf` so existing pid
assertions hold; and the wording of every claim (§ Decision 1, 6, 7).

## Implementation notes

Where the V0.11 code (in development) departs from the decision text above
or makes it concrete, checked against the code and `docs/REQUIREMENTS.md`.
Retrying a service through the init mailbox is not a departure: it is
Decision 9 as written.

- **Decision 3.** The process part of the view is counts only; the
  `truncated` flag marks service rows beyond eight
  (`sysview::MAX_SERVICES`).
- **Decisions 5 and 10.** The proposal record carries the runbook id, which
  must equal the condition (`unknown_runbook` otherwise), and not the
  runbook's SHA-256: the agent cites the id and a digest prefix in its
  printed diagnosis (`AGENT-RUNBOOK id= sha=`), and the audit records name
  the condition. `proposal_submitted` and `action_executed` carry
  16-hex-digit prefixes of the model and view digests; the other records
  name the proposal id. Agent-supplied names are validated as
  `[a-z0-9-]{1,15}` — no underscore, stricter than written.
- **Decision 6.** The agent files one proposal per run, a paused scheduler
  first, not one per actionable condition: the kernel keeps one pending
  proposal per submitter.
- **Decision 7.** N is 500 ticks (5 s); the TTL is 6000 ticks (60 s). The
  checks run in the order format → model digest → view binding → view age
  → recomputation → applicability → table, so a false diagnosis is refused
  as `diagnosis_mismatch` before applicability is looked at (without the
  recomputation it is refused only as `not_applicable` — PROP11-001's
  control). `propose` returns `ERR_INVAL` for a malformed record,
  `ERR_AGAIN` for `already_pending` and `table_full`, and `ERR_PERM` for
  every refusal made on the kernel's own knowledge. Pending proposals
  past their TTL expire whenever a proposal is filed and when the console
  runs `proposals`, `approve` or `deny` (at first only on the console
  commands, so a full table stayed full until the operator acted; fixed in
  S11), and a proposal outlives the process that filed it.
- **Decision 8.** The Ring 3 control-character escaping is its own
  requirement, OUT11-001 (`kernel_core::linebuf::write_escaped`, applied by
  `console_out::emit` after the marker rewrite); since the S11 review it also
  escapes the invisible characters that reorder or hide text. The review
  found that "a program cannot print that prefix" did not hold in V0.10's
  rewrite itself: it ran one output chunk at a time, so a prefix split
  across chunks got through (OUT11-002), and `ps` printed a spawn path a
  program chose (SEC11-002). Both are fixed, so the decision's premise now
  holds as written.
- **Decision 9.** The verification windows are 100 ticks for
  `resume-scheduler` (other processes' quanta must rise and the scheduler
  must not be paused) and 300 ticks for `retry-service`. The kernel waits
  up to 3 s for each acknowledgement from init and withdraws the command if
  none comes. The watch posts `stop` from inside init's own report, so the
  rollback lands before init can restart the service again. The kernel
  kills the pid named in init's `stop` acknowledgement only if it is a live
  child of init running the program `/etc/init.conf` gives the service
  (S11; at first it killed whatever pid init named, which gave init kill
  authority it does not otherwise hold). If init fetched a `retry` and then
  did not acknowledge it in time, the kernel rolls it back rather than
  assume nothing started (S11). A
  fixture, `/bin/flakyd`, was added as init's fourth service so the success
  path of `retry-service` is exercised; tickd, flapd and inferd keep pids
  2–4.
- **Decision 11.** The console grants the two bits through the caps list
  of `run`, `bg` or `rsh`; without a list those commands give
  `CAP_LEGACY_FULL`, which excludes them. A service definition naming one
  is refused (`console_only_capability`), not silently narrowed; a package
  manifest naming one is narrowed by `caps::delegate`, like any request
  beyond the launcher's authority.
- **Step S3.** The kernel keeps a decoded copy of the model for
  recomputation only when the initramfs file matches the compiled-in
  digest; otherwise the boot check prints `initramfs_match=false` and no
  proposal can pass (the recomputation step refuses it `unknown_model`).
  The check is never fatal.
