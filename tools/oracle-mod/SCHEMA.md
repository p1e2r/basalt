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

# Control channel (scripted input) — drives the real client

A second, independent channel next to the request/response files above: line-based JSON over TCP on
`127.0.0.1:<port>` (`-Dbasalt.oracle.ctl.port`, launch.py env `ORACLE_CTL_PORT`, default **25789**).
It lets a harness play the real vanilla client without a human: connect to a server, walk, look,
dig, place, open the inventory and click, and read back what the client sent and what it holds.
Client side in Rust: `crates/check/src/client/oracle.rs` (`Ctl`); users: `basalt-check client-input`
and `basalt-check client-clicks`.

**Protocol.** One JSON object per line, one response line per request, in request order.
Request `{"id": <any>, "op": "<name>", ...}`; response `{"id", "ok": true|false, "tick": T, ...}` (on
failure `"error"` holds a stack trace). Requests are queued and executed on the client main thread
at the start of client ticks (`Minecraft.tick` HEAD, via a mixin) in order; `T` is the client tick
the command ran on (blocking commands: the tick it completed on). Ticks are counted from client
start. **Send a whole script in one go (pipelined) for tick-exact timing**; a request/response
round trip costs about a tick.

| op | arguments | effect |
|---|---|---|
| `ping` | | `{"pong":true}` |
| `connect` | `addr` "host:port" | `ConnectScreen.startConnecting` (as the multiplayer screen's join) |
| `disconnect` | | leave the world |
| `wait` | `ticks` | **blocking**: later commands run `ticks` ticks after this one started |
| `wait_join` | `timeout` (ticks, 1200) | **blocking** until in a world with no screen up |
| `wait_screen` | `screen` (class simple name or `none`), `timeout` (200) | **blocking** until that screen is shown |
| `move` | `forward` `back` `left` `right` (bool, sticky) | hold/release the movement keys |
| `jump` `sneak` `sprint` `attack` `use` | `action`: `tap` (default, `ticks`=1) / `press` / `hold` / `release` | press the key mapping (attack/use go through the client's own handleKeybinds: dig, place) |
| `key` | `name` (drop, swap, pick, inventory, a hotbar/other mapping name such as `key.inventory`), `action` as above | any key mapping |
| `hotbar` | `slot` 0-8 | tap that hotbar key |
| `look` | `yaw`, `pitch`, `relative` (bool) | set the player's rotation (absolute or added) |
| `inventory` | `action`: `open` / `close` / `toggle` | open = tap the inventory key; close = the screen's onClose (sends container_close) |
| `escape` | | close the screen, or open the pause menu |
| `click` | `slot`, `button`, `mode` 0-6 | the open container screen's own `slotClicked(slot, slot, button, ContainerInput)`: the client predicts and serialises exactly what the mouse would. Modes: 0 pickup, 1 quick move, 2 swap, 3 clone, 4 throw, 5 quick craft, 6 pickup all; slot -999 = outside; mode 5 takes the raw quick-craft button (`header & 3 | type << 2`) |
| `drag` | `slots` [..], `button` 0 left / 1 right / 2 middle | quick-craft: start, one packet per slot, end |
| `chat` / `command` | `text` | send a chat message / command (no leading slash) |
| `mark` | `label` | write `{"tick","mark"}` into the packet log |
| `log` | `file` (open+truncate) or `close` | serverbound packet log, see below |
| `flush` | | flush the packet log; returns the packet count |
| `grab` | `on` (bool) | whether the mod forces the mouse-grabbed flag while in a world (default on: an unfocused window never grabs, and attack needs it) |
| `release_all` | | release every held key |
| `query` | | state, below |
| `quit` | | close the client |

`query` response fields: `in_world`, `screen` (class simple name or `none`), `overlay`, `packets`,
and in a world `pos` [x,y,z], `yaw`, `pitch`, `on_ground`, `sneaking`, `sprinting`, `selected`,
`health`, `food`, `saturation`, `held` `{id,count[,damage]}`, `hit` (`{type, pos, face, block}` of the
crosshair target) and `menu` `{id, type, state_id, carried, slots:[{id,count[,damage]}...]}` for the
open container (type `player` for the inventory menu).

## Serverbound packet log (JSONL)

`log` opens a file; from then on every packet the client sends (login, configuration and play) is
written, one line each: `{"tick": T, "seq": n, "wall_ms": ms, "name": "<class>", "hex": "<packet id
varint + body, as on the wire, decompressed>"}`; `{"tick", "mark": "..."}` lines are the script's marks
(plus one `pipeline ...` line when a connection is first seen). `tick` is the tick the packet was
handed to the connection (a mixin on `Connection.send` stamps it; a netty handler next to the encoder
reads the bytes, so the log shows exactly the wire format). Decode `hex` with the protocol ids of the
state that was current (play after the first `ServerboundFinishConfigurationPacket`). basalt-client's
`--script FILE --script-log OUT` writes the same format (`tick`, `seq`, `hex`, marks) for diffing.
