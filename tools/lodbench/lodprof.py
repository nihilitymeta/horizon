#!/usr/bin/env python3
"""Profile the game's CPU during a far-terrain flight: perf record (call graphs by frame
pointers) and perf stat (cycles, instructions, cache misses) of the whole process, from the
container (perf is there). Usage: lodprof.py --build DIR --label NAME [--mesh on|off]"""
import argparse, json, os, shutil, subprocess, tempfile, time
from pathlib import Path

BASE = Path(os.environ.get("HORIZON_BENCH_BASE", Path.home() / "dev/horizon-wt"))
HERE = Path(__file__).resolve().parent
RES, OUT, TMP = BASE / "bench-data/26.3", BASE / "bench-results", BASE / "tmp"
START, END, PITCH = (560.0, 260.0, 860.0), (752.0, 260.0, 860.0), 30.0


def ctl(build, sock, env, *words, check=True):
    out = subprocess.run([str(build / "horizon-ctl"), "--socket", str(sock), *map(str, words)],
                         env=env, capture_output=True, text=True)
    line = out.stdout.strip().splitlines()[-1] if out.stdout.strip() else "{}"
    reply = json.loads(line) if line.startswith("{") else {"error": out.stderr.strip() or line}
    if check and "error" in reply:
        raise RuntimeError(f"{words}: {reply}")
    return reply


def perf(*args):
    return ["distrobox", "enter", "horizon", "--", "perf", *args]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--build", required=True)
    p.add_argument("--label", required=True)
    p.add_argument("--mesh", default="on")
    p.add_argument("--size", default="2560x1440")
    p.add_argument("--seconds", type=float, default=10.0)
    a = p.parse_args()
    build = Path(a.build).resolve()
    TMP.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(dir=TMP, prefix="h-"))
    scratch = {k: tempfile.mkdtemp(dir=TMP, prefix=f"{k[:3]}-") for k in
               ("HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME")}
    env = dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO="1", HORIZON_NO_DISCORD="1",
               HORIZON_LAUNCHER_NO_SELF_UPDATE="1", HORIZON_TELEMETRY_URL="off", **scratch)
    (home / "options.txt").write_text("version:4790\nenableVsync:false\nmaxFps:260\nrenderDistance:32\n"
                                      "simulationDistance:12\nfov:0.0\nguiScale:0\ninactivityFpsLimit:minimized\n")
    (home / "horizon-graphics.txt").write_text(
        f"preset:custom\nlodDistance:256\nfarDetail:full\nmeshShaders:{a.mesh}\ntaa:true\nfxaa:false\n"
        "presentMode:immediate\noverlay:off\nantiLag:false\n")
    os.chdir(home)
    sock = home / "control.sock"
    w, h = a.size.split("x")
    # The game runs under perf record from the start (perf-wrap.sh): perf cannot attach to a
    # running process at perf_event_paranoid 2, but may profile its own child.
    data = OUT / f"{a.label}.perf.data"
    env["PERF_WRAP_BIN"] = str(build / "horizon-client")
    env["PERF_WRAP_OUT"] = str(data)
    ctl(build, sock, env, "launch", "--world", "LodProf", "--seed", "1234", "--creative", "--render-distance", "32",
        "--resources", str(RES), "--no-pause", "--weather", "clear", "--time", "6000",
        "--game", str(HERE / "perf-wrap.sh"), "--width", w, "--height", h, "--offscreen")
    result = {"label": a.label, "build": str(build), "mesh": a.mesh}
    try:
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "tp", *START)
        ctl(build, sock, env, "look", 0, PITCH)
        ctl(build, sock, env, "time", 6000)
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "lod-distance", 256)
        t0, stable = time.time(), 0
        while time.time() - t0 < 200:
            s = ctl(build, sock, env, "status")
            lod = s.get("lod") or {}
            done = lod.get("target_tiles", 0) > 0 and lod.get("sampled_target_tiles", 0) >= lod.get("target_tiles", 0)
            stable = stable + 1 if done and s.get("pending_uploads", 1) == 0 else 0
            if stable >= 5:
                break
            time.sleep(2)
        result["lod_seconds"] = round(time.time() - t0, 1)
        print(f"lod settled in {result['lod_seconds']}s: {lod.get('sampled_target_tiles')}/{lod.get('target_tiles')}", flush=True)
        ctl(build, sock, env, "mesh-shaders", a.mesh)
        time.sleep(1)
        ctl(build, sock, env, "fly", *START, *END, 4, PITCH)
        ctl(build, sock, env, "tp", *START)
        time.sleep(1)
        summ = ctl(build, sock, env, "fly", *START, *END, a.seconds, PITCH)
        result["fly"] = summ
        print(f"fly: {summ.get('avg_fps',0):7.1f} fps  cpu {((summ.get('cpu') or {}).get('avg_ms') or 0):6.2f}  gpu {((summ.get('gpu') or {}).get('avg_ms') or 0):6.2f}ms", flush=True)
    finally:
        ctl(build, sock, env, "quit", check=False)
        for _ in range(100):
            if not sock.exists():
                break
            time.sleep(0.2)
        (OUT / f"{a.label}.json").write_text(json.dumps(result, indent=1))
        # The report over the whole run: the main thread's hot symbols dominate anyway.
        if data.exists() and data.stat().st_size > 0:
            rep = subprocess.run(perf("report", "-i", str(data), "--no-children", "--sort", "comm,symbol", "--stdio",
                                      "--percent-limit", "0.8", "-g", "none"), capture_output=True, text=True).stdout
            (OUT / f"{a.label}.perf.txt").write_text(rep)
            lines = [l for l in rep.splitlines() if l.strip() and not l.startswith("#")]
            print("\n".join(lines[:40]), flush=True)
        else:
            print("no perf data", flush=True)
        os.chdir(BASE)
        shutil.rmtree(home, ignore_errors=True)
        for d in scratch.values():
            shutil.rmtree(d, ignore_errors=True)


main()
