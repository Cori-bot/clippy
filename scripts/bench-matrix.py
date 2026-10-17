"""Matrix benchmark: resolution x fps through `clippy replay`, one CSV row per run.

    python scripts/bench-matrix.py --res 480p,720p,1080p,1440p --fps 30,60,90,120 --runs 1 --out docs/perf/matrix.csv

Each run uses a throw-away %APPDATA% (its own config.toml and clip folder), starts
`clippy replay --auto-clip C --exit-after N`, samples GPU load with `nvidia-smi dmon`
and the process CPU time with GetProcessTimes, then parses the engine's own log lines
(closing `perf` line, `clip saved`, `clip snapshot lock`) and reads the produced MP4
(box parser: real width/height/frame count/duration). Keep something moving on the
captured monitor (e.g. a looping video) or frames will not arrive.
"""
import argparse, csv, ctypes, os, re, shutil, struct, subprocess, sys, tempfile, time
from ctypes import wintypes

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "clippy.exe")

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
k32.OpenProcess.restype = wintypes.HANDLE
k32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4


def cpu_seconds(pid):
    h = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return None
    try:
        c, e, k, u = (wintypes.FILETIME() for _ in range(4))
        if not k32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e), ctypes.byref(k), ctypes.byref(u)):
            return None
        ft = lambda f: ((f.dwHighDateTime << 32) | f.dwLowDateTime) / 1e7
        return ft(k) + ft(u)
    finally:
        k32.CloseHandle(h)


def probe_mp4(path):
    """Width, height, sample count and duration of the first video track."""
    data = open(path, "rb").read()

    def boxes(buf, start, end):
        i = start
        while i + 8 <= end:
            size, typ = struct.unpack(">I4s", buf[i:i + 8])
            hdr = 8
            if size == 1:
                size = struct.unpack(">Q", buf[i + 8:i + 16])[0]
                hdr = 16
            elif size == 0:
                size = end - i
            yield typ.decode("latin1"), i + hdr, i + size
            i += size

    def find(buf, start, end, path):
        for typ, s, e in boxes(buf, start, end):
            if typ == path[0]:
                if len(path) == 1:
                    return s, e
                r = find(buf, s, e, path[1:])
                if r:
                    return r
        return None

    out = {}
    moov = find(data, 0, len(data), ["moov"])
    if not moov:
        return out
    for typ, s, e in boxes(data, *moov):
        if typ != "trak":
            continue
        hdlr = find(data, s, e, ["mdia", "hdlr"])
        if not hdlr or data[hdlr[0] + 8:hdlr[0] + 12] != b"vide":
            continue
        tkhd = find(data, s, e, ["tkhd"])
        ver = data[tkhd[0]]
        off = tkhd[0] + (4 + 8 + 4 + 4 + 4 + 8 + 2 + 2 + 2 + 2 + 36 if ver == 0 else 4 + 16 + 4 + 4 + 8 + 8 + 2 + 2 + 2 + 2 + 36)
        w, h = struct.unpack(">II", data[off:off + 8])
        out["mp4_w"], out["mp4_h"] = w >> 16, h >> 16
        stsz = find(data, s, e, ["mdia", "minf", "stbl", "stsz"])
        out["mp4_frames"] = struct.unpack(">I", data[stsz[0] + 8:stsz[0] + 12])[0]
        mdhd = find(data, s, e, ["mdia", "mdhd"])
        v = data[mdhd[0]]
        if v == 0:
            ts, dur = struct.unpack(">II", data[mdhd[0] + 12:mdhd[0] + 20])
        else:
            ts, dur = struct.unpack(">IQ", data[mdhd[0] + 20:mdhd[0] + 32])
        out["mp4_seconds"] = round(dur / ts, 2) if ts else 0
        out["mp4_fps"] = round(out["mp4_frames"] / out["mp4_seconds"], 1) if out["mp4_seconds"] else 0
        break
    return out


def parse_log(text):
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    r = {}
    m = re.search(r"replay session closing (.*)", text)
    if m:
        line = m.group(1)
        for k in ("ws_mb", "gpu_dedicated_mb", "gpu_shared_mb", "cpu_ring_mb", "frames", "dropped", "paced", "ring_packets"):
            mm = re.search(rf"\b{k}=([\d.]+)", line)
            if mm:
                r[k] = float(mm.group(1))
        for stage, key in (("latency", "on_frame"), ("pump", "pump"), ("convert", "convert"), ("encode", "encode")):
            mm = re.search(rf"\b{stage}=n=(\d+) p50<=([\d.]+)(us|ms) p99<=([\d.]+)(us|ms) max=([\d.]+)(us|ms)", line)
            if mm:
                f = lambda v, u: float(v) * (1.0 if u == "ms" else 0.001)
                r[f"{key}_p99_ms"] = f(mm.group(4), mm.group(5))
                r[f"{key}_max_ms"] = f(mm.group(6), mm.group(7))
    m = re.search(r"clip saved: (.+?\.mp4) \(([\d.]+) s video / ([\d.]+) s audio, (\d+) packets, ([\d.]+) MB, muxed in (\d+) ms\)", text)
    if m:
        r.update(clip_path=m.group(1), clip_s=float(m.group(2)), clip_packets=int(m.group(4)), clip_mb=float(m.group(5)), mux_ms=int(m.group(6)))
    m = re.search(r"lock_wait_us=(\d+) lock_held_us=(\d+)", text)
    if m:
        r["lock_held_ms"] = int(m.group(2)) / 1000
    m = re.search(r"replay buffer running: (\d+)x(\d+) at (\d+) fps, (\d+) kbps", text)
    if m:
        r["enc_w"], r["enc_h"], r["kbps"] = int(m.group(1)), int(m.group(2)), int(m.group(4))
    r["errors"] = len(re.findall(r"\b(ERROR|WARN)\b", text))
    return r


def run_one(res, fps, args, extra_cfg):
    tmp = tempfile.mkdtemp(prefix="clippy-bench-")
    try:
        appdata = os.path.join(tmp, "appdata")
        clips = os.path.join(tmp, "clips")
        os.makedirs(os.path.join(appdata, "clippy"))
        os.makedirs(clips)
        cfg = f'fps = {fps}\nresolution = "{res}"\nreplay_seconds = {args.replay}\nclip_dir = {clips!r}\nstats_seconds = 0\nclip_sound = false\ndiscord_presence = false\n{extra_cfg}'
        open(os.path.join(appdata, "clippy", "config.toml"), "w").write(cfg)
        env = dict(os.environ, APPDATA=appdata)
        log = os.path.join(tmp, "run.log")
        dmon = subprocess.Popen(["nvidia-smi", "dmon", "-s", "u", "-d", "1"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        t0 = time.time()
        with open(log, "w") as lf:
            p = subprocess.Popen([EXE, "replay", "-v", "--auto-clip", str(args.clip_at), "--exit-after", str(args.seconds)], env=env, stdout=lf, stderr=subprocess.STDOUT)
            last_cpu = 0.0
            while p.poll() is None:
                c = cpu_seconds(p.pid)
                if c is not None:
                    last_cpu = c
                time.sleep(0.5)
                if time.time() - t0 > args.seconds + 60:
                    p.kill()
        wall = time.time() - t0
        dmon.terminate()
        rows = [l.split() for l in dmon.stdout.read().splitlines() if l.strip() and not l.startswith("#")]
        sm = [float(r[1]) for r in rows if len(r) >= 5]
        enc = [float(r[3]) for r in rows if len(r) >= 5]
        # skip the first 3 s of start-up and the last 2 s of teardown
        sm, enc = sm[3:-2] or sm, enc[3:-2] or enc
        out = parse_log(open(log, errors="replace").read())
        out.update(res=res, fps_target=fps, wall_s=round(wall, 1), cpu_pct=round(100 * last_cpu / wall, 1) if wall else 0,
                   gpu_sm_avg=round(sum(sm) / len(sm), 1) if sm else None, gpu_enc_avg=round(sum(enc) / len(enc), 1) if enc else None,
                   gpu_sm_max=max(sm) if sm else None)
        if out.get("frames"):
            out["fps_eff"] = round(out["frames"] / max(1.0, args.seconds - 1.0), 1)
        clip = out.get("clip_path")
        if clip and os.path.exists(clip):
            out.update(probe_mp4(clip))
        return out
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


FIELDS = ["res", "fps_target", "enc_w", "enc_h", "kbps", "frames", "fps_eff", "dropped", "paced", "on_frame_p99_ms", "on_frame_max_ms",
          "pump_p99_ms", "convert_p99_ms", "encode_p99_ms", "encode_max_ms", "cpu_pct", "gpu_sm_avg", "gpu_sm_max", "gpu_enc_avg", "ws_mb",
          "cpu_ring_mb", "ring_packets", "clip_s", "clip_mb", "mux_ms", "lock_held_ms", "mp4_w", "mp4_h", "mp4_fps", "mp4_seconds", "errors"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--res", default="480p,720p,1080p,1440p")
    ap.add_argument("--fps", default="30,60,90,120")
    ap.add_argument("--runs", type=int, default=1)
    ap.add_argument("--seconds", type=int, default=30)
    ap.add_argument("--clip-at", type=int, default=20)
    ap.add_argument("--replay", type=int, default=15)
    ap.add_argument("--cfg", default="", help="extra config.toml lines, e.g. 'capture_method = \"dd\"'")
    ap.add_argument("--out", default=os.path.join(ROOT, "docs", "perf", "matrix.csv"))
    args = ap.parse_args()
    new = not os.path.exists(args.out)
    with open(args.out, "a", newline="") as f:
        w = csv.DictWriter(f, fieldnames=FIELDS, extrasaction="ignore")
        if new:
            w.writeheader()
        for res in args.res.split(","):
            for fps in (int(x) for x in args.fps.split(",")):
                for i in range(args.runs):
                    r = run_one(res, fps, args, args.cfg.replace("\\n", "\n"))
                    w.writerow(r)
                    f.flush()
                    print(f"{res}@{fps} #{i + 1}: " + ", ".join(f"{k}={r.get(k)}" for k in
                          ("enc_w", "enc_h", "frames", "fps_eff", "dropped", "on_frame_p99_ms", "cpu_pct", "gpu_enc_avg", "ws_mb", "mp4_w", "mp4_h", "mp4_fps")), flush=True)


if __name__ == "__main__":
    sys.exit(main())
