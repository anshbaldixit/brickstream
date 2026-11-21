#!/usr/bin/env python3
"""Dev loop: build and test the Rust pipeline inside a persistent Linux container.

Run from the repo root with the venv's Python, e.g. `.venv/Scripts/python.exe fast/dev.py fx`.

  up                       build the dev image if missing (--rebuild to force) and start container ss-fast-dev
  build [--debug]          cargo build --release (or a debug build)
  test [ARGS...]           cargo test --release [ARGS...]
  fx [--intern]            kit fixtures, then tests/fixtures-extra (if it has cases)
  dev [--intern]           data/dev -> out/fast-dev[-intern], compared with kit/dev-answer
  seeds [--intern]         data/s*-1m -> out/fast-<seed>[-intern], compared with out/oracle-<seed> when present
                           (with --intern also compared with the normal-mode output)
  oracle [SEED...]         docs/tools/oracle.py (reference oracle) on the 1M seeds -> out/oracle-<seed> (slow, parallel)
  run DATA OUT [--intern]  run the binary on container paths (/work/..., /vol/...)
  image [--no-cache]       docker build -f fast/Dockerfile -t ss:fast . (timed)
  sh CMD...                any command inside the container (workdir /work/fast)

Inside the container: the repo is /work, the timing volumes are /vol/s3-20m and /vol/s4-60m (read-only),
/vol/out is a scratch volume for big outputs, and the binary is /target/release/fast.
Paths are passed to docker as list arguments, so the apostrophe in the repo path is safe.
"""
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NAME = "ss-fast-dev"
IMAGE = "ss-fast-dev"
BIN = os.environ.get("FAST_BIN", "/target/release/fast")
KIT = "/work/kit/starter-duckdb/starter-duckdb"
RUN_FIXTURES = f"{KIT}/scripts/run_fixtures.py"
COMPARE = f"{KIT}/scripts/compare.py"
MOUNTS = [
    f"type=bind,src={ROOT},dst=/work",
    "type=volume,src=ss-fast-cargo,dst=/usr/local/cargo/registry",
    "type=volume,src=ss-fast-target,dst=/target",
    "type=volume,src=ss-s3-20m,dst=/vol/s3-20m,readonly",
    "type=volume,src=ss-s4-60m,dst=/vol/s4-60m,readonly",
    "type=volume,src=ss-fast-out,dst=/vol/out",
]


def docker(*args, check=True, capture=False):
    p = subprocess.run(["docker", *args], text=True, capture_output=capture)
    if check and p.returncode != 0:
        if capture:
            sys.stderr.write(p.stdout + p.stderr)
        sys.exit(f"docker {args[0]} failed (exit {p.returncode})")
    return p


def up(rebuild=False):
    have_image = docker("image", "inspect", IMAGE, check=False, capture=True).returncode == 0
    if rebuild or not have_image:
        docker("build", "-t", IMAGE, "-f", str(ROOT / "fast" / "dev.Dockerfile"), str(ROOT / "fast"))
    state = docker("inspect", "-f", "{{.State.Running}}", NAME, check=False, capture=True)
    if state.returncode == 0 and rebuild:
        docker("rm", "-f", NAME, capture=True)
        state = docker("inspect", "-f", "{{.State.Running}}", NAME, check=False, capture=True)
    if state.returncode != 0:
        mounts = [arg for spec in MOUNTS for arg in ("--mount", spec)]
        docker("run", "-d", "--name", NAME, *mounts, IMAGE, "sleep", "infinity", capture=True)
    elif state.stdout.strip() != "true":
        docker("start", NAME, capture=True)


def ex(*cmd, env=None, check=False):
    """Run a command in the dev container; returns the exit code."""
    flags = [f for k, v in (env or {}).items() for f in ("-e", f"{k}={v}")]
    code = subprocess.run(["docker", "exec", "-w", "/work/fast", *flags, NAME, *cmd]).returncode
    if check and code != 0:
        sys.exit(code)
    return code


def mode_env(intern):
    return {"FAST_FORCE_INTERN": "1"} if intern else {}


def run_binary(data, out, intern=False):
    ex("sh", "-c", f'rm -rf "{out}" && mkdir -p "{out}"', check=True)
    start = time.time()
    code = ex(BIN, data, out, env=mode_env(intern))
    print(f"[dev.py] {data} -> {out}: exit {code}, {time.time() - start:.2f} s wall (docker exec)", file=sys.stderr)
    return code


def compare(a, b):
    return ex("python3", COMPARE, a, b)


def fixtures(intern):
    env = mode_env(intern)
    codes = [ex("python3", RUN_FIXTURES, f"{KIT}/fixtures", "--", BIN, env=env)]
    extra = ROOT / "tests" / "fixtures-extra"
    if any(p.is_dir() for p in extra.iterdir()):
        print("\n--- tests/fixtures-extra ---", flush=True)
        codes.append(ex("python3", RUN_FIXTURES, "/work/tests/fixtures-extra", "--", BIN, env=env))
    return max(codes)


def seed_names():
    return sorted(p.name for p in (ROOT / "data").glob("s*-1m") if p.is_dir())


def seeds(intern):
    failures = []
    suffix = "-intern" if intern else ""
    for name in seed_names():
        out = f"/work/out/fast-{name}{suffix}"
        if run_binary(f"/work/data/{name}", out, intern) != 0:
            failures.append(name)
            continue
        oracle = ROOT / "out" / f"oracle-{name}"
        if oracle.is_dir():
            print(f"[dev.py] {name}: fast{suffix} vs oracle", flush=True)
            if compare(out, f"/work/out/oracle-{name}") != 0:
                failures.append(name)
        if intern and (ROOT / "out" / f"fast-{name}").is_dir():
            print(f"[dev.py] {name}: fast-intern vs fast", flush=True)
            if compare(out, f"/work/out/fast-{name}") != 0:
                failures.append(name)
    print(f"[dev.py] seeds: {len(failures)} failures {failures}")
    return 1 if failures else 0


def oracle(names):
    names = names or seed_names()

    def one(name):
        out = f"/work/out/oracle-{name}"
        start = time.time()
        code = ex("sh", "-c", f'rm -rf "{out}" && mkdir -p "{out}" && python3 /work/docs/tools/oracle.py /work/data/{name} "{out}"')
        return name, code, time.time() - start

    with ThreadPoolExecutor(max_workers=6) as pool:
        for name, code, seconds in pool.map(one, names):
            print(f"[dev.py] oracle {name}: exit {code}, {seconds:.0f} s", flush=True)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    command, args = sys.argv[1], sys.argv[2:]
    intern = "--intern" in args
    if "--quick" in args:
        global BIN
        BIN = "/target/quick/fast"
    args = [a for a in args if a not in ("--intern", "--quick")]
    if command == "image":
        start = time.time()
        extra = ["--no-cache"] if "--no-cache" in args else []
        code = subprocess.run(["docker", "build", *extra, "-f", "fast/Dockerfile", "-t", "ss:fast", "."], cwd=ROOT).returncode
        print(f"[dev.py] image build: exit {code}, {time.time() - start:.0f} s", file=sys.stderr)
        return code
    up(rebuild=command == "up" and "--rebuild" in args)
    if command == "up":
        return 0
    if command == "build":
        return ex("cargo", "build", *([] if "--debug" in args else ["--release"]))
    if command == "test":
        return ex("cargo", "test", "--release", *args)
    if command == "fx":
        return fixtures(intern)
    if command == "dev":
        out = "/work/out/fast-dev" + ("-intern" if intern else "")
        if run_binary("/work/data/dev", out, intern) != 0:
            return 1
        return compare(out, "/work/kit/dev-answer/dev-answer")
    if command == "seeds":
        return seeds(intern)
    if command == "oracle":
        oracle(args)
        return 0
    if command == "run":
        return run_binary(args[0], args[1], intern)
    if command == "sh":
        return ex(*args)
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main())
