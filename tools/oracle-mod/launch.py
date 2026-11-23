#!/usr/bin/env python3
"""Launch the installed vanilla 26.2 client under Fabric Loader with the test-only oracle mod.
TEST-ONLY: never shipped. Game dir: vanilla/oracle/game (separate from vanilla/mcclient).
No server join at launch: the mesh oracle works from the title screen; the control channel
(127.0.0.1:ORACLE_CTL_PORT, see SCHEMA.md) can connect to a server and drive the client.

usage: tools/oracle-mod/launch.py            # launch (blocks until the client exits)
       tools/oracle-mod/launch.py --classpath  # print the vanilla compile classpath and exit
env:   MC_DIR (default ~/Library/Application Support/minecraft), MC_VERSION (26.2), JAVA,
       ORACLE_DIR (default vanilla/oracle), ORACLE_CTL_PORT (scripted-input control channel, default 25790)
"""
import json, os, platform, subprocess, sys, zipfile
from pathlib import Path

repo = Path(__file__).resolve().parent.parent.parent
mc = Path(os.environ.get("MC_DIR", Path.home() / "Library/Application Support/minecraft"))
ver = os.environ.get("MC_VERSION", "26.2")
oracle = Path(os.environ.get("ORACLE_DIR", repo / "vanilla" / "oracle")).resolve()
vj = json.loads((mc / "versions" / ver / f"{ver}.json").read_text())

osname = {"Darwin": "osx", "Linux": "linux", "Windows": "windows"}[platform.system()]
arch = "arm64" if platform.machine() in ("arm64", "aarch64") else "x86_64"

def allowed(rules):
    if not rules: return True
    ok = False
    for r in rules:
        o = r.get("os", {})
        match = (not o.get("name") or o["name"] == osname) and (not o.get("arch") or o["arch"] == arch or (o["arch"] == "x86" and arch == "x86_64"))
        if "features" in r: match = False
        if match: ok = r["action"] == "allow"
    return ok

game_dir = oracle / "game"
natives = game_dir / "natives"
cp = []
for lib in vj["libraries"]:
    if not allowed(lib.get("rules")): continue
    art = lib.get("downloads", {}).get("artifact")
    if art:
        p = mc / "libraries" / art["path"]
        if p.exists(): cp.append(str(p))
        else: print("missing library", p, file=sys.stderr)
client_jar = mc / "versions" / ver / f"{ver}.jar"

if "--classpath" in sys.argv:
    print(os.pathsep.join(cp + [str(client_jar)]))
    sys.exit(0)

fabric = oracle / "fabric"
mod = oracle / "basalt-oracle.jar"
if not mod.exists(): sys.exit(f"{mod} missing: run tools/oracle-mod/build.sh first")
natives.mkdir(parents=True, exist_ok=True)
for p in cp:
    if "natives" in p:
        with zipfile.ZipFile(p) as z:
            for n in z.namelist():
                if n.endswith((".dylib", ".jnilib", ".so", ".dll")): (natives / Path(n).name).write_bytes(z.read(n))
# fabric loader + its libraries first (it bundles a newer ASM than the game may ship)
loader_cp = sorted(str(p) for p in fabric.glob("*.jar"))
full_cp = loader_cp + [p for p in cp if "/org/ow2/asm/" not in p] + [str(client_jar)]

opts = game_dir / "options.txt"
if not opts.exists():
    opts.write_text("pauseOnLostFocus:false\nonboardAccessibility:false\ntutorialStep:none\nskipMultiplayerWarning:true\n"
                    "joinedFirstServer:true\nsoundCategory_master:0.0\nao:true\n")

subs = {"auth_player_name": "Oracle", "version_name": ver, "game_directory": str(game_dir), "assets_root": str(mc / "assets"),
        "assets_index_name": vj["assetIndex"]["id"], "auth_uuid": "00000000000000000000000000000000", "auth_access_token": "0",
        "clientid": "0", "auth_xuid": "0", "user_type": "legacy", "version_type": "release", "natives_directory": str(natives),
        "launcher_name": "basalt-oracle", "launcher_version": "1", "classpath": os.pathsep.join(full_cp)}

def expand(args):
    out = []
    for a in args:
        if isinstance(a, dict):
            if not allowed(a.get("rules")): continue
            v = a["value"]; out += v if isinstance(v, list) else [v]
        else: out.append(a)
    res = []
    for a in out:
        for k, v in subs.items(): a = a.replace("${" + k + "}", v)
        res.append(a)
    return res

jvm = expand(vj["arguments"]["jvm"])
# from Fabric's official launcher profile for loader 0.20.1 / 26.2 (meta.fabricmc.net)
jvm += ["-XX:+UnlockDiagnosticVMOptions", "-XX:-UseBASE64Intrinsics", "-DFabricMcEmu= net.minecraft.client.main.Main ",
        f"-Dfabric.addMods={mod}", f"-Dbasalt.oracle.dir={oracle}",
        f"-Dbasalt.oracle.ctl.port={os.environ.get('ORACLE_CTL_PORT', '25790')}"]
game = expand(vj["arguments"]["game"]) + ["--width", "640", "--height", "360"]
java = os.environ.get("JAVA", "java")
cmd = [java, "-Xmx3G"] + jvm + ["net.fabricmc.loader.impl.launch.knot.KnotClient"] + game
print("launching oracle client", ver, "game dir", game_dir, file=sys.stderr)
os.chdir(game_dir)
sys.stderr.flush()
os.execvp(java, cmd)  # become the JVM so killing this process stops the client
