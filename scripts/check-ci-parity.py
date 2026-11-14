#!/usr/bin/env python3
"""CI parity (V1-REL-006): every QEMU leg in .github/workflows/ci.yml must
assert exactly what scripts/test.ps1 asserts for the same label.

For each label both files define, the `--send` and `--require-order`
sequences must be equal in order, and the `--require`, `--forbid` and
`--expect` values equal as multisets; timeouts may differ (CI runners are
slower). A leg in one file but not the other is a failure too. Values built
at run time are compared by shape: any variable (`$report`, `"${sends[@]}"`,
`$reclaimReport`) counts as the placeholder `$VAR`, so both files must use
one in the same place.

Exit 0 and `CI-PARITY: OK <n> legs` when they agree; otherwise every
difference is printed and the exit code is 1.
"""
import os
import re
import shlex
import sys
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FLAGS = ('--send', '--require', '--require-order', '--forbid', '--expect')
ORDERED = ('--send', '--require-order')


def ps_tokens(text):
    """Tokens of a PowerShell argument list: '...' strings (with '' escapes),
    "..." strings, and bare $variables; comments dropped."""
    out = []
    i = 0
    while i < len(text):
        c = text[i]
        if c == "'":
            j = i + 1
            buf = []
            while j < len(text):
                if text[j] == "'" and j + 1 < len(text) and text[j + 1] == "'":
                    buf.append("'")
                    j += 2
                    continue
                if text[j] == "'":
                    break
                buf.append(text[j])
                j += 1
            out.append(''.join(buf))
            i = j + 1
        elif c == '"':
            j = text.index('"', i + 1)
            out.append(text[i + 1:j])
            i = j + 1
        elif c == '#':
            j = text.find('\n', i)
            i = len(text) if j < 0 else j
        elif c == '$':
            m = re.match(r'\$[A-Za-z_][A-Za-z0-9_]*', text[i:])
            if m:
                out.append('$VAR' if m.group(0) != '$runner' else '')
                i += len(m.group(0))
            else:
                i += 1
        else:
            i += 1
    return [t for t in out if t != '']


def summarize(tokens):
    """Flag values: ordered lists for ORDERED flags, Counters for the rest;
    a variable not following a flag is a splice of generated arguments."""
    ordered = {f: [] for f in ORDERED}
    bags = {f: Counter() for f in FLAGS if f not in ORDERED}
    splices = 0
    i = 0
    while i < len(tokens):
        t = tokens[i]
        if t in FLAGS and i + 1 < len(tokens):
            # Variables interpolated inside a value ("sha256=$modelPin") are
            # compared by position too.
            v = re.sub(r'\$\{?[A-Za-z_][A-Za-z0-9_]*(\[@\])?\}?', '$VAR', tokens[i + 1])
            if t in ORDERED:
                ordered[t].append(v)
            else:
                bags[t][v] += 1
            i += 2
            continue
        # A variable right after an option is that option's value (a disk
        # path, say); anywhere else it splices in generated arguments.
        if t.startswith('$') and not (i > 0 and tokens[i - 1].startswith('--')):
            splices += 1
            ordered['--send'].append('<SPLICE>')
        i += 1
    return ordered, bags, splices


def parse_ps(path):
    src = open(path, encoding='utf-8').read()
    legs = {}
    rx = re.compile(r"& \$runner \(?@\((.*?)'--label', '([^']+)'\)", re.S)
    for m in rx.finditer(src):
        legs.setdefault(m.group(2), []).append(summarize(ps_tokens(m.group(1))))
    return legs


def parse_ci(path):
    src = open(path, encoding='utf-8').read()
    # Drop YAML comment lines, then join shell continuations.
    lines = [l for l in src.split('\n') if not l.strip().startswith('#')]
    src = '\n'.join(lines).replace('\\\n', ' ')
    legs = {}
    rx = re.compile(r'cargo run -q -p qemu-runner --(.*?)--label[ =]+([A-Za-z0-9_-]+)', re.S)
    for m in rx.finditer(src):
        body = m.group(1)
        toks = shlex.split(body, posix=True)
        toks = ['$VAR' if t.startswith('$') else t for t in toks]
        legs.setdefault(m.group(2), []).append(summarize(toks))
    return legs


def main():
    ps = parse_ps(os.path.join(ROOT, 'scripts', 'test.ps1'))
    ci = parse_ci(os.path.join(ROOT, '.github', 'workflows', 'ci.yml'))
    problems = []
    for label in sorted(set(ps) | set(ci)):
        if label not in ps or label not in ci:
            problems.append(f'{label}: only in {"scripts/test.ps1" if label in ps else "ci.yml"}')
            continue
        if len(ps[label]) != 1 or len(ci[label]) != 1:
            problems.append(f'{label}: defined {len(ps[label])}x in test.ps1, {len(ci[label])}x in ci.yml')
            continue
        (po, pb, psp), (co, cb, csp) = ps[label][0], ci[label][0]
        for f in ORDERED:
            if po[f] != co[f]:
                problems.append(f'{label}: {f} sequence differs\n    test.ps1: {po[f]}\n    ci.yml:   {co[f]}')
        for f in pb:
            if pb[f] != cb[f]:
                only_ps = list((pb[f] - cb[f]).elements())
                only_ci = list((cb[f] - pb[f]).elements())
                problems.append(f'{label}: {f} differs; only in test.ps1: {only_ps}; only in ci.yml: {only_ci}')
        if psp != csp:
            problems.append(f'{label}: generated-argument splices differ ({psp} vs {csp})')
    if problems:
        print('\n'.join(problems))
        print(f'CI-PARITY: FAILED {len(problems)} difference(s)')
        return 1
    print(f'CI-PARITY: OK {len(ps)} legs')
    return 0


if __name__ == '__main__':
    sys.exit(main())
