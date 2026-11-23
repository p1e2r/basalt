# Oracle request/response schema

Transport: write `DIR/in/<id>.json` (write to `<id>.json.part`, then rename); the oracle answers
with `DIR/out/<id>.json` (also written via rename) and moves the request to `DIR/in/done/`.
`DIR` = `-Dbasalt.oracle.dir` (default `vanilla/oracle`). `DIR/out/_ready.json` appears once the
client's resources are loaded: `{"ready": true, "handlers": ["ping", "mesh"]}`.

Every request: `{"id": "<unique>", "kind": "<handler>", ...}`.
Every response: `{"id", "kind", "oracle": "vanilla-26.2", <handler fields>, "millis"}`; a
failed handler adds `"error": "<stack trace>"`.

## kind "ping"

Response: `{"pong": true}`.

## kind "mesh" — block meshing

Request:

```json
{"id": "cm-1", "kind": "mesh", "options": {"ao": true, "cull": true},
 "scenes": [{
   "id": 0,
   "origin": [0, 64, 0],
   "blocks": [[0, 0, 0, "minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]"],
              [1, 0, 0, "minecraft:stone"]],
   "light": {"default": [15, 0], "cells": [[0, 1, 0, 7, 3]]},
   "tint": {"grass": 9551193, "foliage": 7842607, "dry_foliage": 10520653, "water": 4159204},
   "cardinal": "default"
 }]}
```

- `blocks`: offsets relative to `origin` (default `[0, 64, 0]`) and full block-state strings;
  the **target** is the block at offset `[0, 0, 0]`. Everything not listed is air.
- `light`: `[sky, block]` per cell (0..15); `default` applies to every unlisted cell
  (default `[15, 0]`). Values are used as given (no propagation).
- `tint`: the biome colours returned for the grass / foliage / dry-foliage / water resolvers
  (defaults shown). `cardinal`: `"default"` or `"nether"` directional shading.
- `options.ao` = smooth lighting on/off, `options.cull` = face culling on/off.

Response:

```json
{"options": {"ao": true, "cull": true}, "total_quads": 6,
 "scenes": [{"id": 0, "quads": [{
   "src": "block", "layer": "solid", "dir": "up", "cull": "up",
   "tex": "minecraft:block/stone", "ti": -1, "tint": 9551193, "shade": true, "emit": 15,
   "v": [[x, y, z, u, v, argb, lightcoords], ... 4 vertices]
 }], "error": "only if this scene failed"}]}
```

Per quad:
- `src`: `block` (block model tesselator) or `fluid` (fluid renderer).
- `layer`: `solid` | `cutout` | `translucent` (chunk section layer).
- `dir`: the quad's face direction (block quads; fluid quads report a placeholder — compare by
  geometry). `cull`: the cull face the quad was declared with (absent = none, `?` = unknown).
- `tex`: sprite name; `u`, `v` are **sprite-local** (0..1 inside that sprite, atlas-independent).
- `ti`: tint index (-1 = untinted); `tint`: resolved RGB of that tint source (block quads only;
  fluid colour is baked into `argb`). `shade`: directional shading enabled. `emit`: per-material
  light emission (only when non-zero).
- `x y z`: block-local position relative to the target (model offsets included).
- `argb`: final vertex colour = tint x directional shade x ambient occlusion (compare RGB only).
- `lightcoords`: packed light, `sky = (lc >> 16) / 16`, `block = (lc & 0xFFFF) / 16` (smooth
  light values can be fractional).

Basalt writes the same schema with `basalt-client --jar vanilla/client.jar --mesh-dump <file|dir>
--out <dir>` (plus `"random_variant": true` on scenes whose block has several weighted models).
