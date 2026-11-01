#!/usr/bin/env python3
"""LOD benchmark on the real GPU, offscreen: render distance 32 + LOD 256 by default.
Usage: lodbench.py --build <target/fast dir> --label NAME [--mesh on|off] [--size 2560x1440] [--taa true]
Uses scratch homes under $HORIZON_BENCH_BASE/tmp (default ~/dev/horizon-wt), never a flatpak's data."""
import argparse, json, os, shutil, subprocess, tempfile, time
from pathlib import Path

BASE = Path(os.environ.get("HORIZON_BENCH_BASE", Path.home() / "dev/horizon-wt"))
HERE = Path(__file__).resolve().parent
RES = BASE / "bench-data/26.3"
OUT = BASE / "bench-results"
TMP = BASE / "tmp"  # short path: unix socket paths are limited to ~108 bytes
SEED, WORLD = "1234", "LodBench"
SCENES = [
    ("village", (-960.0, 125.0, 80.0), (-960.0, 125.0, 272.0), 35.0),
    ("forest", (-1100.0, 120.0, -560.0), (-1100.0, 120.0, -368.0), 25.0),
    ("ocean", (-300.0, 85.0, -420.0), (-300.0, 85.0, -228.0), 10.0),
    ("mountain", (560.0, 260.0, 860.0), (752.0, 260.0, 860.0), 30.0),
]


def ctl(build, sock, env, *words, check=True):
    out = subprocess.run([str(build / "horizon-ctl"), "--socket", str(sock), *map(str, words)],
                         env=env, capture_output=True, text=True)
    line = out.stdout.strip().splitlines()[-1] if out.stdout.strip() else "{}"
    reply = json.loads(line) if line.startswith("{") else {"error": out.stderr.strip() or line}
    if check and "error" in reply:
        raise RuntimeError(f"{words}: {reply}")
    return reply


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--build", required=True)
    p.add_argument("--label", required=True)
    p.add_argument("--mesh", default="off")
    p.add_argument("--rd", default="32")
    p.add_argument("--lod", default="256")
    p.add_argument("--size", default="1280x720")
    p.add_argument("--taa", default="false")
    p.add_argument("--scenes")
    p.add_argument("--seconds", type=float, default=12.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--lod-wait", type=float, default=240.0)
    p.add_argument("--window", action="store_true", help="show a window instead of --offscreen")
    a = p.parse_args()
    build = Path(a.build).resolve()
    TMP.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(dir=TMP, prefix="h-"))
    scratch = {k: tempfile.mkdtemp(dir=TMP, prefix=f"{k[:3]}-") for k in
               ("HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME")}
    env = dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO="1", HORIZON_NO_DISCORD="1",
               HORIZON_LAUNCHER_NO_SELF_UPDATE="1", HORIZON_TELEMETRY_URL="off", **scratch)
    # maxFps 260 is "unlimited" (1000 gets clamped to ~118); inactivityFpsLimit stops the AFK 30 fps cap.
    (home / "options.txt").write_text(
        f"version:4790\nenableVsync:false\nmaxFps:260\nrenderDistance:{a.rd}\nsimulationDistance:12\n"
        "fov:0.0\nguiScale:0\ninactivityFpsLimit:minimized\n")
    (home / "horizon-graphics.txt").write_text(
        f"preset:custom\nlodDistance:{a.lod}\nfarDetail:full\nmeshShaders:{a.mesh}\n"
        f"taa:{a.taa}\nfxaa:false\npresentMode:immediate\noverlay:off\nantiLag:false\n")
    os.chdir(home)
    sock = home / "control.sock"
    w, h = a.size.split("x")
    launch = ["launch", "--world", WORLD, "--seed", SEED, "--creative", "--render-distance", a.rd,
              "--resources", str(RES), "--no-pause", "--weather", "clear", "--time", "6000",
              "--game", str(build / "horizon-client"), "--width", w, "--height", h]
    if not a.window:
        launch.append("--offscreen")
    res = {"label": a.label, "build": str(build), "mesh": a.mesh, "rd": a.rd, "lod": a.lod,
           "size": a.size, "taa": a.taa, "scenes": {}}
    ctl(build, sock, env, *launch)
    try:
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "lod-distance", a.lod)
        st = ctl(build, sock, env, "status")
        res["status0"] = st
        print("renderer:", {k: st.get("renderer", {}).get(k) for k in ("gpu", "size", "driver")}, flush=True)
        for name, start, end, pitch in SCENES:
            if a.scenes and name not in a.scenes.split(","):
                continue
            ctl(build, sock, env, "tp", *start)
            ctl(build, sock, env, "look", 0, pitch)
            ctl(build, sock, env, "time", 6000)
            ctl(build, sock, env, "wait-ready", 300)
            t0, stable, lod = time.time(), 0, {}
            while time.time() - t0 < a.lod_wait:
                s = ctl(build, sock, env, "status")
                lod = s.get("lod") or {}
                done = lod.get("target_tiles", 0) > 0 and lod.get("sampled_target_tiles", 0) >= lod.get("target_tiles", 0)
                idle = s.get("pending_chunks", 1) == 0 and s.get("pending_uploads", 1) == 0
                stable = stable + 1 if (done and idle) else 0
                if stable >= 3:
                    break
                time.sleep(2)
            print(f"  {name}: lod {lod.get('sampled_target_tiles')}/{lod.get('target_tiles')} after {time.time()-t0:.0f}s", flush=True)
            res.setdefault("lod_ready", {})[name] = lod
            time.sleep(a.settle)
            summ = ctl(build, sock, env, "fly", *start, *end, a.seconds, pitch)
            res["scenes"][name] = summ
            print(f"{name:<9} {summ.get('avg_fps',0):7.1f} fps  1%low {summ.get('low_1pct_fps',0):7.1f}"
                  f"  p99 {summ.get('p99_ms',0):6.2f}ms  cpu {((summ.get('cpu') or {}).get('avg_ms') or 0):6.2f}"
                  f"  gpu {((summ.get('gpu') or {}).get('avg_ms') or 0):6.2f}ms", flush=True)
    finally:
        ctl(build, sock, env, "quit", check=False)
        for _ in range(100):
            if not sock.exists():
                break
            time.sleep(0.2)
        OUT.mkdir(parents=True, exist_ok=True)
        log = home / "logs" / "latest.log"
        if log.exists():
            shutil.copy(log, OUT / f"{a.label}.log")
        (OUT / f"{a.label}.json").write_text(json.dumps(res, indent=2))
        os.chdir(BASE)
        shutil.rmtree(home, ignore_errors=True)
        for d in scratch.values():
            shutil.rmtree(d, ignore_errors=True)


main()
