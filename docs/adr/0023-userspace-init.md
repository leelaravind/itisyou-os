# ADR-0023: A userspace init, started as pid 1

## Status

Accepted for V0.10 (INIT10-001, INIT10-002, INIT10-003). Supersedes the boot
half of ADR-0014: the persistent services are no longer a static table in the
kernel. ADR-0014's on-demand supervisor (`svc`) is unchanged.

## Context

Since V0.8 the kernel itself started `tickd` and `flapd` from a static table
(`services::BACKGROUND`), watched them, and restarted them. That made the
kernel the policy owner for every persistent service: adding one meant
rebuilding the kernel, and the kernel console's supervisor ran only when the
console let it. V0.10's always-on scheduler (ADR-0022) and process tree
(PROC10-002) make it possible for an ordinary Ring 3 process to do that job.

## Decision

1. **`/sbin/init` is a Ring 3 program** (`user/sysinit`, no allocator). It
   reads `/etc/init.conf` with `fs_read`, parses it with the host-tested
   `kernel_core::initconf` (bounded grammar: at most 8 services, names, paths,
   `caps=`, `restart=always|on-failure|never`, `after=`, program arguments),
   starts the services in dependency order, and supervises them with
   `wait_nohang` and `sleep` under `kernel_core::supervise` — the V0.8 restart
   policy (a daemon's clean exit is a failure; at most 3 restarts).
2. **Its authority is fixed by the kernel, not by the file**: spawn, IPC,
   reading files under `/etc`, and reporting services (0x413). A service gets
   exactly the capabilities its line names, delegated from init's live
   handles, so never more than init holds; and it inherits init's `/etc`
   filesystem sandbox.
3. **The kernel's service table is a view fed by init** through `svc_report`
   (syscall 39, SVC10-001). The kernel checks what it can know itself — the
   reported pid is the reporter's own child, the caps printed are its own
   record of that child, rows belong to their creator — and prints the V0.8
   lines with `supervisor=<pid>`. Names and policies are init's claim,
   attributed.
4. **Boot order**: the kernel starts init (pid 1, a child of the console) and
   registers it as the adopter of orphans, enables always-on scheduling, then
   lets slices run until init reports `ready` (at most 5 s) before the console
   starts. Settling first makes the boot deterministic — init is pid 1 and its
   services follow in config order — even though the test harness types every
   command the moment the console is up.
5. **Init is supervised by the kernel**: if it dies, every process below it
   ends with it (the new init starts its services afresh; rows it owned are
   taken over by name), and the kernel starts a new init from the next
   background slice — at most 3 times, then it gives up and orphans are
   reaped automatically again.

## Consequences

- A service is added by editing `/etc/init.conf` (in the initramfs; there is
  no persistent config or reload yet).
- `services-bg-bios` keeps every V0.8 assertion except two pids, which move
  from 1/2 to 2/3 because init is pid 1.
- The selftest image starts no init: its process table stays quiescent.
- Trust: a supervisor could misname a service or misstate its policy; every
  line it causes names it, and the kernel's own facts (pid, parentage, caps)
  are never taken from the report.
