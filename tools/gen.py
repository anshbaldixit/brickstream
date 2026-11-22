#!/usr/bin/env python3
"""Generate a Shopping Sessions dataset with the organizers' generator (image ss-gen).

Usage (from the repo root):
  .venv/Scripts/python.exe tools/gen.py --seed N --events E [--clicks C] DEST

  DEST  a host folder (e.g. data/s2-1m), or vol:NAME for a Docker named volume
        (use volumes for the big timing sets: 20M and 60M events)
  --clicks defaults to events / 4, the dev set's ratio (1M events, 250k clicks)

The dev set is: --seed 1 --events 1000000 --clicks 250000 data/dev
"""
import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
import os
IMAGE = os.environ.get("GEN_IMAGE", "ss-gen")
GEN_SRC = os.environ.get("GEN_SRC", "kit/generator/generator")


def ensure_image():
    if subprocess.run(["docker", "image", "inspect", IMAGE], capture_output=True).returncode == 0:
        return
    print(f"building {IMAGE} ...", file=sys.stderr)
    subprocess.run(["docker", "build", "-t", IMAGE, "-f", str(ROOT / "tools" / "generator.Dockerfile"),
                    str(ROOT / GEN_SRC)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--seed", type=int, required=True)
    parser.add_argument("--events", type=int, required=True)
    parser.add_argument("--clicks", type=int)
    parser.add_argument("dest")
    args = parser.parse_args()
    clicks = args.clicks if args.clicks is not None else args.events // 4

    ensure_image()
    if args.dest.startswith("vol:"):
        target = f"type=volume,src={args.dest[4:]},dst=/out"
    else:
        path = Path(args.dest).resolve()
        path.mkdir(parents=True, exist_ok=True)
        target = f"type=bind,src={path},dst=/out"
    subprocess.run(["docker", "run", "--rm", "--mount", target, IMAGE,
                    "--seed", str(args.seed), "--events", str(args.events), "--clicks", str(clicks),
                    "--output", "/out"], check=True)
    print(f"generated seed {args.seed}: {args.events} events, {clicks} clicks -> {args.dest}")


if __name__ == "__main__":
    main()
