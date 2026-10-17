"""Soak test: run `clippy replay` for a long time and watch memory/handles/threads.

    python scripts/soak.py --res 1440p --fps 120 --minutes 15 --out docs/perf/soak-1440p120.csv

Samples every --every seconds (working set, private bytes, handles, threads) and
fails (exit 1) if working set or handles keep growing after the ring has filled:
last-third average vs middle-third average, more than 10 % / 25 handles apart.
Needs something moving on the captured monitor.
"""
import argparse, csv, os, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "clippy.exe")


def sample(pid):
    cmd = (f"$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; if ($p) {{ '{{0}},{{1}},{{2}},{{3}}' -f "
           "$p.WorkingSet64, $p.PrivateMemorySize64, $p.HandleCount, $p.Threads.Count }")
    out = subprocess.run(["powershell", "-NoProfile", "-Command", cmd], capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    ws, priv, handles, threads = (int(x) for x in out.split(","))
    return ws / 1048576, priv / 1048576, handles, threads


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--res", default="1080p")
    ap.add_argument("--fps", type=int, default=60)
    ap.add_argument("--minutes", type=float, default=15)
    ap.add_argument("--every", type=int, default=30)
    ap.add_argument("--out", default=os.path.join(ROOT, "docs", "perf", "soak.csv"))
    args = ap.parse_args()
    secs = int(args.minutes * 60)
    tmp = tempfile.mkdtemp(prefix="clippy-soak-")
    appdata = os.path.join(tmp, "appdata")
    os.makedirs(os.path.join(appdata, "clippy"))
    clips = os.path.join(tmp, "clips")
    os.makedirs(clips)
    open(os.path.join(appdata, "clippy", "config.toml"), "w").write(
        f'fps = {args.fps}\nresolution = "{args.res}"\nclip_dir = {clips!r}\nclip_sound = false\ndiscord_presence = false\n')
    log = open(os.path.join(tmp, "run.log"), "w")
    p = subprocess.Popen([EXE, "replay", "-v", "--auto-clip", str(secs - 10), "--exit-after", str(secs)],
                         env=dict(os.environ, APPDATA=appdata), stdout=log, stderr=subprocess.STDOUT)
    rows = []
    t0 = time.time()
    while p.poll() is None:
        time.sleep(args.every)
        s = sample(p.pid)
        if s:
            rows.append((round(time.time() - t0), *s))
            print(f"t={rows[-1][0]:5d}s ws={s[0]:7.1f} MB private={s[1]:7.1f} MB handles={s[2]} threads={s[3]}", flush=True)
    with open(args.out, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["t_s", "ws_mb", "private_mb", "handles", "threads"])
        w.writerows(rows)
    log.close()
    text = open(os.path.join(tmp, "run.log"), errors="replace").read()
    closing = [l for l in text.splitlines() if "replay session closing" in l]
    print(closing[-1][-400:] if closing else "no closing line")
    if len(rows) < 6:
        print("too few samples")
        return 1
    n = len(rows)
    mid = rows[n // 3: 2 * n // 3]
    last = rows[2 * n // 3:]
    avg = lambda rs, i: sum(r[i] for r in rs) / len(rs)
    ws_growth = (avg(last, 1) - avg(mid, 1)) / avg(mid, 1)
    handle_growth = avg(last, 3) - avg(mid, 3)
    print(f"working set growth middle->last third: {ws_growth * 100:+.1f} %, handles {handle_growth:+.1f}")
    ok = ws_growth <= 0.10 and handle_growth <= 25
    print("SOAK PASSED" if ok else "SOAK FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
