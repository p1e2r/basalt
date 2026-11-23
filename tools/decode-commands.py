#!/usr/bin/env python3
"""Decode a clientbound `commands` packet body (Brigadier node graph) into a nested dict.
usage: tools/decode-commands.py PACKET.bin [--json] [--only cmd,cmd] ; needs the generated
packets/registries (target/*/build/basalt-data-*/out/generated.rs) for parser ids."""
import glob, json, re, struct, sys, os

def parser_names():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    fs = sorted(glob.glob(os.path.join(root, 'target/*/build/basalt-data-*/out/generated.rs')), key=os.path.getmtime)
    g = open(fs[-1]).read()
    m = re.search(r'pub const COMMAND_ARGUMENT_TYPE: &\[&str\] = &\[(.*?)\];', g, re.S)
    return ['minecraft:' + n if ':' not in n else n for n in re.findall(r'"([^"]+)"', m.group(1))]

PN = parser_names()

class R:
    def __init__(s, b): s.b, s.p = b, 0
    def u8(s): v = s.b[s.p]; s.p += 1; return v
    def vi(s):
        r = sh = 0
        while True:
            x = s.u8(); r |= (x & 0x7f) << sh; sh += 7
            if not x & 0x80: return r
    def st(s): n = s.vi(); v = s.b[s.p:s.p + n].decode(); s.p += n; return v
    def f(s, fmt): n = struct.calcsize(fmt); v = struct.unpack('>' + fmt, s.b[s.p:s.p + n])[0]; s.p += n; return v

def props(r, parser):
    if parser in ('brigadier:float', 'brigadier:double', 'brigadier:integer', 'brigadier:long'):
        fmt = {'brigadier:float': 'f', 'brigadier:double': 'd', 'brigadier:integer': 'i', 'brigadier:long': 'q'}[parser]
        fl = r.u8(); d = {}
        if fl & 1: d['min'] = r.f(fmt)
        if fl & 2: d['max'] = r.f(fmt)
        return d
    if parser == 'brigadier:string': return {'type': ['word', 'phrase', 'greedy'][r.vi()]}
    if parser == 'minecraft:entity': fl = r.u8(); return {'amount': 'single' if fl & 1 else 'multiple', 'type': 'players' if fl & 2 else 'entities'}
    if parser == 'minecraft:score_holder': fl = r.u8(); return {'amount': 'multiple' if fl & 1 else 'single'}
    if parser == 'minecraft:time': return {'min': r.f('i')}
    if parser in ('minecraft:resource_or_tag', 'minecraft:resource_or_tag_key', 'minecraft:resource', 'minecraft:resource_key', 'minecraft:resource_selector'): return {'registry': r.st()}
    return {}

def decode(b):
    r = R(b); n = r.vi(); nodes = []
    for _ in range(n):
        fl = r.u8(); kind = fl & 3; ch = [r.vi() for _ in range(r.vi())]
        node = {'flags': fl, 'kind': ['root', 'literal', 'argument'][kind] if kind < 3 else '?', 'children': ch}
        if fl & 8: node['redirect'] = r.vi()
        if kind >= 1: node['name'] = r.st()
        if kind == 2:
            pid = r.vi(); node['parser'] = PN[pid]; node['properties'] = props(r, node['parser'])
            if fl & 0x10: node['suggestions'] = r.st()
        nodes.append(node)
    root = r.vi()
    assert r.p == len(b), f'trailing bytes: {len(b) - r.p}'
    return nodes, root

def tree(nodes, i, seen=()):
    n = nodes[i]; out = {'type': n['kind'], 'executable': bool(n['flags'] & 4), 'restricted': bool(n['flags'] & 0x20)}
    if 'parser' in n: out['parser'] = n['parser']; out['properties'] = n['properties']
    if 'suggestions' in n: out['suggestions'] = n['suggestions']
    if 'redirect' in n: out['redirect'] = nodes[n['redirect']].get('name', 'root')
    out['children'] = {nodes[c]['name']: tree(nodes, c) for c in n['children']} if i not in seen else {}
    return out

if __name__ == '__main__':
    nodes, root = decode(open(sys.argv[1], 'rb').read())
    t = tree(nodes, root)
    only = sys.argv[sys.argv.index('--only') + 1].split(',') if '--only' in sys.argv else None
    if only: t['children'] = {k: v for k, v in t['children'].items() if k in only}
    if '--json' in sys.argv: print(json.dumps(t, indent=1, sort_keys=True))
    else:
        print(len(nodes), 'nodes;', len(t['children']), 'top-level commands'); print(' '.join(sorted(t['children'])))
        flags = sorted({n['flags'] for n in nodes}); print('flag values seen:', [hex(f) for f in flags])
        print('suggestion types:', sorted({n['suggestions'] for n in nodes if 'suggestions' in n}))
