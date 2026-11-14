# Local Linux run of the CI workflow — 2026-11-14

**This is not a GitHub Actions run.** It is the `run:` steps of
`.github/workflows/ci.yml`, executed unchanged and in order by
`scripts/run-ci-local.py` on one Linux machine. It changes the state of no
requirement: V1-REL-001, V1-REL-002 and V1-REP-004 need a GitHub CI run and
stay BLOCKED.

It is, however, the first time every step of the V1.0 workflow has been
executed on Linux at all — v1.0.0 was released on the Windows gate alone —
and it found three legs that fail there. Two were then fixed on the branch
and the whole job was run again; one is left for the owner.

## Environment

| | |
|---|---|
| Host | a cloud container: Ubuntu 24.04.5 LTS, kernel 6.18, 4 CPUs, 15 GiB RAM, no KVM (TCG only) |
| Rust | rustc 1.99.0-nightly (ad3d0bc14 2026-07-31), i.e. the pinned `nightly-2026-08-01` |
| QEMU | 8.2.2 (`qemu-system-x86` 1:8.2.2+ds-0ubuntu1.18) — the project develops against 11.1.0 on Windows |
| OVMF | `ovmf` 2024.02-2ubuntu0.10, `/usr/share/OVMF/OVMF_CODE_4M.fd` |
| Node | v22.23.4 |
| Trees | **Run 1** — the v1.0.0 code: the tip of `v1.0/ipc` (`bcfd83b`, one documentation commit after the `v1.0.0` tag `702119e`) plus documentation and `scripts/run-ci-local.py` only (kernel job at `8c9cf07`, reproducibility at `bd379de`). **Run 2** — the same plus the two fixes below (`555da18`). |

## Results

| Job / step | Run 1 (v1.0.0 code) | Run 2 (with the two fixes) |
|---|---|---|
| CI parity (`check-ci-parity.py`) | `CI-PARITY: OK 74 legs` | the same |
| Unsafe inventory | `OK 196 sites in 29 files, 49 rows` | the same |
| `cargo fmt --all -- --check` | pass | pass |
| Clippy, host packages, `-D warnings` | pass | pass |
| Clippy, kernel, `-D warnings` | pass | pass |
| Host unit tests (`kernel-core`, `image-builder`, `qemu-runner`) | 555 passed, 0 failed, 48 ignored | 556 passed, 0 failed, 48 ignored |
| Build bootable images | pass (8 images) | pass (8 images) |
| QEMU matrix, 58 workflow steps | **55 passed, 3 failed** | **57 passed, 1 failed** |
| `website` job (`npm ci`, `npm run verify`) | pass: consistency check, `astro check`, 22 pages built | pass (re-run after the documentation changes) |
| `reproducibility` job (two clean builds from two fresh checkouts) | identical digests for all 8 images | identical digests for all 8 images |
| `secret-scan` job (Gitleaks action) | not run — the action is not available outside GitHub. The patterns of `scripts/secret-scan.ps1` were run over every tracked text file before each push instead: no match. | the same |

### The three legs that failed in run 1

| Step | Label | Why it failed | Now |
|---|---|---|---|
| 46 | `fault-contain-uefi` | the workflow step passed `--uefi` without `--firmware`; the runner stopped before QEMU started (`scripts/test.ps1` gets the path from `OVMF_CODE`, which the workflow does not set) | **fixed**: one line in the workflow, no assertion changed, parity still OK; passes |
| 65 | `ai-retry-bios` | `rollback_kill_refused ... not_an_instance` for an instance that had already exited by itself: a race in the ACT11-002 rollback. 3 failures in 5 further isolated runs on the idle host | **fixed** in the kernel (`kernel_core::initctl::rollback_kill`, host-tested): 8 passes in 8 isolated runs, six through `rollback_instance_gone` and two through the kill; passes in run 2 |
| 45 | `fault-contain-bios` | `int1` (opcode 0xF1) raised `#UD`, not `#DB`: QEMU 8.2.2 compiles the instruction out. The program was contained | **still failing** — emulator version, the kernel is correct; needs the owner (`docs/proposed/README.md`) |

Leg 45 will fail in the same way on GitHub's `ubuntu-24.04` runner, which
installs the same QEMU package and runs the same step. **A green GitHub CI
run is therefore not possible until it is decided**, whatever happens to
billing.

The `ai-retry-bios` fix keeps every refusal it could not rule out: a live
process that is not an instance, an ended process that is not an instance,
and a pid that is not in the table are all still refused and audited as
denials. Only a process the kernel can confirm as an instance — still in the
table, a child of the live init, running the service's program — and that
has already ended is reported as gone, with its own audit record.

`net-ipv6-responder-bios`, the other leg the V1.0 checkpoint recorded as
flaky, passed in both runs (twice is not evidence it is stable).

### Linux-built image digests

Two independent clean builds agree on every digest, for each tree. None of
these is the published v1.0.0 image, which was built on Windows and differs
(path separators in panic locations), and none is a CI build.

Run 1, the v1.0.0 code:

```
itisyou-kernel-bios.img            6784000  191af423903dfd6150ae1cedff3d105c7f3880a14fac5c97e09fa5afcc0b2aba
itisyou-kernel-uefi.img            6356992  b8ceb7485dadc9213f7021a8a4367cdc5368b1397805e2fe38d991e2f66974a3
itisyou-kernel-selftest-bios.img   6784000  b5c0ecbfceed2bc24efebcae23e620320bf1c9059ddd4aab896516311d146728
itisyou-kernel-selftest-uefi.img   6356992  067fcffaa1dfa9730941da1c9f86560ba58daf8991600d187d3da61e1f7249c4
itisyou-kernel-panictest-bios.img  2589696  1ec753f6883ead67599ed1cc267140ecd730bf380c8ee9bb800d97e8c68a4908
itisyou-kernel-panictest-uefi.img  2162688  07aa573a434c886786f722b720d2de178cd3c6d45df1ef0eed2ab6bf36ae0f29
itisyou-fs-persist-bios.img        5735424  4b8c13fb050fe47b58e5f3dc97fc75c5fd38bb67f927dce24ea1d4d15ea4d3aa
itisyou-fs-persist-uefi.img        5308416  6730275d297c4709c64ddc5eead83b16db92d27bce2b3aae340937f3b4df20b3
```

Run 2, with the kernel fix — only the two interactive-kernel images change:

```
itisyou-kernel-bios.img            6784000  0ceeff44726f65c97ec0a7f2ce2a46938a371b91ca110d3545d0a121376b966a
itisyou-kernel-uefi.img            6356992  980c1920d38f1126c4d7216e9f47dedb01ca06aa762fee599dede44d42824e95
```

## What was not done

- No GitHub Actions run, so no CI run id and no CI-built digest to stamp.
- The Windows gate (`scripts/verify.ps1`, `scripts/test.ps1`,
  `release-boot-test.ps1`) was not run; PowerShell is not on this host.
- The published v1.0.0 images were not downloaded or boot-tested.
- Each full matrix was run once; only `ai-retry-bios` was repeated.
- The recorded mutation controls for ACT11-002 (a compromised init naming
  another process, and the others in its requirement row) were not re-run
  against the kernel fix: their scripts are not in the repository. The
  host test covers the decision for those inputs; the kernel path was
  exercised only by `ai-retry-bios`.
- `fault-contain-bios` was not tried on a newer QEMU, so "it passes on QEMU
  10 or later" is read from QEMU's source, not observed.
- The version was not changed: the branch's kernel differs from v1.0.0 and
  still reports `itisyou-os 1.0.0`.
- Nothing was deployed, tagged, merged or pushed to `main`.

## Reproduce

```sh
sudo apt-get install -y --no-install-recommends qemu-system-x86 ovmf python3-yaml
python3 scripts/run-ci-local.py                 # kernel job
python3 scripts/run-ci-local.py --job website
```
