# Local Linux run of the CI workflow — 2026-11-14

**This is not a GitHub Actions run.** It is the `run:` steps of
`.github/workflows/ci.yml`, executed unchanged and in order by
`scripts/run-ci-local.py` on one Linux machine. It changes the state of no
requirement: V1-REL-001, V1-REL-002 and V1-REP-004 need a GitHub CI run and
stay BLOCKED.

It is, however, the first time every step of the V1.0 workflow has been
executed on Linux at all — v1.0.0 was released on the Windows gate alone —
and it found three legs that fail there.

## Environment

| | |
|---|---|
| Host | a cloud container: Ubuntu 24.04.5 LTS, kernel 6.18, 4 CPUs, 15 GiB RAM, no KVM (TCG only) |
| Rust | rustc 1.99.0-nightly (ad3d0bc14 2026-07-31), i.e. the pinned `nightly-2026-08-01` |
| QEMU | 8.2.2 (`qemu-system-x86` 1:8.2.2+ds-0ubuntu1.18) — the project develops against 11.1.0 on Windows |
| OVMF | `ovmf` 2024.02-2ubuntu0.10, `/usr/share/OVMF/OVMF_CODE_4M.fd` |
| Node | v22.23.4 |
| Tree | branch `claude/gifted-cori-vzs71o`: the tip of `v1.0/ipc` (`bcfd83b`, one documentation commit after the `v1.0.0` tag `702119e`) plus documentation and `scripts/run-ci-local.py`. No kernel, user-program, harness or workflow file differs from `v1.0/ipc`. Kernel job run at `8c9cf07`, reproducibility at `bd379de`. |

## Results

| Job / step | Result |
|---|---|
| CI parity (`check-ci-parity.py`) | `CI-PARITY: OK 74 legs` |
| Unsafe inventory | `UNSAFE-INVENTORY: OK 196 sites in 29 files, 49 rows` |
| `cargo fmt --all -- --check` | pass |
| Clippy, host packages, `-D warnings` | pass |
| Clippy, kernel, `-D warnings` | pass |
| Host unit tests (`kernel-core`, `image-builder`, `qemu-runner`) | 555 passed, 0 failed, 48 ignored |
| Build bootable images | pass (8 images) |
| QEMU matrix, 58 workflow steps | **55 passed, 3 failed** (below) |
| `website` job (`npm ci`, `npm run verify`) | pass: consistency check, `astro check`, 22 pages built |
| `reproducibility` job (two clean builds from two fresh checkouts) | **identical digests for all 8 images** |
| `secret-scan` job (Gitleaks action) | not run — the action is not available outside GitHub. The patterns of `scripts/secret-scan.ps1` were run over every tracked text file instead: no match. |

### The three failing legs

Root causes and proposed fixes: `docs/proposed/README.md`. None is applied.

| Step | Label | Why it failed | Kind |
|---|---|---|---|
| 45 | `fault-contain-bios` | `int1` (opcode 0xF1) raised `#UD`, not `#DB`: QEMU 8.2.2 compiles the instruction out. The program was contained. | emulator version; the kernel is correct |
| 46 | `fault-contain-uefi` | the workflow step passes `--uefi` without `--firmware`; the runner stops before QEMU starts | workflow defect |
| 65 | `ai-retry-bios` | `rollback_kill_refused ... not_an_instance` for an instance that had already exited: a race in the ACT11-002 rollback. 3 failures in 5 further isolated runs on the idle host | kernel defect (reporting), present in v1.0.0 |

Legs 45 and 46 will fail in the same way on GitHub's `ubuntu-24.04` runner,
which installs the same QEMU package and runs the same step. Leg 65 will fail
there about every other run. **A green GitHub CI run of v1.0 is therefore not
possible until these are dealt with**, whatever happens to billing.

`net-ipv6-responder-bios`, the other leg the V1.0 checkpoint recorded as
flaky, passed in this run (once; that is not evidence it is stable).

### Linux-built image digests

These are the bytes this host builds from the tree above; two independent
clean builds agree on every one. They are **not** the published v1.0.0
images, which were built on Windows and differ (path separators in panic
locations), and they are not a CI build.

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

## What was not done

- No GitHub Actions run, so no CI run id and no CI-built digest to stamp.
- The Windows gate (`scripts/verify.ps1`, `scripts/test.ps1`,
  `release-boot-test.ps1`) was not run; PowerShell is not on this host.
- The published v1.0.0 images were not downloaded or boot-tested.
- The matrix was run once in full; only `ai-retry-bios` was repeated.
- The proposed fixes were tried (legs 46 and 65 pass with them) and then
  removed from the tree; the mutation controls for ACT11-002 were not run.
- `fault-contain-bios` was not tried on a newer QEMU.
- Nothing was deployed, tagged, merged or pushed to `main`.

## Reproduce

```sh
sudo apt-get install -y --no-install-recommends qemu-system-x86 ovmf python3-yaml
python3 scripts/run-ci-local.py                 # kernel job
python3 scripts/run-ci-local.py --job website
```
