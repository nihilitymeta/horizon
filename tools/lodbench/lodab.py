#!/usr/bin/env python3
"""In-process A/B of the GPU-driven far terrain: one game, LOD 256 / rd 32, the camera held
over a vantage point until the far terrain is loaded, then screenshots with mesh shaders on,
off and on again (same resident tiles), plus frame stats for each.
Usage: lodab.py --build DIR --label NAME [--size WxH] [--taa true] [--spot mountain|village]"""
import argparse, json, os, shutil, subprocess, tempfile, time
from pathlib import Path

BASE = Path(os.environ.get("HORIZON_BENCH_BASE", Path.home() / "dev/horizon-wt"))
HERE = Path(__file__).resolve().parent
RES, OUT, TMP = BASE / "bench-data/26.3", BASE / "bench-results", BASE / "tmp"
SPOTS = {
    "mountain": ((560.0, 260.0, 860.0), (563.0, 260.0, 860.0), 4.0),
    "village": ((-960.0, 160.0, 80.0), (-957.0, 160.0, 80.0), 12.0),
    "ocean": ((-300.0, 120.0, -420.0), (-297.0, 120.0, -420.0), 8.0),
    "ground": ((-960.0, 78.0, 80.0), (-957.0, 78.0, 80.0), -2.0),
    "valley": ((560.0, 150.0, 860.0), (563.0, 150.0, 860.0), 0.0),
}


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
    p.add_argument("--size", default="2560x1440")
    p.add_argument("--taa", default="true")
    p.add_argument("--spot", default="mountain")
    p.add_argument("--lod-wait", type=float, default=150.0)
    p.add_argument("--fly-seconds", type=float, default=8.0)
    p.add_argument("--workers", default="0")
    p.add_argument("--cpus", help="confine the game to these CPUs (taskset list)")
    a = p.parse_args()
    build = Path(a.build).resolve()
    TMP.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(dir=TMP, prefix="h-"))
    scratch = {k: tempfile.mkdtemp(dir=TMP, prefix=f"{k[:3]}-") for k in
               ("HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME")}
    env = dict(os.environ, HORIZON_HOME=str(home), HORIZON_NO_AUDIO="1", HORIZON_NO_DISCORD="1",
               HORIZON_LAUNCHER_NO_SELF_UPDATE="1", HORIZON_TELEMETRY_URL="off", HORIZON_SPIKES="0", HORIZON_SHADER_PACK_TIMING="1", **scratch)
    (home / "options.txt").write_text("version:4790\nenableVsync:false\nmaxFps:260\nrenderDistance:32\n"
                                      "simulationDistance:12\nfov:0.0\nguiScale:0\ninactivityFpsLimit:minimized\n")
    (home / "horizon-graphics.txt").write_text(
        f"preset:custom\nlodDistance:256\nfarDetail:full\nmeshShaders:on\ntaa:{a.taa}\nfxaa:false\n"
        f"presentMode:immediate\noverlay:off\nantiLag:false\nworkers:{a.workers}\n")
    os.chdir(home)
    sock = home / "control.sock"
    w, h = a.size.split("x")
    OUT.mkdir(parents=True, exist_ok=True)
    start, end, pitch = SPOTS[a.spot]
    game = str(build / "horizon-client")
    if a.cpus:
        env["WRAP_BIN"], env["WRAP_CPUS"] = game, a.cpus
        game = str(HERE / "taskset-wrap.sh")
    ctl(build, sock, env, "launch", "--world", "LodAB", "--seed", "1234", "--creative", "--render-distance", "32",
        "--resources", str(RES), "--no-pause", "--weather", "clear", "--time", "6000",
        "--game", game, "--width", w, "--height", h, "--offscreen")
    result = {"label": a.label, "build": str(build), "size": a.size, "taa": a.taa, "spot": a.spot}
    hover = None
    try:
        ctl(build, sock, env, "wait-ready", 300)
        if a.spot in ("ground", "valley"):
            # Two blocks over the surface at the spot: the highest non-air block.
            ctl(build, sock, env, "tp", start[0], 320.0, start[2])
            ctl(build, sock, env, "wait-ready", 300)
            top = None
            for y in range(320, 40, -1):
                b = ctl(build, sock, env, "block", int(start[0]), y, int(start[2]))
                name = str(b.get("block") or b.get("id") or b.get("state") or b)
                if y % 40 == 0 or top is None and "air" not in name:
                    print("  probe", y, name[:80], flush=True)
                if "air" not in name:
                    top = y
                    break
            print("ground at", top, flush=True)
            start = (start[0], float((top or 70) + 3), start[2])
            end = (end[0], start[1], end[2])
        ctl(build, sock, env, "tp", *start)
        ctl(build, sock, env, "look", 0, pitch)
        ctl(build, sock, env, "time", 6000)
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "lod-distance", 256)
        # Hold the camera in the air: a very slow flight over the whole wait.
        hover = subprocess.Popen([str(build / "horizon-ctl"), "--socket", str(sock), "fly", *map(str, start),
                                  *map(str, end), str(a.lod_wait), str(pitch)], env=env,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        t0, stable = time.time(), 0
        while time.time() - t0 < a.lod_wait:
            s = ctl(build, sock, env, "status")
            lod = s.get("lod") or {}
            done = lod.get("target_tiles", 0) > 0 and lod.get("sampled_target_tiles", 0) >= lod.get("target_tiles", 0)
            stable = stable + 1 if done and s.get("pending_uploads", 1) == 0 else 0
            if stable >= 5 and time.time() - t0 > 60:
                break
            time.sleep(2)
        result["lod"] = lod
        print(f"lod {lod.get('sampled_target_tiles')}/{lod.get('target_tiles')} quads {lod.get('resident_quads')} after {time.time()-t0:.0f}s", flush=True)
        shots = {}
        for step, mode in enumerate(["on", "off", "on"]):
            r = ctl(build, sock, env, "mesh-shaders", mode)
            time.sleep(1.0)
            st = ctl(build, sock, env, "status")
            png = OUT / f"{a.label}-{step}-{mode}.png"
            ctl(build, sock, env, "screenshot", png)
            time.sleep(0.5)
            shots[f"{step}-{mode}"] = {"toggle": r, "renderer": {k: st.get("renderer", {}).get(k) for k in
                                       ("mesh_shaders", "mesh_tiles", "mesh_culled", "mesh_counts", "drawn_sections", "section_draws")},
                                       "fps": st.get("fps"), "png": str(png)}
            print(f"{step} {mode}: {shots[f'{step}-{mode}']}", flush=True)
        result["shots"] = shots
        # Frame stats, mesh on and off, over the same short flight, once the
        # hover's own flight (the game's benchmark) is over.
        hover.wait()
        hover = None
        stats = {}
        # Alternating, so tile refinement still settling lands on both alike.
        ctl(build, sock, env, "gpu-passes", "on")
        for i, mode in enumerate(["off", "on", "off", "on"]):
            ctl(build, sock, env, "mesh-shaders", mode)
            ctl(build, sock, env, "tp", *start)
            time.sleep(1.0)
            summ = ctl(build, sock, env, "fly", *start, *end, a.fly_seconds, pitch)
            stats[f"{i}-{mode}"] = summ
            print(f"fly {i} mesh {mode}: {summ.get('avg_fps',0):7.1f} fps  1%low {summ.get('low_1pct_fps',0):7.1f}"
                  f"  cpu {((summ.get('cpu') or {}).get('avg_ms') or 0):6.2f}  gpu {((summ.get('gpu') or {}).get('avg_ms') or 0):6.2f}ms"
                  + "  [" + " ".join(f"{k} {v:.2f}" for k, v in (summ.get("phases_avg_ms") or {}).items()) + "]", flush=True)
            st = ctl(build, sock, env, "status")
            print(f"   counts {st.get('renderer', {}).get('mesh_counts')}", flush=True)
            passes = ctl(build, sock, env, "stats").get("gpu_passes") or {}
            if passes:
                print("   gpu passes: " + " ".join(f"{k} {v:.2f}" for k, v in sorted(passes.items(), key=lambda kv: -kv[1])), flush=True)
        result["fly"] = stats
    finally:
        if hover:
            hover.terminate()
        ctl(build, sock, env, "quit", check=False)
        for _ in range(100):
            if not sock.exists():
                break
            time.sleep(0.2)
        log = home / "logs" / "latest.log"
        if log.exists():
            shutil.copy(log, OUT / f"{a.label}.log")
        (OUT / f"{a.label}.json").write_text(json.dumps(result, indent=1))
        os.chdir(BASE)
        shutil.rmtree(home, ignore_errors=True)
        for d in scratch.values():
            shutil.rmtree(d, ignore_errors=True)


main()
