#!/usr/bin/env python3
"""Launch the official 26.2 client (already installed by the Minecraft launcher) in offline
mode, joining a server directly. Uses a separate game dir (vanilla/mcclient) so the user's
own options/worlds are untouched. For testing Basalt against the real client.

usage: tools/run-vanilla-client.py HOST:PORT [NAME]
env:   MC_DIR (default ~/Library/Application Support/minecraft), MC_VERSION (26.2), JAVA
"""
import json, os, platform, subprocess, sys, zipfile
from pathlib import Path

repo = Path(__file__).resolve().parent.parent
mc = Path(os.environ.get("MC_DIR", Path.home() / "Library/Application Support/minecraft"))
ver = os.environ.get("MC_VERSION", "26.2")
server = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1:25565"
name = sys.argv[2] if len(sys.argv) > 2 else "Tester"
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

game_dir = repo / "vanilla" / "mcclient"
natives = game_dir / "natives"
natives.mkdir(parents=True, exist_ok=True)
cp = []
for lib in vj["libraries"]:
    if not allowed(lib.get("rules")): continue
    art = lib.get("downloads", {}).get("artifact")
    if art:
        p = mc / "libraries" / art["path"]
        if p.exists(): cp.append(str(p))
        else: print("missing library", p, file=sys.stderr)
        if "natives" in art["path"]:
            with zipfile.ZipFile(p) as z:
                for n in z.namelist():
                    if n.endswith((".dylib", ".jnilib", ".so", ".dll")): (natives / Path(n).name).write_bytes(z.read(n))
cp.append(str(mc / "versions" / ver / f"{ver}.jar"))

opts = game_dir / "options.txt"
if not opts.exists():
    opts.write_text("pauseOnLostFocus:false\nonboardAccessibility:false\ntutorialStep:none\nskipMultiplayerWarning:true\njoinedFirstServer:true\nsoundCategory_master:0.0\nrenderDistance:8\n")

subs = {"auth_player_name": name, "version_name": ver, "game_directory": str(game_dir), "assets_root": str(mc / "assets"),
        "assets_index_name": vj["assetIndex"]["id"], "auth_uuid": "00000000000000000000000000000000", "auth_access_token": "0",
        "clientid": "0", "auth_xuid": "0", "user_type": "legacy", "version_type": "release", "natives_directory": str(natives),
        "launcher_name": "basalt-test", "launcher_version": "1", "classpath": os.pathsep.join(cp), "quickPlayMultiplayer": server}

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
game = expand(vj["arguments"]["game"]) + ["--quickPlayMultiplayer", server]
java = os.environ.get("JAVA", "java")
cmd = [java, "-Xmx2G"] + jvm + [vj["mainClass"]] + game
print("launching", ver, "as", name, "->", server, file=sys.stderr)
sys.exit(subprocess.call(cmd, cwd=game_dir))
