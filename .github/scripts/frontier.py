"""Time candidate images the way the scorer runs them, on one runner, and check every run.

Env: WORK (datasets + images.json), ROUNDS (measured repeats), SIZES (e.g. "s3 s4").
images.json: {variant_name: {"image": tag, "env": {"K": "V"}}}; the first variant is the gold reference.
Prints CHECKED / FRONTIER lines and writes WORK/result.json.
Needs Python 3.11+ (Docker timestamps carry nanoseconds) and polars.
"""
import json
import os
import random
import shutil
import statistics
import subprocess
import tempfile
from datetime import datetime
from pathlib import Path

import polars as pl

work = Path(os.environ["WORK"])
rounds = int(os.environ.get("ROUNDS", "5"))
sizes = os.environ.get("SIZES", "s3 s4").split()
workers = len(os.sched_getaffinity(0))          # the runner's vCPUs (4 on a hosted runner)
variants = json.loads((work / "images.json").read_text())
names = list(variants)


def docker(*args, capture=False):
    cmd = ["docker", *map(str, args)]
    if capture:
        return subprocess.check_output(cmd, text=True).strip()
    subprocess.run(cmd, check=True)


def stamp(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def run_once(variant, data, out):
    """One container run with the scorer's flags; returns start-to-exit seconds on Docker's clock."""
    out.mkdir(mode=0o777)
    out.chmod(0o777)
    scratch = out.parent / (out.name + "-scratch")
    scratch.mkdir(mode=0o777)
    scratch.chmod(0o777)
    env = []
    for key, value in variants[variant]["env"].items():
        env += ["-e", f"{key}={value}"]
    cid = docker(
        "create", "--network", "none", "--read-only", "--tmpfs", "/tmp:rw,size=64m,mode=1777",
        "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "4096",
        "--memory=12g", "--memory-swap=12g", "--user", "1000:1000", "--cpus", workers,
        "-e", "DATA_DIR=/data", "-e", "OUTPUT_DIR=/output", "-e", "SCRATCH_DIR=/scratch", "-e", "HOME=/scratch",
        "-e", f"NCPU={workers}", "-e", "MEM_GB=12", *env,
        "-v", f"{data}:/data:ro", "-v", f"{out}:/output", "-v", f"{scratch}:/scratch",
        variants[variant]["image"], capture=True)
    try:
        docker("start", "-a", cid)
        code, start, end = docker("inspect", "-f", "{{.State.ExitCode}} {{.State.StartedAt}} {{.State.FinishedAt}}",
                                  cid, capture=True).split()
        if code != "0":
            raise RuntimeError(f"{variant} exited with {code}")
        return (stamp(end) - stamp(start)).total_seconds()
    finally:
        docker("rm", cid, capture=True)
        shutil.rmtree(scratch, ignore_errors=True)


def same(a, b, table, keys):
    x = pl.read_parquet(a / table / "*.parquet").sort(keys)
    y = pl.read_parquet(b / table / "*.parquet").sort(keys)
    if x.height != y.height or set(x.columns) != set(y.columns) or not x.equals(y.select(x.columns)):
        raise ValueError(f"{table} differs ({x.height} vs {y.height} rows)")


result = {"workers": workers, "lscpu": subprocess.check_output(["lscpu"], text=True), "variants": variants, "data": {}}
failed_any = []
for size in sizes:
    data = work / size
    with tempfile.TemporaryDirectory(dir=work) as tmp:
        root = Path(tmp)
        gold = root / "gold"
        run_once(names[0], data, gold)                      # reference output; also warms the page cache
        times = {n: [] for n in names}
        failed = {}
        cycle = list(names)
        random.Random(7).shuffle(cycle)
        for it in range(rounds + 1):                         # iteration 0 is a discarded warm-up
            order = cycle[it % len(cycle):] + cycle[:it % len(cycle)]
            for n in order:
                if n in failed:
                    continue
                out = root / f"t{it}-{n}"
                try:
                    seconds = run_once(n, data, out)
                    same(gold, out, "user_daily", ["user_id", "day"])
                    same(gold, out, "campaign_daily", ["campaign_id", "day"])
                    if it:
                        times[n].append(seconds)
                    print(f"CHECKED {size} {n} trial={it} seconds={seconds:.5f}", flush=True)
                except Exception as exc:                     # a mismatch disqualifies the variant for this size
                    failed[n] = str(exc)
                    print(f"VALIDATION_FAILED {size} {n}: {exc}", flush=True)
                finally:
                    shutil.rmtree(out, ignore_errors=True)
        for n, v in times.items():
            if n not in failed and v:
                print(f"FRONTIER {size} {n} median={statistics.median(v):.5f} min={min(v):.5f} samples={v}", flush=True)
        result["data"][size] = {"times": times, "failures": failed}
        (work / "result.json").write_text(json.dumps(result, indent=2))
        failed_any += [(size, n, m) for n, m in failed.items()]
if failed_any:
    raise SystemExit(f"validation failures: {failed_any}")
