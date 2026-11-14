# Local Linux run of the CI workflow — 2026-11-14

**Runs 1 to 3 below are not GitHub Actions runs** (the GitHub runs that
followed them have their own section near the end). It is the `run:` steps of
`.github/workflows/ci.yml`, executed unchanged and in order by
`scripts/run-ci-local.py` on one Linux machine. It changes the state of no
requirement: V1-REL-001, V1-REL-002 and V1-REP-004 need a GitHub CI run and
stay BLOCKED.

It is, however, the first time every step of the V1.0 workflow has been
executed on Linux at all — v1.0.0 was released on the Windows gate alone —
and it found three legs that fail there. Two were fixed on the branch and
the whole job was run again (run 2); the third was put to the owner, who
chose to keep its assertion and change the emulator, and the whole job was
run a third time (run 3): every step passes.

## Environment

| | |
|---|---|
| Host | a cloud container: Ubuntu 24.04.5 LTS, kernel 6.18, 4 CPUs, 15 GiB RAM, no KVM (TCG only) |
| Rust | rustc 1.99.0-nightly (ad3d0bc14 2026-07-31), i.e. the pinned `nightly-2026-08-01` |
| QEMU | Runs 1 and 2: 8.2.2 (`qemu-system-x86` 1:8.2.2+ds-0ubuntu1.18). Run 3: **11.1.0**, built from the release tarball by `scripts/ci-install-qemu.sh` (SHA-256 `6ee1d1a6…a858`, release signature by key `CEACC9E15534EBABB82D3FA03353C9CEF108B584` checked) — the version the project develops against on Windows. Verified from `/proc/<pid>/exe` that the legs ran that binary. |
| OVMF | `ovmf` 2024.02-2ubuntu0.10, `/usr/share/OVMF/OVMF_CODE_4M.fd` |
| Node | v22.23.4 |
| Trees | **Run 1** — the v1.0.0 code: the tip of `v1.0/ipc` (`bcfd83b`, one documentation commit after the `v1.0.0` tag `702119e`) plus documentation and `scripts/run-ci-local.py` only (kernel job at `8c9cf07`, reproducibility at `bd379de`). **Run 2** — the same plus the two fixes below (`555da18`). **Run 3** — run 2 plus the pinned-QEMU install step in the workflow (`e5b48e9`); the kernel and the images are those of run 2. |

## Results

| Job / step | Run 1 (v1.0.0 code, QEMU 8.2.2) | Run 2 (two fixes, QEMU 8.2.2) | Run 3 (two fixes, QEMU 11.1.0) |
|---|---|---|---|
| CI parity (`check-ci-parity.py`) | `CI-PARITY: OK 74 legs` | the same | the same |
| Unsafe inventory | `OK 196 sites in 29 files, 49 rows` | the same | the same |
| `cargo fmt --all -- --check` | pass | pass | pass |
| Clippy, host packages, `-D warnings` | pass | pass | pass |
| Clippy, kernel, `-D warnings` | pass | pass | pass |
| Host unit tests (`kernel-core`, `image-builder`, `qemu-runner`) | 555 passed, 0 failed, 48 ignored | 556 passed, 0 failed, 48 ignored | 556 passed, 0 failed, 48 ignored |
| Build bootable images | pass (8 images) | pass (8 images) | pass (the digests of run 2) |
| QEMU matrix, 58 workflow steps | **55 passed, 3 failed** | **57 passed, 1 failed** | **58 passed, 0 failed** |
| `website` job (`npm ci`, `npm run verify`) | pass: consistency check, `astro check`, 22 pages built | pass | pass (re-run after the last documentation change) |
| `reproducibility` job (two clean builds from two fresh checkouts) | identical digests for all 8 images | identical digests for all 8 images | not re-run: the kernel and builder are unchanged since run 2 and the build gave run 2's digests |
| `secret-scan` job (Gitleaks action) | not run — the action is not available outside GitHub. The patterns of `scripts/secret-scan.ps1` were run over every tracked text file before each push instead: no match. | the same | the same |

### The three legs that failed in run 1

| Step | Label | Why it failed | Now |
|---|---|---|---|
| 46 | `fault-contain-uefi` | the workflow step passed `--uefi` without `--firmware`; the runner stopped before QEMU started (`scripts/test.ps1` gets the path from `OVMF_CODE`, which the workflow does not set) | **fixed**: one line in the workflow, no assertion changed, parity still OK; passes |
| 65 | `ai-retry-bios` | `rollback_kill_refused ... not_an_instance` for an instance that had already exited by itself: a race in the ACT11-002 rollback. 3 failures in 5 further isolated runs on the idle host | **fixed** in the kernel (`kernel_core::initctl::rollback_kill`, host-tested): 8 passes in 8 isolated runs, six through `rollback_instance_gone` and two through the kill; passes in run 2 |
| 45 | `fault-contain-bios` | `int1` (opcode 0xF1) raised `#UD`, not `#DB`: QEMU 8.2.2 compiles the instruction out (behind `WANT_ICEBP` up to 9.0.0; decoded as `INT1` from 9.1.0 — read from QEMU's sources at five release tags). The program was contained; the kernel is correct | **fixed by changing the emulator, by the owner's decision**: the workflow builds the pinned QEMU 11.1.0. The assertion is unchanged, and on that QEMU the leg prints `exception=debug vector=1 cs_rpl=3` and `Fault { vector: 1 }`; passes in run 3 |

Step numbers are those of runs 1 and 2; the workflow has one more step since
(the QEMU cache), so each QEMU step's index is one higher in run 3.

The `ai-retry-bios` fix keeps every refusal it could not rule out: a live
process that is not an instance, an ended process that is not an instance,
and a pid that is not in the table are all still refused and audited as
denials. Only a process the kernel can confirm as an instance — still in the
table, a child of the live init, running the service's program — and that
has already ended is reported as gone, with its own audit record.

The emulator change has a cost: QEMU 8.2.2, which earlier releases' CI ran
on, is no longer exercised by anything. On it the matrix passes 57 of 58.
`docs/V1_ACCEPTANCE.md` V1-ENV-001 still names it as a tested environment
and was not amended.

`net-ipv6-responder-bios`, the other leg the V1.0 checkpoint recorded as
flaky, passed in all three runs (three passes are not evidence it is
stable).

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

## The GitHub Actions runs that followed

After run 3 the owner made the repository public and asked for a pull
request, so the workflow ran on GitHub for the first time since v0.10.0.
This section is the only part of this file that is a GitHub run.

| | |
|---|---|
| Run | `43651248948` — https://github.com/leelaravind/itisyou-os/actions/runs/43651248948 |
| Trigger | pull request #3, `claude/gifted-cori-vzs71o` into `main`, head `d810a32` |
| Kernel build, tests, QEMU boot matrix | success (23:03:12–23:23:05 UTC, including the first, uncached build of QEMU 11.1.0) |
| Reproducible boot images (two clean builds) | success — identical digests for every image |
| Website check + build | success |
| Secret scan (Gitleaks) | success |

The owner merged pull request #3 at 23:34 UTC (merge commit `4e337d2` on
`main`). Two more runs of the same tree followed:

| Run | Trigger | Head | Result |
|---|---|---|---|
| `43651248948` | pull request #3 | `d810a32` | all four jobs success (kernel job 19 min 53 s, QEMU built from scratch) |
| `43653136859` | pull request #3, after a documentation commit | `4a08a99` | all four jobs success (kernel job 15 min 40 s, QEMU restored from the cache) |
| `43653887781` | push to `main`: the merge of pull request #3 | `4e337d2` | all four jobs success (kernel job 24 min 13 s; `main` has its own cache scope, so QEMU was built again) |

Three consecutive green runs. The three heads differ only in documentation
and the merge commit: the kernel, user programs, harness and workflow are
the same in all three.

These are runs on commits that are not the v1.0.0 release commit and carry
a kernel change after it. They therefore do not by themselves
satisfy V1-REL-001 (the matrix on the release commit), V1-REL-002 (three
consecutive clean runs) or V1-REP-004 (the tag on a green-CI commit); those
rows stay BLOCKED until the owner decides which commit and version to stamp.

CI-built digests (the `reproducibility` job's manifest):

```
itisyou-kernel-bios.img            6784000  fdcc8edb087a56df595428c369ee2bb5419361e087a3944488b66ab52f31c48b
itisyou-kernel-uefi.img            6356992  d433c5a9bcfd7c1aa9bd4e9f028bba79f26d205cd42a9d3a3144b7c1b2f435ff
itisyou-kernel-selftest-bios.img   6784000  7cb60889656a3ea8af22fd93704a00157f850bf10d8cd8aff629a4bad164f1f5
itisyou-kernel-selftest-uefi.img   6356992  a079735182fd9fef0e8fc1128f5a07b1b49dc9caa9463f6eb64480fc650fb25f
itisyou-kernel-panictest-bios.img  2589696  c6f4e0c1a87c9e22971b96efefb7afc8ef960733e93a9cfc4913beb007a58ffb
itisyou-kernel-panictest-uefi.img  2162688  0e0068159b6bfb814f2338560b8502ecb9c68bb1e56ee2eab3fdad46df6547f9
itisyou-fs-persist-bios.img        5735424  893cccff652c300ca0756341f8f3979b47c4c1238cf6f15faa658266e686dab3
itisyou-fs-persist-uefi.img        5308416  cb769f123e6048730f829bc613109aff8f49b410cdbe7231ceebfc77156547de
```

**These differ from the digests this host built from the same tree** (run
2/3 above), for every image. Each host reproduces its own bytes — two clean
builds agree on GitHub, and two agree here, from different checkout
directories — but the two hosts do not agree with each other. The cause was
not isolated. The likely one is the mechanism the project already records
for Windows against Linux: absolute paths in panic locations, here the
toolchain and cargo registry under `/root` on this host and `/home/runner`
on GitHub's. So "reproducible" currently means "on the same host layout",
and a published digest can only be re-derived on a host laid out like the
one that built it.

## What was not done

- No requirement row was stamped with the GitHub run: which commit and
  version it should certify is the owner's decision.
- The Windows gate (`scripts/verify.ps1`, `scripts/test.ps1`,
  `release-boot-test.ps1`) was not run; PowerShell is not on this host.
- The published v1.0.0 images were not downloaded or boot-tested.
- Each full matrix was run once; only `ai-retry-bios` was repeated.
- The recorded mutation controls for ACT11-002 (a compromised init naming
  another process, and the others in its requirement row) were not re-run
  against the kernel fix: their scripts are not in the repository. The
  host test covers the decision for those inputs; the kernel path was
  exercised only by `ai-retry-bios`.
- Only QEMU 8.2.2 and 11.1.0 were run. "From 9.1.0" is read from QEMU's
  sources.
- The version was not changed: the branch's kernel differs from v1.0.0 and
  still reports `itisyou-os 1.0.0`.
- Nothing was deployed or tagged. The merge to `main` was the owner's, through
  pull request #3; the agent pushed only to its own branch.

## Reproduce

```sh
# Run as root (as in a container), the workflow's own install step installs
# OVMF and the build dependencies and builds the pinned QEMU. As a normal
# user that step is skipped: follow docs/BUILD_AND_RUN.md, "Linux host".
apt-get install -y --no-install-recommends python3-yaml
python3 scripts/run-ci-local.py                 # kernel job
python3 scripts/run-ci-local.py --job website
```
