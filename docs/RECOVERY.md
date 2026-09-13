# Recovery procedures

## Git recovery

- Every meaningful step lands as a checkpoint commit; `git log --oneline`
  shows the progression. Recover any prior state with
  `git checkout <sha> -- <path>` (file-level) or a new branch at `<sha>`.
- Nothing in the workflow force-pushes or rewrites history; `origin`
  (GitHub, private) is the recovery source of truth after each push.
- Lost-work check: `git reflog` retains local HEAD movements.

## Failed build / corrupted artifacts

- All build output is disposable: delete `target/` and rerun
  `scripts\build.ps1`. Images in `target/images/` are regenerated from
  source; checksums in `manifest.txt` verify integrity.
- Toolchain corruption: `rustup toolchain uninstall nightly-2026-08-01`
  then rerun any cargo command (rust-toolchain.toml re-installs pinned
  toolchain + components into `E:\toolchains\rustup`).

## QEMU hangs / stale processes

- `tools/qemu-runner` kills its own QEMU on timeout. If an interactive
  `run-qemu.ps1` session hangs, close the window or
  `Stop-Process -Name qemu-system-x86_64` — only ever affects the
  disposable guest; the images on disk are read-created per build.

## Website deployment rollback

- Deployment topology and rollback commands live in `docs/DEPLOYMENT.md`
  once the first deployment lands. Static-asset deployments are atomic;
  rollback = redeploy the previous commit (`git checkout <sha> -- website`
  → build → deploy) or use Cloudflare's previous-deployment rollback where
  the account exposes it.

## CI diagnosis

- CI mirrors `scripts/verify.ps1` stages; a red job names its stage.
  Reproduce locally with the matching script; artifacts uploaded by CI
  contain the same `*.result.json` + serial logs as local runs.
