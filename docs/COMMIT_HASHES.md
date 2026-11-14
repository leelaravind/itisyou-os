# Commit hashes cited in the documentation

**Written 2026-11-14 from a fresh clone of `origin`** (all branches and tags
fetched, full history: 153 commits up to the tip of `v1.0/ipc`).

## What was found

The documentation, `status/current.json`, `website/src/data/downloads.json`
and `website/src/lib/releases.ts` cite commits by abbreviated or full hash.
**None of the cited hashes exists in the repository on GitHub.** Every one of
them was checked with `git cat-file -e <hash>^{commit}` against the complete
history of every branch and tag; none resolves.

The commits themselves are there — the same subjects, in the same order, with
the same dates — under different hashes. The most likely explanation is that
the history on GitHub was rewritten after the documents were written (for
example an author/committer rewrite or a secret scrub before a public CI
window), or that the documents were written in a local repository whose
history differs from the pushed one. This clone cannot tell which. **The
owner's local repository (`E:\Project\itisyou-os`) is the place to check:**
`git cat-file -e 5593e50^{commit}` there says whether the cited hashes are
local-only.

Nothing was rewritten to hide this. The cited hashes are left exactly as they
were written, because they are part of the recorded evidence (CI runs and
deployments were recorded against them); this file is the correction.

## Release tags — what they point at today

Read from the tags on `origin`, so these are certain.

| Tag | Commit today | Date | Subject |
|---|---|---|---|
| `v0.1.0` | `1e8e026` | 2026-09-02 | Final V0.1 session checkpoint: CI green, requirements reconciled |
| `v0.2.0` | `41800e3` | 2026-09-02 | Stamp V0.2 verification (commit e3ff63a) in status; fix milestone double-prefix |
| `v0.3.0` | `031e0ba` | 2026-09-02 | Fix shell-test version assertion (0.2.0-dev -> 0.3.0-dev) |
| `v0.4.0` | `aae1421` | 2026-09-02 | V0.4 persistent storage: NVMe writes + ITFS filesystem + reboot persistence |
| `v0.5.0` | `cb6a9c0` | 2026-09-02 | V0.5: sync Cargo.lock workspace versions to 0.5.0-dev |
| `v0.6.0` | `cd710db` | 2026-09-02 | V0.6: stamp status commit 99a8a30 (0.6.0-dev) |
| `v0.7.0` | `fc37a09` | 2026-09-03 | stamp final V0.7 release evidence |
| `v0.8.0` | `59f48a9` | 2026-09-05 | v0.8: stamp release evidence — CI 33987853808, deployed to production |
| `v0.8.1` | `3786811` | 2026-09-13 | v0.8.1: stamp final release evidence — CI 34750316003, reproducible images published |
| `v0.9.0` | `6a068bb` | 2026-09-13 | v0.9.0: stamp release evidence - CI 34760629701, published images, site live |
| `v0.10.0` | `373168a` | 2026-09-13 | v0.10.0: stamp release evidence - CI 34782271250, published images, site live |
| `v1.0.0` | `702119e` | 2026-09-14 | v1.0.0: record the release commit in status/current.json |

## Cited release commits — mapped by unique subject

Each mapping below was made from a commit subject that occurs exactly once in
the history and from the parent chain under the tag; it is an identification
by subject and position, not by content hash.

| Cited hash | Cited as | Commit today | Subject today |
|---|---|---|---|
| `5593e50` | v1.0.0 release commit (`status/current.json`, `downloads.json`, `releases.ts`) | `00c9ad5` | v1.0.0: release ITISYOU OS V1.0 — Experimental Personal OS (locally verified) |
| `b1087c2` | v1.0.0 stamp | `ae5e0cf` | v1.0.0: stamp the release commit hash into downloads.json and releases.ts |
| `e2bae80` | commit carrying tag `v1.0.0` | `702119e` | v1.0.0: record the release commit in status/current.json |
| `c936bc7` | v0.10.0 release commit (CI `34782271250`) | `a626838` | v0.10.0: release commit - version 0.10.0 |
| `96f5d68` | v0.10.0 stamp commit carrying the tag (CI `34783340401`) | `373168a` | v0.10.0: stamp release evidence - CI 34782271250, published images, site live |
| `f06673e` | v0.9.0 release commit (CI `34760629701`) | `fa49aeb` | v0.9.0: release commit - version 0.9.0 |
| `997dbb4` | v0.9.0 stamp commit carrying the tag (CI `34761869462`) | `6a068bb` | v0.9.0: stamp release evidence - CI 34760629701, published images, site live |
| `bf32b53` | v0.8.1 content commit (CI `34750316003`) | `0308b17` (inferred) | v0.8.1: make release images reproducible from a clean build — the parent of the stamp commit that names CI `34750316003` |

A GitHub Actions run id recorded in the documents (for example
`34782271250`) names a run against the *cited* hash. If the history was
rewritten, GitHub shows that run against a commit that is no longer on any
branch; the run happened, but it cannot be re-checked against today's hash.

## Cited hashes not mapped

These 46 hashes are cited (gate tables, development story, session
checkpoints, older release entries) and do not exist on `origin`. They were
not mapped: the documents do not give a subject that identifies each one
uniquely, and guessing would fabricate evidence.

`00b5eea` `06a1eca` `08d836c` `09584ad` `0ead17e` `11e9fb5` `1a4bf96` `1d21c7f` `1e8a909` `1f08bf5`
`387b459` `397ab80` `42399b9` `4964cc1` `4ceaa4c` `5a4b008` `5bd2470` `5be3c0c` `5f8acab` `650a16f`
`6be344e` `6c5415c` `7367b89` `82d72be` `84d6b24` `8af917c` `8c47222` `8cafb28` `8fafda5` `93f6c51`
`9ca320c` `b0c4d05` `b1af235` `b5cc23d` `b99024b` `caae006` `cccf5a7` `ceb24f9` `d3ae42f` `d83942b`
`e12329b` `e20b4d5` `e329649` `e6e3496` `ee8d57c` `ff1af2c`

## What to do on the owner's machine

1. In `E:\Project\itisyou-os`, run `git cat-file -e 5593e50^{commit}`.
   - If it resolves, the local history is the documented one and `origin` is
     a rewritten copy: decide which is authoritative before pushing anything
     from that repository (a plain `git push` would be rejected or would
     reintroduce the old history).
   - If it does not resolve, the old objects are gone and this file is the
     only bridge between the documents and the history.
2. At the next release stamp, cite hashes read from `origin` after the push
   (`git ls-remote origin`), not from the local working copy.
