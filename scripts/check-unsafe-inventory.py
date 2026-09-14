#!/usr/bin/env python3
"""Unsafe inventory check (V1-SEC-005).

Every `unsafe` in the kernel (`kernel/src`) and in the Ring 3 support
library (`user/ulib`) - block, fn, impl or extern - must be covered by a
row of `docs/UNSAFE_INVENTORY.md`, and every function a row names must
still contain one.

A site is covered when some row's Location cell names its file (relative to
`kernel/src`, or `user/ulib`) and either names the function the site is in -
`file.rs::fn`, `file.rs::{a, b}`, or the function's name anywhere in the
cell - or names the file alone (a row about every unsafe in that file). The
`#[unsafe(...)]` attribute is not an unsafe block and is not counted;
comments are ignored. `kernel-core` forbids unsafe code outright (checked
here too).

Exit 0 with `UNSAFE-INVENTORY: OK <sites> sites in <files> files, <rows>
rows`, otherwise every uncovered site and stale function is listed.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TREES = [('kernel/src', ''), ('user/ulib/src', 'user/ulib::')]
UNSAFE = re.compile(r'\bunsafe\b(?!\s*\()')
FN = re.compile(r'\bfn\s+([A-Za-z_][A-Za-z0-9_]*)')


def strip_comments(line, in_block):
    """Remove // and /* */ comments (string literals containing them are
    rare enough in this tree to ignore)."""
    out = ''
    i = 0
    while i < len(line):
        if in_block:
            j = line.find('*/', i)
            if j < 0:
                return out, True
            i = j + 2
            in_block = False
            continue
        if line.startswith('//', i):
            break
        if line.startswith('/*', i):
            in_block = True
            i += 2
            continue
        out += line[i]
        i += 1
    return out, in_block


def sites():
    found = []
    for tree, prefix in TREES:
        base = os.path.join(ROOT, tree)
        for dirpath, _, files in os.walk(base):
            for name in sorted(files):
                if not name.endswith('.rs'):
                    continue
                path = os.path.join(dirpath, name)
                rel = os.path.relpath(path, base).replace(os.sep, '/')
                if prefix:
                    rel = 'user/ulib'
                enclosing = None
                in_block = False
                for n, raw in enumerate(open(path, encoding='utf-8'), 1):
                    code, in_block = strip_comments(raw, in_block)
                    code = re.sub(r'#\[unsafe\([^)]*\)\]', '', code)
                    m = FN.search(code)
                    if UNSAFE.search(code):
                        # `unsafe impl Trait for Type` is named by its type;
                        # `unsafe fn f` by itself; anything else by the most
                        # recent fn above it.
                        imp = re.search(r'unsafe\s+impl\b[^{]*?\bfor\s+([A-Za-z_][A-Za-z0-9_]*)', code) \
                            or re.search(r'unsafe\s+impl\s+([A-Za-z_][A-Za-z0-9_]*)', code)
                        fn = imp.group(1) if imp else (m.group(1) if m else enclosing)
                        found.append((rel, n, fn, code.strip()))
                    if m:
                        enclosing = m.group(1)
    return found


def rows():
    text = open(os.path.join(ROOT, 'docs', 'UNSAFE_INVENTORY.md'), encoding='utf-8').read()
    out = []
    for line in text.splitlines():
        cells = [c.strip() for c in line.split('|')]
        if len(cells) < 4 or not cells[1].isdigit():
            continue
        out.append((int(cells[1]), cells[2]))
    return out


PATH = r'(user/ulib|[A-Za-z0-9_/]+\.rs)'
IDENT = r'[A-Za-z_][A-Za-z0-9_]*'


def parse_location(cell):
    """(pairs, bare): the (file, name) pairs a Location cell names -
    `file.rs::fn`, `file.rs::module::fn` (every segment counts: a type, a
    module or the function), `file.rs::{a, b}` - and the files it names
    without any `::` (rows about every unsafe in that file)."""
    pairs = set()
    tied = set()
    rx = re.compile(PATH + r'((?:::' + IDENT + r')*)(?:::\{([^}]*)\})?')
    for m in rx.finditer(cell):
        f, segs, group = m.group(1), m.group(2), m.group(3)
        names = [s for s in segs.split('::') if s]
        if group:
            names += [n.strip().strip('`') for n in group.split(',') if n.strip()]
        if names:
            tied.add(f)
            pairs.update((f, n) for n in names)
    bare = {m.group(1) for m in re.finditer(PATH, cell)} - tied
    return pairs, bare


def same_file(site_file, named):
    return site_file == named or site_file.endswith('/' + named)


def main():
    s = sites()
    r = [(num, cell, *parse_location(cell)) for num, cell in rows()]
    problems = []
    for rel, n, fn, code in s:
        ok = False
        for _, cell, pairs, bare in r:
            if any(same_file(rel, f) and fn == name for f, name in pairs):
                ok = True
            elif any(same_file(rel, f) for f in bare):
                # A row about the whole file, or one naming the function in
                # its prose.
                ok = True
            if ok:
                break
        if not ok:
            problems.append(f'uncovered: {rel}:{n} in {fn}: {code[:90]}')
    # Stale: a name a row ties to a file that holds no unsafe site under it.
    names_by_file = {}
    for rel, _, fn, _ in s:
        names_by_file.setdefault(rel, set()).add(fn)
    for num, cell, pairs, bare in r:
        for f in bare | {f for f, _ in pairs}:
            if not any(same_file(rel, f) for rel in names_by_file):
                problems.append(f'stale row {num}: {f} has no unsafe code')
        for f, name in pairs:
            files = [rel for rel in names_by_file if same_file(rel, f)]
            # A pair is live when one of its segments names a site's fn
            # or type; module and type segments are checked through the
            # function segment that follows them in the same cell.
            if files and not any(name in names_by_file[x] for x in files):
                segs_live = any(
                    (f, other) in pairs and any(other in names_by_file[x] for x in files)
                    for other in {n for g, n in pairs if g == f}
                )
                if not segs_live:
                    problems.append(f'stale row {num}: {f}::{name} has no unsafe code')
    site_files = set(names_by_file)
    core = os.path.join(ROOT, 'crates', 'kernel-core', 'src', 'lib.rs')
    if '#![forbid(unsafe_code)]' not in open(core, encoding='utf-8').read():
        problems.append('kernel-core no longer forbids unsafe code')
    if problems:
        print('\n'.join(problems))
        print(f'UNSAFE-INVENTORY: FAILED {len(problems)} problem(s)')
        return 1
    print(f'UNSAFE-INVENTORY: OK {len(s)} sites in {len(site_files)} files, {len(r)} rows')
    return 0


if __name__ == '__main__':
    sys.exit(main())
