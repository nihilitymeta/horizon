#!/usr/bin/env python3
"""LOD streaming comparison: teleport to a fresh spot, LOD 256 / rd 32, then sample the LOD stats
and take a screenshot at fixed times. Usage: lodload.py --build DIR --label NAME --mesh on|off"""
import argparse, json, os, shutil, subprocess, tempfile, time
from pathlib import Path

BASE = Path(os.environ.get("HORIZON_BENCH_BASE", Path.home() / "dev/horizon-wt"))
HERE = Path(__file__).resolve().parent
RES, OUT, TMP = BASE / "bench-data/26.3", BASE / "bench-results", BASE / "tmp"
SHOTS = [8, 20, 40, 70, 110]  # seconds after the LOD distance is set


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
    p.add_argument("--size", default="1920x1080")
    a = p.parse_args()
    build = Path(a.build).resolve()
    TMP.mkdir(parents=True, exist_ok=True)
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
    OUT.mkdir(parents=True, exist_ok=True)
    ctl(build, sock, env, "launch", "--world", "LodLoad", "--seed", "1234", "--creative", "--render-distance", "32",
        "--resources", str(RES), "--no-pause", "--weather", "clear", "--time", "6000",
        "--game", str(build / "horizon-client"), "--width", w, "--height", h, "--offscreen")
    samples = []
    try:
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "tp", 560.0, 260.0, 860.0)
        ctl(build, sock, env, "look", 0, 4)
        ctl(build, sock, env, "time", 6000)
        ctl(build, sock, env, "wait-ready", 300)
        ctl(build, sock, env, "lod-distance", 256)
        # hold the camera in the air: a very slow flight for the whole sampling window
        hover = subprocess.Popen([str(build / "horizon-ctl"), "--socket", str(sock), "fly", "560", "260", "860",
                                  "563", "260", "860", str(SHOTS[-1] + 10), "4"], env=env,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        t0, shots = time.time(), list(SHOTS)
        while time.time() - t0 < SHOTS[-1] + 2:
            s = ctl(build, sock, env, "status")
            lod, r = s.get("lod") or {}, s.get("renderer") or {}
            t = time.time() - t0
            samples.append({"t": round(t, 1), "fps": s.get("fps"), "active_tiles": lod.get("active_tiles"),
                            "resident_quads": lod.get("resident_quads"), "sampled": lod.get("sampled_target_tiles"),
                            "target": lod.get("target_tiles"), "levels": lod.get("levels"),
                            "pending_obs": lod.get("pending_observations"), "refined": lod.get("refined_samples"),
                            "pending_uploads": s.get("pending_uploads")})
            if shots and t >= shots[0]:
                png = OUT / f"{a.label}-t{shots.pop(0):03d}.png"
                ctl(build, sock, env, "screenshot", png, check=False)
            time.sleep(2)
    finally:
        try:
            hover.terminate()
        except Exception:
            pass
        ctl(build, sock, env, "quit", check=False)
        for _ in range(100):
            if not sock.exists():
                break
            time.sleep(0.2)
        (OUT / f"{a.label}-load.json").write_text(json.dumps(samples, indent=1))
        os.chdir(BASE)
        shutil.rmtree(home, ignore_errors=True)
        for d in scratch.values():
            shutil.rmtree(d, ignore_errors=True)
    for s in samples[::4]:
        print(s["t"], "active", s["active_tiles"], "quads", s["resident_quads"], "sampled", s["sampled"], "/", s["target"],
              "pend_obs", s["pending_obs"], "refined", s["refined"], "fps", s["fps"])


main()
