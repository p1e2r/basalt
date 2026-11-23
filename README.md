# Basalt

A clean-room reimplementation of **Minecraft: Java Edition 26.2** (protocol 776) in Rust.
Not affiliated with Mojang or Microsoft. No Mojang code or assets are included:

- Game data (block states, registries, packet ids, worldgen JSON) is produced on your machine
  by the official server jar's built-in data generator (`tools/extract-data.sh`).
- The client loads textures, models and sounds from **your own** `client.jar` at runtime.

## Crates
| crate | what |
|---|---|
| `basalt-nbt` | NBT read/write (file + network form) |
| `basalt-data` | Tables generated at build time from the data generator reports |
| `basalt-protocol` | VarInt/packet framing, compression, text components |
| `basalt-world` | Chunk storage, Anvil region save/load |
| `basalt-worldgen` | Data-driven overworld generation (noise router, biomes, surface, carvers, features) |
| `basalt-server` | Offline-mode server vanilla 26.2 clients can join |
| `basalt-client` | winit + wgpu client |
| `basalt-probe` | Protocol probe / logging proxy for interop testing |

## Build
```sh
./tools/extract-data.sh      # needs Java 25+, downloads the official server jar
cargo build --release
./target/release/basalt-server
./target/release/basalt-client --jar /path/to/26.2/client.jar --server 127.0.0.1:25565
```

## License
MIT — see LICENSE.
