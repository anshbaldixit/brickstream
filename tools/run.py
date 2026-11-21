#!/usr/bin/env python3
"""Run a Docker image the way the Shopping Sessions scorer does, and time it.

Usage (from the repo root):
  .venv/Scripts/python.exe tools/run.py IMAGE DATA OUT [--repeat N] [--cpus 8] [--mem 24g]

  DATA  a host folder, or vol:NAME for a Docker named volume (mounted read-only at /data)
  OUT   a host folder (emptied first; gets user_daily/ and campaign_daily/),
        or vol:NAME (recreated each run; faster for timing big datasets, copy out with tools/fetch.py)

Timing comes from Docker's own StartedAt/FinishedAt for the container, the same span the
scorer times (container start to exit), so docker CLI overhead is not counted.
For big datasets use volumes for DATA and OUT: Windows bind mounts are slow and distort timings.

Also works as the command for the organizers' fixture runner, which appends DATA OUT:
  .venv/Scripts/python.exe kit/starter-duckdb/starter-duckdb/scripts/run_fixtures.py \
      kit/starter-duckdb/starter-duckdb/fixtures -- .venv/Scripts/python.exe tools/run.py IMAGE
"""
import argparse
import datetime as dt
import json
import shutil
import statistics
import subprocess
import sys
import uuid
from pathlib import Path

SCRATCH_VOLUME = "ss-scratch"
HELPER_IMAGE = "busybox:1.36"


def docker(*args, check=True):
    return subprocess.run(["docker", *args], check=check, text=True, capture_output=True)


def mount(spec, target, readonly=False):
    if spec.startswith("vol:"):
        value = f"type=volume,src={spec[4:]},dst={target}"
    else:
        value = f"type=bind,src={Path(spec).resolve()},dst={target}"
    return ["--mount", value + (",readonly" if readonly else "")]


def fresh_volume(name):
    """Recreate a named volume, empty and owned by uid 1000 like the scorer's writable dirs."""
    docker("volume", "rm", "-f", name, check=False)
    docker("volume", "create", name)
    docker("run", "--rm", *mount(f"vol:{name}", "/v"), HELPER_IMAGE, "chown", "1000:1000", "/v")


def fresh_output(spec):
    if spec.startswith("vol:"):
        fresh_volume(spec[4:])
    else:
        path = Path(spec)
        if path.exists():
            shutil.rmtree(path)
        path.mkdir(parents=True)


def parse_time(stamp):
    # Docker gives RFC 3339 with nanoseconds, e.g. 2026-11-21T00:00:01.123456789Z
    stamp = stamp.rstrip("Z")
    if "." in stamp:
        whole, fraction = stamp.split(".", 1)
        stamp = f"{whole}.{fraction[:6].ljust(6, '0')}"
    return dt.datetime.fromisoformat(stamp)


def run_once(image, data, out, cpus, mem):
    fresh_output(out)
    fresh_volume(SCRATCH_VOLUME)
    name = f"ss-run-{uuid.uuid4().hex[:10]}"
    mem_gb = mem.lower().rstrip("g")
    command = [
        "docker", "run", "--name", name, "--platform", "linux/amd64",
        "--network", "none", "--read-only", "--tmpfs", "/tmp:rw,size=64m,mode=1777",
        "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
        "--pids-limit", "4096", "--cpus", str(cpus), "--memory", mem, "--memory-swap", mem,
        "--user", "1000:1000",
        *mount(data, "/data", readonly=True), *mount(out, "/output"), *mount(f"vol:{SCRATCH_VOLUME}", "/scratch"),
        "-e", "DATA_DIR=/data", "-e", "OUTPUT_DIR=/output", "-e", "SCRATCH_DIR=/scratch", "-e", "HOME=/scratch",
        "-e", f"NCPU={int(float(cpus))}", "-e", f"MEM_GB={mem_gb}",
        image,
    ]
    # Container stdout goes to our stderr so callers' stdout stays clean.
    subprocess.run(command, stdout=sys.stderr, stderr=sys.stderr)
    state = json.loads(docker("inspect", name).stdout)[0]["State"]
    docker("rm", "-f", name, check=False)
    seconds = (parse_time(state["FinishedAt"]) - parse_time(state["StartedAt"])).total_seconds()
    return state["ExitCode"], state.get("OOMKilled", False), seconds


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("image")
    parser.add_argument("data")
    parser.add_argument("out")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--cpus", default="8")
    parser.add_argument("--mem", default="24g")
    args = parser.parse_args()

    times = []
    for attempt in range(1, args.repeat + 1):
        code, oom, seconds = run_once(args.image, args.data, args.out, args.cpus, args.mem)
        print(f"run {attempt}: exit {code}{' OOM-KILLED' if oom else ''}, {seconds:.3f} s", file=sys.stderr)
        if code != 0 or oom:
            return code or 137
        times.append(seconds)
    if args.repeat > 1:
        print(f"median of {len(times)}: {statistics.median(times):.3f} s  (runs: {', '.join(f'{t:.3f}' for t in times)})")
    else:
        print(f"time: {times[0]:.3f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
