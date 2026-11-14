#!/usr/bin/env python3
"""Run a job of .github/workflows/ci.yml on a local Linux machine.

The workflow file stays the single source of truth: this script executes each
step's `run:` block exactly as written (bash -e -o pipefail, the step's
working directory and environment), in order, and records one line per step.
`uses:` steps (checkout, cache, artifact upload) are skipped and listed as
such. It is NOT a GitHub Actions run and its result must never be recorded as
one - it is the same commands on another Linux host.

Usage:
  python3 scripts/run-ci-local.py                    # the whole `kernel` job
  python3 scripts/run-ci-local.py --job website
  python3 scripts/run-ci-local.py --only "TCP stream"   # steps whose name matches
  python3 scripts/run-ci-local.py --from 13 --to 20     # by step index
  python3 scripts/run-ci-local.py --list

Prerequisites for the `kernel` job: rustup (the pinned toolchain installs
itself) and PyYAML. The workflow's own "Install QEMU + OVMF" step installs
the rest (apt packages, and the pinned QEMU through
scripts/ci-install-qemu.sh). It needs root, so it is skipped with
--skip-install (the default when not running as root): then install OVMF and
the build dependencies yourself, run scripts/ci-install-qemu.sh, and put its
bin directory first on PATH. A step that appends to $GITHUB_PATH takes effect
for the following steps of the same invocation only, so a partial run
(--from/--only) that leaves the install step out needs that PATH too.

Exit status: 0 if every executed step passed, 1 otherwise.
"""

import argparse
import os
import pathlib
import subprocess
import sys
import time

try:
    import yaml
except ImportError:
    sys.exit("run-ci-local: PyYAML is required (pip install pyyaml)")

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--job", default="kernel")
    ap.add_argument("--only", action="append", default=[], help="substring of a step name (repeatable)")
    ap.add_argument("--from", dest="first", type=int, default=0)
    ap.add_argument("--to", dest="last", type=int, default=10**6)
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--keep-going", action="store_true", default=True)
    ap.add_argument("--stop-on-failure", dest="keep_going", action="store_false")
    ap.add_argument("--skip-install", action="store_true", default=os.geteuid() != 0)
    ap.add_argument("--log-dir", default=str(ROOT / "target" / "ci-local"))
    args = ap.parse_args()

    workflow = yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))
    jobs = workflow["jobs"]
    if args.job not in jobs:
        sys.exit(f"run-ci-local: no job {args.job!r}; jobs: {', '.join(jobs)}")
    steps = jobs[args.job]["steps"]
    base_env = dict(os.environ)
    base_env.update({k: str(v) for k, v in (workflow.get("env") or {}).items()})
    base_env.update({k: str(v) for k, v in (jobs[args.job].get("env") or {}).items()})

    log_dir = pathlib.Path(args.log_dir) / args.job
    log_dir.mkdir(parents=True, exist_ok=True)
    # GITHUB_PATH, as on a runner: a step appends directories to this file and
    # every later step finds them at the front of PATH.
    github_path = log_dir / "github_path"
    github_path.write_text("", encoding="utf-8")
    base_env["GITHUB_PATH"] = str(github_path)
    seen_paths = 0
    results = []
    for index, step in enumerate(steps):
        name = step.get("name") or step.get("uses") or f"step {index}"
        if args.list:
            print(f"{index:3d}  {'uses' if 'uses' in step else 'run '}  {name}")
            continue
        if index < args.first or index > args.last:
            continue
        if args.only and not any(o.lower() in name.lower() for o in args.only):
            continue
        if "run" not in step:
            results.append((index, "SKIP-USES", 0.0, name))
            continue
        if args.skip_install and name.lower().startswith("install qemu"):
            results.append((index, "SKIP-INSTALL", 0.0, name))
            continue
        env = dict(base_env)
        env.update({k: str(v) for k, v in (step.get("env") or {}).items()})
        cwd = ROOT / step.get("working-directory", ".")
        log = log_dir / f"{index:02d}.log"
        started = time.time()
        with log.open("wb") as out:
            out.write(f"# {name}\n".encode())
            out.flush()
            rc = subprocess.run(
                ["bash", "-e", "-o", "pipefail", "-c", step["run"]],
                cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT,
            ).returncode
        took = time.time() - started
        added = [l.strip() for l in github_path.read_text(encoding="utf-8").splitlines() if l.strip()]
        for directory in added[seen_paths:]:
            base_env["PATH"] = directory + os.pathsep + base_env.get("PATH", "")
        seen_paths = len(added)
        status = "PASS" if rc == 0 else f"FAIL({rc})"
        results.append((index, status, took, name))
        print(f"{index:3d}  {status:12s} {took:7.1f}s  {name}", flush=True)
        if rc != 0 and not args.keep_going:
            break

    if args.list:
        return 0
    summary = log_dir / "summary.tsv"
    summary.write_text(
        "".join(f"{i}\t{s}\t{t:.1f}\t{n}\n" for i, s, t, n in results), encoding="utf-8"
    )
    ran = [r for r in results if r[1].startswith(("PASS", "FAIL"))]
    failed = [r for r in ran if r[1].startswith("FAIL")]
    print(f"CI-LOCAL: job={args.job} ran={len(ran)} passed={len(ran) - len(failed)} "
          f"failed={len(failed)} skipped={len(results) - len(ran)} logs={log_dir}")
    for i, s, _, n in failed:
        print(f"  FAILED {i}: {n}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
