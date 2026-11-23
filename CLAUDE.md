# Basalt — handoff notes for Claude Code

Clean-room Rust reimplementation of Minecraft Java 26.2 (protocol 776). MIT licensed.
Never translate decompiled Mojang code; match behaviour by observing a real server/client
over the wire. Never commit Mojang data/assets (data/, vanilla/, *.jar are gitignored).

Read NOTES.md for verified packet formats.

## Status (handed off from a cloud session, 2026-11-23)
- Done & working: crates/nbt, crates/data (tables generated from ./data at build time),
  crates/protocol (framing, compression, varints, NBT, text), crates/probe (joins a server and
  dumps packets; `basalt-probe proxy LISTEN TARGET DUMPDIR` logs both directions).
- crates/client: WIP (~1.6k lines: networking, jar asset loading, wgpu renderer scaffolding).
  NOT yet compile-checked — first job is `cargo build -p basalt-client` and fix errors.
- Not started: crates/world (chunk storage + Anvil), crates/server, crates/worldgen.

## Local setup (macOS)
1. Java 25 + Rust. `./tools/extract-data.sh` (downloads the official server jar, runs its data
   generator into ./data). Required before any build.
2. Test server: run vanilla in vanilla/srv with online-mode=false,
   network-compression-threshold=-1, level-seed=12345, server-port=25566.
3. Test client: the user's normal launcher with 26.2, joining localhost.
   (NOTES.md's /home/claude paths refer to the old cloud machine.)

## Plan
1. Fix client build; get it joining vanilla on 25566 and rendering chunks.
2. crates/world + crates/server: real 26.2 client can join, walk, break/place, chat, multiplayer.
3. crates/worldgen: data-driven noise router, multi-noise biomes, surface rules, carvers,
   features; compare to vanilla seed 12345 chunk-by-chunk. API:
   `Generator::new(seed).generate_chunk(cx, cz) -> GeneratedChunk { blocks: Vec<[u16;4096]>
   (24 sections, y<<8|z<<4|x), biomes: Vec<[u8;64]> (y<<4|z<<2|x) }`.
