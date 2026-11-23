#!/usr/bin/env python3
"""Compare the Brigadier trees of two captured `commands` packets for every top-level command they
share (structure, executable/restricted flags, parsers, properties, suggestion types, redirects).
usage: tools/compare-commands.py VANILLA.bin BASALT.bin   (capture with `basalt-probe HOST:PORT 3 DIR`;
vanilla needs the probe op'd: `op Probe`). Exit 1 on any difference."""
import importlib.util, os, sys
spec = importlib.util.spec_from_file_location('dc', os.path.join(os.path.dirname(os.path.abspath(__file__)), 'decode-commands.py'))
dc = importlib.util.module_from_spec(spec); spec.loader.exec_module(dc)

def load(p): nodes, root = dc.decode(open(p, 'rb').read()); return dc.tree(nodes, root)

def diff(a, b, path, out):
    for k in ('type', 'executable', 'restricted', 'parser', 'properties', 'suggestions', 'redirect'):
        if a.get(k) != b.get(k): out.append(f'{path}: {k} vanilla={a.get(k)!r} basalt={b.get(k)!r}')
    for name in sorted(set(a['children']) | set(b['children'])):
        if name not in a['children']: out.append(f'{path}/{name}: only in basalt')
        elif name not in b['children']: out.append(f'{path}/{name}: only in vanilla')
        else: diff(a['children'][name], b['children'][name], f'{path}/{name}', out)

v, b = load(sys.argv[1]), load(sys.argv[2])
shared = sorted(set(v['children']) & set(b['children'])); out = []
for c in shared: diff(v['children'][c], b['children'][c], c, out)
print(f'{len(shared)} shared commands: {" ".join(shared)}')
print('only in basalt:', ' '.join(sorted(set(b['children']) - set(v['children']))) or '-')
for line in out[:40]: print('DIFF', line)
print('RESULT commands_tree_diffs=%d' % len(out))
sys.exit(1 if out else 0)
