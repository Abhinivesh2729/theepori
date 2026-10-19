#!/usr/bin/env python3
"""M0 perf harness (SF-20): measures release builds on this Mac.

  tpd cold start to first answered request, IPC round trip p50/p95,
  desktop process start to first paint (cold + warm),
  and checks that closing the desktop app leaves tpd running.

Usage: scripts/perf.py [--runs N] [--out docs/perf/m0.json]
Build first: cargo build --release -p tpd -p tp-cli && (cd apps/desktop && npx tauri build)
"""
import argparse, json, os, platform, shutil, signal, statistics, subprocess, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REL = ROOT / "target" / "release"
TPD, TP, APP = REL / "tpd", REL / "tp", REL / "theepori-desktop"


def pct(xs, p):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(round((len(xs) - 1) * p)))], 2)


def tp(factory, *args):
    return subprocess.run([TP, "--factory", factory, *args], capture_output=True, text=True)


def wait_ready(factory, timeout=10):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if tp(factory, "daemon", "status", "--json").returncode == 0:
            return True
        time.sleep(0.005)
    return False


def tpd_cold_start(factory):
    t = time.monotonic()
    proc = subprocess.Popen([TPD, "--factory", factory], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sock = Path(factory) / "tpd.sock"
    while not sock.exists():
        time.sleep(0.001)
    assert wait_ready(factory), "tpd did not answer"
    return proc, (time.monotonic() - t) * 1000


def desktop_ready(factory, out):
    env = dict(os.environ, TP_FACTORY=factory, TP_PERF_OUT=str(out), TP_PERF_EXIT="1")
    out.unlink(missing_ok=True)
    proc = subprocess.Popen([APP], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        raise SystemExit("desktop app did not report ready within 30s")
    return json.loads(out.read_text())["ready_ms"]


def machine():
    def sh(*a):
        return subprocess.run(a, capture_output=True, text=True).stdout.strip()
    return {
        "model": sh("sysctl", "-n", "hw.model"),
        "cpu": sh("sysctl", "-n", "machdep.cpu.brand_string"),
        "memory_gb": round(int(sh("sysctl", "-n", "hw.memsize") or 0) / 2**30),
        "macos": platform.mac_ver()[0],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--out", default=str(ROOT / "docs" / "perf" / "m0.json"))
    args = ap.parse_args()
    for b in (TPD, TP, APP):
        if not b.exists():
            raise SystemExit(f"{b} missing; build release binaries first (see --help)")

    scratch = Path(tempfile.mkdtemp(prefix="tpperf", dir="/tmp"))
    factory = str(scratch / "f")
    tp(factory, "setup")
    try:
        starts, procs = [], None
        for _ in range(args.runs):
            proc, ms = tpd_cold_start(factory)
            starts.append(ms)
            proc.send_signal(signal.SIGTERM)
            proc.wait(timeout=10)
        proc, _ = tpd_cold_start(factory)
        bench = json.loads(tp(factory, "daemon", "bench", "-n", "2000", "--json").stdout)

        ready = [desktop_ready(factory, scratch / "ready.json") for _ in range(args.runs)]

        # Closing the GUI must not stop tpd.
        gui = subprocess.Popen([APP], env=dict(os.environ, TP_FACTORY=factory),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(2)
        gui.terminate()
        gui.wait(timeout=10)
        survived = tp(factory, "daemon", "status", "--json").returncode == 0
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=10)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    report = {
        "milestone": "M0",
        "measured_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "machine": machine(),
        "build": "release",
        "tpd_cold_start_ms": {"runs": [round(x, 1) for x in starts], "p50": pct(starts, .5), "p95": pct(starts, .95)},
        "ipc_status_roundtrip_ms": {k: round(v, 3) for k, v in bench.items() if k.endswith("_ms")} | {"n": bench["n"]},
        "desktop_first_paint_ms": {"cold": round(ready[0], 1), "warm_runs": [round(x, 1) for x in ready[1:]],
                                   "warm_p95": pct(ready[1:] or ready, .95)},
        "gui_close_leaves_tpd_running": survived,
        "targets": {"warm_start_p95_ms": 500, "cold_start_ms": 1000, "interaction_p95_ms": 50},
    }
    report["within_targets"] = (
        report["desktop_first_paint_ms"]["warm_p95"] <= 500
        and report["desktop_first_paint_ms"]["cold"] <= 1000
        and report["ipc_status_roundtrip_ms"]["p95_ms"] <= 50
        and survived
    )
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    raise SystemExit(0 if report["within_targets"] else 1)


if __name__ == "__main__":
    main()
